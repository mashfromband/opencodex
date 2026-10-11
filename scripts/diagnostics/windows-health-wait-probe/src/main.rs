#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod win;
use serde_json::{Value, json};
use std::os::windows::process::CommandExt;
use std::{
    env,
    fs::OpenOptions,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}
struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn snapshot(pid: u32, born: u64, budget: Duration) -> Value {
    let outcome = (|| -> std::io::Result<Option<Vec<u8>>> {
        let mut child = Worker(
            Command::new(env::current_exe()?)
                .args(["--snapshot", &pid.to_string(), &born.to_string()])
                .creation_flags(0x08000000)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .stdout(Stdio::piped())
                .spawn()?,
        );
        let stdout = child.0.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.take(65537).read_to_end(&mut bytes).map(|_| bytes)
        });
        let deadline = Instant::now() + budget;
        let ok = loop {
            if let Some(exit) = child.0.try_wait()? {
                break exit.success();
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        drop(child);
        let bytes = reader
            .join()
            .map_err(|_| std::io::Error::other("worker_reader"))??;
        Ok((ok && bytes.len() <= 65536).then_some(bytes))
    })();
    match outcome {
        Ok(Some(bytes)) => {
            serde_json::from_slice(&bytes).unwrap_or(json!({"error":"worker_schema"}))
        }
        _ => json!({"error":"worker_timeout_or_failure"}),
    }
}

fn probe(port: u16, pid: u32, timeout: Duration) -> Value {
    let start_ms = epoch_ms();
    let started = Instant::now();
    let mut received_ms = None;
    let mut sent_ms = None;
    let outcome = (|| -> std::io::Result<bool> {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let mut socket = TcpStream::connect_timeout(&address, timeout)?;
        let left = || {
            timeout
                .checked_sub(started.elapsed())
                .filter(|d| !d.is_zero())
                .ok_or(std::io::ErrorKind::TimedOut)
        };
        let request =
            format!("GET /healthz HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        let mut pending = request.as_bytes();
        while !pending.is_empty() {
            socket.set_write_timeout(Some(left()?))?;
            let written = socket.write(pending)?;
            if written == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            pending = &pending[written..];
        }
        sent_ms = Some(epoch_ms());
        let mut bytes = Vec::new();
        loop {
            socket.set_read_timeout(Some(left()?))?;
            let mut chunk = [0; 1024];
            let count = socket.read(&mut chunk)?;
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.len() > 16384 {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            if count == 0 {
                received_ms = Some(epoch_ms());
                break;
            }
            if let Some(split) = bytes.windows(4).position(|x| x == b"\r\n\r\n") {
                let header = std::str::from_utf8(&bytes[..split])
                    .map_err(|_| std::io::ErrorKind::InvalidData)?;
                let lengths: Vec<_> = header
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .filter(|(n, _)| n.eq_ignore_ascii_case("content-length"))
                    .collect();
                if lengths.len() > 1
                    || header
                        .lines()
                        .any(|s| s.to_ascii_lowercase().starts_with("transfer-encoding:"))
                {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                if let Some((_, length)) = lengths.first() {
                    let length = length
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| std::io::ErrorKind::InvalidData)?;
                    if length > 16384 {
                        return Err(std::io::ErrorKind::InvalidData.into());
                    }
                    if bytes.len() == split + 4 + length {
                        received_ms = Some(epoch_ms());
                        break;
                    }
                }
            }
        }
        let split = bytes
            .windows(4)
            .position(|x| x == b"\r\n\r\n")
            .ok_or(std::io::ErrorKind::InvalidData)?;
        let header =
            std::str::from_utf8(&bytes[..split]).map_err(|_| std::io::ErrorKind::InvalidData)?;
        let body: Value = serde_json::from_slice(&bytes[split + 4..])
            .map_err(|_| std::io::ErrorKind::InvalidData)?;
        Ok(header
            .lines()
            .next()
            .and_then(|l| l.split_ascii_whitespace().nth(1))
            == Some("200")
            && body["pid"] == pid
            && body["service"] == "opencodex"
            && body["status"] == "ok")
    })();
    json!({"start_ms":start_ms,"end_ms":epoch_ms(),"sent_ms":sent_ms,"received_ms":received_ms,"elapsed_ms":started.elapsed().as_millis(),
        "healthy":outcome.as_ref().copied().unwrap_or(false),"error":outcome.err().map(|e|format!("{:?}",e.kind()))})
}

fn within_response_wait(health: &Value, at: u64) -> bool {
    health["sent_ms"].as_u64().is_some_and(|sent| {
        health["received_ms"]
            .as_u64()
            .or_else(|| health["end_ms"].as_u64())
            .is_some_and(|end| sent <= at && at < end)
    })
}

fn during_probe(
    port: u16,
    pid: u32,
    timeout: Duration,
    thresholds: &[Duration],
    mut capture: impl FnMut() -> Value,
) -> (Value, Vec<Value>) {
    let started = Instant::now();
    let (sender, receiver) = mpsc::channel();
    let network = std::thread::spawn(move || {
        let _ = sender.send(probe(port, pid, timeout));
    });
    let mut captures = Vec::new();
    for threshold in thresholds {
        match receiver.recv_timeout(threshold.saturating_sub(started.elapsed())) {
            Ok(value) => {
                let _ = network.join();
                return (value, captures);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = network.join();
                return (json!({"error":"network_worker"}), captures);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let began = epoch_ms();
        let value = capture();
        captures.push(
            json!({"capture_started_ms":began,"capture_ended_ms":epoch_ms(),"snapshot":value}),
        );
    }
    let value = receiver.recv().unwrap_or(json!({"error":"network_worker"}));
    let _ = network.join();
    (value, captures)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "--resolve-syscall" {
        println!("{}", win::resolve_local_syscall(args[1].parse()?));
        return Ok(());
    }
    if args.len() == 3 && args[0] == "--snapshot" {
        let process = win::Process::open(args[1].parse()?)?;
        if process.born != args[2].parse::<u64>()? || !process.live() {
            return Err("identity_changed".into());
        }
        println!("{}", process.wait_snapshot());
        return Ok(());
    }
    if args.len() != 5 {
        return Err("PID BORN PORT SECONDS NEW_OUTPUT".into());
    }
    let (pid, born, port, seconds) = (
        args[0].parse::<u32>()?,
        args[1].parse::<u64>()?,
        args[2].parse::<u16>()?,
        args[3].parse::<u64>()?,
    );
    if pid == 0 || port == 0 || !(1..=1800).contains(&seconds) {
        return Err("invalid_bounds".into());
    }
    let process = win::Process::open(pid)?;
    if process.born != born || !process.live() {
        return Err("identity_changed".into());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[4])?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let (mut total, mut delayed, mut timeout_count) = (0, 0, 0);
    while Instant::now() < deadline && process.live() {
        let start = Instant::now();
        let before = process.metrics();
        let timeout = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(20));
        let (health, captures) = during_probe(
            port,
            pid,
            timeout,
            &[
                Duration::from_millis(100),
                Duration::from_secs(5),
                Duration::from_secs(10),
            ],
            || snapshot(pid, born, Duration::from_millis(1200)),
        );
        total += 1;
        if !captures.is_empty() {
            delayed += 1
        }
        if health["healthy"] != true {
            timeout_count += 1
        }
        if !captures.is_empty() || total % 30 == 1 || health["healthy"] != true {
            // Match actual snapshot time against the network end, including races.
            let end = health["received_ms"]
                .as_u64()
                .or_else(|| health["end_ms"].as_u64())
                .unwrap_or(0);
            let mut captures = captures;
            for c in &mut captures {
                c["during_health_wait"] = json!(c["snapshot"]["at_ms"].as_u64().is_some_and(
                    |at| at < end && at >= health["start_ms"].as_u64().unwrap_or(u64::MAX)
                ));
                c["during_response_wait"] = json!(
                    c["snapshot"]["at_ms"]
                        .as_u64()
                        .is_some_and(|at| within_response_wait(&health, at))
                );
            }
            writeln!(
                output,
                "{}",
                json!({"kind":"observation","pid":pid,"born":born,"health":health,"before":before,"after":process.metrics(),"captures":captures,"target_live":process.live()})
            )?;
            output.flush()?;
        }
        std::thread::sleep(
            Duration::from_secs(2)
                .saturating_sub(start.elapsed())
                .min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    writeln!(
        output,
        "{}",
        json!({"kind":"finished","pid":pid,"born":born,"total":total,"delayed":delayed,"not_healthy":timeout_count,"target_live":process.live(),"at_ms":epoch_ms()})
    )?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_wait_excludes_connect_and_completed_reply_boundaries() {
        let healthy = json!({"start_ms":10,"sent_ms":20,"received_ms":40,"end_ms":45});
        assert!(!within_response_wait(&healthy, 10));
        assert!(!within_response_wait(&healthy, 19));
        assert!(within_response_wait(&healthy, 20));
        assert!(within_response_wait(&healthy, 39));
        assert!(!within_response_wait(&healthy, 40));
        assert!(!within_response_wait(&healthy, 45));
        assert!(!within_response_wait(
            &json!({"start_ms":10,"sent_ms":null,"end_ms":45}),
            30
        ));
        assert!(within_response_wait(
            &json!({"sent_ms":20,"received_ms":null,"end_ms":45}),
            44
        ));
        assert!(!within_response_wait(
            &json!({"sent_ms":20,"received_ms":null,"end_ms":45}),
            45
        ));
    }
    fn listener(delay: Duration) -> (u16, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0; 1024];
            let mut request = Vec::new();
            while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                let n = s.read(&mut buf).unwrap();
                assert!(n > 0 && request.len() + n <= 8192);
                request.extend_from_slice(&buf[..n]);
            }
            std::thread::sleep(delay);
            let body = r#"{"pid":77,"service":"opencodex","status":"ok"}"#;
            let _ = write!(
                s,
                "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
        });
        (port, server)
    }
    #[test]
    fn capture_overlaps_real_delayed_health_and_success_is_not_timeout() {
        let (port, server) = listener(Duration::from_millis(180));
        let mut count = 0;
        let (health, captures) = during_probe(
            port,
            77,
            Duration::from_secs(1),
            &[Duration::from_millis(25), Duration::from_millis(75)],
            || {
                count += 1;
                json!({"at_ms":epoch_ms()})
            },
        );
        server.join().unwrap();
        assert_eq!(health["healthy"], true, "{health}");
        assert_eq!(count, 2);
        assert_eq!(captures.len(), 2);
        for c in captures {
            assert!(
                c["snapshot"]["at_ms"].as_u64().unwrap() < health["received_ms"].as_u64().unwrap()
            );
        }
    }
    #[test]
    fn genuine_deadline_collects_while_waiting_then_drains_network_worker() {
        let (port, server) = listener(Duration::from_millis(300));
        let (health, captures) = during_probe(
            port,
            77,
            Duration::from_millis(100),
            &[Duration::from_millis(25)],
            || json!({"at_ms":epoch_ms()}),
        );
        server.join().unwrap();
        assert_eq!(health["healthy"], false);
        assert_eq!(captures.len(), 1);
        assert!(health["elapsed_ms"].as_u64().unwrap() >= 100);
    }
}
