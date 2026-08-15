//! Windows power control via `powercfg` and the NDIS power-management cmdlets.
//!
//! Windows splits the problem across three places, and missing any one of them
//! still lets the machine drop:
//!   1. the lid-close *action* (per power scheme, separate AC and DC values),
//!   2. the sleep/hibernate timeouts and the low/critical battery actions,
//!   3. the Wi-Fi adapter's own power saving, which is what actually severs the
//!      connection even when the OS stays up.
//!
//! NOTE: this file is written but has not been compiled or run — the machine it
//! was developed on is macOS. Treat it as a first draft.

use std::collections::BTreeMap;
use std::os::windows::process::CommandExt;
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::{ApplyOutcome, Baseline};
use crate::protocol::{Guards, Profile};

/// Keeps a console window from flashing up on every powercfg call.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const SUB_BUTTONS: &str = "4f971e89-eebd-4455-a8de-9e59040e7347";
const SETTING_LID_ACTION: &str = "5ca83367-6e45-459f-a27b-476b1d01c936";

const SUB_BATTERY: &str = "e73a048d-bf27-4f12-9731-8b2076e8891f";
const SETTING_CRITICAL_ACTION: &str = "637ea02f-bbcb-4015-8e2c-a1c7b9c0b546";
const SETTING_LOW_ACTION: &str = "d8742dcb-3e6a-4b3c-b3fe-374623cdcf06";

const SUB_SLEEP: &str = "238c9fa8-0aad-41ed-83f4-97be242c8f20";
const SETTING_STANDBY_IDLE: &str = "29f6c1db-86da-48c5-9fdb-f2b67b1f44da";
const SETTING_HIBERNATE_IDLE: &str = "9d7815a6-7ee4-497e-8888-515a05f02364";

const SUB_VIDEO: &str = "7516b95f-f776-4464-8c53-06167f40cc99";
const SETTING_DISPLAY_IDLE: &str = "3c0bc021-c8a8-4e07-a973-6b14cbcb2b7e";

const SUB_WIRELESS: &str = "19cbb8fa-5279-450e-9fac-8a3d5fedd0c1";
const SETTING_WIRELESS_POWER: &str = "12bbebe6-58d6-4636-95bb-3217ef867c1a";

/// Every (subgroup, setting) pair we drive, with the value we want.
/// `0` means "do nothing / never / maximum performance" for all of these.
const MANAGED: &[(&str, &str, &str)] = &[
    (SUB_BUTTONS, SETTING_LID_ACTION, "0"), // lid close -> do nothing
    (SUB_BATTERY, SETTING_CRITICAL_ACTION, "0"), // critical battery -> do nothing
    (SUB_BATTERY, SETTING_LOW_ACTION, "0"), // low battery -> do nothing
    (SUB_SLEEP, SETTING_STANDBY_IDLE, "0"), // never sleep on idle
    (SUB_SLEEP, SETTING_HIBERNATE_IDLE, "0"), // never hibernate on idle
    (SUB_WIRELESS, SETTING_WIRELESS_POWER, "0"), // Wi-Fi -> maximum performance
];

#[derive(Debug, Serialize, Deserialize)]
pub struct WinBaseline {
    /// "<sub>/<setting>" -> (ac_index, dc_index)
    powercfg: BTreeMap<String, (u32, u32)>,
    /// adapter name -> "Enabled" | "Disabled" | "Unsupported"
    adapters: BTreeMap<String, String>,
}

pub fn capture_baseline() -> Result<Baseline, String> {
    let mut powercfg = BTreeMap::new();
    for (sub, setting, _) in
        MANAGED
            .iter()
            .chain(std::iter::once(&(SUB_VIDEO, SETTING_DISPLAY_IDLE, "0")))
    {
        if let Some(pair) = query_indices(sub, setting) {
            powercfg.insert(format!("{sub}/{setting}"), pair);
        }
    }
    if powercfg.is_empty() {
        return Err("powercfg reported no readable settings".into());
    }

    let adapters = query_adapters();
    serde_json::to_value(WinBaseline { powercfg, adapters }).map_err(|e| e.to_string())
}

pub fn apply(baseline: &Baseline, profile: Profile) -> Result<ApplyOutcome, String> {
    let base: WinBaseline =
        serde_json::from_value(baseline.clone()).map_err(|e| format!("corrupt baseline: {e}"))?;
    let mut warnings = Vec::new();
    let mut ok: BTreeMap<&str, bool> = BTreeMap::new();

    for (sub, setting, value) in MANAGED {
        if !base.powercfg.contains_key(&format!("{sub}/{setting}")) {
            continue; // not supported here; skipping keeps restore honest
        }
        let good = set_both(&mut warnings, sub, setting, value, value);
        ok.insert(setting, good);
    }

    // Adapter-level radio power saving. Without this Windows parks the NIC even
    // while the system is technically awake, and the SSH session dies.
    let mut wifi_ok = *ok.get(SETTING_WIRELESS_POWER).unwrap_or(&true);
    for name in base.adapters.keys() {
        if !set_adapter_power(name, "Disabled") {
            warnings.push(format!(
                "{name}: could not disable adapter power management"
            ));
            wifi_ok = false;
        }
    }

    let display = if profile.keep_display {
        set_both(&mut warnings, SUB_VIDEO, SETTING_DISPLAY_IDLE, "0", "0")
    } else {
        restore_setting(&base, SUB_VIDEO, SETTING_DISPLAY_IDLE, &mut warnings);
        false
    };

    activate(&mut warnings);

    Ok(ApplyOutcome {
        guards: Guards {
            lid: *ok.get(SETTING_LID_ACTION).unwrap_or(&false),
            battery: *ok.get(SETTING_CRITICAL_ACTION).unwrap_or(&false)
                && *ok.get(SETTING_LOW_ACTION).unwrap_or(&false),
            wifi: wifi_ok,
            display,
        },
        warnings,
    })
}

pub fn restore(baseline: &Baseline) -> Result<(), String> {
    let base: WinBaseline =
        serde_json::from_value(baseline.clone()).map_err(|e| format!("corrupt baseline: {e}"))?;
    let mut warnings = Vec::new();

    for key in base.powercfg.keys() {
        if let Some((sub, setting)) = key.split_once('/') {
            restore_setting(&base, sub, setting, &mut warnings);
        }
    }
    for (name, state) in &base.adapters {
        if state == "Enabled" {
            set_adapter_power(name, "Enabled");
        }
    }
    activate(&mut warnings);

    if !warnings.is_empty() {
        eprintln!("[claude-awake] restore warnings: {}", warnings.join("; "));
    }
    Ok(())
}

fn restore_setting(base: &WinBaseline, sub: &str, setting: &str, warnings: &mut Vec<String>) {
    if let Some((ac, dc)) = base.powercfg.get(&format!("{sub}/{setting}")) {
        set_both(warnings, sub, setting, &ac.to_string(), &dc.to_string());
    }
}

fn set_both(warnings: &mut Vec<String>, sub: &str, setting: &str, ac: &str, dc: &str) -> bool {
    let a = powercfg(&["/setacvalueindex", "SCHEME_CURRENT", sub, setting, ac]);
    let d = powercfg(&["/setdcvalueindex", "SCHEME_CURRENT", sub, setting, dc]);
    if !a || !d {
        warnings.push(format!("could not set {setting}"));
    }
    a && d
}

/// powercfg edits the stored scheme; nothing takes effect until it is re-activated.
fn activate(warnings: &mut Vec<String>) {
    if !powercfg(&["/setactive", "SCHEME_CURRENT"]) {
        warnings.push("could not activate the power scheme".into());
    }
}

fn powercfg(args: &[&str]) -> bool {
    Command::new("powercfg.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Parses `Current AC/DC Power Setting Index: 0x00000001` out of a powercfg query.
fn query_indices(sub: &str, setting: &str) -> Option<(u32, u32)> {
    let out = Command::new("powercfg.exe")
        .args(["/query", "SCHEME_CURRENT", sub, setting])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut ac = None;
    let mut dc = None;
    for line in text.lines() {
        let line = line.trim();
        let value = || {
            let raw = line.rsplit(':').next()?.trim();
            let raw = raw.strip_prefix("0x").unwrap_or(raw);
            u32::from_str_radix(raw, 16).ok()
        };
        if line.starts_with("Current AC Power Setting Index") {
            ac = value();
        } else if line.starts_with("Current DC Power Setting Index") {
            dc = value();
        }
    }
    Some((ac?, dc?))
}

fn query_adapters() -> BTreeMap<String, String> {
    let script = "Get-NetAdapter -Physical | Where-Object { $_.MediaType -eq 'Native 802.11' -or \
                  $_.PhysicalMediaType -like '*802.11*' } | ForEach-Object { \
                  $p = Get-NetAdapterPowerManagement -Name $_.Name -ErrorAction SilentlyContinue; \
                  \"$($_.Name)|$(if ($p) { $p.AllowComputerToTurnOffDevice } else { 'Unsupported' })\" }";
    let out = match Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return BTreeMap::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().split_once('|'))
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect()
}

fn set_adapter_power(name: &str, state: &str) -> bool {
    let script = format!(
        "Set-NetAdapterPowerManagement -Name '{}' -AllowComputerToTurnOffDevice {} -ErrorAction Stop",
        name.replace('\'', "''"),
        state
    );
    Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
