use serde_json::{Map, Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

const MAX_HEADER: usize = 8192;
const MAX_BODY: usize = 65536;

#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    Token,
    Connect,
    Timeout,
    Io,
    Http,
    Framing,
    Json,
    Identity,
    Schema,
}

impl Failure {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Token => "token_unavailable",
            Self::Connect => "connect_failure",
            Self::Timeout => "deadline",
            Self::Io => "io_failure",
            Self::Http => "http_failure",
            Self::Framing => "framing_failure",
            Self::Json => "json_failure",
            Self::Identity => "identity_changed",
            Self::Schema => "schema_failure",
        }
    }
}

pub fn token_valid(token: &str) -> bool {
    !token.is_empty() && token.len() <= 1024 && token.bytes().all(|b| (33..=126).contains(&b))
}

fn left(start: Instant, budget: Duration) -> Result<Duration, Failure> {
    let remaining = budget.saturating_sub(start.elapsed());
    if remaining.is_zero() {
        Err(Failure::Timeout)
    } else {
        Ok(remaining)
    }
}

fn io_kind(error: std::io::Error) -> Failure {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        Failure::Timeout
    } else {
        Failure::Io
    }
}

fn body_length(header: &[u8]) -> Result<usize, Failure> {
    let header = std::str::from_utf8(header).map_err(|_| Failure::Framing)?;
    let mut lines = header.split("\r\n");
    let mut status = lines
        .next()
        .ok_or(Failure::Framing)?
        .split_ascii_whitespace();
    if !matches!(status.next(), Some("HTTP/1.1" | "HTTP/1.0")) || status.next() != Some("200") {
        return Err(Failure::Http);
    }
    let mut length = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(Failure::Framing)?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(Failure::Framing);
        }
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(Failure::Framing);
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| Failure::Framing)?,
            );
        }
    }
    length.filter(|&n| n <= MAX_BODY).ok_or(Failure::Framing)
}

// Only a fixed loopback management route is issued; no redirects or arbitrary URLs.
pub fn fetch_memory(
    port: u16,
    pid: u32,
    token: &str,
    budget: Duration,
    peer_matches: &dyn Fn(SocketAddr) -> bool,
) -> Result<Value, Failure> {
    if !token_valid(token) {
        return Err(Failure::Token);
    }
    let started = Instant::now();
    let address: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut stream = TcpStream::connect_timeout(&address, left(started, budget)?).map_err(|e| {
        if matches!(
            e.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ) {
            Failure::Timeout
        } else {
            Failure::Connect
        }
    })?;
    // Check the held target after connection, before any credential-bearing bytes.
    if !peer_matches(stream.local_addr().map_err(io_kind)?) {
        return Err(Failure::Identity);
    }
    let request = format!(
        "GET /api/system/memory HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let mut remaining = request.as_bytes();
    while !remaining.is_empty() {
        stream
            .set_write_timeout(Some(left(started, budget)?))
            .map_err(io_kind)?;
        let n = match stream.write(remaining) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.map_err(io_kind)?,
        };
        if n == 0 {
            return Err(Failure::Io);
        }
        remaining = &remaining[n..];
    }
    let mut bytes = Vec::new();
    let mut framing = None;
    loop {
        if let Some((split, length)) = framing
            && bytes.len() >= split + 4 + length
        {
            let body = &bytes[split + 4..split + 4 + length];
            let raw: Value = serde_json::from_slice(body).map_err(|_| Failure::Json)?;
            return project(&raw, pid);
        }
        if framing.is_none() && bytes.len() > MAX_HEADER {
            return Err(Failure::Framing);
        }
        stream
            .set_read_timeout(Some(left(started, budget)?))
            .map_err(io_kind)?;
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).map_err(io_kind)?;
        if n == 0 {
            return Err(Failure::Framing);
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > MAX_HEADER + MAX_BODY {
            return Err(Failure::Framing);
        }
        if framing.is_none()
            && let Some(split) = bytes.windows(4).position(|v| v == b"\r\n\r\n")
        {
            if split > MAX_HEADER {
                return Err(Failure::Framing);
            }
            framing = Some((split, body_length(&bytes[..split])?));
        }
    }
}

// Unknown names and string values never pass through this closed projection.
pub fn project(raw: &Value, pid: u32) -> Result<Value, Failure> {
    if raw["pid"].as_u64() != Some(u64::from(pid)) {
        return Err(Failure::Identity);
    }
    let mut out = Map::new();
    for name in [
        "pid",
        "rss",
        "external",
        "heapUsed",
        "heapTotal",
        "arrayBuffers",
        "activeTurnCount",
    ] {
        let number = raw[name].as_u64().ok_or(Failure::Schema)?;
        out.insert(name.into(), json!(number));
    }
    for (output, path) in [
        ("heapSize", "/jscHeap/heapSize"),
        ("heapCapacity", "/jscHeap/heapCapacity"),
        ("objectCount", "/jscHeap/objectCount"),
        ("protectedObjectCount", "/jscHeap/protectedObjectCount"),
        ("extraMemorySize", "/jscHeap/extraMemorySize"),
        ("globalObjectCount", "/jscHeap/globalObjectCount"),
        ("miAllocated", "/jscHeap/mimalloc/malloc_normal/current"),
        ("miHuge", "/jscHeap/mimalloc/malloc_huge/current"),
        ("miCommitted", "/jscHeap/mimalloc/committed/current"),
        ("miReserved", "/jscHeap/mimalloc/reserved/current"),
        ("retainedBytes", "/appOwnedBytes/retainedBytes"),
        ("pinnedBytes", "/appOwnedBytes/pinnedBytes"),
        (
            "translatorBytes",
            "/appOwnedBytes/observedInFlight/translator_buffers/currentBytes",
        ),
        ("stateCount", "/responseState/count"),
        ("residentCount", "/responseState/residentCount"),
        ("spillStubCount", "/responseState/spillStubCount"),
        ("tombstoneCount", "/responseState/tombstoneCount"),
        ("stateBytes", "/responseState/totalBytes"),
        ("spillPayloadBytes", "/responseState/spillPayloadBytes"),
        ("spillWrites", "/responseState/spillWrites"),
        ("spillWriteFailures", "/responseState/spillWriteFailures"),
        (
            "spillConsecutiveFailures",
            "/responseState/spillWriteConsecutiveFailures",
        ),
        (
            "spillLastFailureAt",
            "/responseState/spillLastWriteFailureAt",
        ),
        (
            "spillLastSuccessAt",
            "/responseState/spillLastWriteSuccessAt",
        ),
        ("spillFiles", "/responseSpill/files"),
        ("spillDiskBytes", "/responseSpill/bytes"),
    ] {
        out.insert(
            output.into(),
            raw.pointer(path)
                .and_then(Value::as_u64)
                .map_or(Value::Null, |n| json!(n)),
        );
    }
    out.insert(
        "spillHealthy".into(),
        match raw
            .pointer("/responseState/spillWriteStatus")
            .and_then(Value::as_str)
        {
            Some("healthy") => json!(true),
            Some("degraded") => json!(false),
            _ => Value::Null,
        },
    );
    // Closed class names only. Never copy arbitrary object names or strings.
    for name in [
        "Array",
        "Object",
        "String",
        "Uint8Array",
        "ArrayBuffer",
        "Promise",
        "InternalPromise",
        "Response",
        "Request",
        "AbortController",
        "AbortSignal",
        "ReadableStream",
        "ReadableStreamDefaultReader",
        "WebSocket",
        "JSWebSocket",
        "HTTPClientRequest",
        "Function",
        "LexicalEnvironment",
    ] {
        out.insert(
            format!("objects_{name}"),
            raw.pointer("/jscHeap/objectTypeCounts")
                .and_then(|types| types.get(name))
                .and_then(Value::as_u64)
                .map_or(Value::Null, |n| json!(n)),
        );
    }
    out.insert(
        "spillCapacityRefusal".into(),
        match raw.pointer("/responseState/spillLastWriteFailureCode") {
            Some(Value::Null) => json!(false),
            Some(Value::String(code)) if code == "ECAPACITY" => json!(true),
            _ => Value::Null,
        },
    );
    out.insert(
        "watchdogSamples".into(),
        raw.pointer("/watchdog/samples")
            .and_then(Value::as_array)
            .map_or(Value::Null, |samples| {
                Value::Array(
                    samples
                        .iter()
                        .take(60)
                        .map(|sample| {
                            let row: Map<String, Value> = [
                                "at",
                                "rss",
                                "heapUsed",
                                "heapTotal",
                                "external",
                                "arrayBuffers",
                            ]
                            .into_iter()
                            .map(|name| {
                                (
                                    name.to_owned(),
                                    sample[name].as_u64().map_or(Value::Null, |n| json!(n)),
                                )
                            })
                            .collect();
                            Value::Object(row)
                        })
                        .collect(),
                )
            }),
    );
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn fixture() -> Value {
        json!({"pid": 77, "rss": 1, "external": 2, "heapUsed": 3,
        "heapTotal": 4, "arrayBuffers": 5, "activeTurnCount": 6})
    }

    #[test]
    fn watchdog_history_is_bounded_and_drops_every_unknown_or_string_field() {
        let mut raw = fixture();
        raw["watchdog"] = json!({"samples":(0..70).map(|i| json!({"at":i,"rss":2,"heapUsed":3,"heapTotal":4,"external":5,"arrayBuffers":6,"token":"fixture-secret","observedMetric":"fixture-private"})).collect::<Vec<_>>()});
        let projected = project(&raw, 77).unwrap();
        let rows = projected["watchdogSamples"].as_array().unwrap();
        assert_eq!(rows.len(), 60);
        assert_eq!(rows[59]["at"], 59);
        assert!(rows.iter().all(|row| row.as_object().unwrap().len() == 6));
        assert!(!projected.to_string().contains("fixture-"));
        raw["watchdog"]["samples"][0]["external"] = json!("fixture-private");
        assert!(project(&raw, 77).unwrap()["watchdogSamples"][0]["external"].is_null());
        assert!(project(&fixture(), 77).unwrap()["watchdogSamples"].is_null());
    }

    #[test]
    fn secret_fields_and_unrecognized_values_never_pass_projection() {
        let mut raw = fixture();
        raw["token"] = json!("fixture-secret");
        raw["responseState"] =
            json!({"totalBytes":"fixture-private", "spillLastWriteFailureCode":"fixture-secret"});
        raw["jscHeap"] = json!({"objectCount":99, "objects":"fixture-private"});
        let value = project(&raw, 77).unwrap();
        assert_eq!(value["objectCount"], 99);
        assert!(value["stateBytes"].is_null());
        assert!(value["spillCapacityRefusal"].is_null());
        assert!(!value.to_string().contains("fixture-"));
    }

    #[test]
    fn wrong_process_and_invalid_required_scalar_are_rejected() {
        assert_eq!(project(&fixture(), 78), Err(Failure::Identity));
        let mut raw = fixture();
        raw["external"] = json!("private");
        assert_eq!(project(&raw, 77), Err(Failure::Schema));
    }

    #[test]
    fn ambiguous_oversized_and_chunked_framing_are_rejected() {
        for header in [
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2",
            "HTTP/1.1 200 OK\r\nContent-Length: 65537",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 2",
        ] {
            assert_eq!(body_length(header.as_bytes()), Err(Failure::Framing));
        }
        assert_eq!(
            body_length(b"HTTP/1.1 302 Found\r\nContent-Length: 0"),
            Err(Failure::Http)
        );
    }

    #[test]
    fn invalid_token_rejects_before_connect() {
        assert_eq!(
            fetch_memory(1, 77, "injected\r\nheader", Duration::from_secs(1), &|_| {
                true
            }),
            Err(Failure::Token)
        );
        assert!(!token_valid(&"x".repeat(1025)));
    }

    #[test]
    fn fragmented_real_loopback_response_is_projected_without_waiting_for_eof() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 256];
            while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                let n = peer.read(&mut chunk).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&chunk[..n]);
            }
            assert!(request.starts_with(b"GET /api/system/memory HTTP/1.1\r\n"));
            let body = fixture().to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            for chunk in response.as_bytes().chunks(3) {
                peer.write_all(chunk).unwrap();
            }
            // The client must complete on Content-Length even while the peer remains open.
            let mut eof = [0];
            assert_eq!(peer.read(&mut eof).unwrap(), 0);
        });
        assert_eq!(
            fetch_memory(port, 77, "synthetic", Duration::from_secs(2), &|_| true).unwrap()["external"],
            2
        );
        server.join().unwrap();
    }

    #[test]
    fn response_body_wait_has_one_shared_deadline() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 256];
            while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                let n = peer.read(&mut chunk).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&chunk[..n]);
                assert!(request.len() <= 4096);
            }
            peer.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n{")
                .unwrap();
            let mut eof = [0];
            assert_eq!(peer.read(&mut eof).unwrap(), 0);
        });
        assert_eq!(
            fetch_memory(port, 77, "synthetic", Duration::from_millis(200), &|_| true),
            Err(Failure::Timeout)
        );
        server.join().unwrap();
    }

    #[test]
    fn exited_target_after_connect_receives_no_credential_bytes() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut first = [0];
            assert_eq!(peer.read(&mut first).unwrap(), 0);
        });
        assert_eq!(
            fetch_memory(
                port,
                77,
                "synthetic-secret",
                Duration::from_secs(2),
                &|local| {
                    assert!(local.ip().is_loopback());
                    assert_ne!(local.port(), 0);
                    false
                }
            ),
            Err(Failure::Identity)
        );
        server.join().unwrap();
    }

    #[test]
    fn absent_unknown_and_known_statuses_remain_distinct() {
        let mut raw = fixture();
        assert!(project(&raw, 77).unwrap()["spillHealthy"].is_null());
        raw["responseState"] =
            json!({"spillWriteStatus":"unknown", "spillLastWriteFailureCode":"unknown"});
        assert!(project(&raw, 77).unwrap()["spillHealthy"].is_null());
        raw["responseState"] =
            json!({"spillWriteStatus":"healthy", "spillLastWriteFailureCode":null});
        let healthy = project(&raw, 77).unwrap();
        assert_eq!(healthy["spillHealthy"], true);
        assert_eq!(healthy["spillCapacityRefusal"], false);
        raw["responseState"] =
            json!({"spillWriteStatus":"degraded", "spillLastWriteFailureCode":"ECAPACITY"});
        let degraded = project(&raw, 77).unwrap();
        assert_eq!(degraded["spillHealthy"], false);
        assert_eq!(degraded["spillCapacityRefusal"], true);
    }
}
