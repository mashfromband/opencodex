//! Reuses the repository's native-main-owner child unchanged. Its synthetic hosts
//! are intercepted inside that child; this experiment excludes native fetch transport.
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn request(port: u16, body: &[u8]) -> std::io::Result<Option<u16>> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    write!(
        stream,
        "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAuthorization: Bearer synthetic-fixture-only\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    let mut sent = 0;
    while sent < body.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        stream.set_write_timeout(Some(left))?;
        let n = stream.write(&body[sent..])?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        sent += n;
    }
    let mut status = Vec::new();
    let mut chunk = [0; 8192];
    let mut response_bytes = 0;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(left))?;
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if !status.contains(&b'\n') {
            status.extend_from_slice(&chunk[..n.min(512)]);
        }
        response_bytes += n;
        if response_bytes > 1024 * 1024 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
    }
    Ok(super::http_status(&status))
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 6 {
        return Err("--isolated BUN_EXE REPO ROOT NODE_MODULES BODY_MIB CONCURRENCY".into());
    }
    let mib: usize = args[4].parse()?;
    let concurrent: usize = args[5].parse()?;
    if !(1..=32).contains(&mib) || !(1..=40).contains(&concurrent) || mib * concurrent > 640 {
        return Err("bounded pressure limit exceeded".into());
    }
    let root = Path::new(&args[2]);
    fs::create_dir(root)?;
    let codex = root.join("codex");
    let home = root.join("opencodex");
    fs::create_dir(&codex)?;
    fs::create_dir(&home)?;
    // Every home is private to this child; no real account or config is copied.
    fs::write(
        codex.join("config.toml"),
        "cli_auth_credentials_store = \"file\"\n",
    )?;
    fs::write(
        home.join("config.json"),
        serde_json::to_vec_pretty(&json!({
            "port":0,"hostname":"127.0.0.1","defaultProvider":"direct",
            "providers":{"direct":{"adapter":"openai-chat","baseUrl":"https://direct.example.com/v1","authMode":"forward"}},
            "codexAccounts":[],"autoSwitchThreshold":0
        }))?,
    )?;
    let system_root = std::env::var_os("SystemRoot").ok_or("missing Windows system root")?;
    let mut child = OwnedChild(
        Command::new(&args[0])
            .env_clear()
            .env("SystemRoot", &system_root)
            .env("WINDIR", &system_root)
            .env("PATH", Path::new(&system_root).join("System32"))
            .env("TEMP", root)
            .env("TMP", root)
            .arg("--cpu-prof")
            .arg(format!("--cpu-prof-dir={}", root.display()))
            .arg("--cpu-prof-name=cpu.cpuprofile")
            .arg(Path::new(&args[1]).join("tests/helpers/native-main-owner-child.ts"))
            .current_dir(&args[1])
            .creation_flags(0x08000000)
            .env("NODE_PATH", &args[3])
            .env("HOME", root)
            .env("USERPROFILE", root)
            .env("APPDATA", root.join("appdata"))
            .env("LOCALAPPDATA", root.join("localappdata"))
            .env("CODEX_HOME", &codex)
            .env("OPENCODEX_HOME", &home)
            .env("NATIVE_OWNER_CODEX_HOME", &codex)
            .env("NATIVE_OWNER_CONFIG_DIR", &home)
            .env(
                "NATIVE_OWNER_KEY",
                "XFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFw=",
            )
            .env("CODEX_CI", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let output = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            if let Some(line) = line.strip_prefix("@@native-owner@@")
                && let Ok(row) = serde_json::from_str::<Value>(line)
            {
                let _ = tx.send(row);
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(90);
    let port = loop {
        if let Some(status) = child.0.try_wait()? {
            return Err(format!("fixture exited: {status}").into());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("fixture startup deadline".into());
        }
        if let Ok(row) = rx.recv_timeout(left.min(Duration::from_millis(500)))
            && row["event"] == "listening"
        {
            break row["port"].as_u64().ok_or("invalid port")? as u16;
        }
    };
    let pid = child.0.id();
    let process = Arc::new(super::win::Process::open(pid)?);
    let born = process.born;
    // One connection proves the fixture's own identity before pressure is generated.
    let health = super::probe(port, pid, Duration::from_secs(5));
    if health["identity_matches"] != true {
        return Err("fixture identity not verified".into());
    }
    let before = process.metrics();
    let mut body = b"{\"model\":\"direct/direct-model\",\"input\":\"".to_vec();
    body.extend(std::iter::repeat_n(b'x', mib * 1024 * 1024));
    body.extend_from_slice(b"\",\"stream\":true}");
    let body = Arc::new(body);
    let wire_bytes = body.len();
    let complete = Arc::new(AtomicBool::new(false));
    let signal = complete.clone();
    let metrics_file = root.join("health-and-memory.jsonl");
    let sampled_process = Arc::clone(&process);
    let sampler = thread::spawn(move || -> std::io::Result<(u64, u64)> {
        let mut file = File::create(metrics_file)?;
        let mut samples = 0;
        let mut max_ms = 0;
        while !signal.load(Ordering::Acquire) && sampled_process.live() {
            let health = super::probe(port, pid, Duration::from_secs(3));
            max_ms = max_ms.max(health["elapsed_ms"].as_u64().unwrap_or(0));
            samples += 1;
            writeln!(
                file,
                "{}",
                json!({"at_ms":super::epoch_ms(),"pid":pid,"born":born,"probe":health,"metrics":sampled_process.metrics()})
            )?;
            thread::sleep(Duration::from_millis(50));
        }
        Ok((samples, max_ms))
    });
    let start = Instant::now();
    let mut clients = Vec::new();
    for _ in 0..concurrent {
        let body = body.clone();
        clients.push(thread::spawn(move || {
            request(port, &body)
                .map(|s| json!({"status":s}))
                .unwrap_or_else(|err| json!({"error_kind":format!("{:?}",err.kind())}))
        }));
    }
    let results: Vec<_> = clients.into_iter().map(|c| c.join().unwrap()).collect();
    let duration_ms = start.elapsed().as_millis();
    thread::sleep(Duration::from_secs(2));
    complete.store(true, Ordering::Release);
    let (samples, max_probe_ms) = sampler.join().map_err(|_| "sampler panicked")??;
    if !process.live() {
        return Err("fixture exited during measurement".into());
    }
    let result = json!({"pid":pid,"born":born,"port":port,"wire_bytes":wire_bytes,"concurrency":concurrent,"requests":results,"duration_ms":duration_ms,"health_samples":samples,"max_probe_ms":max_probe_ms,"before":before,"after":process.metrics(),"native_fetch_included":false});
    fs::write(
        root.join("result.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    super::emit(result);
    // Ask this fixture to finish so Bun can publish its CPU profile; fallback
    // termination is confined to the owned child handle.
    if let Some(stdin) = child.0.stdin.as_mut() {
        let _ = writeln!(stdin, "{{\"op\":\"stop\",\"id\":\"benchmark-stop\"}}");
    }
    let stop_deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < stop_deadline && child.0.try_wait()?.is_none() {
        thread::sleep(Duration::from_millis(50));
    }
    drop(child);
    output.join().map_err(|_| "stdout reader panicked")?;
    Ok(())
}
