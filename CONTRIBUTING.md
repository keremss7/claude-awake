# Contributing

Thanks for taking a look. This is a small, focused tool — the bar for changes is "does it make
the core promise more reliable", not "does it add a feature".

## The two most useful contributions

**1. Windows validation.** The Windows implementation compiles and is checked in CI, but it has
not run on physical hardware. If you try it, open an issue with what happened — the service
registration, whether the pill tracks your terminal, whether the lid actually stays awake.
Even "it worked" is valuable.

**2. Terminal detection.** If the pill does not appear for your terminal:

```bash
cargo run --manifest-path src-tauri/Cargo.toml --example windows
```

Click into your terminal, note the printed owner name, and add it to `TERMINALS` in
[`src-tauri/src/tracker/mod.rs`](src-tauri/src/tracker/mod.rs). That is a one-line PR and always
welcome.

## Setup

```bash
npm install
npm run app:dev     # builds the helper, then runs the app against the Vite dev server
```

The privileged helper needs a one-time install to do anything meaningful:

```bash
sudo bash scripts/install-helper.sh                            # macOS
powershell -ExecutionPolicy Bypass -File scripts/install-helper.ps1   # Windows, elevated
```

Without it the app still runs — it just falls back to unprivileged idle-sleep prevention and
reports `setup required` in the panel.

### Working on the UI without rebuilding Rust

Both surfaces render in a plain browser, with state driven by the query string:

```bash
npm run dev
open 'http://localhost:5183/?surface=overlay&protecting=1&claude=1&expanded=1'
open 'http://localhost:5183/?surface=panel&theme=light&helper=missing'
```

Recognised parameters: `surface`, `theme`, `mode`, `protecting`, `claude`, `helper`, `detail`,
`secs`, `app`, `display`, `autostart`, `expanded`.

## Before you open a PR

```bash
npm run lint                                                   # tsc --noEmit
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

If you touch platform code, check that the *other* platform still type-checks. You do not need
a Windows machine for this:

```bash
rustup target add x86_64-pc-windows-msvc
cargo clippy --manifest-path src-tauri/Cargo.toml --lib --target x86_64-pc-windows-msvc -- -D warnings
```

(The `--lib` matters: the binary needs `llvm-rc` to build Windows resources, the library does
not, and all the platform-specific code lives in the library. Running clippy rather than just
`check` is worth it — a couple of lints only fire on the platform whose `cfg` branch is
active.)

## House rules for the privileged helper

Anything in `src-tauri/src/helper/` and `src-tauri/src/power/` runs as root or LocalSystem.
Changes there get read closely:

- **Never execute anything derived from a request.** The `Request` enum is the entire
  vocabulary and every value written to the system is a compile-time constant.
- **Never write a setting you cannot restore.** Only touch keys the machine actually reported
  in the baseline snapshot.
- **Every new exit path must restore.** If you add one, make sure it goes through
  `restore_on_exit`, and think about what happens if the process is killed instead.
- **No new dependencies** without a strong reason. The helper is small so that it can be read
  end to end.

## Commit messages

Plain imperative present tense — `fix pill sizing loop on first mount`. No prefix convention is
enforced.
