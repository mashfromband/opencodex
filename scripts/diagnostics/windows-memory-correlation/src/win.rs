use serde_json::{Value, json};
use std::{
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows_sys::Win32::{
    Foundation::{FILETIME, WAIT_TIMEOUT},
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_CONNECTIONS,
    },
    Networking::WinSock::AF_INET,
    System::{
        ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        },
        Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SYNCHRONIZE,
            PROCESS_VM_READ, QueryFullProcessImageNameW, WaitForSingleObject,
        },
    },
};

pub struct Process {
    handle: OwnedHandle,
    pub born: u64,
    pid: u32,
}
fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}
impl Process {
    pub fn open(pid: u32, expected_born: u64) -> Result<Self, &'static str> {
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return Err("process_open_failed");
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let (mut born, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if unsafe { GetProcessTimes(raw, &mut born, &mut exit, &mut kernel, &mut user) } == 0
            || ticks(born) != expected_born
        {
            return Err("process_identity_changed");
        }
        let mut path = [0u16; 32768];
        let mut length = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(raw, 0, path.as_mut_ptr(), &mut length) } == 0
            || !String::from_utf16_lossy(&path[..length as usize])
                .to_ascii_lowercase()
                .ends_with("\\bun.exe")
        {
            return Err("process_image_mismatch");
        }
        let process = Self {
            handle,
            born: ticks(born),
            pid,
        };
        if !process.live() {
            return Err("process_exited");
        }
        Ok(process)
    }
    pub fn live(&self) -> bool {
        unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) == WAIT_TIMEOUT }
    }
    pub fn connected_peer_matches(&self, port: u16, local: std::net::SocketAddr) -> bool {
        if !self.live() || !local.is_ipv4() || !local.ip().is_loopback() {
            return false;
        }
        let mut needed = 0;
        unsafe {
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                &mut needed,
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_CONNECTIONS,
                0,
            );
        }
        for _ in 0..3 {
            if !(4..=16 * 1024 * 1024).contains(&needed) {
                return false;
            }
            let mut buffer = vec![0u8; needed as usize];
            let code = unsafe {
                GetExtendedTcpTable(
                    buffer.as_mut_ptr().cast(),
                    &mut needed,
                    0,
                    AF_INET as u32,
                    TCP_TABLE_OWNER_PID_CONNECTIONS,
                    0,
                )
            };
            if code == 122 {
                continue;
            }
            if code != 0 || needed < 4 || needed as usize > buffer.len() {
                return false;
            }
            let count = u32::from_ne_bytes(buffer[..4].try_into().unwrap()) as usize;
            let stride = size_of::<MIB_TCPROW_OWNER_PID>();
            if count > (needed as usize - 4) / stride {
                return false;
            }
            for index in 0..count {
                let row = unsafe {
                    std::ptr::read_unaligned(
                        buffer
                            .as_ptr()
                            .add(4 + index * stride)
                            .cast::<MIB_TCPROW_OWNER_PID>(),
                    )
                };
                if connection_row_matches(&row, self.pid, port, local.port()) {
                    return self.live();
                }
            }
            return false;
        }
        false
    }
    pub fn metrics(&self) -> Value {
        let mut m = PROCESS_MEMORY_COUNTERS_EX {
            cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        let raw = self.handle.as_raw_handle();
        if unsafe {
            GetProcessMemoryInfo(
                raw,
                (&mut m as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
                size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            )
        } == 0
        {
            return json!({"error":"metrics_unavailable"});
        }
        let (mut born, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let cpu =
            if unsafe { GetProcessTimes(raw, &mut born, &mut exit, &mut kernel, &mut user) } != 0 {
                Some(ticks(kernel) + ticks(user))
            } else {
                None
            };
        json!({"privateBytes":m.PrivateUsage,"workingSet":m.WorkingSetSize,"cpu100ns":cpu})
    }
}

fn connection_row_matches(
    row: &MIB_TCPROW_OWNER_PID,
    pid: u32,
    port: u16,
    local_port: u16,
) -> bool {
    let loopback = u32::from_ne_bytes([127, 0, 0, 1]);
    row.dwOwningPid == pid
        && row.dwState == 5
        && row.dwLocalAddr == loopback
        && row.dwRemoteAddr == loopback
        && u16::from_be(row.dwLocalPort as u16) == port
        && u16::from_be(row.dwRemotePort as u16) == local_port
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn server_connection_tuple_must_match_every_owner_field() {
        let mut row = MIB_TCPROW_OWNER_PID {
            dwOwningPid: 77,
            dwState: 5,
            dwLocalAddr: u32::from_ne_bytes([127, 0, 0, 1]),
            dwRemoteAddr: u32::from_ne_bytes([127, 0, 0, 1]),
            dwLocalPort: 10100u16.to_be() as u32,
            dwRemotePort: 40000u16.to_be() as u32,
        };
        assert!(connection_row_matches(&row, 77, 10100, 40000));
        assert!(!connection_row_matches(&row, 78, 10100, 40000));
        assert!(!connection_row_matches(&row, 77, 10101, 40000));
        assert!(!connection_row_matches(&row, 77, 10100, 40001));
        row.dwState = 2;
        assert!(!connection_row_matches(&row, 77, 10100, 40000));
        row.dwState = 5;
        row.dwLocalAddr = 0;
        assert!(!connection_row_matches(&row, 77, 10100, 40000));
    }
}
