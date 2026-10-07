# Windows loopback HTTP transport control

This optional Rust control helps investigate [issue 6671](https://github.com/lidge-jun/opencodex/issues/6671).
It runs the unchanged `tests/helpers/native-main-owner-child.ts` in private fixture
homes, with a cleared child environment and synthetic protocol values. A Rust
loopback upstream exercises Bun's real fetch through the `openai-chat` adapter.
The helper's intercepted synthetic hostnames are not used for measured requests.
Bun must support `--no-env-file` and `--no-orphans` (the controls use Bun 1.4.0).
Dotenv loading is disabled; Bun owns cleanup of the fixture's descendants.

This is **plaintext loopback HTTP**. It exercises neither ChatGPT OAuth, TLS,
HTTP/2 nor the Codex passthrough adapter. A healthy control does not rule out a
stall on those paths and is not a cause fix.

Build and validate on Windows:

```powershell
cargo nextest run --locked --manifest-path scripts/diagnostics/windows-transport-control/Cargo.toml --test-threads 10
cargo clippy --locked --manifest-path scripts/diagnostics/windows-transport-control/Cargo.toml --all-targets -- -D warnings
cargo build --release --locked --manifest-path scripts/diagnostics/windows-transport-control/Cargo.toml
```

Run with a new fixture directory under scratch space owned by the caller:

```powershell
ocx-transport-control.exe BUN_EXE REPO_ROOT NEW_FIXTURE_ROOT NODE_MODULES BODY_MIB CONCURRENCY WAVES HOLD_MS [READ_DELAY_MS IDLE_SECONDS] | Out-String
```

The release executable has the Windows GUI subsystem. Pipe or redirect output
when starting it from PowerShell; direct scheduled invocation should redirect
stderr too. JSON artifacts in the new directory are the measurement record.
The process never sends pressure to the production listener.

Bodies contain synthetic ASCII text. Limits are 1–32 MiB per body, 1–40 clients,
640 MiB per wave and 1–8 waves. Each network operation shares a deadline; response
reads have a 1 MiB ceiling. The upstream can pause 0–20 ms per 16 KiB read, retain
its stream for 200–5000 ms and allow 10–300 seconds of post-load observation.
The last two arguments are optional together, defaulting to 0 ms and 10 seconds.

Before pressure, the control checks the child listener's identity, the effective
provider configuration and a complete preflight exchange at its own upstream.
It refuses to measure a silently substituted default configuration. Alternating
waves complete normally or close downstream after a unique upstream content
marker. Upstream counters distinguish completed bodies, first content writes,
mid-upload failures, stream closes, capacity refusals and error kinds by phase.
Client cancellation does not by itself prove that the upstream was interrupted:
use the settled counters for that distinction. Non-200 or non-cancelled clients
remain explicit outcomes rather than being described as passing.
A completed client also requires a successful terminal event and no failed event;
HTTP 200 followed by an SSE failure or premature EOF is not a complete exchange.

Samples include the held child's creation identity, Windows counters, actual
probe times and scalar authenticated memory snapshots. The latter add work and
do not impose a constant sample cadence. Windows private bytes, Bun external,
heap usage and registered-store retention are different counters; a short idle
plateau is not evidence of a persistent leak. The tool does not force GC.

The original process handle is used for measurement and owned-child cleanup.
Only that fixture tree is stopped. The fixture can spawn bounded ACL helpers; check
that these have settled before removing its homes. It can also perform normal
startup background work, so this control makes no blanket claim of zero other
network contacts. No real account is configured and no real credentials are
copied. Bun may not produce CPU/heap profiles before the bounded stop fallback.
The heap snapshot is taken at exit, after fixture shutdown, and is not the heap
at an earlier sampling instant. It also adds shutdown work.

**Keep artifacts private.** Synthetic child stderr, error messages and CPU/heap
profiles may contain source paths. Fixture homes contain generated state and
must not be published wholesale. Publish a reviewed scalar summary only, then
clean up the owned child, fixture homes and build artifacts.
