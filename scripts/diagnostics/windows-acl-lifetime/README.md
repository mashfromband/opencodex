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
