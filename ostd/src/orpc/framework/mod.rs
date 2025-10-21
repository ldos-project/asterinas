// SPDX-License-Identifier: MPL-2.0
//! The module containing implementations of the ORPC framework.
pub mod errors;

mod integration_test;

use alloc::{sync::Weak, vec::Vec};
use core::{
    fmt::Display,
    panic::{RefUnwindSafe, UnwindSafe},
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    prelude::{Arc, Box},
    sync::Mutex,
    task::{Task, TaskOptions, scheduler},
};

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

/// Start a new server thread. This should only be called while spawning a server.
pub fn spawn_thread<T: Server + Send + RefUnwindSafe + 'static>(
    server: Arc<T>,
    body: impl (FnOnce() -> Result<(), Box<dyn core::error::Error>>) + Send + UnwindSafe + 'static,
) {
    TaskOptions::new({
        move || {
            if let Result::Err(payload) = crate::panic::catch_unwind({
                let server = server.clone();
                move || {
                    Server::orpc_server_base(server.as_ref()).attach_task();
                    let _server_context = CurrentServer::enter_server_context(server.as_ref());
                    if let Result::Err(e) = body() {
                        Server::orpc_server_base(server.as_ref()).abort(&e);
                    }
                }
            }) {
                Server::orpc_server_base(server.as_ref())
                    .abort(&errors::RPCError::from_panic(payload));
            }
        }
    })
    .spawn()
    .unwrap();
}

/// Methods to access the current server.
pub struct CurrentServer {
    _private: (),
}

impl CurrentServer {
    /// Check if the current server has aborted
    pub fn is_aborted() -> bool {
        Task::current()
            .unwrap()
            .server()
            .borrow()
            .clone()
            .map(|s| s.orpc_server_base().is_aborted())
            .unwrap_or(false)
    }
    /// Check the if the current server has aborted and panic if it has. This should be called periodically from all
    /// server threads to guarantee that servers will crash fully if any part of them crashes. (This is analogous to a
    /// cancelation point in pthreads.)
    pub fn abort_point() {
        if let Some(s) = Task::current().unwrap().server().borrow().as_ref() {
            s.orpc_server_base().abort_point();
        }
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
        let curr_task = Task::current().unwrap();
        let previous_server = curr_task.server().take();
        if let Some(s) = server.orpc_server_base().get_ref() {
            curr_task.server().replace(Some(s));
        }
        CurrentServerChangeGuard(previous_server)
    }
}

/// Guard for entering a server context. When dropped, the current tasks's server is set to
/// `self.0`.
pub struct CurrentServerChangeGuard(Option<Arc<dyn Server + Sync + RefUnwindSafe + Send>>);

impl Drop for CurrentServerChangeGuard {
    fn drop(&mut self) {
        Task::current().unwrap().server().replace(self.0.clone());
    }
}

#[cfg(ktest)]
mod test {
    use core::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };

    use ostd_macros::ktest;
    use snafu::{Whatever, whatever};

    use super::*;
    use crate::{
        orpc::{oqueue::generic_test, sync::Blocker},
        sync::Waker,
    };

    struct InfiniteBlocker;

    impl Blocker for InfiniteBlocker {
        fn should_try(&self) -> bool {
            false
        }

        fn prepare_to_wait(&self, _waker: &Arc<Waker>) {}
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

        fn orpc_start_threads(server: &Arc<TestServer<F>>) -> Result<(), Whatever> {
            let res = TaskOptions::new({
                let server = server.clone();

                move || {
                    if let Err(payload) = crate::panic::catch_unwind({
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
                            .abort(&errors::RPCError::from_panic(payload));
                    }
                    server.thread_exited.store(true, Ordering::SeqCst);
                }
            })
            .spawn();
            let _ = whatever!(res, "Failed to spawn thread");
            Ok(())
        }

        fn spawn(f: F) -> Result<Arc<Self>, Whatever> {
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

    #[ktest]
    fn abort_while_blocking() {
        let barrier0 = Arc::new(AtomicBool::new(false));
        let barrier1 = Arc::new(AtomicBool::new(false));
        let server = TestServer::spawn({
            let barrier0 = barrier0.clone();
            let barrier1 = barrier1.clone();
            move || {
                barrier0.store(true, Ordering::Relaxed);
                let _guard = OnDrop(|| {
                    barrier1.store(true, Ordering::Relaxed);
                });
                Task::current().unwrap().block_on(&[&InfiniteBlocker]);
            }
        })
        .unwrap();

        while !barrier0.load(Ordering::Relaxed) {
            Task::yield_now();
        }
        server.base.abort(&"test");

        while !barrier1.load(Ordering::Relaxed) {
            Task::yield_now();
        }
        // TODO: Fix potential flake.
        generic_test::sleep(Duration::from_millis(100));

        assert!(server.thread_exited.load(Ordering::SeqCst));
    }
}
