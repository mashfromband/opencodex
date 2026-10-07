#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use serde_json::{Value, json};
use std::{
    env,
    fs::OpenOptions,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
mod bench;
#[cfg(windows)]
mod win;

fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

fn emit(row: Value) {
    let _ = writeln!(std::io::stdout().lock(), "{row}");
}

struct OwnedWorker(std::process::Child);
impl Drop for OwnedWorker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn bounded_worker_output(
    mut command: std::process::Command,
    timeout: Duration,
) -> std::io::Result<Option<Vec<u8>>> {
    use std::process::Stdio;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = OwnedWorker(command.spawn()?);
    let stdout = child.0.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    let success = loop {
        if let Some(status) = child.0.try_wait()? {
            break status.success();
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(child);
    let bytes = reader
        .join()
        .map_err(|_| std::io::Error::other("snapshot reader panic"))??;
    Ok((success && bytes.len() <= 64 * 1024).then_some(bytes))
}

#[cfg(windows)]
fn bounded_snapshot(pid: u32, born: u64, timeout: Duration) -> Value {
    use std::os::windows::process::CommandExt;
    let result = (|| -> std::io::Result<Option<Vec<u8>>> {
        let mut command = std::process::Command::new(env::current_exe()?);
        command
            .args(["--snapshot", &pid.to_string(), &born.to_string()])
            .creation_flags(0x08000000);
        bounded_worker_output(command, timeout)
    })();
    match result {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"snapshot_error":"invalid_worker_result"})),
        Ok(None) => json!({"snapshot_error":"worker_timeout_or_failure"}),
        Err(error) => json!({"snapshot_error":format!("{:?}", error.kind())}),
    }
}

fn http_status(bytes: &[u8]) -> Option<u16> {
    let line = bytes.split(|&b| b == b'\n').next()?;
    if !line.ends_with(b"\r") {
        return None;
    }
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split_whitespace();
    if !matches!(parts.next()?, "HTTP/1.1" | "HTTP/1.0") {
        return None;
    }
    let status = parts.next()?;
    if status.len() != 3 {
        return None;
    }
    status.parse().ok()
}

fn health_identity(bytes: &[u8], pid: u32, port: u16) -> bool {
    let Some(split) = bytes.windows(4).position(|v| v == b"\r\n\r\n") else {
        return false;
    };
    let Ok(body) = serde_json::from_slice::<Value>(&bytes[split + 4..]) else {
        return false;
    };
    body["service"] == "opencodex"
        && body["status"] == "ok"
        && body["pid"] == pid
        && body["port"] == port
}

fn content_length_complete(bytes: &[u8]) -> bool {
    let Some(split) = bytes.windows(4).position(|v| v == b"\r\n\r\n") else {
        return false;
    };
    let Ok(headers) = std::str::from_utf8(&bytes[..split]) else {
        return false;
    };
    let length = headers.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    });
    length.is_some_and(|n| bytes.len().saturating_sub(split + 4) >= n)
}

fn probe(port: u16, pid: u32, timeout: Duration) -> Value {
    let start_at = epoch_ms();
    let start = Instant::now();
    let address: SocketAddr = ([127, 0, 0, 1], port).into();
    let result = (|| -> std::io::Result<(Option<u16>, bool)> {
        let mut stream = TcpStream::connect_timeout(&address, timeout)?;
        stream.set_write_timeout(Some(
            timeout
                .saturating_sub(start.elapsed())
                .max(Duration::from_millis(1)),
        ))?;
        write!(
            stream,
            "GET /healthz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        )?;
        let mut received = Vec::with_capacity(512);
        let mut chunk = [0; 512];
        while received.len() < 8192 {
            let left = timeout.saturating_sub(start.elapsed());
            if left.is_zero() {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            stream.set_read_timeout(Some(left))?;
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            received.extend_from_slice(&chunk[..n]);
            if content_length_complete(&received) {
                break;
            }
        }
        Ok((
            http_status(&received),
            health_identity(&received, pid, port),
        ))
    })();
    let (status, identity_matches, error) = match result {
        Ok((status, identity)) => (status, identity, None),
        Err(err) => (None, false, Some(format!("{:?}", err.kind()))),
    };
    json!({"start_ms":start_at,"end_ms":epoch_ms(),"elapsed_ms":start.elapsed().as_millis(),"status":status,"identity_matches":identity_matches,"error_kind":error})
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    #[cfg(windows)]
    if args.first().is_some_and(|arg| arg == "--snapshot") && args.len() == 3 {
        let process = win::Process::open(args[1].parse()?)?;
        if process.born != args[2].parse::<u64>()? || !process.live() {
            return Err("snapshot process identity changed".into());
        }
        emit(process.wait_snapshot());
        return Ok(());
    }
    #[cfg(windows)]
    if args.first().is_some_and(|arg| arg == "--isolated") {
        return bench::run(&args[1..]);
    }
    if args.len() != 4 {
        return Err("Usage: ocx-cause-probe PID PORT SECONDS OUTPUT_JSONL (read-only, fixed process identity)".into());
    }
    let target: u32 = args[0].parse()?;
    let port: u16 = args[1].parse()?;
    let seconds: u64 = args[2].parse()?;
    if target == 0 || port == 0 || !(1..=1800).contains(&seconds) {
        return Err("invalid bounded observation".into());
    }
    #[cfg(windows)]
    let process = win::Process::open(target)?;
    #[cfg(not(windows))]
    return Err("Windows observation only".into());
    #[cfg(windows)]
    {
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[3])?;
        let start = Instant::now();
        let mut next_snapshot = Duration::ZERO;
        let mut samples = 0;
        let mut late = 0;
        let deadline = start + Duration::from_secs(seconds);
        while Instant::now() < deadline {
            let begun = Instant::now();
            if !process.live() {
                writeln!(
                    out,
                    "{}",
                    json!({"kind":"process_exited","at_ms":epoch_ms(),"pid":target})
                )?;
                break;
            }
            let health = probe(
                port,
                target,
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(1500)),
            );
            let delayed = health["status"] != 200
                || health["identity_matches"] != true
                || health["elapsed_ms"].as_u64().unwrap_or(0) >= 250;
            if delayed {
                late += 1;
            }
            let capture = delayed || start.elapsed() >= next_snapshot;
            let metrics = process.metrics();
            let snapshot = if capture {
                next_snapshot = start.elapsed() + Duration::from_secs(60);
                bounded_snapshot(
                    target,
                    process.born,
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(1200)),
                )
            } else {
                Value::Null
            };
            writeln!(
                out,
                "{}",
                json!({"kind":"observation","pid":target,"born":process.born,"probe":health,"metrics":metrics,"wait_snapshot":snapshot})
            )?;
            out.flush()?;
            samples += 1;
            if capture {
                emit(json!({"samples":samples,"late":late,"elapsed_s":start.elapsed().as_secs()}));
            }
            std::thread::sleep(
                Duration::from_millis(1000)
                    .saturating_sub(begun.elapsed())
                    .min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        emit(
            json!({"kind":"finished","samples":samples,"late":late,"elapsed_s":start.elapsed().as_secs()}),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};

    #[cfg(windows)]
    #[test]
    fn exit_259_fixture() {
        if env::var_os("OCX_CAUSE_PROBE_EXIT_FIXTURE").is_some() {
            std::process::exit(259);
        }
    }

    #[cfg(windows)]
    #[test]
    fn exit_code_259_is_not_a_live_process() {
        use std::os::windows::{io::AsRawHandle, process::CommandExt};
        use std::process::{Command, Stdio};
        let mut child = Command::new(env::current_exe().unwrap())
            .args(["--exact", "tests::exit_259_fixture", "--nocapture"])
            .env("OCX_CAUSE_PROBE_EXIT_FIXTURE", "1")
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(259));
        assert!(!win::handle_live(child.as_raw_handle()));
    }

    #[test]
    fn snapshot_block_fixture() {
        if env::var_os("OCX_CAUSE_PROBE_BLOCK_FIXTURE").is_some() {
            std::thread::sleep(Duration::from_secs(30));
        }
    }

    #[test]
    fn stalled_snapshot_worker_is_bounded_and_reaped() {
        let mut command = std::process::Command::new(env::current_exe().unwrap());
        command
            .args(["--exact", "tests::snapshot_block_fixture", "--nocapture"])
            .env("OCX_CAUSE_PROBE_BLOCK_FIXTURE", "1");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let start = Instant::now();
        assert!(
            bounded_worker_output(command, Duration::from_millis(250))
                .unwrap()
                .is_none()
        );
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn status_requires_complete_http_line() {
        assert_eq!(http_status(b"HTTP/1.1 200 OK\r\nbody"), Some(200));
        assert_eq!(http_status(b"HTTP/1.1 503 Unavailable\r\n"), Some(503));
        assert_eq!(http_status(b"HTTP/1.1 200 OK"), None);
        assert_eq!(http_status(b"not HTTP 200\r\n"), None);
    }

    #[test]
    fn probe_reads_fragmented_status_line() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                s.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 4096);
            }
            s.write_all(b"HTTP/1.1 2").unwrap();
            thread::sleep(Duration::from_millis(15));
            let body = format!(
                "{{\"service\":\"opencodex\",\"status\":\"ok\",\"pid\":77,\"port\":{port}}}"
            );
            write!(s, "00 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        });
        let result = probe(port, 77, Duration::from_secs(2));
        assert_eq!(result["status"], 200);
        assert_eq!(result["identity_matches"], true);
        task.join().unwrap();
    }

    #[test]
    fn partial_drip_does_not_extend_absolute_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            for _ in 0..60 {
                if s.write_all(b"H").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(15));
            }
        });
        let start = Instant::now();
        let result = probe(port, 77, Duration::from_millis(120));
        assert!(result["status"].is_null());
        assert!(start.elapsed() < Duration::from_millis(700));
        task.join().unwrap();
    }

    #[test]
    fn foreign_healthy_process_does_not_match_target() {
        let data = b"HTTP/1.1 200 OK\r\n\r\n{\"service\":\"opencodex\",\"status\":\"ok\",\"pid\":2,\"port\":3}";
        assert!(!health_identity(data, 1, 3));
        assert!(!health_identity(data, 2, 4));
        assert!(health_identity(data, 2, 3));
    }
}
