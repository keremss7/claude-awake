//! Windows transport: a real Windows Service speaking over a named pipe.
//!
//! Two details matter for security here:
//!
//!   * The pipe carries an explicit DACL. The default descriptor on a LocalSystem
//!     service denies normal users, so without this the UI could never connect.
//!     Access is granted to interactive users only — not to every service account
//!     on the box.
//!   * The first instance is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`. Without
//!     it, a process that starts earlier could squat the name and impersonate the
//!     helper to the UI.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL,
    INVALID_HANDLE_VALUE,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_WAIT,
};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::{define_windows_service, service_control_handler, service_dispatcher};

use super::{handle_line, Helper};
use crate::protocol::PIPE_NAME;

pub const SERVICE_NAME: &str = "ClaudeAwakeHelper";

/// Grant: LocalSystem and Administrators full control; interactive users read and
/// write. Interactive-only keeps other service accounts out.
const PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)";

const MAX_PIPE_INSTANCES: u32 = 8;
const PIPE_BUFFER: u32 = 8 * 1024;

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    // Self-installation. Doing it here rather than through `sc.exe` avoids the
    // command-line quoting rules that make service paths with spaces so
    // error-prone, and it keeps the installer and the manual script in step.
    if has("--install-service") {
        report(install_service(), "installed");
        return;
    }
    if has("--uninstall-service") {
        report(uninstall_service(), "removed");
        return;
    }

    // `--console` runs the identical loop in the foreground, which is the only
    // practical way to debug the daemon without attaching to a service.
    if has("--console") {
        super::recover_from_unclean_exit();
        serve();
        return;
    }
    if let Err(e) = service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        // Started outside the SCM (someone double-clicked it). Degrade to console
        // mode rather than exiting silently.
        eprintln!(
            "[claude-awake] not started by the service manager ({e}); running in console mode"
        );
        super::recover_from_unclean_exit();
        serve();
    }
}

fn report(result: windows_service::Result<()>, verb: &str) {
    match result {
        Ok(()) => println!("Claude Awake helper service {verb}."),
        Err(e) => {
            eprintln!("Could not {} the service: {e}", verb.trim_end_matches("ed"));
            eprintln!("Run this from an elevated (Administrator) prompt.");
            std::process::exit(1);
        }
    }
}

/// Registers this executable as an auto-starting service and starts it.
/// Idempotent: re-running upgrades the binary path in place.
pub fn install_service() -> windows_service::Result<()> {
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )?;

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from("Claude Awake Helper"),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: std::env::current_exe().map_err(windows_service::Error::Winapi)?,
        launch_arguments: vec![],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };

    let access = ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS;
    let service = match manager.create_service(&info, access) {
        Ok(service) => service,
        // Already registered: point it at the current binary instead of failing,
        // so reinstalling over an old version does the right thing.
        Err(_) => {
            let service = manager.open_service(SERVICE_NAME, access)?;
            service.change_config(&info)?;
            service
        }
    };
    service.set_description(
        "Applies and reverts the power settings that let Claude Awake keep this \
         machine running with the lid closed.",
    )?;
    let _ = service.start(&[] as &[&std::ffi::OsStr]);
    Ok(())
}

/// Stops and deletes the service. The service restores the power settings on its
/// own stop path, so this cannot leave a machine unable to sleep.
pub fn uninstall_service() -> windows_service::Result<()> {
    use windows_service::service::{ServiceAccess, ServiceState as State};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
    )?;

    if service.query_status()?.current_state != State::Stopped {
        let _ = service.stop();
        // Give the stop handler time to roll the power settings back before the
        // service record disappears.
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(200));
            if service.query_status()?.current_state == State::Stopped {
                break;
            }
        }
    }
    service.delete()?;
    Ok(())
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        eprintln!("[claude-awake] service failed: {e}");
    }
}

fn run_service() -> windows_service::Result<()> {
    let status_handle =
        service_control_handler::register(SERVICE_NAME, move |control| match control {
            ServiceControl::Interrogate => {
                service_control_handler::ServiceControlHandlerResult::NoError
            }
            ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
                super::request_shutdown();
                wake_accept_loop();
                service_control_handler::ServiceControlHandlerResult::NoError
            }
            _ => service_control_handler::ServiceControlHandlerResult::NotImplemented,
        })?;

    status_handle.set_service_status(status(ServiceState::Running))?;
    super::recover_from_unclean_exit();
    serve();
    status_handle.set_service_status(status(ServiceState::Stopped))?;
    Ok(())
}

fn status(state: ServiceState) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    }
}

/// `ConnectNamedPipe` blocks with no cancellation, so a stop request opens a
/// throwaway client connection to push the loop past it.
fn wake_accept_loop() {
    let _ = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_NAME);
}

fn serve() {
    let helper = Arc::new(Mutex::new(Helper::default()));
    super::spawn_watchdog(Arc::clone(&helper));
    eprintln!("[claude-awake] helper listening on {PIPE_NAME}");

    let mut first_instance = true;
    while !super::shutdown_requested() {
        match Pipe::accept(first_instance) {
            Ok(pipe) => {
                first_instance = false;
                let helper = Arc::clone(&helper);
                // One thread per request: a wedged client must not block the next.
                std::thread::spawn(move || pipe.serve_one(&helper));
            }
            Err(e) => {
                eprintln!("[claude-awake] pipe accept failed: {e}");
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }

    super::restore_on_exit(&helper);
}

/// Owns one connected pipe instance and disconnects on drop.
struct Pipe(HANDLE);

// The handle is only ever touched by the thread that owns the `Pipe`.
unsafe impl Send for Pipe {}

impl Pipe {
    fn accept(first_instance: bool) -> std::io::Result<Pipe> {
        let descriptor = SecurityDescriptor::build()?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0 .0,
            bInheritHandle: false.into(),
        };

        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first_instance {
            open_mode |= FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_FIRST_PIPE_INSTANCE.0);
        }

        let handle = unsafe {
            CreateNamedPipeW(
                &HSTRING::from(PIPE_NAME),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                MAX_PIPE_INSTANCES,
                PIPE_BUFFER,
                PIPE_BUFFER,
                0,
                Some(&attributes),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }

        // A client that connected between create and connect is a success, not
        // an error — Windows reports it as ERROR_PIPE_CONNECTED.
        let connected = unsafe { ConnectNamedPipe(handle, None) };
        if connected.is_err() && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            let err = std::io::Error::last_os_error();
            unsafe { CloseHandle(handle).ok() };
            return Err(err);
        }
        Ok(Pipe(handle))
    }

    fn serve_one(&self, helper: &Mutex<Helper>) {
        let file = unsafe {
            use std::os::windows::io::FromRawHandle;
            std::mem::ManuallyDrop::new(std::fs::File::from_raw_handle(self.0 .0 as _))
        };
        let mut writer: &std::fs::File = &file;
        let mut line = String::new();
        if BufReader::new(&*file).read_line(&mut line).is_err() {
            return;
        }
        let reply = handle_line(helper, &line);
        let _ = writer.write_all(format!("{reply}\n").as_bytes());
        let _ = writer.flush();
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            let _ = DisconnectNamedPipe(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

/// Wraps the descriptor allocated by the SDDL converter so it is always freed.
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    fn build() -> std::io::Result<SecurityDescriptor> {
        let sddl: Vec<u16> = PIPE_SDDL.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
            .map_err(|e| std::io::Error::other(format!("pipe DACL: {e}")))?;
        }
        Ok(SecurityDescriptor(descriptor))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0 .0)));
            }
        }
    }
}
