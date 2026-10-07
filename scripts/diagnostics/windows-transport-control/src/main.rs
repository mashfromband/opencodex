#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod control;

#[cfg(windows)]
fn main() {
    use std::io::Write;
    if let Err(error) = control::run() {
        let _ = writeln!(std::io::stderr().lock(), "{error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This synthetic control requires Windows process counters.");
    std::process::exit(1);
}
