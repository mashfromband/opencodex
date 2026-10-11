# Windows health-wait probe

Standalone Rust diagnostics for an already running, explicitly identified Bun
process. Windows only. It reads process/thread metadata and `/healthz`; it does
not suspend, restart, change, or collect a heap from the target.

`ocx-cause-probe PID BORN PORT SECONDS NEW_JSONL`

Build with `cargo build --locked --release --bin ocx-cause-probe`. Keep its Cargo
target separate from another worktree. The executable has the Windows GUI
subsystem; retain its process handle and exit code when launching it hidden.
The process birth time is the Windows FILETIME value, not a formatted date. This
version expects the bundled Bun beneath the current user's roaming npm
opencodex installation; it rejects another executable rather than accepting an
arbitrary image argument.

The probe uses one 20-second deadline for a health exchange. During a slow
exchange it attempts wait-chain captures at 0.1, 5, and 10 seconds. Capture workers
have a 1.2-second budget. A held thread handle must still match its PID, birth
time, and liveness before and after capture. The oldest thread is a hypothesis,
not an assertion that it is JavaScript's main thread. Object names and raw child
commands are omitted.

The session follows waits beyond the first foreign process with
`WCT_OUT_OF_PROC_FLAG`. Zero flags can still report the first foreign thread;
the option is needed to follow that thread's next dependency. A native
three-process mutex regression verifies this distinction while the mutex owner
keeps both children waiting. It checks the new query reaches that owner and a
simultaneous zero-flag control stops at the intermediate child. This is not an
explanation for an older single-node capture: unblocked threads or unsupported
synchronization elements can also produce one node.

`during_health_wait` requires the snapshot's measured timestamp to be strictly
before the health response's final received byte. `during_response_wait` also
requires the complete health request to have been sent. A capture after the
response is not evidence of its blocking cause. Healthy observations are
sampled in the output: use the final `total`, not the row count. Recover the
actual executable exit code separately from the `finished` record.

Validation: two real loopback tests and the native process-boundary regression,
Rustfmt, Clippy with warnings
denied, release build, and independent Must 0 review. No Linux/macOS execution is
claimed. A completed live 900-second observation is evidence of that window,
not a diagnosis of an older, unrecorded stall.

Native AMD64 captures include the numeric last system call and a match against
complete local ntdll syscall stubs. Unsupported architectures, unknown/hooked
stubs and failed queries remain explicitly unavailable. Pointer arguments are
never printed, saved or dereferenced. A recorded last call does not prove that
call currently blocks or identify its JavaScript source function. Process IO
includes non-file operations; page faults include both soft and hard faults.

`ocx-cause-probe --resolve-syscall NUMBER` resolves a local unsigned 16-bit number
without querying a target process. It has the same native AMD64 and complete
stub checks, and returns null for an unmapped number.
