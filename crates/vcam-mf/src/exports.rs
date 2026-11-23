//! COM DLL exports. Registration writes HKLM (the Frame Server service reads
//! the machine-wide class registration), so `regsvr32` must run elevated —
//! the installer does that; we never self-register at runtime.

use super::{Factory, CLSID};
use std::ffi::c_void;
use windows::core::{w, Interface, GUID, HRESULT, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_POINTER, HMODULE, S_FALSE, S_OK};
use windows::Win32::System::Com::IClassFactory;
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::System::Registry::*;

#[no_mangle]
pub extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if clsid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked non-null; COM guarantees a valid GUID pointer.
    if unsafe { *clsid } != CLSID {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let f: IClassFactory = Factory.into();
    // SAFETY: forwarding the caller's riid/ppv to QueryInterface.
    unsafe { f.query(riid, ppv) }
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    // ponytail: stay loaded for the host's lifetime instead of tracking object counts.
    S_FALSE
}

fn key_path() -> HSTRING {
    HSTRING::from(format!(r"SOFTWARE\Classes\CLSID\{{{:?}}}", CLSID))
}

fn module_path() -> windows::core::Result<String> {
    let mut h = HMODULE::default();
    let mut buf = [0u16; 1024];
    // SAFETY: looks up the module containing this function; `buf` is a valid
    // writable buffer of the given length.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(DllCanUnloadNow as *const () as *const u16),
            &mut h,
        )?;
        let n = GetModuleFileNameW(h, &mut buf) as usize;
        Ok(String::from_utf16_lossy(&buf[..n]))
    }
}

fn set_str(key: HKEY, name: PCWSTR, value: &str) -> windows::core::Result<()> {
    let v = HSTRING::from(value);
    // SAFETY: `v` is NUL-terminated UTF-16 that outlives the call; length in bytes includes the NUL.
    let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, (v.len() + 1) * 2) };
    // SAFETY: valid open key and value buffer.
    unsafe { RegSetValueExW(key, name, 0, REG_SZ, Some(bytes)).ok() }
}

#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    let r = (|| -> windows::core::Result<()> {
        let path = module_path()?;
        let mut key = HKEY::default();
        let sub = HSTRING::from(format!(r"{}\InprocServer32", key_path()));
        // SAFETY: creating a registry key under HKLM with a valid path; the
        // handle is closed below.
        unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                &sub,
                0,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            )
            .ok()?;
        }
        let r = set_str(key, PCWSTR::null(), &path)
            .and_then(|_| set_str(key, w!("ThreadingModel"), "Both"));
        // SAFETY: closing the key we opened.
        unsafe {
            let _ = RegCloseKey(key);
        }
        r
    })();
    r.map_or_else(|e| e.code(), |_| S_OK)
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    // SAFETY: deleting our own CLSID subtree under HKLM.
    let r = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, &key_path()) };
    if r.is_ok() || r == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        S_OK
    } else {
        r.to_hresult()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn clsid_string_matches_framebus() {
        assert_eq!(format!("{{{:?}}}", super::CLSID), framebus::MF_SOURCE_CLSID);
    }
}
