use core::ffi::c_void;

use unwinding::abi::{_Unwind_Backtrace, _Unwind_GetIP, UnwindContext, UnwindReasonCode};

use crate::early_println;

/// The number of stack frames to capture [`CapturedStackTrace`]. This defines the size of that data
/// structure.
pub const STACK_DEPTH: usize = 8;

/// A compact stack trace representation with at most [`STACK_DEPTH`] frames.
#[derive(Clone, Default)]
pub struct CapturedStackTrace {
    #[cfg(feature = "capture_stacks")]
    frames: tinyvec::ArrayVec<[usize; STACK_DEPTH]>,
}

impl CapturedStackTrace {
    /// Print the stack trace as a bare series of addresses.
    pub fn print(&self) {
        #[cfg(feature = "capture_stacks")]
        {
            for frame in self.frames.iter() {
                early_println!("0x{:x}", frame);
            }
        }
    }

    /// Capture the first [`STACK_DEPTH`] frames of the stack. This will skip over the first `skip`
    /// frames counting from the caller to this function. So, with `skip = 0` the first captured
    /// frame will be the caller to this function.
    #[cfg(feature = "capture_stacks")]
    pub fn capture(skip: usize) -> Self {
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
    pub fn capture() -> Self {
        Default::default()
    }
}
