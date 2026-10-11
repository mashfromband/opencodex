use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}
// Only the new owned child's loopback port; never a production URL.
pub fn probe(port: u16, pid: u32, budget: Duration) -> Value {
    let start_ms = now_ms();
    let started = Instant::now();
    let left = || {
        budget
            .checked_sub(started.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or(std::io::ErrorKind::TimedOut)
    };
    let result = (|| -> std::io::Result<bool> {
        let mut socket =
            TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), left()?)?;
        socket.set_write_timeout(Some(left()?))?;
        socket
            .write_all(b"GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")?;
        let mut bytes = Vec::new();
        loop {
            socket.set_read_timeout(Some(left()?))?;
            let mut buffer = [0; 1024];
            let count = socket.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            if bytes.len() + count > 4096 {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        let split = bytes
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .ok_or(std::io::ErrorKind::InvalidData)?;
        let head =
            std::str::from_utf8(&bytes[..split]).map_err(|_| std::io::ErrorKind::InvalidData)?;
        let value: Value = serde_json::from_slice(&bytes[split + 4..])
            .map_err(|_| std::io::ErrorKind::InvalidData)?;
        Ok(head
            .lines()
            .next()
            .and_then(|line| line.split_ascii_whitespace().nth(1))
            == Some("200")
            && value["pid"] == pid
            && value["status"] == "ok")
    })();
    json!({"start_ms":start_ms,"end_ms":now_ms(),"elapsed_ms":started.elapsed().as_millis(),
        "healthy":result.as_ref().copied().unwrap_or(false),"error":result.err().map(|e|format!("{:?}",e.kind()))})
}
