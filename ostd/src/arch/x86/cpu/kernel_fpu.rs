// SPDX-License-Identifier: MPL-2.0

use alloc::boxed::Box;
use core::marker::PhantomData;

use super::context::{FpuContext, init_kernel_fpu_control_registers};
use crate::{
    irq::InterruptLevel,
    task::{CurrentTask, FpuContextAccess, Task},
};

/// A struct signifying the kernel is within an fpu section
/// Enables nested sections and dropping the last nested one ensures that the user fpu state is
/// restored if applicable
pub struct InKernelFpuSection {
    task: CurrentTask,
    #[cfg(debug_assertions)]
    expected_depth: usize,
    _not_send: PhantomData<*const ()>,
}

impl Drop for InKernelFpuSection {
    fn drop(&mut self) {
        assert!(self.task.fpu_section_depth() > 0);
        #[cfg(debug_assertions)]
        debug_assert_eq!(self.task.fpu_section_depth(), self.expected_depth);
        let depth = self.task.fpu_section_depth() - 1;
        self.task.set_fpu_section_depth(depth);
    }
}

/// Start a kernel fpu section and ensure that the following invariants are true:
/// - The kernel is not servicing an interrupt
/// - The user fpu state is saved
/// - Nested kernel fpu regions do not accidentally clobber each other
/// - In debug mode kernel fpu sections should resolve in the order they were created.
pub fn fpu_begin() -> InKernelFpuSection {
    assert!(InterruptLevel::current().is_task_context());
    let task = Task::current().expect("fpu_begin called without a current task");

    task.with_kernel_fpu_context(|kernel_context| {
        kernel_context.get_or_insert_with(|| Box::new(FpuContext::new()));
    });

    let outermost = task.fpu_section_depth() == 0;
    if outermost {
        if crate::task::try_with_current_fpu_context(|context| context.enter_kernel()).is_none() {
            task.enter_kernel();
        }
        init_kernel_fpu_control_registers();
    }
    let expected_depth = task.fpu_section_depth() + 1;
    task.set_fpu_section_depth(expected_depth);

    InKernelFpuSection {
        task,
        #[cfg(debug_assertions)]
        expected_depth,
        _not_send: PhantomData,
    }
}

/// Used to ensure that the kernel fpu sections save their state before a context switch
pub(crate) fn before_switch(_next_task: &Task) {
    if let Some(current_task) = Task::current()
        && current_task.fpu_section_depth() > 0
    {
        current_task.with_kernel_fpu_context(|context| {
            context.as_mut().unwrap().save();
        });
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::sync::Arc;
    use core::{
        arch::asm,
        sync::atomic::{AtomicBool, Ordering},
    };

    use super::fpu_begin;
    use crate::{
        prelude::ktest,
        task::{Task, TaskOptions},
    };

    #[ktest]
    fn nested_sections_preserve_depth() {
        let input = [1.5_f64, 2.5];
        let outer_value = [3.5_f64, 4.5];
        let inner_value = [5.5_f64, 6.5];
        let mut output = [0_f64; 2];
        unsafe { asm!("movupd xmm15, [{}]", in(reg) input.as_ptr(), options(nostack)) };
        let outer = fpu_begin();
        assert_eq!(Task::current().unwrap().fpu_section_depth(), 1);
        unsafe { asm!("movupd xmm15, [{}]", in(reg) outer_value.as_ptr(), options(nostack)) };

        let inner = fpu_begin();
        assert_eq!(Task::current().unwrap().fpu_section_depth(), 2);
        unsafe { asm!("movupd xmm15, [{}]", in(reg) inner_value.as_ptr(), options(nostack)) };
        drop(inner);

        unsafe { asm!("movupd [{}], xmm15", in(reg) output.as_mut_ptr(), options(nostack)) };
        assert_eq!(output, inner_value);
        assert_eq!(Task::current().unwrap().fpu_section_depth(), 1);
        drop(outer);

        unsafe { asm!("movupd [{}], xmm15", in(reg) output.as_mut_ptr(), options(nostack)) };
        assert_eq!(output, inner_value);
        assert_eq!(Task::current().unwrap().fpu_section_depth(), 0);
    }

    #[ktest]
    fn active_section_survives_task_switch() {
        let input = [7.5_f64, 8.5];
        let task_value = [9.5_f64, 10.5];
        let mut output = [0_f64; 2];
        let switched = Arc::new(AtomicBool::new(false));
        let switched_clone = switched.clone();

        unsafe { asm!("movupd xmm15, [{}]", in(reg) input.as_ptr(), options(nostack)) };
        let section = fpu_begin();
        unsafe { asm!("movupd xmm15, [{}]", in(reg) input.as_ptr(), options(nostack)) };

        let task = Arc::new(
            TaskOptions::new(move || {
                unsafe {
                    asm!("movupd xmm15, [{}]", in(reg) task_value.as_ptr(), options(nostack))
                };
                switched_clone.store(true, Ordering::Release);
                Task::yield_now();
            })
            .build()
            .unwrap(),
        );
        task.run();

        while !switched.load(Ordering::Acquire) {
            Task::yield_now();
        }

        unsafe { asm!("movupd [{}], xmm15", in(reg) output.as_mut_ptr(), options(nostack)) };
        assert_eq!(output, input);
        drop(section);
    }
}
