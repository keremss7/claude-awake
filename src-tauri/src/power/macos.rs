//! macOS power control via `pmset`.
//!
//! Why `pmset` and not an IOKit assertion: assertions (what `caffeinate` uses)
//! only cover *idle* sleep. Closing the lid triggers clamshell sleep, which no
//! userland assertion can veto — the only lever is `pmset disablesleep 1`, and
//! that is root-only. Hence this code living in the helper.

use std::collections::BTreeMap;
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::{ApplyOutcome, Baseline};
use crate::protocol::{Guards, Profile};

const PMSET: &str = "/usr/bin/pmset";

/// Settings applied in both power scopes. Each is only touched if the machine
/// actually reports it (see `apply`), which guarantees we can always put it back.
const BOTH_SCOPES: &[(&str, &str)] = &[
    ("sleep", "0"),         // no idle system sleep
    ("disksleep", "0"),     // no disk spindown mid-task
    ("standby", "0"),       // no deep standby (kills the network)
    ("autopoweroff", "0"),  // Intel-only sibling of standby
    ("tcpkeepalive", "1"),  // keep TCP sessions alive
    ("womp", "1"),          // wake for network access
    ("ttyskeepawake", "1"), // an active tty holds the machine up
];

/// Battery-scope-only settings. This is the "ignore the battery" half.
const BATTERY_ONLY: &[(&str, &str)] = &[
    ("lowpowermode", "0"), // never throttle into Low Power Mode
    ("lessbright", "0"),   // no auto-dim on battery
];

#[derive(Debug, Serialize, Deserialize)]
pub struct MacBaseline {
    battery: BTreeMap<String, String>,
    ac: BTreeMap<String, String>,
    /// `SleepDisabled` from `pmset -g`; it is absent from `pmset -g custom`.
    sleep_disabled: String,
}

pub fn capture_baseline() -> Result<Baseline, String> {
    let custom = run_capture(&["-g", "custom"])?;
    let (battery, ac) = parse_custom(&custom);
    if battery.is_empty() && ac.is_empty() {
        return Err("pmset -g custom returned no settings".into());
    }
    let live = run_capture(&["-g"]).unwrap_or_default();
    let sleep_disabled = parse_flat(&live)
        .get("SleepDisabled")
        .cloned()
        .unwrap_or_else(|| "0".into());

    serde_json::to_value(MacBaseline {
        battery,
        ac,
        sleep_disabled,
    })
    .map_err(|e| e.to_string())
}

pub fn apply(baseline: &Baseline, profile: Profile) -> Result<ApplyOutcome, String> {
    let base: MacBaseline =
        serde_json::from_value(baseline.clone()).map_err(|e| format!("corrupt baseline: {e}"))?;
    let mut warnings = Vec::new();

    // The one setting that is not in `pmset -g custom` and the only one that
    // actually survives a lid close.
    let lid = set(&mut warnings, "-a", "disablesleep", "1");

    let mut battery_ok = true;
    let mut wifi_ok = true;

    for (key, value) in BOTH_SCOPES {
        // Only touch what this machine reports, so `restore` can always undo it.
        let in_batt = base.battery.contains_key(*key);
        let in_ac = base.ac.contains_key(*key);
        if !in_batt && !in_ac {
            continue;
        }
        let ok = if in_batt && in_ac {
            set(&mut warnings, "-a", key, value)
        } else if in_batt {
            set(&mut warnings, "-b", key, value)
        } else {
            set(&mut warnings, "-c", key, value)
        };
        match *key {
            "womp" | "tcpkeepalive" => wifi_ok &= ok,
            "sleep" | "standby" => battery_ok &= ok,
            _ => {}
        }
    }

    for (key, value) in BATTERY_ONLY {
        // `lowpowermode` is written under that name but read back as `powermode`.
        let probe = if *key == "lowpowermode" {
            "powermode"
        } else {
            key
        };
        if !base.battery.contains_key(probe) {
            continue;
        }
        battery_ok &= set(&mut warnings, "-b", key, value);
    }

    let display = if profile.keep_display {
        set(&mut warnings, "-a", "displaysleep", "0")
    } else {
        // Not an error — the user opted out, so put the baseline value back in
        // case a previous profile had held it on.
        restore_display(&base, &mut warnings);
        false
    };

    Ok(ApplyOutcome {
        guards: Guards {
            lid,
            battery: battery_ok,
            wifi: wifi_ok,
            display,
        },
        warnings,
    })
}

pub fn restore(baseline: &Baseline) -> Result<(), String> {
    let base: MacBaseline =
        serde_json::from_value(baseline.clone()).map_err(|e| format!("corrupt baseline: {e}"))?;
    let mut warnings = Vec::new();

    set(&mut warnings, "-a", "disablesleep", &base.sleep_disabled);

    let mut keys: Vec<&str> = BOTH_SCOPES.iter().map(|(k, _)| *k).collect();
    keys.push("displaysleep");
    for key in keys {
        if let Some(v) = base.battery.get(key) {
            set(&mut warnings, "-b", key, v);
        }
        if let Some(v) = base.ac.get(key) {
            set(&mut warnings, "-c", key, v);
        }
    }

    // powermode 1 means Low Power Mode was on; anything else means it was off.
    if let Some(pm) = base.battery.get("powermode") {
        let want = if pm == "1" { "1" } else { "0" };
        set(&mut warnings, "-b", "lowpowermode", want);
    }
    if let Some(v) = base.battery.get("lessbright") {
        set(&mut warnings, "-b", "lessbright", v);
    }

    if warnings.is_empty() {
        Ok(())
    } else {
        // Report but do not fail: a partial restore is still worth keeping.
        eprintln!("[claude-awake] restore warnings: {}", warnings.join("; "));
        Ok(())
    }
}

fn restore_display(base: &MacBaseline, warnings: &mut Vec<String>) {
    if let Some(v) = base.battery.get("displaysleep") {
        set(warnings, "-b", "displaysleep", v);
    }
    if let Some(v) = base.ac.get("displaysleep") {
        set(warnings, "-c", "displaysleep", v);
    }
}

/// Returns whether the setting took. Failures are collected, never fatal — some
/// keys simply do not exist on Apple Silicon.
fn set(warnings: &mut Vec<String>, scope: &str, key: &str, value: &str) -> bool {
    match Command::new(PMSET).args([scope, key, value]).output() {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
            warnings.push(format!("{key}={value} rejected: {msg}"));
            false
        }
        Err(e) => {
            warnings.push(format!("could not run pmset for {key}={value}: {e}"));
            false
        }
    }
}

fn run_capture(args: &[&str]) -> Result<String, String> {
    let out = Command::new(PMSET)
        .args(args)
        .output()
        .map_err(|e| format!("could not run pmset: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "pmset {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `pmset -g custom` prints two indented blocks. Keys may contain spaces
/// ("Sleep On Power Button"), so the *last* token on a line is the value.
fn parse_custom(text: &str) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut battery = BTreeMap::new();
    let mut ac = BTreeMap::new();
    let mut target: Option<bool> = None; // Some(true) = battery

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            target = match trimmed {
                t if t.starts_with("Battery Power") => Some(true),
                t if t.starts_with("AC Power") => Some(false),
                _ => None,
            };
            continue;
        }
        let Some((key, value)) = split_setting(trimmed) else {
            continue;
        };
        match target {
            Some(true) => battery.insert(key, value),
            Some(false) => ac.insert(key, value),
            None => continue,
        };
    }
    (battery, ac)
}

fn parse_flat(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter(|l| l.starts_with([' ', '\t']))
        .filter_map(|l| split_setting(l.trim()))
        .collect()
}

fn split_setting(line: &str) -> Option<(String, String)> {
    // Trailing parenthetical annotations exist on live output, e.g.
    // `sleep 0 (sleep prevented by caffeinate)`. Cut them before splitting.
    let line = line.split('(').next()?.trim();
    let (key, value) = line.rsplit_once(char::is_whitespace)?;
    let key = key.trim();
    if key.is_empty() || value.is_empty() {
        return None;
    }
    Some((key.to_string(), value.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Battery Power:\n Sleep On Power Button 1\n powermode            0\n sleep                1\nAC Power:\n sleep                0\n womp                 1\n";

    #[test]
    fn parses_both_scopes_and_spaced_keys() {
        let (batt, ac) = parse_custom(SAMPLE);
        assert_eq!(batt.get("sleep").map(String::as_str), Some("1"));
        assert_eq!(batt.get("powermode").map(String::as_str), Some("0"));
        assert_eq!(
            batt.get("Sleep On Power Button").map(String::as_str),
            Some("1")
        );
        assert_eq!(ac.get("sleep").map(String::as_str), Some("0"));
        assert_eq!(ac.get("womp").map(String::as_str), Some("1"));
        assert!(!ac.contains_key("powermode"));
    }

    #[test]
    fn strips_live_annotations() {
        let flat = parse_flat(" SleepDisabled\t\t0\n sleep 0 (sleep prevented by caffeinate)\n");
        assert_eq!(flat.get("SleepDisabled").map(String::as_str), Some("0"));
        assert_eq!(flat.get("sleep").map(String::as_str), Some("0"));
    }
}
