// SPDX-License-Identifier: MPL-2.0

//! `print` and `println` macros
//!
//! FIXME: It will print to all `virtio-console` devices, which is not a good choice.
//!

use alloc::{collections::btree_map::BTreeMap, fmt, string::String, sync::Arc};
use core::fmt::Write;

use aster_console::AnyConsoleDevice;
use ostd::sync::{LocalIrqDisabled, SpinLockGuard};

/// Prints the formatted arguments to the standard output.
pub fn _print(args: fmt::Arguments, append_nl: bool) {
    // We must call `all_devices_lock` instead of `all_devices` here, as `all_devices` invokes the
    // `clone` method of `String` and `Arc`, which may lead to a deadlock when there is low memory
    // in the heap. (The heap allocator will log a message when memory is low.)
    //
    // Also, holding the lock will prevent the logs from interleaving.
    if let Some(devices) = aster_console::all_devices_lock() {
        struct Printer<'a>(
            SpinLockGuard<'a, BTreeMap<String, Arc<dyn AnyConsoleDevice>>, LocalIrqDisabled>,
        );
        impl Write for Printer<'_> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                self.0
                    .values()
                    .for_each(|console| console.send(s.as_bytes()));
                Ok(())
            }
        }

        let mut printer = Printer(devices);
        printer.write_fmt(args).unwrap();
        if append_nl {
            printer.write_char('\n').unwrap();
        }
    } else {
        // Fall-back to early_print if we have no console setup.
        ostd::console::early_print(args);
    }
}

/// Copied from Rust std: <https://github.com/rust-lang/rust/blob/master/library/std/src/macros.rs>
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        $crate::_print(format_args!($($arg)*), false);
    }};
}

/// Copied from Rust std: <https://github.com/rust-lang/rust/blob/master/library/std/src/macros.rs>
#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {{
        $crate::_print(format_args!($($arg)*), true);
    }};
}

#[cfg(ktest)]
mod test {
    use ostd::prelude::*;

    #[ktest]
    fn test_println() {
        crate::println!("test");
    }
}
