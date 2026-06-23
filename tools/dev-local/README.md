# Native local development on arm64 (no Docker)

This directory contains a **reproducible, Docker-free** dev setup for building
and running Asterinas natively on an **arm64 Linux host** — e.g. an OrbStack
Ubuntu VM on an Apple Silicon Mac.

Nothing here edits any tracked repo file. Everything lives under
`tools/dev-local/` and is driven by three scripts.

---

## Why this is needed: the three architectures

`make docker` exists because Asterinas' build pulls together components from
*three different architectures*, and the upstream solution is "do everything
inside one amd64 container". Locally on arm64 we untangle them:

| Layer | Arch | Notes |
|-------|------|-------|
| **Host** (your Mac / OrbStack VM) | arm64 | where the toolchain runs |
| **Build toolchain** | native arm64 | Rust cross-compiles, *fast*, no emulation |
| **The kernel** | **x86_64** | `rust-toolchain.toml` targets `x86_64-unknown-none` |
| **The guest userspace (initramfs)** | **x86_64** | it boots *inside* the x86 kernel, so it must be x86 |

Key consequences:

* **No acceleration.** KVM/HVF only accelerate a *same-arch* guest. An x86
  guest on an arm64 host must use QEMU **TCG** (software emulation). Boots are
  slow; builds are native-fast.
* **The guest userspace is irreducibly x86_64.** We don't need an x86 *host*,
  but the initramfs files *are* x86 binaries. busybox is statically linked, so
  a minimal boot needs just that one binary + the vDSO — no glibc harvesting.

## What `make docker` does (for reference)

1. Order-only prereq `./.cache/generated` (`Makefile`): `docker create`s a
   throwaway container from `ldosproject/asterinas:<DOCKER_IMAGE_VERSION>` and
   `docker cp`s `/root/.cargo` + `/root/.rustup` out of it into `./.cache/`.
2. `docker run --privileged --device=/dev/kvm -v $(pwd):/root/asterinas ...`
   drops you into a shell, where `make build` / `make run` call `cargo osdk`.

On an arm64 Mac that flow breaks because (a) `--device=/dev/kvm` doesn't exist,
and (b) the image is amd64-only, so it runs under slow Rosetta emulation. This
setup sidesteps all of it.

---

## Quick start

```bash
# 1. One-time provisioning (Phase 1 needs sudo for apt; rest is user-local)
tools/dev-local/setup-arm64.sh            # or: apt | rust | guest individually

# 2. Build + run (cross-compile kernel, boot to busybox shell under TCG)
tools/dev-local/run-local.sh              # Ctrl-A X to quit qemu
tools/dev-local/run-local.sh --build-only # just cross-compile
```

## What each script does

* **`setup-arm64.sh`** — installs host packages (`qemu-system-x86`, cross
  compiler, etc.), installs rustup + the pinned nightly + `cargo-osdk`, and
  cross-builds a static x86_64 busybox (`CONFIG_STATIC` + standalone shell,
  matching the project's container config) plus fetches the x86 vDSO into
  `guest-sysroot/`. Idempotent. Sub-commands: `apt`, `rust`, `guest`.
* **`build-initramfs.sh`** — packs `guest-sysroot/` into a minimal
  `initramfs.cpio.gz` (busybox + applet symlinks + vDSO + `/etc`). Bypasses
  `test/Makefile` entirely.
* **`run-local.sh`** — creates the scratch disk images that
  `tools/qemu_args.sh` expects, builds the initramfs if needed, and runs
  `cargo osdk run --boot-method=qemu-direct --grub-boot-protocol=multiboot
  --initramfs=...` with `OVMF=off`. This is exactly `make run
  BOOT_PROTOCOL=multiboot ENABLE_KVM=0` but with our own initramfs and without
  the failing `test/Makefile` initramfs prerequisite.

## Layout

```
tools/dev-local/
├── README.md
├── setup-arm64.sh
├── build-initramfs.sh
├── run-local.sh
├── guest-sysroot/        # staged x86 guest binaries (busybox, vdso64.so)
└── .work/                # scratch: downloads, busybox build tree, initramfs
```

`guest-sysroot/` and `.work/` are generated; add them to your local ignore if
you don't want them tracked.

---

## Verified working

This pipeline boots end-to-end on an Apple Silicon Mac (OrbStack Ubuntu 24.04
aarch64): native arm64 build → x86_64 kernel → QEMU TCG → unpacks the minimal
initramfs → mounts ext2 → spawns init → interactive busybox shell:

```
[kernel] Spawn init thread
[kernel] Hello world from kernel!
   _   ___ _____ ___ ___ ___ _  _   _   ___        (ASTERINAS banner)
~ #                                                (busybox prompt)
```

## Gotchas discovered (and how the scripts handle them)

These are non-obvious failure modes that the scripts work around automatically.
If you adapt this for another machine, keep them in mind:

1. **`unwinding 0.2.9` breaks the build.** OSDK builds in its own generated
   workspace (`target/osdk/aster-nix-run-base/`) with a *separate* `Cargo.lock`
   that ignores the repo pin. `ostd` declares `unwinding = "0.2.8"` (caret), so a
   fresh resolution grabs `0.2.9`, which fails against this nightly
   (`catch_unwind` intrinsic returns `i32`, not `bool`). `run-local.sh` pins it
   back to `0.2.8` in the generated lock. *(This bites anyone doing a clean build
   today, Docker included — it's time-triggered, not arch-specific.)*
2. **Incremental compilation corrupts the kernel link.** Rust incremental
   compilation intermittently emits a ~20-byte truncated kernel binary when
   relinking after a dependency change. Booting that gives QEMU's misleading
   `linux kernel too old to load a ram disk`. `run-local.sh` sets
   `CARGO_INCREMENTAL=0` and asserts the image is a real ELF (>1 MB) after build.
3. **`/ext2`, `/exfat`, `/raid1` must exist in the initramfs.** The kernel's
   `lazy_init()` mounts block devices onto these paths with `.unwrap()`; if the
   directories are missing it panics with `ENOENT`. `build-initramfs.sh` creates
   them.
4. **`/usr/bin/busybox` must be a real file, not a relative symlink.** A
   `../bin/busybox` symlink from `/usr/bin/` resolves to itself; the kernel then
   can't exec init. We copy the binary to both `/bin` and `/usr/bin`.
5. **Host port forwarding conflicts.** `tools/qemu_args.sh` forwards guest port
   22 to host 22 by default, which is privileged and taken by the VM's sshd —
   QEMU aborts. `run-local.sh` remaps it via `SSH_PORT=10022`.

## Troubleshooting

* **`Could not set up host forwarding rule 'tcp::NNNN'`** — a previous QEMU is
  still running and holding the forwarded ports (orphaned when a run is killed).
  Kill it: `pkill -9 -f qemu-system-x86_64` (or `make kill_qemu`).
* **`linux kernel too old to load a ram disk`** — the kernel image is a corrupt
  stub. Force a clean relink: `rm -rf target/osdk
  target/x86_64-unknown-none/debug/aster-nix-osdk-bin
  target/x86_64-unknown-none/debug/.fingerprint/aster-nix-osdk-bin-*` then re-run.
  `run-local.sh`'s post-build assertion does this for you and asks you to re-run.
* **Quit QEMU** — `Ctrl-A` then `X`.

## Performance — making the emulation less slow

An x86 guest on an arm64 host **cannot** be hardware-accelerated (KVM/HVF are
same-arch only; Rosetta only translates x86 *user-space* binaries, not a
kernel). QEMU must use TCG (software binary translation), which is inherently
~5–20× slower than KVM. There is no flag that closes that gap. What *does* help,
locally:

| Knob | Effect |
|---|---|
| `RELEASE=1 run-local.sh` | Optimized kernel — **~14× smaller image** (8.4 MB vs 120 MB) and far fewer instructions for TCG to translate. Biggest easy win. |
| `RELEASE_LTO=1 run-local.sh` | Even faster to run, much slower to compile. |
| `SMP=4 run-local.sh` | More vCPUs + auto-enables multi-threaded TCG (`-accel tcg,thread=multi`) so QEMU uses multiple host cores. Helps multi-threaded guest workloads, not single-threaded boot. Give the OrbStack VM several cores. |
| `cargo osdk test` (ktests) | For iterating on kernel *logic*, avoids a full boot+userspace cycle. |

For genuine near-native speed, run the **same source on a real x86 Linux host
with KVM** (`make run`) — a cheap x86 cloud VM works. Keep the Mac for
native-fast builds + correctness checks.

Debug vs release: the default build is debug (unoptimized, easy to debug, slow
under TCG). Use `RELEASE=1` once you care about runtime speed.

## Scope & limitations

* **Tier 1 only.** This boots to a shell and runs kernel/ktests. It does **not**
  build the benchmark/app userspace (redis, nginx, fio, lmbench, dropbear,
  gvisor syscall tests). Those are custom x86 builds that live only in the
  amd64 dev image. To enable them ("Tier 2"), harvest those trees once from
  `ldosproject/asterinas:<version>` (e.g. `docker export` via OrbStack's Docker
  on the Mac) into `guest-sysroot/`, then build the initramfs from them.
* **GRUB / UEFI boot.** `grub-pc-bin` isn't packaged for arm64, so the default
  `grub-rescue-iso` boot method is impractical here. We use `qemu-direct` +
  multiboot, which needs neither GRUB nor OVMF.
* **Speed.** Kernel builds are native arm64 (fast). Guest execution is TCG
  (slow boots, fine for development/correctness).
