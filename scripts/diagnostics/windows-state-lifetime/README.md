# Windows state-publication lifetime experiment

Standalone Rust supervision of the real installed state/spill APIs in an owned
Bun 1.4.2 child. All three 128 KiB input items are synthetic. Home directories,
files, server, GC, and injected delays belong only to that child.

`todo1936-state-lifetime BUN INSTALLED_SOURCE NEW_OUTPUT`

Build with `cargo build --locked --release`. Use absolute paths, a new output
directory, hidden launch, and a retained process handle to recover the actual
exit code. The child is monitored with a 60-second deadline. The experiment's
normal path waits for the actual child and ACL subprocess exits. Exceptional
OS failure to terminate a child is not a tested bounded-teardown guarantee.

The existing async ACL test seam first gates publication, then runs the actual
icacls executable. While gated, three pending candidates keep their original
input items alive. After release and durable publication, the queue becomes
empty, three small spill stubs remain, and the input WeakRefs become dead after
owned-child GC. This discriminates pending ownership from post-settlement
garbage; it is not a full request/authentication scenario.

One existing fsync seam sleeps 2.5 seconds before performing real fsync. An
atomic marker supplies the ephemeral listener port to Rust, which requests
`/health` during that synchronous work. The successful example delayed health
by 2.517 seconds. This demonstrates a blocking call path in
`writeResponseSpillDurablyAsync`; it does not establish that production had slow
fsync, or reproduce the older 20-second incident or the 23 GB external counter.

The final result/verified JSON and retained process record establish the normal
success path. Rustfmt, Clippy with warnings denied, release, and independent
Must 0 review passed. No production data, production GC, stress test, or other-OS
validation is claimed.
