use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[allow(dead_code)]
#[path = "../../windows-cause-probe/src/win.rs"]
mod win;

fn at_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}
fn epoch_ms() -> u128 {
    at_ms()
}
fn remaining(deadline: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(io::ErrorKind::TimedOut.into())
    } else {
        Ok(left)
    }
}
fn status(bytes: &[u8]) -> Option<u16> {
    std::str::from_utf8(bytes.split(|b| *b == b'\n').next()?)
        .ok()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}
fn get(port: u16, path: &str, timeout: Duration) -> io::Result<(u16, Value)> {
    let deadline = Instant::now() + timeout;
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut socket = TcpStream::connect_timeout(&addr, remaining(deadline)?)?;
    socket.set_write_timeout(Some(remaining(deadline)?))?;
    write!(
        socket,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Opencodex-Api-Key: transport-fixture-admin\r\nConnection: close\r\n\r\n"
    )?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        socket.set_read_timeout(Some(remaining(deadline)?))?;
        let n = socket.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > 128 * 1024 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if let Some(split) = bytes.windows(4).position(|x| x == b"\r\n\r\n") {
            let length = String::from_utf8_lossy(&bytes[..split])
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
            if length.is_some_and(|n| bytes.len() >= split + 4 + n) {
                break;
            }
        }
    }
    let split = bytes
        .windows(4)
        .position(|x| x == b"\r\n\r\n")
        .ok_or(io::ErrorKind::InvalidData)?;
    Ok((
        status(&bytes).ok_or(io::ErrorKind::InvalidData)?,
        serde_json::from_slice(&bytes[split + 4..])?,
    ))
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Default)]
struct UpstreamCounters {
    requests: AtomicU64,
    body_bytes: AtomicU64,
    completed: AtomicU64,
    peer_closed: AtomicU64,
    errors: AtomicU64,
    active: AtomicU64,
    accepted: AtomicU64,
    first_chunk_written: AtomicU64,
    mid_upload: AtomicU64,
    before_content_closed: AtomicU64,
    dropped_at_capacity: AtomicU64,
    error_kinds: Mutex<std::collections::BTreeMap<String, u64>>,
}
impl UpstreamCounters {
    fn snapshot(&self) -> Value {
        json!({"requests":self.requests.load(Ordering::Relaxed),"body_bytes":self.body_bytes.load(Ordering::Relaxed),
            "completed":self.completed.load(Ordering::Relaxed),"peer_closed":self.peer_closed.load(Ordering::Relaxed),
            "errors":self.errors.load(Ordering::Relaxed),"active":self.active.load(Ordering::Relaxed),
            "accepted":self.accepted.load(Ordering::Relaxed),"first_chunk_written":self.first_chunk_written.load(Ordering::Relaxed),
            "mid_upload":self.mid_upload.load(Ordering::Relaxed),"dropped_at_capacity":self.dropped_at_capacity.load(Ordering::Relaxed),
            "before_content_closed":self.before_content_closed.load(Ordering::Relaxed),
            "error_kinds":*self.error_kinds.lock().unwrap()})
    }
}

fn upstream_request(
    mut socket: TcpStream,
    counters: &UpstreamCounters,
    hold: Duration,
    read_delay: Duration,
    phase: &mut &'static str,
) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut bytes = Vec::new();
    let mut chunk = [0; 16384];
    let split = loop {
        socket.set_read_timeout(Some(remaining(deadline)?))?;
        let n = socket.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(split) = bytes.windows(4).position(|x| x == b"\r\n\r\n") {
            break split;
        }
        if bytes.len() > 65536 {
            return Err(io::ErrorKind::InvalidData.into());
        }
    };
    let headers = std::str::from_utf8(&bytes[..split]).map_err(|_| io::ErrorKind::InvalidData)?;
    *phase = "method";
    if !headers
        .lines()
        .next()
        .is_some_and(|line| line.starts_with("POST /v1/chat/completions "))
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    *phase = "content_length";
    let length = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<u64>().ok())
                .flatten()
        })
        .ok_or(io::ErrorKind::InvalidData)?;
    if length > 64 * 1024 * 1024 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    *phase = "body_read";
    let mut received = (bytes.len() - split - 4) as u64;
    while received < length {
        if !read_delay.is_zero() {
            thread::sleep(read_delay);
        }
        socket.set_read_timeout(Some(remaining(deadline)?))?;
        let n = socket.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        received += n as u64;
    }
    let sequence = counters.requests.fetch_add(1, Ordering::Relaxed) + 1;
    let id = format!("chatcmpl-transport-{sequence}");
    counters.body_bytes.fetch_add(received, Ordering::Relaxed);
    *phase = "response_headers";
    socket.set_write_timeout(Some(remaining(deadline)?))?;
    socket.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
    )?;
    let sentinel = format!("ocx-upstream-marker-{sequence}");
    *phase = "first_chunk";
    let begin = json!({"id":id,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":sentinel},"finish_reason":null}]});
    write!(socket, "data: {begin}\n\n")?;
    counters.first_chunk_written.fetch_add(1, Ordering::Relaxed);
    *phase = "stream_write";
    let until = Instant::now() + hold;
    while Instant::now() < until {
        thread::sleep(Duration::from_millis(100));
        socket.set_write_timeout(Some(remaining(deadline)?))?;
        socket.write_all(b": fixture-pulse\n\n")?;
    }
    let end = json!({"id":id,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}});
    write!(socket, "data: {end}\n\ndata: [DONE]\n\n")?;
    let _ = socket.shutdown(Shutdown::Both);
    Ok(())
}

struct Upstream {
    port: u16,
    counters: Arc<UpstreamCounters>,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}
impl Upstream {
    fn start(hold: Duration, read_delay: Duration) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let counters = Arc::new(UpstreamCounters::default());
        let stop = Arc::new(AtomicBool::new(false));
        let owned_stop = stop.clone();
        let owned_counters = counters.clone();
        let join = thread::spawn(move || {
            let mut workers = Vec::new();
            while !owned_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        owned_counters.accepted.fetch_add(1, Ordering::Relaxed);
                        if owned_counters.active.load(Ordering::Acquire) >= 40 {
                            owned_counters
                                .dropped_at_capacity
                                .fetch_add(1, Ordering::Relaxed);
                            drop(socket);
                            continue;
                        }
                        let state = owned_counters.clone();
                        state.active.fetch_add(1, Ordering::AcqRel);
                        workers.push(thread::spawn(move || {
                            let mut phase = "header_read";
                            match upstream_request(socket, &state, hold, read_delay, &mut phase) {
                                Ok(()) => {
                                    state.completed.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(err) => {
                                    *state
                                        .error_kinds
                                        .lock()
                                        .unwrap()
                                        .entry(format!("{phase}:{:?}", err.kind()))
                                        .or_default() += 1;
                                    if matches!(
                                        err.kind(),
                                        io::ErrorKind::BrokenPipe
                                            | io::ErrorKind::ConnectionReset
                                            | io::ErrorKind::ConnectionAborted
                                            | io::ErrorKind::UnexpectedEof
                                    ) {
                                        if phase == "stream_write" {
                                            state.peer_closed.fetch_add(1, Ordering::Relaxed);
                                        } else if phase == "first_chunk"
                                            || phase == "response_headers"
                                        {
                                            state
                                                .before_content_closed
                                                .fetch_add(1, Ordering::Relaxed);
                                        } else {
                                            state.mid_upload.fetch_add(1, Ordering::Relaxed);
                                        }
                                    } else {
                                        state.errors.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                            state.active.fetch_sub(1, Ordering::AcqRel);
                        }));
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Ok(Self {
            port,
            counters,
            stop,
            join: Some(join),
        })
    }
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[derive(Default)]
struct MarkerScanner(Vec<u8>, bool, bool);
impl MarkerScanner {
    fn feed(&mut self, bytes: &[u8]) -> bool {
        const MARKER: &[u8] = b"\"delta\":\"ocx-upstream-marker-";
        self.0.extend_from_slice(bytes);
        let found = self.0.windows(MARKER.len()).any(|x| x == MARKER);
        self.1 |= self
            .0
            .windows(27)
            .any(|x| x == b"\"type\":\"response.completed\"");
        self.2 |= self
            .0
            .windows(24)
            .any(|x| x == b"\"type\":\"response.failed\"");
        if self.0.len() > 4096 {
            self.0.drain(..self.0.len() - 4096);
        }
        found
    }
}

fn request(port: u16, body: &[u8], cancel: bool) -> io::Result<Value> {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(40);
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut socket = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    socket.set_write_timeout(Some(remaining(deadline)?))?;
    write!(
        socket,
        "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nX-Opencodex-Api-Key: transport-fixture-data\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    let mut sent = 0;
    while sent < body.len() {
        socket.set_write_timeout(Some(remaining(deadline)?))?;
        let n = socket.write(&body[sent..])?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        sent += n;
    }
    let mut header = Vec::new();
    let mut chunk = [0; 8192];
    let mut received = 0;
    let mut markers = MarkerScanner::default();
    loop {
        socket.set_read_timeout(Some(remaining(deadline)?))?;
        let n = socket.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if header.len() < 8192 {
            header.extend_from_slice(&chunk[..n.min(8192 - header.len())]);
        }
        received += n;
        if received > 1024 * 1024 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let marker_seen = markers.feed(&chunk[..n]);
        // A unique marker is supplied only by the loopback upstream's actual content.
        // Locally generated response.created/heartbeat events never trigger this close.
        if cancel && marker_seen {
            socket.shutdown(Shutdown::Both)?;
            return Ok(
                json!({"status":status(&header),"cancelled_after_upstream_marker":true,"cancel_operation":"shutdown_both","completed_event_seen":markers.1,"failed_event_seen":markers.2,"elapsed_ms":started.elapsed().as_millis()}),
            );
        }
    }
    let error = header
        .windows(4)
        .position(|x| x == b"\r\n\r\n")
        .and_then(|split| serde_json::from_slice::<Value>(&header[split + 4..]).ok());
    Ok(
        json!({"status":status(&header),"cancelled_after_upstream_marker":false,"marker_not_seen":cancel,"completed_event_seen":markers.1,"failed_event_seen":markers.2,"response_bytes":received,"elapsed_ms":started.elapsed().as_millis(),"error_code":error.as_ref().map(|v|&v["error"]["code"]),"error_type":error.as_ref().map(|v|&v["error"]["type"]),"fixture_error_message":error.as_ref().map(|v|&v["error"]["message"])}),
    )
}

fn sample(
    file: &mut fs::File,
    process: &win::Process,
    pid: u32,
    port: u16,
    phase: &str,
    memory: bool,
    upstream: &UpstreamCounters,
) -> io::Result<()> {
    let started = Instant::now();
    let health = match get(port, "/healthz", Duration::from_millis(1500)) {
        Ok((status, value)) => {
            json!({"status":status,"identity_matches":value["pid"] == pid && value["port"] == port && value["service"] == "opencodex" && value["status"] == "ok"})
        }
        Err(err) => json!({"error_kind":format!("{:?}",err.kind())}),
    };
    let elapsed = started.elapsed().as_millis();
    let internal = if memory {
        Some(
            match get(port, "/api/system/memory", Duration::from_secs(2)) {
                Ok((status, value)) => json!({"status":status,"value":value}),
                Err(err) => json!({"error_kind":format!("{:?}",err.kind())}),
            },
        )
    } else {
        None
    };
    writeln!(
        file,
        "{}",
        json!({"at_ms":at_ms(),"pid":pid,"born":process.born,"phase":phase,"health":health,"health_ms":elapsed,"windows":process.metrics(),"internal":internal,"upstream":upstream.snapshot()})
    )
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() != 8 && args.len() != 10 {
        return Err("BUN REPO NEW_ROOT NODE_MODULES BODY_MIB CONCURRENCY WAVES HOLD_MS [READ_DELAY_MS IDLE_SECONDS]".into());
    }
    let mib: usize = args[4].parse()?;
    let concurrent: usize = args[5].parse()?;
    let waves: usize = args[6].parse()?;
    let hold_ms: u64 = args[7].parse()?;
    let read_delay_ms: u64 = args.get(8).map(|v| v.parse()).transpose()?.unwrap_or(0);
    let idle_seconds: u64 = args.get(9).map(|v| v.parse()).transpose()?.unwrap_or(10);
    if !(1..=32).contains(&mib)
        || !(1..=40).contains(&concurrent)
        || mib * concurrent > 640
        || !(1..=8).contains(&waves)
        || !(200..=5000).contains(&hold_ms)
        || read_delay_ms > 20
        || !(10..=300).contains(&idle_seconds)
    {
        return Err("bounded control limits exceeded".into());
    }
    let root = PathBuf::from(&args[2]);
    fs::create_dir(&root)?;
    let codex = root.join("codex");
    let home = root.join("opencodex");
    fs::create_dir(&codex)?;
    fs::create_dir(&home)?;
    fs::write(
        codex.join("config.toml"),
        "cli_auth_credentials_store = \"file\"\n",
    )?;
    let upstream = Upstream::start(
        Duration::from_millis(hold_ms),
        Duration::from_millis(read_delay_ms),
    )?;
    fs::write(
        home.join("config.json"),
        serde_json::to_vec_pretty(&json!({
            "port":0,"hostname":"127.0.0.1","defaultProvider":"probe-local",
            "providers":{"probe-local":{"adapter":"openai-chat","baseUrl":format!("http://127.0.0.1:{}/v1",upstream.port),"authMode":"local","allowPrivateNetwork":true}},
            "codexAccounts":[],"autoSwitchThreshold":0
        }))?,
    )?;
    let system = env::var_os("SystemRoot").ok_or("missing system root")?;
    let mut child = OwnedChild(
        Command::new(&args[0])
            .env_clear()
            .env("SystemRoot", &system)
            .env("WINDIR", &system)
            .env("PATH", Path::new(&system).join("System32"))
            .env("TEMP", &root)
            .env("TMP", &root)
            .env("HOME", &root)
            .env("USERPROFILE", &root)
            .env("APPDATA", root.join("appdata"))
            .env("LOCALAPPDATA", root.join("localappdata"))
            .env("CODEX_HOME", &codex)
            .env("OPENCODEX_HOME", &home)
            .env("NODE_PATH", &args[3])
            .env("NATIVE_OWNER_CODEX_HOME", &codex)
            .env("NATIVE_OWNER_CONFIG_DIR", &home)
            .env(
                "NATIVE_OWNER_KEY",
                "XFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFw=",
            )
            .env("OPENCODEX_ADMIN_AUTH_TOKEN", "transport-fixture-admin")
            .env("OPENCODEX_API_AUTH_TOKEN", "transport-fixture-data")
            .env("CODEX_CI", "1")
            .arg("--cpu-prof")
            .arg("--no-env-file")
            .arg("--no-orphans")
            .arg("--heap-prof")
            .arg(format!("--heap-prof-dir={}", root.display()))
            .arg("--heap-prof-name=heap.heapsnapshot")
            .arg(format!("--cpu-prof-dir={}", root.display()))
            .arg("--cpu-prof-name=cpu.cpuprofile")
            .arg(Path::new(&args[1]).join("tests/helpers/native-main-owner-child.ts"))
            .current_dir(&args[1])
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(fs::File::create(
                root.join("synthetic-child.stderr.txt"),
            )?))
            .spawn()?,
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(value) = line
                .strip_prefix("@@native-owner@@")
                .and_then(|line| serde_json::from_str::<Value>(line).ok())
            {
                let _ = tx.send(value);
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(90);
    let port = loop {
        if child.0.try_wait()?.is_some() {
            return Err("fixture exited before listening".into());
        }
        if let Ok(row) = rx.recv_timeout(remaining(deadline)?.min(Duration::from_millis(500)))
            && row["event"] == "listening"
        {
            break u16::try_from(row["port"].as_u64().ok_or("invalid port")?)?;
        }
    };
    let pid = child.0.id();
    let process = Arc::new(win::Process::open(pid)?);
    let (code, health) = get(port, "/healthz", Duration::from_secs(5))?;
    if code != 200 || health["pid"] != pid || health["port"] != port {
        return Err("fixture identity mismatch".into());
    }
    // The fixture emits listening before asynchronous native-owner recovery settles.
    thread::sleep(Duration::from_secs(2));
    let (cfg_status, cfg) = get(port, "/api/config", Duration::from_secs(3))?;
    fs::write(
        root.join("config-shape.json"),
        serde_json::to_vec_pretty(
            &json!({"status":cfg_status,"default_provider_matches":cfg["defaultProvider"] == "probe-local","provider_names":cfg["providers"].as_object().map(|v|v.keys().collect::<Vec<_>>()),"selected_adapter":cfg["providers"]["probe-local"]["adapter"],"selected_auth_mode":cfg["providers"]["probe-local"]["authMode"]}),
        )?,
    )?;
    if cfg_status != 200
        || cfg["defaultProvider"] != "probe-local"
        || cfg["providers"]["probe-local"]["authMode"] != "local"
    {
        return Err("fixture configuration was not active; no pressure generated".into());
    }
    let preflight = request(
        port,
        b"{\"model\":\"probe-local/probe-model\",\"input\":\"synthetic\",\"stream\":true}",
        false,
    )?;
    let settle = Instant::now() + Duration::from_secs(1);
    while upstream.counters.active.load(Ordering::Acquire) != 0 && Instant::now() < settle {
        thread::sleep(Duration::from_millis(5));
    }
    if preflight["status"] != 200
        || preflight["completed_event_seen"] != true
        || preflight["failed_event_seen"] != false
        || upstream.counters.requests.load(Ordering::Relaxed) != 1
        || upstream.counters.completed.load(Ordering::Relaxed) != 1
    {
        fs::write(
            root.join("preflight-refusal.json"),
            serde_json::to_vec_pretty(
                &json!({"result":preflight,"upstream":upstream.counters.snapshot()}),
            )?,
        )?;
        return Err("native transport preflight did not complete; no pressure generated".into());
    }
    let mut file = fs::File::create(root.join("samples.jsonl"))?;
    sample(
        &mut file,
        &process,
        pid,
        port,
        "before",
        true,
        &upstream.counters,
    )?;
    let mut body = b"{\"model\":\"probe-local/probe-model\",\"input\":\"".to_vec();
    body.extend(std::iter::repeat_n(b'x', mib * 1024 * 1024));
    body.extend_from_slice(b"\",\"stream\":true}");
    let body = Arc::new(body);
    let mut wave_results = Vec::new();
    for wave in 0..waves {
        let upstream_before = upstream.counters.snapshot();
        let cancel = wave % 2 == 1;
        let phase = if cancel { "cancel" } else { "complete" };
        let mut clients = Vec::new();
        for _ in 0..concurrent {
            let body = body.clone();
            clients.push(thread::spawn(move || {
                request(port, &body, cancel)
                    .unwrap_or_else(|err| json!({"error_kind":format!("{:?}",err.kind())}))
            }));
        }
        while clients.iter().any(|x| !x.is_finished()) {
            sample(
                &mut file,
                &process,
                pid,
                port,
                phase,
                false,
                &upstream.counters,
            )?;
            thread::sleep(Duration::from_millis(100));
        }
        let results: Vec<_> = clients.into_iter().map(|x| x.join().unwrap()).collect();
        sample(
            &mut file,
            &process,
            pid,
            port,
            "post-wave",
            true,
            &upstream.counters,
        )?;
        let clients_ok = results.iter().all(|r| {
            r["status"] == 200
                && r["failed_event_seen"] == false
                && if cancel {
                    r["cancelled_after_upstream_marker"] == true
                } else {
                    r["completed_event_seen"] == true
                }
        });
        wave_results.push(json!({"wave":wave,"mode":phase,"results":results,"clients_ok":clients_ok,"upstream_before":upstream_before,"upstream":upstream.counters.snapshot()}));
        // Give all upstream sockets their normal bounded completion/abort window.
        let until = Instant::now() + Duration::from_millis(hold_ms + 1000);
        while Instant::now() < until {
            sample(
                &mut file,
                &process,
                pid,
                port,
                "settle",
                false,
                &upstream.counters,
            )?;
            thread::sleep(Duration::from_millis(200));
        }
        wave_results.last_mut().unwrap()["upstream_after_settle"] = upstream.counters.snapshot();
    }
    let idle_until = Instant::now() + Duration::from_secs(idle_seconds);
    let mut next_internal = Instant::now();
    while Instant::now() < idle_until {
        let memory = Instant::now() >= next_internal;
        if memory {
            next_internal = Instant::now() + Duration::from_secs(10);
        }
        sample(
            &mut file,
            &process,
            pid,
            port,
            "idle",
            memory,
            &upstream.counters,
        )?;
        thread::sleep(Duration::from_millis(250));
    }
    if !process.live() {
        return Err("owned child exited during control".into());
    }
    let result = json!({"pid":pid,"born":process.born,"wire_bytes":body.len(),"concurrency":concurrent,"waves":wave_results,
        "hold_ms":hold_ms,"transport_class":"loopback-http-openai-chat-bun-fetch","tls":false,"oauth":false,"read_delay_ms":read_delay_ms,"idle_seconds":idle_seconds,"upstream":upstream.counters.snapshot(),"final_windows":process.metrics()});
    fs::write(
        root.join("result.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    let _ = writeln!(std::io::stdout().lock(), "{result}");
    if let Some(stdin) = child.0.stdin.as_mut() {
        let _ = writeln!(stdin, "{{\"op\":\"stop\",\"id\":\"transport-stop\"}}");
    }
    let stop_deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < stop_deadline && child.0.try_wait()?.is_none() {
        thread::sleep(Duration::from_millis(50));
    }
    drop(child);
    reader.join().map_err(|_| "reader panicked")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locally_created_stream_event_does_not_cancel() {
        let mut scan = MarkerScanner::default();
        assert!(!scan.feed(b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_local\"}}\n\n"));
    }
    #[test]
    fn content_marker_is_detected_across_every_socket_read_boundary() {
        let frame=b"event: response.output_text.delta\ndata: {\"delta\":\"ocx-upstream-marker-25\",\"type\":\"response.output_text.delta\"}\n\n";
        let marker_end = frame
            .windows(29)
            .position(|x| x == b"\"delta\":\"ocx-upstream-marker-")
            .unwrap()
            + 29;
        for split in 0..marker_end {
            let mut scan = MarkerScanner::default();
            assert!(!scan.feed(&frame[..split]));
            assert!(scan.feed(&frame[split..]), "split {split}");
        }
    }
    #[test]
    fn marker_after_large_preamble_and_before_large_suffix_is_not_truncated() {
        let mut scan = MarkerScanner::default();
        assert!(!scan.feed(&vec![b'x'; 16384]));
        let mut frame = b"data: {\"delta\":\"ocx-upstream-marker-5\"}\n\n".to_vec();
        frame.extend(std::iter::repeat_n(b'y', 16384));
        assert!(scan.feed(&frame));
        assert!(scan.0.len() <= 4096);
    }

    #[test]
    fn successful_and_failed_terminals_survive_fragmented_reads() {
        let mut completed = MarkerScanner::default();
        for byte in b"event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n" {
            completed.feed(&[*byte]);
        }
        assert!(completed.1);
        assert!(!completed.2);
        let mut failed = MarkerScanner::default();
        for byte in b"event: response.failed\ndata: {\"type\":\"response.failed\"}\n\n" {
            failed.feed(&[*byte]);
        }
        assert!(!failed.1);
        assert!(failed.2);
    }
}
