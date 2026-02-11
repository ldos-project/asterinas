# cargo osdk test

`cargo osdk test` is used to
execute kernel mode unit test by starting QEMU.
The usage is as follows:

```bash
cargo osdk test [TESTNAME] [OPTIONS] 
```

## Arguments 

`TESTNAME`:
Only run tests containing this string in their names

## Options

The options are the same as those of `cargo osdk build` with the addition of `--gdb-server` from `cargo osdk run`.
Refer to the [documentation of `cargo osdk build`](build.md) and [documentation of `cargo osdk run`](run.md)
for more details.

The `cargo osdk debug` command will *not* work with `cargo osdk test` instead, you must use `gdb`
directly. The command should be:
```
gdb <KERNEL IMAGE> -ex "target remote <DEBUG ADDR>"
```
The kernel image path is printed during `cargo osdk test` in a line like: `Kernel image: <KERNEL IMAGE>`. 
The debug address is whatever you passed to `--gdb-server`'s `addr=`.

For example,
```
$ cargo osdk test --gdb-server wait-client,vscode,addr=:1234
[...]
Kernel image: /root/asterinas/target/x86_64-unknown-none/debug/ostd-osdk-bin
[...]
```
and in another terminal,
```
$  gdb /root/asterinas/target/x86_64-unknown-none/debug/ostd-osdk-bin -ex "target remote :1234"
```

## Examples
- Execute tests that include *foo* in their names 
using QEMU with 3GB of memory

```bash
cargo osdk test foo --qemu-args="-m 3G"
```
