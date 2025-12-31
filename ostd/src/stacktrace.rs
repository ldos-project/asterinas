// SPDX-License-Identifier: MPL-2.0

//! Utilities for quickly capturing and storing stack traces.
//!
//! This provides a way to capture a stack trace at interesting points and print it later if some
//! error occurs. A good example is capturing the stack when a lock is acquired and then printing
//! that stack of a deadlock is detected.
//!
//! This does nothing and consumes no space unless the `capture_stacks` feature is enabled.
//!
//! ## Displaying [`CapturedStackTrace`]
//!
//! The `Display` implementation on [`CapturedStackTrace`] formats the trace in the form:
//!
//! ```text
//! stacktrace[0xdeadbeef, ..., 0xdeadbeef]
//! ```
//!
//! where the `0xdeadbeef` are actually the PCs at each stack frame. This format is not really human
//! readable, but there is a tool to parse it. `cargo osdk enhance-log` will process a log file and
//! add a nicely formatted stack trace after any line containing the above. See
//! `docs/src/osdk/reference/commands/enhance-log.md` for more details.

/// The number of stack frames to capture [`CapturedStackTrace`]. This defines the size of that data
/// structure.
pub const STACK_DEPTH: usize = 8;

/// A compact stack trace representation with at most [`STACK_DEPTH`] frames.
#[derive(Clone, Copy, Default)]
pub struct CapturedStackTrace {
    #[cfg(feature = "capture_stacks")]
    frames: tinyvec::ArrayVec<[usize; STACK_DEPTH]>,
}

impl core::fmt::Debug for CapturedStackTrace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&self, f)
    }
}

impl core::fmt::Display for CapturedStackTrace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("stacktrace[")?;
        #[cfg(feature = "capture_stacks")]
        {
            let mut first = true;
            for frame in self.frames.iter() {
                if !first {
                    f.write_str(", ")?;
                }
                write!(f, "0x{:x}", frame)?;
                first = false;
            }
        }
        #[cfg(not(feature = "capture_stacks"))]
        {
            f.write_str("not captured")?;
        }
        f.write_str("]")?;
        Ok(())
    }
}

impl CapturedStackTrace {
    /// Capture the first [`STACK_DEPTH`] frames of the stack. This will skip over the first `skip`
    /// frames counting from the caller to this function. So, with `skip = 0` the first captured
    /// frame will be the caller to this function.
    pub fn capture(skip: usize) -> Self {
        #[cfg(feature = "capture_stacks")]
        {
            use core::ffi::c_void;

            use unwinding::abi::{
                _Unwind_Backtrace, _Unwind_GetIP, UnwindContext, UnwindReasonCode,
            };

            struct CallbackData {
                count: usize,
                skip: usize,
                res: CapturedStackTrace,
            }
            extern "C" fn callback(
                unwind_ctx: &UnwindContext<'_>,
                arg: *mut c_void,
            ) -> UnwindReasonCode {
                let data = unsafe { &mut *(arg as *mut CallbackData) };
                let pc = _Unwind_GetIP(unwind_ctx);
                if pc == 0 {
                    return UnwindReasonCode::NORMAL_STOP;
                }
                if data.count >= data.skip {
                    if data.res.frames.try_push(pc).is_some() {
                        // Stop if there is no more space available in the frames vec.
                        return UnwindReasonCode::NORMAL_STOP;
                    }
                }
                data.count += 1;
                UnwindReasonCode::NO_REASON
            }

            let mut data = CallbackData {
                skip: skip + 1,
                count: 0,
                res: Default::default(),
            };
            _Unwind_Backtrace(callback, &mut data as *mut _ as _);
            data.res
        }
        #[cfg(not(feature = "capture_stacks"))]
        {
            let _ = skip;
            Default::default()
        }
    }
}
