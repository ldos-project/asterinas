// SPDX-License-Identifier: MPL-2.0

//! Tasks are the unit of code execution.

pub mod atomic_mode;
mod kernel_stack;
mod preempt;
mod processor;
pub mod scheduler;
mod utils;

use alloc::sync::Weak;
use core::{
    any::Any,
    borrow::Borrow,
    cell::{Cell, RefCell, SyncUnsafeCell},
    fmt::Display,
    ops::Deref,
    panic::RefUnwindSafe,
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
};

use kernel_stack::KernelStack;
use processor::current_task;
use spin::Once;
use utils::ForceSync;

pub use self::{
    preempt::{DisabledPreemptGuard, disable_preempt, halt_cpu},
    scheduler::info::{AtomicCpuId, TaskScheduleInfo},
};
pub(crate) use crate::arch::task::{TaskContext, context_switch};
use crate::{cpu::context::UserContext, prelude::*, sync::Mutex, trap::in_interrupt_context};

static POST_SCHEDULE_HANDLER: Once<fn()> = Once::new();

/// Injects a handler to be executed after scheduling.
pub fn inject_post_schedule_handler(handler: fn()) {
    POST_SCHEDULE_HANDLER.call_once(|| handler);
}

/// A task that executes a function to the end.
///
/// Each task is associated with per-task data and an optional user space.
/// If having a user space, the task can switch to the user space to
/// execute user code. Multiple tasks can share a single user space.
pub struct Task {
    #[expect(clippy::type_complexity)]
    func: ForceSync<Cell<Option<Box<dyn FnOnce() + Send>>>>,

    data: Box<dyn Any + Send + Sync>,
    local_data: ForceSync<Box<dyn Any + Send>>,

    user_ctx: Option<Arc<UserContext>>,
    ctx: SyncUnsafeCell<TaskContext>,
    /// kernel stack, note that the top is SyscallFrame/TrapFrame
    kstack: KernelStack,

    /// If we have switched this task to a CPU.
    ///
    /// This is to enforce not context switching to an already running task.
    /// See [`processor::switch_to_task`] for more details.
    switched_to_cpu: AtomicBool,

    schedule_info: TaskScheduleInfo,

    server: ForceSync<RefCell<Option<Arc<dyn Server + Sync + Send + RefUnwindSafe>>>>,
}

impl core::fmt::Debug for Task {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Task")
            .field("func", &self.func)
            .field("data", &self.data)
            .field("local_data", &self.local_data)
            .field("user_ctx", &self.user_ctx)
            .field("ctx", &self.ctx)
            .field("kstack", &self.kstack)
            .field("switched_to_cpu", &self.switched_to_cpu)
            .field("schedule_info", &self.schedule_info)
            // server's implementation might not be Debug, so omit it for now
            .finish()
    }
}

impl Task {
    /// Gets the current task.
    ///
    /// It returns `None` if the function is called in the bootstrap context.
    pub fn current() -> Option<CurrentTask> {
        let current_task = current_task()?;

        // SAFETY: `current_task` is the current task.
        Some(unsafe { CurrentTask::new(current_task) })
    }

    /// Get a reference to the current server managing this task.
    pub fn server(
        &self,
    ) -> &RefCell<Option<Arc<dyn Server + Sync + Send + RefUnwindSafe + 'static>>> {
        unsafe { &self.server.get() }
    }

    pub(super) fn ctx(&self) -> &SyncUnsafeCell<TaskContext> {
        &self.ctx
    }

    /// Sets thread-local storage pointer.
    pub fn set_tls_pointer(&self, tls: usize) {
        let ctx_ptr = self.ctx.get();

        // SAFETY: it's safe to set user tls pointer in kernel context.
        unsafe { (*ctx_ptr).set_tls_pointer(tls) }
    }

    /// Gets thread-local storage pointer.
    pub fn tls_pointer(&self) -> usize {
        let ctx_ptr = self.ctx.get();

        // SAFETY: it's safe to get user tls pointer in kernel context.
        unsafe { (*ctx_ptr).tls_pointer() }
    }

    /// Yields execution so that another task may be scheduled.
    ///
    /// Note that this method cannot be simply named "yield" as the name is
    /// a Rust keyword.
    #[track_caller]
    pub fn yield_now() {
        scheduler::yield_now()
    }

    /// Kicks the task scheduler to run the task.
    ///
    /// BUG: This method highly depends on the current scheduling policy.
    #[track_caller]
    pub fn run(self: &Arc<Self>) {
        scheduler::run_new_task(self.clone());
    }

    /// Returns the task data.
    pub fn data(&self) -> &Box<dyn Any + Send + Sync> {
        &self.data
    }

    /// Get the attached scheduling information.
    pub fn schedule_info(&self) -> &TaskScheduleInfo {
        &self.schedule_info
    }

    /// Returns the user context of this task, if it has.
    pub fn user_ctx(&self) -> Option<&Arc<UserContext>> {
        if self.user_ctx.is_some() {
            Some(self.user_ctx.as_ref().unwrap())
        } else {
            None
        }
    }

    /// Saves the FPU state for user task.
    pub fn save_fpu_state(&self) {
        let Some(user_ctx) = self.user_ctx.as_ref() else {
            return;
        };
        user_ctx.fpu_state().save();
    }

    /// Restores the FPU state for user task.
    pub fn restore_fpu_state(&self) {
        let Some(user_ctx) = self.user_ctx.as_ref() else {
            return;
        };
        user_ctx.fpu_state().restore();
    }
}

/// Options to create or spawn a new task.
pub struct TaskOptions {
    func: Option<Box<dyn FnOnce() + Send>>,
    data: Option<Box<dyn Any + Send + Sync>>,
    local_data: Option<Box<dyn Any + Send>>,
    user_ctx: Option<Arc<UserContext>>,
}

impl TaskOptions {
    /// Creates a set of options for a task.
    pub fn new<F>(func: F) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        Self {
            func: Some(Box::new(func)),
            data: None,
            local_data: None,
            user_ctx: None,
        }
    }

    /// Sets the function that represents the entry point of the task.
    pub fn func<F>(mut self, func: F) -> Self
    where
        F: Fn() + Send + 'static,
    {
        self.func = Some(Box::new(func));
        self
    }

    /// Sets the data associated with the task.
    pub fn data<T>(mut self, data: T) -> Self
    where
        T: Any + Send + Sync,
    {
        self.data = Some(Box::new(data));
        self
    }

    /// Sets the local data associated with the task.
    pub fn local_data<T>(mut self, data: T) -> Self
    where
        T: Any + Send,
    {
        self.local_data = Some(Box::new(data));
        self
    }

    /// Sets the user context associated with the task.
    pub fn user_ctx(mut self, user_ctx: Option<Arc<UserContext>>) -> Self {
        self.user_ctx = user_ctx;
        self
    }

    /// Builds a new task without running it immediately.
    pub fn build(self) -> Result<Task> {
        /// all task will entering this function
        /// this function is mean to executing the task_fn in Task
        extern "C" fn kernel_task_entry() -> ! {
            // SAFETY: The new task is switched on a CPU for the first time, `after_switching_to`
            // hasn't been called yet.
            unsafe { processor::after_switching_to() };

            let current_task = Task::current()
                .expect("no current task, it should have current task in kernel task entry");

            current_task.restore_fpu_state();

            // SAFETY: The `func` field will only be accessed by the current task in the task
            // context, so the data won't be accessed concurrently.
            let task_func = unsafe { current_task.func.get() };
            let task_func = task_func
                .take()
                .expect("task function is `None` when trying to run");
            task_func();

            // Manually drop all the on-stack variables to prevent memory leakage!
            // This is needed because `scheduler::exit_current()` will never return.
            //
            // However, `current_task` _borrows_ the current task without holding
            // an extra reference count. So we do nothing here.

            scheduler::exit_current();
        }

        let kstack = KernelStack::new_with_guard_page()?;

        let mut ctx = SyncUnsafeCell::new(TaskContext::default());
        if let Some(user_ctx) = self.user_ctx.as_ref() {
            ctx.get_mut().set_tls_pointer(user_ctx.tls_pointer());
        };
        ctx.get_mut()
            .set_instruction_pointer(kernel_task_entry as usize);
        // We should reserve space for the return address in the stack, otherwise
        // we will write across the page boundary due to the implementation of
        // the context switch.
        //
        // According to the System V AMD64 ABI, the stack pointer should be aligned
        // to at least 16 bytes. And a larger alignment is needed if larger arguments
        // are passed to the function. The `kernel_task_entry` function does not
        // have any arguments, so we only need to align the stack pointer to 16 bytes.
        ctx.get_mut().set_stack_pointer(kstack.end_vaddr() - 16);

        let new_task = Task {
            func: ForceSync::new(Cell::new(self.func)),
            data: self.data.unwrap_or_else(|| Box::new(())),
            local_data: ForceSync::new(self.local_data.unwrap_or_else(|| Box::new(()))),
            user_ctx: self.user_ctx,
            ctx,
            kstack,
            schedule_info: TaskScheduleInfo {
                cpu: AtomicCpuId::default(),
            },
            switched_to_cpu: AtomicBool::new(false),
            server: ForceSync::new(RefCell::new(None)),
        };

        Ok(new_task)
    }

    /// Builds a new task and runs it immediately.
    #[track_caller]
    pub fn spawn(self) -> Result<Arc<Task>> {
        let task = Arc::new(self.build()?);
        task.run();
        Ok(task)
    }
}

/// The current task.
///
/// This type is not `Send`, so it cannot outlive the current task.
///
/// This type is also not `Sync`, so it can provide access to the local data of the current task.
#[derive(Debug)]
pub struct CurrentTask(NonNull<Task>);

// The intern `NonNull<Task>` contained by `CurrentTask` implies that `CurrentTask` is `!Send` and
// `!Sync`. But it is still good to do this explicitly because these properties are key for
// soundness.
impl !Send for CurrentTask {}
impl !Sync for CurrentTask {}

impl CurrentTask {
    /// # Safety
    ///
    /// The caller must ensure that `task` is the current task.
    unsafe fn new(task: NonNull<Task>) -> Self {
        Self(task)
    }

    /// Returns the local data of the current task.
    ///
    /// Note that the local data is only accessible in the task context. Although there is a
    /// current task in the non-task context (e.g. IRQ handlers), access to the local data is
    /// forbidden as it may cause soundness problems.
    ///
    /// # Panics
    ///
    /// This method will panic if called in a non-task context.
    pub fn local_data(&self) -> &(dyn Any + Send) {
        assert!(!in_interrupt_context());

        let local_data = &self.local_data;

        // SAFETY: The `local_data` field will only be accessed by the current task in the task
        // context, so the data won't be accessed concurrently.
        &**unsafe { local_data.get() }
    }

    /// Returns a cloned `Arc<Task>`.
    pub fn cloned(&self) -> Arc<Task> {
        let ptr = self.0.as_ptr();

        // SAFETY: The current task is always a valid task and it is always contained in an `Arc`.
        unsafe { Arc::increment_strong_count(ptr) };

        // SAFETY: We've increased the reference count in the current `Arc<Task>` above.
        unsafe { Arc::from_raw(ptr) }
    }
}

impl Deref for CurrentTask {
    type Target = Task;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The current task is always a valid task.
        unsafe { self.0.as_ref() }
    }
}

impl AsRef<Task> for CurrentTask {
    fn as_ref(&self) -> &Task {
        self
    }
}

impl Borrow<Task> for CurrentTask {
    fn borrow(&self) -> &Task {
        self
    }
}

/// Trait for manipulating the task context.
pub trait TaskContextApi {
    /// Sets instruction pointer
    fn set_instruction_pointer(&mut self, ip: usize);

    /// Gets instruction pointer
    fn instruction_pointer(&self) -> usize;

    /// Sets stack pointer
    fn set_stack_pointer(&mut self, sp: usize);

    /// Gets stack pointer
    fn stack_pointer(&self) -> usize;
}

/// The primary trait for all server. This provides access to information and capabilities common to all servers.
pub trait Server: Sync + Send + RefUnwindSafe + 'static {
    /// **INTERNAL** User code should never call this directly, however it cannot be private because generated code must
    /// use it.
    ///
    /// Get a reference to the struct implementing all the fundamental server operations. This is effectively the base
    /// class pointer of this server.
    #[doc(hidden)]
    fn orpc_server_base(&self) -> &ServerBase;
}

/// The information and state included in every server. The name comes form it being the "base class" state for all
/// servers.
pub struct ServerBase {
    /// True if the server has been aborted. This usually occurs because a method or thread panicked.
    aborted: AtomicBool,
    /// The servers threads. These are used to verify that all treads have reported themselves as attached and to wake
    /// up the threads of a cancelled server. This is used to make sure threads have attached to OQueues before
    /// returning the `Arc<Server>`. Without this, messages sent to the server immediately after spawning could be lost.
    server_threads: Mutex<Vec<Arc<Task>>>,
    /// A weak reference to this server. This is used to create strong references to the server when only `&dyn Server`
    /// is available.
    weak_this: Weak<dyn Server + Sync + Send + RefUnwindSafe + 'static>,
}

impl ServerBase {
    /// **INTERNAL** User code should never call this directly, however it cannot be private because macro generated
    /// code must use it.
    ///
    /// Create a new `ServerBase` with a cyclical reference to the server containing it.
    #[doc(hidden)]
    pub fn new(weak_this: Weak<dyn Server + Sync + Send + RefUnwindSafe + 'static>) -> Self {
        Self {
            aborted: Default::default(),
            server_threads: Mutex::new(Default::default()),
            weak_this,
        }
    }

    /// **INTERNAL** User code should never call this directly, however it cannot be private because macro generated
    /// code must use it.
    ///
    /// Returns true if the server was aborted.
    #[doc(hidden)]
    pub fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::Relaxed)
    }

    /// **INTERNAL** User code should never call this directly, however it cannot be private because macro generated
    /// code must use it.
    ///
    /// Abort a server.
    #[doc(hidden)]
    pub fn abort(&self, _payload: &impl Display) {
        self.aborted.store(true, Ordering::SeqCst);
        // Wake up all the threads in the server. This assumes that all threads have an abort point
        let server_threads = self.server_threads.lock();
        for s in server_threads.iter() {
            scheduler::unpark_target(s.clone());
        }

        // TODO: Replace with logging.
        // println!("{}", payload);
    }

    /// Attack a task to this server.
    pub fn attach_task(&self) {
        let mut server_threads = self.server_threads.lock();
        server_threads.push(Task::current().unwrap().cloned());
    }

    /// Check if the server has aborted and panic if it has. This should be called periodically from all server threads
    /// to guarantee that servers will crash fully if any part of them crashes. (This is analogous to a cancelation
    /// point in pthreads.)
    pub fn abort_point(&self) {
        if self.is_aborted() {
            panic!("Server aborted in another thread");
        }
    }

    /// Get a strong reference to `self`.
    pub fn get_ref(&self) -> Option<Arc<dyn Server + Sync + RefUnwindSafe + Send>> {
        self.weak_this.upgrade()
    }
}

#[cfg(ktest)]
mod test {
    use crate::prelude::*;

    #[ktest]
    fn create_task() {
        #[expect(clippy::eq_op)]
        let task = || {
            assert_eq!(1, 1);
        };
        let task = Arc::new(
            crate::task::TaskOptions::new(task)
                .data(())
                .build()
                .unwrap(),
        );
        task.run();
    }

    #[ktest]
    fn spawn_task() {
        #[expect(clippy::eq_op)]
        let task = || {
            assert_eq!(1, 1);
        };
        let _ = crate::task::TaskOptions::new(task).data(()).spawn();
    }
}
