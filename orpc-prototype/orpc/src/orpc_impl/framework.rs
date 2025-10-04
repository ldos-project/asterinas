use std::{
    cell::RefCell,
    fmt::Display,
    panic::{RefUnwindSafe, UnwindSafe},
    sync::{
        Arc, Condvar, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    orpc_impl::{Server, ServerRef, errors::RPCError},
    sync::{
        Mutex,
        task::{Task, TaskRef},
    },
};

/// The information and state included in every server. The name comes form it being the "base class" state for all
/// servers.
pub struct ServerBase {
    /// True if the server has been aborted. This usually occurs because a method or thread panicked.
    aborted: AtomicBool,
    /// The servers threads. These are used to verify that all treads have reported themselves as attached and to wake
    /// up the threads of a cancelled server. This is used to make sure threads have attached to OQueues before
    /// returning the `ServerRef`. Without this, messages sent to the server immediately after spawning could be lost.
    server_threads: Mutex<Vec<TaskRef>>,
    /// The condition variable for waiting on [`Self::server_threads`].
    server_threads_condition_variable: Condvar,
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
            server_threads: Default::default(),
            server_threads_condition_variable: Default::default(),
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
    pub fn abort(&self, payload: &impl Display) {
        self.aborted.store(true, Ordering::SeqCst);
        // Wake up all the threads in the server. This assumes that all threads have an abort point
        let server_threads = self.server_threads.lock();
        for s in server_threads.iter() {
            s.unpark();
        }

        // TODO: Replace with logging.
        println!("{}", payload);
    }

    fn attach_task(&self) {
        let mut server_threads = self.server_threads.lock();
        server_threads.push(Task::current().clone());
        self.server_threads_condition_variable.notify_all();
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
    fn get_ref(&self) -> Option<ServerRef<dyn Server + Sync + RefUnwindSafe + Send>> {
        self.weak_this.upgrade()
    }
}

/// Start a new server thread. This should only be called while spawning a server.
pub fn spawn_thread<T: Server + Send + RefUnwindSafe + 'static>(
    server: std::sync::Arc<T>,
    body: impl (FnOnce() -> Result<(), Box<dyn std::error::Error>>) + Send + UnwindSafe + 'static,
) {
    std::thread::spawn({
        move || {
            if let Result::Err(payload) = std::panic::catch_unwind({
                let server = server.clone();
                move || {
                    Server::orpc_server_base(server.as_ref()).attach_task();
                    let _server_context = CurrentServer::enter_server_context(server.as_ref());
                    if let Result::Err(e) = body() {
                        Server::orpc_server_base(server.as_ref()).abort(&e);
                    }
                }
            }) {
                Server::orpc_server_base(server.as_ref()).abort(&RPCError::from_panic(payload));
            }
        }
    });
}

thread_local! {
    // PERFORMANCE: The implementation of `thread_local` relies on LLVM devirtualization to eliminate a function call
    // during access. A better performing implementation may be needed to eliminate the overhead of internal abort
    // points (like in blocking implementations). In Mariposa, we could put the current server in the task struct making
    // the access a couple of pointer steps from a CPU-local (which does not have these performance issues).
    static CURRENT_SERVER: RefCell<Option<Arc<dyn Server + Sync + RefUnwindSafe + Send>>> = RefCell::new(None);
}

/// Methods to access the current server.
pub struct CurrentServer {
    _private: (),
}

impl CurrentServer {
    /// Check the if the current server has aborted and panic if it has. This should be called periodically from all
    /// server threads to guarantee that servers will crash fully if any part of them crashes. (This is analogous to a
    /// cancelation point in pthreads.)
    pub fn abort_point() {
        CURRENT_SERVER.with(|r| {
            if let Some(s) = r.borrow().as_ref() {
                s.orpc_server_base().abort_point();
            }
        });
    }

    /// **INTERNAL** User code should never call this directly, however it cannot be private because macro generated
    /// code must use it.
    ///
    /// Enter a server context by changing the current server. This is used in the implementations of methods and server
    /// threads.
    #[doc(hidden)]
    pub fn enter_server_context(server: &dyn Server) -> CurrentServerChangeGuard {
        // TODO:PERFORMANCE:The overhead of using a strong reference here is potentially significant. Instead, we should
        // probably use unsafe to just use a pointer, assuming we can guarantee dynamic scoping and rule out leaking the
        // reference.
        let previous_server = CURRENT_SERVER.take();
        CURRENT_SERVER.set(server.orpc_server_base().get_ref());
        CurrentServerChangeGuard(previous_server)
    }
}

pub struct CurrentServerChangeGuard(Option<Arc<dyn Server + Sync + RefUnwindSafe + Send>>);

impl Drop for CurrentServerChangeGuard {
    fn drop(&mut self) {
        CURRENT_SERVER.set(std::mem::take(&mut self.0));
    }
}

#[cfg(test)]
mod test {
    use std::{
        panic::catch_unwind,
        sync::Barrier,
        thread::{self, sleep},
        time::Duration,
    };

    use snafu::Whatever;

    use crate::{orpc_impl::errors::RPCError, sync::blocker::Blocker};

    use super::*;

    struct InfinitBlocker;

    impl Blocker for InfinitBlocker {
        fn should_try(&self) -> bool {
            false
        }

        fn prepare_to_wait(&self, _task: &crate::sync::task::TaskRef) {}

        fn finish_wait(&self, _task: &crate::sync::task::TaskRef) {}
    }

    struct TestServer<F: Fn()> {
        f: F,
        base: ServerBase,
        thread_exited: AtomicBool,
    }

    impl<F: Fn() + Sync + Send + RefUnwindSafe + 'static> Server for TestServer<F> {
        fn orpc_server_base(&self) -> &ServerBase {
            &self.base
        }
    }

    impl<F: Fn() + Sync + Send + RefUnwindSafe + 'static> TestServer<F> {
        fn main_thread(&self) -> Result<(), Whatever> {
            (self.f)();
            Ok(())
        }

        fn orpc_start_threads(server: &ServerRef<TestServer<F>>) -> Result<(), Whatever> {
            thread::spawn({
                let server = server.clone();

                move || {
                    if let Err(payload) = catch_unwind({
                        let server = server.clone();
                        move || {
                            server.orpc_server_base().attach_task();
                            let _server_context =
                                CurrentServer::enter_server_context(server.as_ref());
                            if let Err(e) = server.main_thread() {
                                // TODO: An actual logging operation.
                                server.orpc_server_base().abort(&e);
                            }
                        }
                    }) {
                        // TODO: An actual logging operation.
                        server
                            .orpc_server_base()
                            .abort(&RPCError::from_panic(payload));
                    }
                    server.thread_exited.store(true, Ordering::SeqCst);
                }
            });
            Ok(())
        }

        fn spawn(f: F) -> Result<ServerRef<Self>, Whatever> {
            let server = Arc::<Self>::new_cyclic(|weak_this| Self {
                f,
                base: ServerBase::new(weak_this.clone()),
                thread_exited: AtomicBool::new(false),
            });
            Self::orpc_start_threads(&server)?;
            Ok(server)
        }
    }

    struct OnDrop<F: Fn()>(F);

    impl<F: Fn()> Drop for OnDrop<F> {
        fn drop(&mut self) {
            (self.0)()
        }
    }

    #[test]
    fn abort_while_blocking() {
        let barrier = Arc::new(Barrier::new(2));
        let server = TestServer::spawn({
            let barrier = barrier.clone();
            move || {
                barrier.wait();
                let _guard = OnDrop(|| {
                    barrier.wait();
                });
                Task::current().block_on(&[&InfinitBlocker])
            }
        })
        .unwrap();

        barrier.wait();

        server.base.abort(&"test");

        barrier.wait();
        // TODO: Fix potential flake.
        sleep(Duration::from_millis(100));

        assert!(server.thread_exited.load(Ordering::SeqCst));
    }
}
