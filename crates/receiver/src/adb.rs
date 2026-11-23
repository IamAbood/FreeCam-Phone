//! USB transport via `adb forward`.

use anyhow::{bail, Result};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Finds adb: configured path, then PATH, then the usual SDK locations.
pub fn find(configured: &str) -> Option<PathBuf> {
    if !configured.is_empty() {
        return Some(PathBuf::from(configured));
    }
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|p| p.join("adb.exe")));
    }
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(v) = std::env::var_os(var) {
            candidates.push(PathBuf::from(v).join("platform-tools").join("adb.exe"));
        }
    }
    if let Some(v) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(v).join("Android\\Sdk\\platform-tools\\adb.exe"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("adb.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn run(adb: &PathBuf, args: &[&str]) -> Result<String> {
    let out = Command::new(adb)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if !out.status.success() {
        bail!(
            "adb {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Serials of attached, authorised devices.
pub fn devices(adb: &PathBuf) -> Vec<String> {
    run(adb, &["devices"])
        .map(|s| parse_devices(&s))
        .unwrap_or_default()
}

fn parse_devices(out: &str) -> Vec<String> {
    out.lines()
        .skip(1)
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let serial = it.next()?;
            (it.next()? == "device").then(|| serial.to_string())
        })
        .collect()
}

/// Forwards `127.0.0.1:local` to the phone's server port.
pub fn forward(adb: &PathBuf, serial: &str, local: u16) -> Result<()> {
    run(
        adb,
        &[
            "-s",
            serial,
            "forward",
            &format!("tcp:{local}"),
            &format!("tcp:{}", protocol::DEFAULT_PORT),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_device_list() {
        let out = "List of devices attached\nR58M123\tdevice\nemulator-5554\tdevice\nXYZ\tunauthorized\n\n";
        assert_eq!(super::parse_devices(out), ["R58M123", "emulator-5554"]);
    }
}
