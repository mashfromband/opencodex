#![cfg(windows)]
#[allow(dead_code)]
#[path = "../src/win.rs"]
mod win;
use std::{
    io::{BufRead, BufReader, Read},
    os::windows::process::CommandExt,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{
        Diagnostics::Debug::{
            CloseThreadWaitChainSession, GetThreadWaitChain, OpenThreadWaitChainSession,
            WAITCHAIN_NODE_INFO, WCT_MAX_NODE_COUNT, WctThreadType,
        },
        Threading::{CreateMutexW, ReleaseMutex},
    },
};
fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}
struct Mutex(HANDLE);
impl Drop for Mutex {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}
struct Fixture(Child);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn spawn_fixture(args: &[&str]) -> (Fixture, u32) {
    let mut fixture = Fixture(
        Command::new(env!("CARGO_BIN_EXE_wct-contract-fixture"))
            .args(args)
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = fixture.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout.take(1025))
            .read_line(&mut line)
            .map(|_| line);
        let _ = sender.send(result);
    });
    let line = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("fixture ready deadline")
        .unwrap();
    reader.join().unwrap();
    let identity: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(identity["pid"], fixture.0.id());
    let tid = identity["tid"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .unwrap();
    (fixture, tid)
}
fn await_exit(fixture: &mut Fixture) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let exit = loop {
        if let Some(exit) = fixture.0.try_wait().unwrap() {
            break exit;
        }
        assert!(Instant::now() < deadline, "fixture exit deadline");
        std::thread::yield_now();
    };
    assert!(exit.success());
}
#[test]
fn nested_foreign_wait_chain_crosses_both_owned_process_boundaries() {
    let name = format!(
        "Local\\ocx-wct-contract-{}-{}",
        std::process::id(),
        epoch_ms()
    );
    let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
    let mutex = Mutex(unsafe { CreateMutexW(core::ptr::null(), 1, wide.as_ptr()) });
    assert!(!mutex.0.is_null());
    let intermediate = format!("{name}-intermediate");
    let (mut holder, _) = spawn_fixture(&[&name, &intermediate]);
    let (mut fixture, tid) = spawn_fixture(&[&intermediate]);
    let session = win::WaitChainSession::open().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut reached = false;
    // The mutex is the barrier. Both children remain waiting until this owner
    // releases it, independent of machine speed. Do not infer readiness by sleep.
    while Instant::now() < deadline {
        let (nodes, cycle) = session.query(tid).unwrap();
        assert!(!cycle);
        reached = nodes.iter().any(|node| {
            node.ObjectType == WctThreadType
                && unsafe { node.Anonymous.ThreadObject.ProcessId } == std::process::id()
        });
        if reached {
            break;
        }
        std::thread::yield_now();
    }
    assert!(reached, "wait chain did not cross both process boundaries");
    let old_session = unsafe { OpenThreadWaitChainSession(0, None) };
    assert!(!old_session.is_null());
    let mut old_count = WCT_MAX_NODE_COUNT;
    let mut old_nodes = [WAITCHAIN_NODE_INFO::default(); WCT_MAX_NODE_COUNT as usize];
    let mut old_cycle = 0;
    let old_result = unsafe {
        GetThreadWaitChain(
            old_session,
            0,
            0,
            tid,
            &mut old_count,
            old_nodes.as_mut_ptr(),
            &mut old_cycle,
        )
    };
    unsafe { CloseThreadWaitChainSession(old_session) };
    assert_ne!(old_result, 0);
    assert_eq!(old_cycle, 0);
    let old_pids: Vec<_> = old_nodes
        .iter()
        .take(old_count.min(WCT_MAX_NODE_COUNT) as usize)
        .filter(|node| node.ObjectType == WctThreadType)
        .map(|node| unsafe { node.Anonymous.ThreadObject.ProcessId })
        .collect();
    assert!(old_pids.contains(&fixture.0.id()));
    assert!(old_pids.contains(&holder.0.id()));
    assert!(
        !old_pids.contains(&std::process::id()),
        "zero flags unexpectedly crossed the second boundary"
    );
    drop(mutex);
    await_exit(&mut fixture);
    await_exit(&mut holder);
}
