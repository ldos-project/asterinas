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

Pages are **copied** today: `restore.rs` reads them from the peer BAR and
writes them with `write_alien`.

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
- (none of the lending steps committed yet)
