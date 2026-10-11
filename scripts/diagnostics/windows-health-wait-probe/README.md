# Windows health-wait probe

Standalone Rust diagnostics for an already running, explicitly identified Bun
process. Windows only. It reads process/thread metadata and `/healthz`; it does
not suspend, restart, change, or collect a heap from the target.

`ocx-cause-probe PID BORN PORT SECONDS NEW_JSONL`

Build from this directory with `cargo build --locked --release`. Keep its Cargo
target separate from another worktree. The executable has the Windows GUI
subsystem; retain its process handle and exit code when launching it hidden.
The process birth time is the Windows FILETIME value, not a formatted date. This
version expects the bundled Bun beneath the current user's roaming npm
opencodex installation; it rejects another executable rather than accepting an
arbitrary image argument.

The probe uses one 20-second deadline for a health exchange. During a slow
exchange it attempts wait-chain captures at 1, 5, and 10 seconds. Capture workers
have a 1.2-second budget. A held thread handle must still match its PID, birth
time, and liveness before and after capture. The oldest thread is a hypothesis,
not an assertion that it is JavaScript's main thread. Object names and raw child
commands are omitted.

`during_health_wait` requires the snapshot's measured timestamp to be strictly
before the health response's final received byte. A capture that finishes after
the response is not evidence of its blocking cause. Healthy observations are
sampled in the output: use the final `total`, not the row count. Recover the
actual executable exit code separately from the `finished` record.

Validation: two real loopback regression tests, Rustfmt, Clippy with warnings
denied, release build, and independent Must 0 review. No Linux/macOS execution is
claimed. A completed live 900-second observation is evidence of that window,
not a diagnosis of an older, unrecorded stall.
