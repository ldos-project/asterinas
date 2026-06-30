# cargo osdk enhance-log

## Overview

`cargo osdk enhance-log` parses a log file and adds formatted stack traces and improves existing
stack traces by resolving symbols. This is specifically useful for stack traces captured with
`CapturedStackTrace` which are printed as a list of addresses and nothing more (for compactness).

This requires the kernel image in use when the stack trace was created. This is generally in a
subdirectory of the `target/osdk/` directory (for example,
`target/osdk/aster-nix/aster-nix-osdk-bin`). This is not the ISO used for booting, but the raw
kernel image.


## Options

`[LOGFILE]`: 
The name of the log file to process. Within Asterinas, `qemu.log` is an appropriate file generated
on every kernel run. Default: stdin

`-b <BINFILE>`, `--bin-file <BINFILE>`: 
The kernel binary to use for symbol resolution.

`-o <FILE>`, `--output <FILE>`: 
The file to output the enhanced log to. Default: stdout

## Examples

Given a log line:

```
[    17.126] ERROR: at /root/asterinas/ostd/src/orpc/oqueue/locking.rs:301:69 on thread 434 on cpu 0 with stack: stacktrace[0xffffffff8835273a, 0xffffffff88138649, 0xffffffff880a88bd, 0xffffffff88560eef, 0xffffffff882ba62c, 0xffffffff884f1d0e, 0xffffffff8835d1a4, 0xffffffff8835d3d3]
```

The command `cargo osdk enhance-log -b target/osdk/aster-nix/aster-nix-osdk-bin qemu.log` can generate the additional lines:

```
   0: ostd::orpc::oqueue::AllocationFailedSnafu<__T0,__T1>::fail::hbdd93d98baf87454 (/root/asterinas/ostd/src/orpc/oqueue/mod.rs:?)
   1: <ostd::orpc::oqueue::locking::LockingQueue<T> as ostd::orpc::oqueue::OQueue<T>>::attach_strong_observer::h27bf2cc79f9adccd (/root/asterinas/ostd/src/orpc/oqueue/locking.rs:301)
   2: ostd::orpc::statistics::OutstandingCounter::spawn::h948060c2e5c7d586 (/root/asterinas/ostd/src/orpc/statistics.rs:57)
   3: aster_nix::fs::utils::page_prefetch::ReadaheadPrefetcher::spawn::h2df9a187437bfd9c (/root/asterinas/kernel/src/fs/utils/page_prefetch.rs:64)
   4: aster_nix::fs::utils::page_cache::PageCache::start_prefetcher::h6d936b02dbc66fc7 (/root/asterinas/kernel/src/fs/utils/page_cache.rs:125)
   5: aster_nix::fs::ext2::block_group::BlockGroup::load::h1c5d0d2262c5e466 (/root/asterinas/kernel/src/fs/ext2/block_group.rs:98)
   6: aster_nix::fs::ext2::fs::Ext2::open::{{closure}}::ha7c2053aacffe8cc (/root/asterinas/kernel/src/fs/ext2/fs.rs:73)
   7: core::result::Result<T,E>::unwrap::h0e8670b4a9bd9163 (/root/.rustup/toolchains/nightly-2025-02-01-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/result.rs:1107)
```

This provides resolves symbols and line information. 

`cargo osdk enhance-log` will also enhance the `pc` lines from the default `ostd` panic traces:

```
  12: fn 0xffffffff88605000 - pc 0xffffffff886050bb / registers:
```

Generates an additional line:

```
  12: aster_nix::init_thread::h70031c4def8c72d6 (/root/asterinas/kernel/src/lib.rs:155)
```