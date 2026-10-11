#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#[cfg(windows)]
mod win;
use serde_json::json;
use std::{
    env,
    fs::{File, OpenOptions},
    io::{Read, Write},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
fn token() -> Result<String, &'static str> {
    let profile = env::var_os("USERPROFILE").ok_or("token_unavailable")?;
    let path = std::path::PathBuf::from(profile).join(".opencodex/admin-api-token");
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "token_unavailable")?
        .take(1025)
        .read_to_end(&mut bytes)
        .map_err(|_| "token_unavailable")?;
    if bytes.len() > 1024 {
        return Err("token_unavailable");
    }
    let token = String::from_utf8(bytes).map_err(|_| "token_unavailable")?;
    let token = token.trim().to_owned();
    if !todo1936_correlation::token_valid(&token) {
        return Err("token_unavailable");
    }
    Ok(token)
}

fn run() -> Result<(), &'static str> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 4 {
        return Err("usage_PID_BORN_SECONDS_NEW_OUTPUT");
    }
    let pid: u32 = args[0].parse().map_err(|_| "invalid_pid")?;
    let born: u64 = args[1].parse().map_err(|_| "invalid_born")?;
    let seconds: u64 = args[2].parse().map_err(|_| "invalid_duration")?;
    if pid == 0 || born == 0 || !(1..=1800).contains(&seconds) {
        return Err("invalid_observation");
    }
    #[cfg(not(windows))]
    {
        let _ = (pid, born, seconds);
        return Err("windows_only");
    }
    #[cfg(windows)]
    {
        let process = win::Process::open(pid, born)?;
        let mut out = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&args[3])
            .map_err(|_| "output_unavailable")?;
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut count = 0;
        while Instant::now() < deadline && process.live() {
            let start_ms = epoch_ms();
            let started = Instant::now();
            let before = process.metrics();
            let budget = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(5));
            let sample = match token() {
                Ok(token) => {
                    match todo1936_correlation::fetch_memory(10100, pid, &token, budget, &|local| {
                        process.connected_peer_matches(10100, local)
                    }) {
                        Ok(value) if process.live() => json!({"ok":true,"memory":value}),
                        Ok(_) => json!({"ok":false,"error":"process_exited"}),
                        Err(error) => json!({"ok":false,"error":error.label()}),
                    }
                }
                Err(error) => json!({"ok":false,"error":error}),
            };
            writeln!(out, "{}", json!({"kind":"sample","startMs":start_ms,"endMs":epoch_ms(),"elapsedMs":started.elapsed().as_millis(),"pid":pid,"born":process.born,"before":before,"after":process.metrics(),"sample":sample})).map_err(|_| "output_write_failed")?;
            out.flush().map_err(|_| "output_flush_failed")?;
            count += 1;
            std::thread::sleep(
                Duration::from_secs(30)
                    .saturating_sub(started.elapsed())
                    .min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        writeln!(out, "{}", json!({"kind":"finished","atMs":epoch_ms(),"pid":pid,"born":born,"samples":count,"processLive":process.live()})).map_err(|_| "output_write_failed")?;
        out.flush().map_err(|_| "output_flush_failed")?;
        Ok(())
    }
}
fn main() {
    if let Err(label) = run() {
        eprintln!("{label}");
        std::process::exit(1);
    }
}
