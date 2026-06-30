// SPDX-License-Identifier: MPL-2.0

//! Kernel "oops" handling.
//!
//! In Asterinas, a Rust panic leads to a kernel "oops". A kernel oops behaves
//! as an exceptional control flow event. If kernel oopses happened too many
//! times, the kernel panics and the system gets halted. Kernel oops are per-
//! thread, so one thread's oops does not affect other threads.
//!
//! Though we can recover from the Rust panics. It is generally not recommended
//! to make Rust panics as a general exception handling mechanism. Handling
//! exceptions with [`Result`] is more idiomatic.

use alloc::{boxed::Box, format, string::String, sync::Arc};
use core::{
    result::Result,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use ostd::{
    panic,
    panic::CaughtPanic,
    stack_info::{Location, StackInfo},
    task::disable_preempt,
};

use super::Thread;
use crate::kcmdline::get_kernel_cmd_line;

// TODO: Control the kernel commandline parsing from the kernel crate. In Linux it can be
// dynamically changed by writing to `/proc/sys/kernel/panic`.
/// If true, then the kernel will abort if any part of the system panics. Otherwise, the panic will
/// unwind and only abort if it is not caught.
static ABORT_ON_PANIC: AtomicBool = AtomicBool::new(false);

/// If abort-on-panic is true, then the kernel will abort if any part of the system panics (even
/// inside an ORPC server). Otherwise, the panic will unwind and only abort if it is not caught.
/// This defaults to `false`.
pub fn set_abort_on_panic(v: bool) {
    ABORT_ON_PANIC.store(v, Ordering::SeqCst);
}

/// Configure the panic system. It will work before this is called. This just configures it based on
/// any user options.
pub(crate) fn configure() {
    set_abort_on_panic(
        get_kernel_cmd_line()
            .and_then(|k| k.get_module_arg_by_name("kernel", "abort_on_panic"))
            .unwrap_or(false),
    );
}

/// The kernel "oops" information.
#[expect(unused)]
pub struct OopsInfo {
    /// The "oops" message.
    pub message: String,
    /// The thread where the "oops" happened.
    pub thread: Arc<Thread>,
}

/// Executes the given function and catches any panics that occur.
///
/// All the panics in the given function will be regarded as oops. If a oops
/// happens, this function returns `None`. Otherwise, it returns the return
/// value of the given function.
///
/// If the kernel is configured to panic on oops, this function will not return
/// when a oops happens.
pub fn catch_panics_as_oops<F, R>(f: F) -> Result<R, CaughtPanic>
where
    F: FnOnce() -> R,
{
    let result = panic::catch_unwind(f);

    match result {
        Ok(result) => Ok(result),
        Err(err) => {
            let caught_panic = err.downcast::<CaughtPanic>().unwrap();

            ostd::error!("Oops! {}", caught_panic);

            let count = OOPS_COUNT.fetch_add(1, Ordering::Relaxed);
            if count >= MAX_OOPS_COUNT {
                // Too many oops. Abort the kernel.
                ostd::error!("Too many oops. The kernel panics.");
                panic::abort();
            }

            Err(*caught_panic)
        }
    }
}

/// The maximum number of oops allowed before the kernel panics.
///
/// It is the same as Linux's default value.
const MAX_OOPS_COUNT: usize = 10_000;

static OOPS_COUNT: AtomicUsize = AtomicUsize::new(0);

#[ostd::panic_handler]
fn panic_handler(info: &core::panic::PanicInfo) -> ! {
    let message = info.message();

    if let Some(thread) = Thread::current() {
        let abort_on_panic = ABORT_ON_PANIC.load(Ordering::Relaxed);
        if !abort_on_panic && info.can_unwind() {
            // TODO(arthurp): This state capture involves memory allocation. So OOM panics will
            // probably need to abort. This could be changed to use a special reserved pool of
            // memory to allow at least some OOM panics to be unwound.

            // skip 3 frames: this function, __ostd_panic_handler, rust_begin_unwind
            let mut stack_info = StackInfo::new(3);
            stack_info.location = info.location().map(|l| Location::from_location(l));

            // Raise the panic and expect it to be caught. If this returns, then the catch_unwind is
            // broken or missing.
            let r = panic::begin_panic(Box::new(ostd::panic::CaughtPanic {
                message: format!("{}", message),
                context: Some(stack_info),
            }));
        }
    }

    let preempt_guard = disable_preempt();
    let thread = Thread::current();
    let context = StackInfo::new(1);

    // Halt the system if the panic is not caught.

    ostd::error!(
        "Uncaught panic:\n\t{}\n\t{}\n\tby thread {:?}",
        message,
        context,
        thread,
    );

    if info.can_unwind() {
        panic::print_stack_trace();
    } else {
        ostd::error!("Backtrace is disabled.");
    }

    panic::abort();
}
