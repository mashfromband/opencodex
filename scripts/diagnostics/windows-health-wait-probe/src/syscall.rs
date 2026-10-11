use serde_json::{Value, json};
use std::mem::size_of;
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::{
    Wdk::System::Threading::{NtQueryInformationThread, ThreadLastSystemCall},
    Win32::{
        Foundation::HANDLE,
        System::{
            LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW, GetProcAddress},
            ProcessStatus::{GetModuleInformation, MODULEINFO},
            Threading::{
                GetCurrentProcess, IsWow64Process2, OpenThread, THREAD_GET_CONTEXT,
                THREAD_QUERY_INFORMATION, THREAD_SYNCHRONIZE,
            },
        },
    },
};

// Fixed to ThreadLastSystemCall on native AMD64. The first argument is never
// dereferenced, printed, or serialized. Layout reference: System Informer's
// phnt ntpsapi.h THREAD_LAST_SYSCALL_INFORMATION (modern Windows).
#[repr(C)]
#[derive(Default)]
struct LastCall {
    _argument: usize,
    number: u16,
    _padding: [u8; 6],
    wait_ticks: u64,
}

fn native_x64(process: HANDLE) -> bool {
    if !cfg!(target_arch = "x86_64") {
        return false;
    }
    let (mut machine, mut native) = (0, 0);
    unsafe {
        IsWow64Process2(process, &mut machine, &mut native) != 0 && machine == 0 && native == 0x8664
    }
}

fn stub_number(bytes: &[u8]) -> Option<u16> {
    // Recognize the entire direct AMD64 syscall stub, including both returns.
    // A hook, truncation or different platform/version shape is unmapped.
    const TAIL: [u8; 16] = [
        0xf6, 0x04, 0x25, 0x08, 0x03, 0xfe, 0x7f, 0x01, 0x75, 0x03, 0x0f, 0x05, 0xc3, 0xcd, 0x2e,
        0xc3,
    ];
    let bytes = bytes.get(..24)?;
    if bytes[..4] != [0x4c, 0x8b, 0xd1, 0xb8] || bytes[8..] != TAIL {
        return None;
    }
    u16::try_from(u32::from_le_bytes(bytes[4..8].try_into().ok()?)).ok()
}

fn symbol_for(number: u16) -> Option<&'static str> {
    let name: Vec<_> = "ntdll.dll".encode_utf16().chain(Some(0)).collect();
    let mut module = core::ptr::null_mut();
    // Pin the loaded local system module before reading its validated image range.
    if unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_PIN, name.as_ptr(), &mut module) } == 0
    {
        return None;
    }
    let mut info = MODULEINFO::default();
    if unsafe {
        GetModuleInformation(
            GetCurrentProcess(),
            module,
            &mut info,
            size_of::<MODULEINFO>() as u32,
        )
    } == 0
    {
        return None;
    }
    let start = info.lpBaseOfDll as usize;
    let end = start.checked_add(info.SizeOfImage as usize)?;
    let symbols: [(&[u8], &str); 21] = [
        (b"NtReadFile\0", "NtReadFile"),
        (b"NtWriteFile\0", "NtWriteFile"),
        (b"NtFlushBuffersFile\0", "NtFlushBuffersFile"),
        (b"NtCreateFile\0", "NtCreateFile"),
        (b"NtOpenFile\0", "NtOpenFile"),
        (b"NtQueryInformationFile\0", "NtQueryInformationFile"),
        (b"NtWaitForSingleObject\0", "NtWaitForSingleObject"),
        (b"NtWaitForMultipleObjects\0", "NtWaitForMultipleObjects"),
        (b"NtRemoveIoCompletion\0", "NtRemoveIoCompletion"),
        (b"NtRemoveIoCompletionEx\0", "NtRemoveIoCompletionEx"),
        (b"NtWaitForAlertByThreadId\0", "NtWaitForAlertByThreadId"),
        (b"NtDelayExecution\0", "NtDelayExecution"),
        (b"NtClose\0", "NtClose"),
        (b"NtYieldExecution\0", "NtYieldExecution"),
        (b"NtQueryAttributesFile\0", "NtQueryAttributesFile"),
        (b"NtCreateUserProcess\0", "NtCreateUserProcess"),
        (b"NtCreateProcessEx\0", "NtCreateProcessEx"),
        (b"NtOpenProcess\0", "NtOpenProcess"),
        (b"NtResumeThread\0", "NtResumeThread"),
        (b"NtQueryInformationProcess\0", "NtQueryInformationProcess"),
        (b"NtAlpcSendWaitReceivePort\0", "NtAlpcSendWaitReceivePort"),
    ];
    let mut found = None;
    for (name, label) in symbols {
        let Some(function) = (unsafe { GetProcAddress(module, name.as_ptr()) }) else {
            continue;
        };
        let address = function as usize;
        if address < start || address.checked_add(24).is_none_or(|last| last > end) {
            continue;
        }
        let mut bytes = [0u8; 24];
        let mut read = 0;
        // Read only this validated local export range, failing closed if its
        // protection changed. Never read a remote pointer or its first argument.
        if unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                address as *const _,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                &mut read,
            )
        } == 0
            || read != bytes.len()
        {
            continue;
        }
        if stub_number(&bytes) == Some(number) {
            if found.is_some() {
                return None;
            }
            found = Some(label);
        }
    }
    found
}

pub(super) fn resolve_local(number: u16) -> Value {
    if !native_x64(unsafe { GetCurrentProcess() }) {
        return json!({"error":"unsupported_architecture"});
    }
    json!({"number":number,"local_stub_symbol":symbol_for(number),"scope":"local native AMD64 stub match only, no target process query"})
}

pub(super) fn sample(process: HANDLE, pid: u32, tid: u32, born: u64) -> Value {
    if !native_x64(process) || !native_x64(unsafe { GetCurrentProcess() }) {
        return json!({"error":"unsupported_architecture"});
    }
    let raw = unsafe {
        OpenThread(
            THREAD_QUERY_INFORMATION | THREAD_GET_CONTEXT | THREAD_SYNCHRONIZE,
            0,
            tid,
        )
    };
    if raw.is_null() {
        return json!({"error":"thread_query_unavailable"});
    }
    let held = super::OwnedHandle(raw);
    if !super::thread_matches(held.0, pid, born) {
        return json!({"error":"thread_changed"});
    }
    let mut info = LastCall::default();
    let mut returned = 0;
    // Class and output type/size are inseparable here. Modern native-AMD64 phnt
    // layout is 24 bytes; all fields are plain integers and start initialized.
    let status = unsafe {
        NtQueryInformationThread(
            held.0,
            ThreadLastSystemCall,
            (&mut info as *mut LastCall).cast(),
            size_of::<LastCall>() as u32,
            &mut returned,
        )
    };
    let at_ms = super::super::epoch_ms();
    if !super::thread_matches(held.0, pid, born) {
        return json!({"error":"thread_changed"});
    }
    if status != 0 || returned != size_of::<LastCall>() as u32 {
        return json!({"error":"query_unavailable","ntstatus":status as u32,"returned_bytes":returned});
    }
    json!({"at_ms":at_ms,"number":info.number,"wait_100ns":info.wait_ticks,"local_stub_symbol":symbol_for(info.number),
        "scope":"recorded last syscall and local stub match; not proof this call currently blocks or names a source function"})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        mem::offset_of,
        sync::mpsc,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME, WAIT_OBJECT_0},
        System::Threading::{
            CreateEventW, GetCurrentThread, GetCurrentThreadId, GetThreadTimes, SetEvent,
            WaitForSingleObject,
        },
    };
    struct Event {
        raw: HANDLE,
        thread: Option<std::thread::JoinHandle<u32>>,
    }
    impl Drop for Event {
        fn drop(&mut self) {
            unsafe { SetEvent(self.raw) };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            unsafe { CloseHandle(self.raw) };
        }
    }
    #[test]
    fn fixed_native_layout_and_complete_stub_match_reject_wrong_or_truncated_code() {
        assert_eq!(size_of::<LastCall>(), 24);
        assert_eq!(offset_of!(LastCall, number), 8);
        assert_eq!(offset_of!(LastCall, wait_ticks), 16);
        let mut bytes = [
            0x4c, 0x8b, 0xd1, 0xb8, 0x34, 0x12, 0, 0, 0xf6, 0x04, 0x25, 0x08, 0x03, 0xfe, 0x7f,
            0x01, 0x75, 0x03, 0x0f, 0x05, 0xc3, 0xcd, 0x2e, 0xc3,
        ];
        assert_eq!(stub_number(&bytes), Some(0x1234));
        assert_eq!(stub_number(&bytes[..23]), None);
        bytes[17] = 4;
        assert_eq!(stub_number(&bytes), None);
        bytes[17] = 3;
        bytes[0] = 0xe9;
        assert_eq!(stub_number(&bytes), None);
    }
    #[test]
    fn owned_live_native_wait_is_sampled_without_suspend_and_wrong_identity_is_rejected() {
        let raw = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) };
        assert!(!raw.is_null());
        let mut event = Event { raw, thread: None };
        let (sender, receiver) = mpsc::channel();
        let raw_bits = raw as usize;
        event.thread = Some(std::thread::spawn(move || {
            let (mut born, mut exit, mut kernel, mut user) = (
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
            );
            assert_ne!(
                unsafe {
                    GetThreadTimes(
                        GetCurrentThread(),
                        &mut born,
                        &mut exit,
                        &mut kernel,
                        &mut user,
                    )
                },
                0
            );
            sender
                .send((unsafe { GetCurrentThreadId() }, super::super::ticks(born)))
                .unwrap();
            unsafe { WaitForSingleObject(raw_bits as HANDLE, 5000) }
        }));
        let (tid, born) = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        let process = unsafe { GetCurrentProcess() };
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut observed = false;
        while Instant::now() < deadline {
            let value = sample(process, std::process::id(), tid, born);
            if value["local_stub_symbol"] == "NtWaitForSingleObject"
                && value["wait_100ns"].as_u64().is_some_and(|n| n > 0)
            {
                observed = true;
                break;
            }
            std::thread::yield_now();
        }
        assert!(
            observed,
            "owned blocked thread had no recognized native last-call record"
        );
        assert_eq!(
            sample(process, std::process::id() + 1, tid, born)["error"],
            "thread_changed"
        );
        assert_eq!(
            sample(process, std::process::id(), tid, born + 1)["error"],
            "thread_changed"
        );
        unsafe { SetEvent(raw) };
        assert_eq!(event.thread.take().unwrap().join().unwrap(), WAIT_OBJECT_0);
    }
}
