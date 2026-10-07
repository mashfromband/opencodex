# Windows Bun cause probe

This optional Rust diagnostic tool records health timing and Windows process
counters while investigating a live Bun listener that stops answering. It is
separate from the proxy runtime and is not a recovery policy. See upstream
[issue 6671](https://github.com/lidge-jun/opencodex/issues/6671).

Build on Windows:

```powershell
cargo test --manifest-path scripts/diagnostics/windows-cause-probe/Cargo.toml
cargo clippy --manifest-path scripts/diagnostics/windows-cause-probe/Cargo.toml --all-targets -- -D warnings
cargo build --release --manifest-path scripts/diagnostics/windows-cause-probe/Cargo.toml
```

The release binary is a Windows GUI-subsystem executable, so starting it from a
scheduled task creates no console. Pipe its stdout when invoking it from
PowerShell to wait for its exit. The JSONL file is the primary record; a missing
stdout handle does not prevent recording.

## Passive observation

```powershell
ocx-cause-probe.exe <PID> <PORT> <SECONDS> <NEW_JSONL_PATH> | Out-String
```

The observer holds a read-only process handle, records its creation identity,
and checks that the health response matches its PID and port. It exits when
that process exits or the observation period ends (1–1800 seconds). It never
controls the observed process. A slow probe triggers an independent WCT worker
with a bounded lifetime; the parent can terminate only that owned worker.

Recorded data consists of probe start/end/status, Windows private bytes,
working set and CPU time, and wait-chain numeric categories/IDs. No command
lines, object names, request bodies, credentials, live stacks or live heap
dumps are recorded. WCT covers a limited set of waits. The oldest thread is a
candidate, not an asserted JavaScript event-loop thread. A missing wait chain
or cycle does not rule out a stall.

## Synthetic pressure control

```powershell
ocx-cause-probe.exe --isolated <BUN_EXE> <REPO_ROOT> <NEW_FIXTURE_ROOT> <NODE_MODULES> <BODY_MIB> <CONCURRENCY> | Out-String
```

This mode reuses the repository's unchanged
`tests/helpers/native-main-owner-child.ts`. It gives that child private homes,
clears ambient environment variables, verifies its ephemeral loopback listener,
and sends uncompressed synthetic JSON. The existing fixture intercepts its
synthetic upstream hosts. No actual AI account is configured. Sizes are bounded
to 1–32 MiB per body, 1–40 concurrent requests and 640 MiB per burst; request
I/O has a shared deadline and a response-size cap. Sampling and final counters
share the originally opened process handle. Cleanup uses the owned child
handle.

The fixture can persist synthetic continuation data in its private home. Its
CPU profile can contain source paths and function frames and belongs in private
investigation evidence. Publish only a reviewed scalar summary. A bounded stop
fallback can prevent Bun from publishing a CPU profile; absence of a profile
must not be described as successful profiling.

This control includes inbound parsing, adapter/state handling and spill
publication, and excludes native upstream fetch transport. Transient multi-GB
pressure and a short liveness delay do not establish a persistent leak or the
cause of a historical long stall. Windows private bytes, working set, Bun
external counters and registered-store bytes are different accounting views.
