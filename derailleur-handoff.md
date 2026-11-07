# derailleur on Asterinas: handoff

Branch `aneesh/derailleur` here, based on Asterinas `main`. Companion repo:
`~/test/vm_live_migration`. Its branch `asterinas` holds the harness, the
`derailleur` tool and docs, and its `linux/` submodule (branch `asterinas`)
holds the Linux server. Read there: `docs/cassette.md` (the protocol),
`docs/asterinas.md` (feasibility, plus a "Prototype" section),
`results/xos_asterinas.txt`. The interface header is
`linux/include/uapi/linux/derailleur_cassette.h`.

## What works (committed)

Linux VM1 → Asterinas VM2 process migration, with Asterinas as the cassette
client.
- `hello`: pid and stack address kept, counter continuous, clean exit.
- Redis 7.4 (6 threads, 11 fds): `DEBUG DIGEST` identical. Under host load,
  0 of 1.25M acknowledged writes lost.

Commits:
- `c224d94`: stage 1. ivshmem driver, cassette client, syscalls 1000–1002,
  `clone3` `set_tid`, vDSO syscall stubs.
- `c01c377`: threads, fds (syscall 1003 `take`), signal dispositions, cwd,
  umask, init-stack adoption.
- `eb93884`: tidy.

Code lives in `kernel/core/src/derailleur/`:
- `device.rs`: ivshmem driver plus transport
- `restore.rs`: the client
- `wire.rs`: header offsets

Hooks elsewhere:
- `process/clone.rs`: `child_tid`, `derailleur_spawn_thread`
- `process/process_vm/{mod,heap,init_stack}.rs`: `derailleur_*` helpers
- `syscall/derailleur.rs` and the entries in `syscall/arch/x86.rs`

The current working tree **lends exclusive anonymous pages in place** from
the peer BAR; other pages are still copied with `write_alien`.
`ASTER_KCMD=derailleur.copy` selects the copy-everything comparison path.

## Current handoff latency

Latest recorded comparisons: x86-64 release build, four vCPUs, default
Mariposa/OQueues, unpinned on the same host.
These are single runs, not medians or tail-latency estimates.
Raw logs are in the companion repo under
`share/results/xos_{lend,copy}_{hello,privmem,mmchurn,redis,redis_load}/`;
the detailed results are in `results/xos_asterinas.txt`.

For Redis under load, the observed client-visible interruption was
**9.272 ms with lending**, versus **36.391 ms with copying**.
This is the longest interval between successful replies on any of four
host connections, including reconnect time; established TCP connections
are still reset during migration.
No acknowledged writes were lost (968983 checked with lending,
1272402 with copying).

The kernel's measured restore interval (`state` + `memory+threads`) was:

| Workload | Lending | Copying |
|---|---:|---:|
| `hello 60` | 1.004 ms | 1.020 ms |
| `wl_privmem 8` | 2.402 ms | 10.468 ms |
| `wl_mmchurn 10000` | 15.147 ms | 158.507 ms |
| Redis idle, 20k-key setup | 6.911 ms | 11.829 ms |
| Redis under load, same setup | 7.963 ms | 35.058 ms |

**Measurement boundary:** this kernel interval starts inside `restore`
(syscall 1001), after `take` has stopped VM1 and the guest tool has
recreated file descriptors.
It ends after memory and thread restoration, before the leader resumes
user mode; it is **not the complete stop-to-resume downtime**.
The copy path also includes its immediate VM1 release RPC in this interval;
the lending path defers release until the process exits and every lent
frame is reclaimed, so that later cleanup is not part of handoff latency.
The Redis reply-gap measurement is the available end-to-end service
interruption indicator; exact stop-to-first-user-instruction timing has
not yet been instrumented.

## Build and run

- Build, in the container:

  ```
  docker run --rm --privileged --network=host -v /dev:/dev \
    -v $HOME/asterinas:/root/asterinas ldosproject/asterinas:0.18.0-ldos-20260831 \
    bash -c "cd /root/asterinas && make kernel BOOT_PROTOCOL=multiboot RELEASE=1"
  ```

  (Omit `RELEASE=1` for debug: about 20 s rebuilds.)
- Run, from `~/test/vm_live_migration`:
  - hello:

    ```
    VM2_OS=asterinas KEXTRA="kho=on liveupdate=1" ./run.sh x premap hello 60
    ```
  - Redis:

    ```
    NET=1 VM2_OS=asterinas XOS_READY=1 HOSTCHECK=asterinas/redis_check.sh \
      KEXTRA="kho=on liveupdate=1" ./run.sh x premap wl_redis_mig.sh 20000
    ```

    Add `HOSTLOAD="12 4"` for the run under load.
- VM2's log is `share/results/x/vm2.log`.

## In progress: lending pages in place (no copy)

Plan, in separate commits so each can become its own PR:

1. **OSTD `mm/frame/peer.rs`.** `PeerMemory::register(IoMem)`:
   - takes the BAR range
   - allocates metadata pages one at a time and initializes them as unused
     slots (as `alloc_meta_frames` does)
   - maps them at `frame_to_meta(range)` and maps the range into the linear
     map with write-back caching (`kspace` helper, `KERNEL_PAGE_TABLE`
     cursor, `MappedItem::Untracked`)
   - then publishes the range (atomics)

   `peer.frame(paddr) -> UFrame` (`Frame::from_unused(paddr,
   PeerFrameMeta, 1)`). `PeerFrameMeta` is untyped, and its `on_drop`
   decrements a live counter.
2. **OSTD hooks:**
   - `meta.rs` `get_slot`: accept peer paddrs above `max_paddr`.
   - `frame/mod.rs:280` (`Frame::drop`) and `unique.rs:167`: don't dealloc
     peer frames.

   `drop_last_in_place` already resets the slot to `REF_COUNT_UNUSED`, so a
   frame can be lent again.
3. **Kernel `device.rs`:** register the peer BAR through `PeerMemory`
   instead of a plain `IoMem`, keeping an `IoMem` for byte reads.
4. **Kernel `restore.rs`:** pages flagged `CAS_GPA_MOVABLE` (exclusive
   anonymous pages on VM1) are mapped as anonymous pages directly. This
   needs a `Vmar` helper that maps a given `UFrame` at an address with the
   mapping's flags (see `vm_mapping.rs` `prepare_page`/`cursor.map(frame,
   prop)`; COW is decided by `reference_count`). Other pages are still
   copied.
5. **Release protocol:** stop sending `release` right after the restore.
   `derailleur done` (syscall 1002) waits until the live peer-frame count is
   0, then sends `release`. VM1 holds its pins until then.
6. **Test:** `wl_privmem` (fork COW, `MADV_DONTNEED`, mremap, stack) needs
   the harness's `XOS_READY=1` workload path. Then `wl_mmchurn` and Redis.
   Record results in `results/xos_asterinas.txt`.

Risks to watch:
- kernel page-table changes after SMP start (newly present PTEs need no
  flush on x86)
- code that assumes frames are below `max_paddr`
- `hugepaged`
- Mariposa OQueue observers of frame events

Progress log:
- Steps 1-2 done: `c82424c` (ostd peer memory). Builds.
- Steps 3-5 done, **builds but untested**: next commit after `c82424c`
  (device registers PeerMemory; restore lends MOVABLE pages in private
  mappings via `Vmar::derailleur_install_frame`; `done()` waits for
  live_frames()==0 then sends release; `derailleur.copy` flag = old copy path).
- Continuation (uncommitted working-tree changes): the guest now calls DONE
  after successfully reaping the child; host lifecycle tests cover exit,
  signal, EINTR, wait errors and DONE errors (`make -C guest check`).
- Fixed peer GPA translation to include the BAR base, and two remaining
  `max_paddr` debug assertions in frame reference operations.
- Fixed RCU reclamation starvation: idle CPUs now report quiescent states in
  `halt_cpu`, and active user tasks report at `UserMode::execute`. Without
  these, four-CPU hello held all lent pages after exit and mmchurn ran out of
  memory. One-CPU comparisons and a temporary RCU CPU-mask probe isolated it.
- Fixed stack growth in BOTH restore paths: reserve VM_GROWSDOWN space up to
  the source RLIMIT_STACK, clipped above existing mappings with a guard page.
  Unlimited source stacks use Asterinas's default reservation (8 MiB).
- DONE keeps its pending-session state on errors and checks release status.
- Release tests pass: hello, wl_privmem, wl_mmchurn, idle Redis, loaded Redis,
  each with lending and `ASTER_KCMD=derailleur.copy`. Every guest exits 0 and
  VM1 reports `offer_exit=0`; peer registration is present in every boot log.
  Redis under load lost 0/968983 acknowledged writes with lending and
  0/1272402 with copying. Details/commands/raw log names are recorded in the
  companion `results/xos_asterinas.txt`.
- Debug lending tests pass for hello, privmem, mmchurn. For short debug
  workloads start VM1 later (`VM_LATE=5`) so it cannot exit before VM2 is
  ready. Baseline release hello and mmchurn also pass.
- Companion `asterinas/role.sh` prints the workload summary and saves
  `/tmp/wl.out` to `/ext2/wl.out`; `bench/xos_check.py` checks logs, not just
  run.sh's status (which can falsely succeed after a timeout, or return 1
  when a completed Redis load subprocess is killed during cleanup).
- `make test` and guest checks pass. Full `make check` is blocked by format
  differences (including untouched files); direct clippy stops on
  `device.rs:210` (`manual_is_multiple_of`). Hugepaged was not explicitly
  enabled. Remaining: independent safety review of peer registration and
  dynamic kernel mappings, hugepaged-on tests, and PR-sized commits when
  requested. Do not treat these runtime tests as a general safety proof.
