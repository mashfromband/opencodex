#![cfg(windows)]
use std::io::Write;
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    System::Threading::{
        CreateMutexW, GetCurrentThreadId, OpenMutexW, ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE,
        WaitForSingleObject,
    },
};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let name = args.first().expect("owned mutex name");
    assert!(name.starts_with("Local\\ocx-wct-contract-"));
    let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
    let owned = args.get(1).map(|name| {
        assert!(name.starts_with("Local\\ocx-wct-contract-"));
        let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        let owned = unsafe { CreateMutexW(core::ptr::null(), 1, name.as_ptr()) };
        assert!(!owned.is_null());
        owned
    });
    let mutex = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE | 1, 0, wide.as_ptr()) };
    assert!(!mutex.is_null());
    println!(
        "{}",
        serde_json::json!({"pid":std::process::id(),"tid":unsafe{GetCurrentThreadId()}})
    );
    std::io::stdout().flush().unwrap();
    let result = unsafe { WaitForSingleObject(mutex, 35000) };
    if result == WAIT_OBJECT_0 {
        unsafe { ReleaseMutex(mutex) };
    }
    unsafe { CloseHandle(mutex) };
    if let Some(owned) = owned {
        unsafe {
            ReleaseMutex(owned);
            CloseHandle(owned);
        }
    }
    assert_eq!(result, WAIT_OBJECT_0);
}
