# Debugging Tools in OSTD

## Context/Stack Capture

OSTD captures context information when errors occur and optionally at other points (see [Mutex
Tacking](#mutex-tracking)). By default, this includes a file, line, task ID, CPU ID, and ORPC server
ID. However, OSTD supports capturing a stack trace as well by enabling the `capture_stacks` feature
at build time.

When enabled, the stack traces are printed as part of context information. For example,

```
at /root/asterinas/ostd/src/sync/mutex.rs:283:27 on task 1 on cpu 0 with stack: stacktrace[0xffffffff882f64d8, 0xffffffff882921ee, 0xffffffff880976bb, 0xffffffff880d7bcf, 0xffffffff880d7bf8, 0
xffffffff880d7b5f, 0xffffffff8809453a, 0xffffffff8833d5f7]
```

This can be processed by the `cargo osdk enhance-log` command to generate a human readable trace.
For example, 

```
Held by: at /root/asterinas/ostd/src/sync/mutex.rs:283:27 on task 1 on cpu 0 with stack: stacktrace[0xffffffff882f64d8, 0xffffffff882921ee, 0xffffffff880976bb, 0xffffffff880d7bcf, 0xffffffff880d7bf8, 0xffffffff880d7b5f, 0xffffffff8809453a, 0xffffffff8833d5f7]
   0: ostd::sync::mutex::Mutex<T>::lock::ha4b564681cc7cad2 (/root/asterinas/ostd/src/sync/mutex.rs:95)
   1: ostd::sync::mutex::test::test_mutex_tracking_threaded::h6b23e7d20b0d86ef (/root/asterinas/ostd/src/sync/mutex.rs:283)
   2: core::ops::function::FnOnce::call_once::h6a33de7eadb05fcb (/root/.rustup/toolchains/nightly-2025-02-01-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/ops/function.rs:250)
   3: unwinding::panicking::catch_unwind::do_call::h34b6a6d54e9dd494 (/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/unwinding-0.2.5/src/panicking.rs:56)
   4: __rust_try (e2q97i96fvjyuwzdh4qxfmf88:?)
   5: unwinding::panicking::catch_unwind::h94943de1d9431cef (/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/unwinding-0.2.5/src/panicking.rs:42)
   6: unwinding::panic::catch_unwind::h6019943d95a1daf9 (/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/unwinding-0.2.5/src/panic.rs:87)
   7: ostd_test::KtestItem::run::h81c2cad0dcab6581 (/root/asterinas/ostd/libs/ostd-test/src/lib.rs:149)
```

You can enable this feature with `--features ostd/capture_stacks` on the `cargo osdk` command line.
(Or the `FEATURES=ostd/capture_stacks` environment variable for OSTDs own makefiles.)

## Mutex Tracking

When debugging deadlocks it is useful to determine, not just which thread hung, but which thread is
holding the lock. To allow this, OSTD supports capturing context when locks are acquired and
printing it when a lock cannot be acquired. This must be enabled in the build by the `track_mutex`
feature, and by calling `Mutex::with_acquire_info(true)` on the specific mutex.

When enabled, the program will log (with level `info`) whenever a mutex blocks:

```
INFO: Failed to acquire lock:
Held by: [information about holder]
Failed at: [information about failing acquire]
```

If [`capture_stacks` is enabled](#contextstack-capture), then the information will include stacks.
You can enable both feature with `--features ostd/track_mutex,ostd/capture_stacks` on the `cargo
osdk` command line. (Or the `FEATURES=ostd/track_mutex,ostd/capture_stacks` environment variable for
OSTDs own makefiles.)
