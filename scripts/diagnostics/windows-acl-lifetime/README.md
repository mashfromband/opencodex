# Windows default ACL lifetime diagnostic

Standalone optional Rust supervisor, Windows only:

`todo1936-acl-lifetime BUN INSTALLED_SOURCE NEW_OUTPUT`

All paths must be absolute and the output directory must be new. The supervisor
starts one hidden Bun child with a cleared environment and separate home,
configuration and temporary directories. It calls the installed, unmodified
`hardenSecretPathAsync` on six small synthetic files. It uses the default ACL
runner, including its original exit-then-read stdout order. The instrumentation
returns each real subprocess unchanged and takes only a WeakRef, plus closed
numeric spawn timings. It never reads subprocess getters or changes the runner.

Collection runs only in that independent child. Output contains closed numeric
memory and class counters; missing class counts stay null. The Rust supervisor
samples its own child, requires actual success and zero active job processes,
and checks six completed harden calls and zero live tracked subprocess objects.
Descendants remain in an owned kill-on-close job. Startup waits for assignment
before importing the ACL module. Child execution has a 90-second deadline and
job reaping a five-second deadline. Error cleanup also uses a bounded wait.

Validation on Windows with Bun 1.4.2: Rustfmt, Clippy with warnings denied,
release build and one complete six-file execution. Nineteen real subprocesses
(one identity lookup, eighteen ACL commands) exited, and all tracked WeakRefs
were collected. Closed heap class counts can include objects outside that
tracked set and need not be zero. No Linux/macOS execution is claimed.

This verifies the default runner's small successful path. It does not prove
production retention, large-file behavior, timeout-path lifetime or the cause
of an older event-loop stall.

Additional modes use the same new-output contract:

| Mode | Execution |
| --- | --- |
| `parallel` | Six concurrent files through the unmodified default ACL runner |
| `burst` | Concurrent default ACL calls with external owned-child health probes |
| `parallel-early` | Explicit test-seam runner starts output reading before exit wait |
| `parallel-ignore` | Explicit test-seam runner discards ACL output; timing control only |
| `native-late`, `native-early` | Six owned Rust emitters, no opencodex imports or ACL |
| `native-stderr-late`, `native-stderr-early` | The same native controls on stderr |
| `native-late-refresh`, `native-stderr-late-refresh` | Late reads, then existing public unref after actual exit and full read |

Native emitters write a fixed small payload. Late reads begin only after actual
exit; early reads begin first. The final tracked WeakRef count remains a strict
zero requirement: unfixed runtimes intentionally exit nonzero when it fails,
even though all six children exited successfully and no job process remains.
Both immediate and later GC observations are kept to distinguish delayed
finalization from a retained root. Early/ignore/refresh controls are not a
replacement for the stock-path evidence. Missing class counters stay null.

Observed on Windows: default concurrent ACL retained twelve of nineteen tracked
objects; early/ignore ACL controls retained none. Native late stdout reads
retained five on Bun 1.4.0, six on 1.4.2, and four on 1.4.3. Native late stderr
retained four on 1.4.2. Early and post-exit refresh controls collected all tracked
objects. Counts describe these finite runs, not a universal rate or the full
production memory total. No production collection is performed.

`--write-bun-test ABSOLUTE_NEW_FILE` emits a temporary Bun-runner regression
beside a Bun checkout's `test/` directory (for example in its `.tmp/`). It uses
that checkout's GC helper and checks both late-output getters. The emitter path
is JSON-escaped. Run the generated file through that checkout's `bun bd test`
to verify a native patch; a system Bun run supplies the unfixed comparison.
The generated file is temporary test input, not another proxy implementation.
