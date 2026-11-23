//! "Start with Windows" via HKCU\...\Run.

use windows::core::{w, HSTRING};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
};

const RUN: windows::core::PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const NAME: windows::core::PCWSTR = w!("FreeCamPhone");

pub fn set(enabled: bool) {
    if !enabled {
        // SAFETY: valid constant key/value names.
        unsafe {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, NAME);
        }
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let cmd = HSTRING::from(format!("\"{}\" --minimized", exe.display()));
    let bytes = (cmd.len() + 1) * 2;
    // SAFETY: `cmd` is a NUL-terminated UTF-16 buffer of `bytes` bytes that
    // outlives the call.
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            RUN,
            NAME,
            REG_SZ.0,
            Some(cmd.as_ptr().cast()),
            bytes as u32,
        );
    }
}
