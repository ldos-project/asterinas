//! Server infrastructure

use core::panic::{RefUnwindSafe, UnwindSafe};

use super::errors::RPCError;
use crate::{
    prelude::{Arc, Box},
    task::{Server, Task, TaskOptions},
};

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
                Server::orpc_server_base(server.as_ref()).abort(&RPCError::from_panic(payload));
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
    /// Check the if the current server has aborted and panic if it has. This should be called periodically from all
    /// server threads to guarantee that servers will crash fully if any part of them crashes. (This is analogous to a
    /// cancelation point in pthreads.)
    pub fn abort_point() {
        Task::current().unwrap().server().borrow().clone().map(|s| {
            s.orpc_server_base().abort_point();
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
        let curr_task = Task::current().unwrap().cloned();
        let previous_server = curr_task.server().take();
        server.orpc_server_base().get_ref().map(|s| {
            curr_task.server().replace(Some(s));
        });
        CurrentServerChangeGuard(previous_server)
    }
}

/// Guard for entering a server context. When dropped, the current tasks's server is set to
/// `self.0`.
pub struct CurrentServerChangeGuard(Option<Arc<dyn Server + Sync + RefUnwindSafe + Send>>);

impl Drop for CurrentServerChangeGuard {
    fn drop(&mut self) {
        Task::current()
            .unwrap()
            .cloned()
            .server()
            .replace(self.0.clone());
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

    use super::*;
    use crate::{orpc_impl::errors::RPCError, sync::blocker::Blocker};

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
