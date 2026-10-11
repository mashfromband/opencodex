# Windows memory correlation

This standalone Rust observer projects only known numeric memory, state, spill,
and watchdog-history fields from the existing loopback management API. It never
copies the raw API response, token, request content, arbitrary object names, or
account identifiers into its output. It does not change server configuration.

`todo1936-correlation PID BORN SECONDS NEW_JSONL`

Build with `cargo build --locked --release`; run on Windows with a retained
process handle so the actual exit code is available. The executable uses the
Windows GUI subsystem. The existing admin token is read privately from the
current user's opencodex home. It is sent only after verifying the connected TCP
tuple belongs to the held target. PID, birth time, and target liveness are checked.
Do not pass credentials on the command line.

`watchdogSamples` projects at most 60 already-recorded rows and six unsigned
numeric fields per row. Missing or malformed optional numbers remain null. In
particular, a null `extraMemorySize`, allocator counter, or object-class count
does not mean zero: the installed API may not expose that field. History is not
another sampling loop, and it does not by itself identify a leak or GC event.

Validation: ten focused Rust tests including real loopback framing/deadline and
credential-rejection cases, scalar-history privacy/bounds coverage, Rustfmt,
Clippy with warnings denied, release, and one actual API-history projection.
Linux/macOS execution is not claimed. Production GC, heap dumps, and pressure
tests are outside this tool's behavior.
