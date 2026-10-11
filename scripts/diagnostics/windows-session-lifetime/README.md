# Windows session lifetime experiment

This Rust supervisor runs small synthetic experiments against an explicitly
provided Bun 1.4.2 executable and unmodified opencodex 2.81.0 source. It launches
only its own hidden child with an empty inherited environment and separate home
directories. No provider account, API key, or real model is used. Endpoints are
loopback, and native transport fallback is an experiment failure.

`todo1936-transport-lifetime BUN INSTALLED_SOURCE NEW_OUTPUT MODE`

Modes: `stock`, `patched`, `header`, `header-patched`, `inbound`, `handler`, `http`.
Build with `cargo build --locked --release`. Paths must be absolute; output must
not exist. Launch the GUI-subsystem executable hidden and retain its actual
process handle/exit code. The supervised child has a 60-second execution deadline.

Each case exchanges six 128 KiB synthetic inputs and 256 KiB output deltas. The
upstream WebSocket pool remains alive with one idle session while the experiment
checks WeakRefs for completed exchange owners after GC in the owned child only.
The patched modes copy the relay and apply its existing two-listener EOF cleanup;
they do not write to the installed source. Header modes call the real header
timeout helper. Inbound mode uses the real SSE-to-WebSocket bridge. Handler mode
copies the actual inbound handler, resolves its other imports to the supplied
source, and injects only the response dispatcher. HTTP mode receives real native
Bun Requests, passes their abort signals through the existing abort-link/header
helpers, and serves their relayed responses through Bun.

The Rust success contract checks the terminal/done counts, idle pool, listed
WeakRef counts, and mode-specific delivery and ownership counters. The printed
scope excludes complete core routing, authentication, state persistence,
HTTPS/HTTP2, and production memory causality. Reader fields that were not
instrumented are not independent proof that a reader was collected.

Verified cases have explicit child/supervisor exit records and independent
Must 0 review. The first inbound-only shell attempt did not retain the supervisor
handle; its exit was unknown, and was not used as final process evidence. A
handler import-resolution failure occurred before child launch and is separate
from the subsequently successful case. Earlier passing modes were not rerun for
the later HTTP delta. Windows Rustfmt/Clippy/release passed; no other OS run or
full application test-suite result is claimed.
