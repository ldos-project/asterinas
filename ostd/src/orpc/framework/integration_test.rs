// SPDX-License-Identifier: MPL-2.0

//! A collection of integration tests for the ORPC/OQueue framework as a whole.

#[cfg(ktest)]
mod test {
    use core::{
        assert_matches::assert_matches,
        cell::Cell,
        panic::{RefUnwindSafe, UnwindSafe},
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::Duration,
    };

    use orpc_macros::{orpc_impl, orpc_server, orpc_trait, select};
    use ostd_macros::ktest;
    use snafu::{ResultExt as _, Whatever};

    use crate::{
        assert_eq_eventually, assert_eventually, assert_matches_eventually,
        orpc::{
            framework::{
                CurrentServer,
                errors::RPCError,
                shutdown::{Shutdown, ShutdownState},
                spawn_thread,
            },
            oqueue::{
                Consumer, OQueueRef, generic_test,
                locking::{LockingQueue, ObservableLockingQueue},
            },
        },
        prelude::{Arc, Box},
    };

    struct AdditionalAmount {
        n: usize,
        trigger_panic: bool,
    }

    #[orpc_trait]
    trait Counter {
        fn atomic_incr(&self, additional: AdditionalAmount) -> Result<usize, RPCError>;
        fn incr_oqueue(&self) -> OQueueRef<AdditionalAmount> {
            LockingQueue::new(8)
        }
    }

    #[orpc_server(Counter)]
    struct ServerAState {
        increment: usize,
        atomic_count: AtomicUsize,
    }

    #[orpc_impl]
    impl Counter for ServerAState {
        fn atomic_incr(&self, additional: AdditionalAmount) -> Result<usize, RPCError> {
            if additional.trigger_panic {
                panic!("Asked to panic");
            }

            let addend = self.increment + additional.n;
            let v = self.atomic_count.fetch_add(addend, Ordering::Relaxed);
            Ok(v + addend)
        }

        fn incr_oqueue(&self) -> OQueueRef<AdditionalAmount>;
    }

    impl ServerAState {
        fn main_thread(
            &self,
            incr_oqueue_consumer: Box<dyn Consumer<AdditionalAmount>>,
        ) -> Result<(), Whatever> {
            let mut _count = 0;
            loop {
                let AdditionalAmount { n, trigger_panic } = incr_oqueue_consumer.consume();
                if trigger_panic {
                    panic!("Asked to panic by message");
                }
                let addend = self.increment + n;
                _count += addend;
                CurrentServer::abort_point();
            }
        }

        fn spawn(
            increment: usize,
            atomic_count: AtomicUsize,
        ) -> Result<Arc<ServerAState>, Whatever> {
            let server = Self::new_with(|orpc_internal, _| Self {
                increment,
                atomic_count,
                orpc_internal,
            });
            spawn_thread(server.clone(), {
                let server = server.clone();
                let incr_oqueue_consumer = server
                    .incr_oqueue()
                    .attach_consumer()
                    .whatever_context("incr_oqueue_consumer")?;
                move || Ok(server.main_thread(incr_oqueue_consumer)?)
            });
            Ok(server)
        }
    }

    #[ktest]
    fn start_server() {
        let _server_ref = ServerAState::spawn(2, AtomicUsize::new(0)).unwrap();
    }

    #[ktest]
    fn direct_call() {
        let server_ref = ServerAState::spawn(2, AtomicUsize::new(0)).unwrap();
        assert_eq!(
            server_ref
                .atomic_incr(AdditionalAmount {
                    n: 1,
                    trigger_panic: false
                })
                .unwrap(),
            3
        );

        let server_ref: Arc<ServerAState> = server_ref;
        assert_eq!(
            server_ref
                .atomic_incr(AdditionalAmount {
                    n: 1,
                    trigger_panic: false
                })
                .unwrap(),
            6
        );

        assert_matches!(
            server_ref.atomic_incr(AdditionalAmount {
                n: 1,
                trigger_panic: true,
            }),
            Err(RPCError::Panic { .. })
        );

        assert_matches!(
            server_ref.atomic_incr(AdditionalAmount {
                n: 1,
                trigger_panic: false,
            }),
            Err(RPCError::ServerMissing)
        );

        assert_matches!(
            server_ref.atomic_incr(AdditionalAmount {
                n: 1,
                trigger_panic: true,
            }),
            Err(RPCError::ServerMissing)
        );
    }

    #[ktest]
    fn message() {
        let server_ref = ServerAState::spawn(2, AtomicUsize::new(0)).unwrap();

        server_ref
            .incr_oqueue()
            .attach_producer()
            .unwrap()
            .produce(AdditionalAmount {
                n: 1,
                trigger_panic: false,
            });

        let server_ref: Arc<ServerAState> = server_ref;

        server_ref
            .incr_oqueue()
            .attach_producer()
            .unwrap()
            .produce(AdditionalAmount {
                n: 1,
                trigger_panic: false,
            });

        server_ref
            .incr_oqueue()
            .attach_producer()
            .unwrap()
            .produce(AdditionalAmount {
                n: 1,
                trigger_panic: true,
            });

        // This is fundamentally racy, but it's very hard to avoid because any reply from the message send above will, by
        // definition, be sent before the panic.
        generic_test::sleep(Duration::from_millis(250));

        assert_matches!(
            server_ref.atomic_incr(AdditionalAmount {
                n: 1,
                trigger_panic: false,
            }),
            Err(RPCError::ServerMissing)
        );
    }

    #[ktest]
    fn panic_in_default_impl() {
        #[orpc_trait]
        trait TestTrait {
            fn f(&self) -> Result<usize, RPCError> {
                panic!();
            }
        }

        #[orpc_server(TestTrait)]
        struct TestServer {}

        #[orpc_impl]
        impl TestTrait for TestServer {}

        impl TestServer {
            fn spawn() -> Result<Arc<Self>, Whatever> {
                let server = Self::new_with(|orpc_internal, _| Self { orpc_internal });
                Ok(server)
            }
        }

        let server_ref = TestServer::spawn().unwrap();
        let server_ref: Arc<dyn TestTrait> = server_ref;

        assert_matches!(server_ref.f(), Err(RPCError::Panic { .. }));
    }

    #[ktest]
    fn non_unwind_safe() {
        #[orpc_trait]
        trait TestTrait {
            fn f(&self) -> Result<usize, RPCError>;
        }

        #[orpc_server(TestTrait)]
        struct TestServer {
            x: Cell<usize>,
        }

        unsafe impl Sync for TestServer {}
        impl !RefUnwindSafe for TestServer {}
        impl !UnwindSafe for TestServer {}

        #[orpc_impl]
        impl TestTrait for TestServer {
            fn f(&self) -> Result<usize, RPCError> {
                self.x.set(2);
                Ok(42)
            }
        }

        impl TestServer {
            fn spawn() -> Result<Arc<Self>, Whatever> {
                let server = Self::new_with(|orpc_internal, _| Self {
                    x: Default::default(),
                    orpc_internal,
                });
                Ok(server)
            }
        }

        let server_ref = TestServer::spawn().unwrap();
        let server_ref: Arc<dyn TestTrait> = server_ref;

        assert_eq!(server_ref.f().unwrap(), 42);
    }

    static SERVER_DROPS: AtomicUsize = AtomicUsize::new(0);

    #[orpc_server(SimpleServerTrait)]
    struct SimpleServer {
        shutdown_state: ShutdownState,
        thread_exited: Arc<AtomicBool>,
    }

    #[orpc_trait]
    trait SimpleServerTrait: Shutdown {
        fn ping(&self) -> Result<(), RPCError>;
    }

    #[orpc_impl]
    impl Shutdown for SimpleServer {
        fn shutdown(&self) -> Result<(), RPCError> {
            self.shutdown_state.shutdown();
            Ok(())
        }
    }

    #[orpc_impl]
    impl SimpleServerTrait for SimpleServer {
        fn ping(&self) -> Result<(), RPCError> {
            self.shutdown_state.check()?;
            Ok(())
        }
    }

    impl SimpleServer {
        fn spawn() -> Result<Arc<Self>, Whatever> {
            let thread_exited = Arc::new(AtomicBool::new(false));
            let server = Self::new_with(|orpc_internal, _| Self {
                shutdown_state: ShutdownState::default(),
                thread_exited: thread_exited.clone(),
                orpc_internal,
            });

            spawn_thread(server.clone(), {
                let server = server.clone();
                let thread_exited = thread_exited.clone();
                move || {
                    let _ = (|| -> Result<(), RPCError> {
                        loop {
                            server.shutdown_state.check()?;
                            crate::task::Task::yield_now();
                        }
                    })();
                    thread_exited.store(true, Ordering::SeqCst);
                    Ok(())
                }
            });

            Ok(server)
        }
    }

    impl Drop for SimpleServer {
        fn drop(&mut self) {
            SERVER_DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[ktest]
    fn server_shutdown_and_cleanup() {
        SERVER_DROPS.store(0, Ordering::SeqCst);
        let server_ref = SimpleServer::spawn().unwrap();

        assert!(server_ref.ping().is_ok());
        assert!(server_ref.shutdown().is_ok());

        assert_eventually!(
            server_ref.thread_exited.load(Ordering::SeqCst),
            timeout = Duration::from_secs(1)
        );
        assert_matches!(server_ref.ping(), Err(RPCError::ServerMissing));

        drop(server_ref);
        assert_eq_eventually!(SERVER_DROPS.load(Ordering::SeqCst), 1);
    }

    #[ktest]
    fn server_restart_after_complete_teardown() {
        SERVER_DROPS.store(0, Ordering::SeqCst);
        let server1 = SimpleServer::spawn().unwrap();
        assert!(server1.ping().is_ok());

        let _ = server1.shutdown();
        assert_eventually!(
            server1.thread_exited.load(Ordering::SeqCst),
            timeout = Duration::from_secs(1)
        );

        drop(server1);
        assert_eq_eventually!(SERVER_DROPS.load(Ordering::SeqCst), 1);

        let server2 = SimpleServer::spawn().unwrap();
        assert!(server2.ping().is_ok());
    }

    #[ktest]
    fn server_restart_with_reused_oqueue() {
        #[orpc_trait]
        trait MessageCounter: Shutdown {
            fn get_processed_count(&self) -> Result<usize, RPCError>;
        }

        #[orpc_server(MessageCounter)]
        struct MessageCounterServer {
            processed_count: AtomicUsize,
            shutdown_state: ShutdownState,
        }

        #[orpc_impl]
        impl Shutdown for MessageCounterServer {
            fn shutdown(&self) -> Result<(), RPCError> {
                self.shutdown_state.shutdown();
                Ok(())
            }
        }

        #[orpc_impl]
        impl MessageCounter for MessageCounterServer {
            fn get_processed_count(&self) -> Result<usize, RPCError> {
                self.shutdown_state.check()?;
                Ok(self.processed_count.load(Ordering::Relaxed))
            }
        }

        impl MessageCounterServer {
            fn spawn_with_queue(queue: OQueueRef<usize>) -> Result<Arc<Self>, Whatever> {
                let server = Self::new_with(|orpc_internal, _| Self {
                    processed_count: AtomicUsize::new(0),
                    shutdown_state: ShutdownState::default(),
                    orpc_internal,
                });

                spawn_thread(server.clone(), {
                    let server = server.clone();
                    let consumer = queue
                        .attach_consumer()
                        .whatever_context("attach consumer")?;
                    let shutdown_consumer = server
                        .shutdown_state
                        .shutdown_oqueue
                        .attach_consumer()
                        .whatever_context("attach shutdown")?;
                    move || {
                        loop {
                            server.shutdown_state.check()?;
                            select!(
                                if let _ = consumer.try_consume() {
                                    server.processed_count.fetch_add(1, Ordering::Relaxed);
                                },
                                if let _ = shutdown_consumer.try_consume() {}
                            );
                            crate::task::Task::yield_now();
                        }
                    }
                });

                Ok(server)
            }
        }

        impl Drop for MessageCounterServer {
            fn drop(&mut self) {
                SERVER_DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }

        SERVER_DROPS.store(0, Ordering::SeqCst);

        let queue: OQueueRef<usize> = LockingQueue::new(8);
        let server1 = MessageCounterServer::spawn_with_queue(queue.clone()).unwrap();

        queue.produce(42).unwrap();
        assert_eq_eventually!(server1.get_processed_count().unwrap(), 1);

        let _ = server1.shutdown();
        assert_matches_eventually!(server1.get_processed_count(), Err(RPCError::ServerMissing));

        drop(server1);
        assert_eq_eventually!(SERVER_DROPS.load(Ordering::SeqCst), 1);

        let server2 = MessageCounterServer::spawn_with_queue(queue.clone()).unwrap();
        assert_eq!(server2.get_processed_count().unwrap(), 0);

        queue.produce(42).unwrap();
        assert_eq_eventually!(server2.get_processed_count().unwrap(), 1);

        let _ = server2.shutdown();
        drop(server2);
        assert_eq_eventually!(SERVER_DROPS.load(Ordering::SeqCst), 2);
    }

    #[ktest]
    fn server_restart_with_reused_oqueue_strong_observers() {
        #[orpc_trait]
        trait MessageCounter: Shutdown {
            fn get_processed_count(&self) -> Result<usize, RPCError>;
        }

        #[orpc_server(MessageCounter)]
        struct MessageCounterServer {
            processed_count: AtomicUsize,
            shutdown_state: ShutdownState,
        }

        #[orpc_impl]
        impl Shutdown for MessageCounterServer {
            fn shutdown(&self) -> Result<(), RPCError> {
                self.shutdown_state.shutdown();
                Ok(())
            }
        }

        #[orpc_impl]
        impl MessageCounter for MessageCounterServer {
            fn get_processed_count(&self) -> Result<usize, RPCError> {
                self.shutdown_state.check()?;
                Ok(self.processed_count.load(Ordering::Relaxed))
            }
        }

        impl MessageCounterServer {
            fn spawn_with_queue(queue: OQueueRef<usize>) -> Result<Arc<Self>, Whatever> {
                let server = Self::new_with(|orpc_internal, _| Self {
                    processed_count: AtomicUsize::new(0),
                    shutdown_state: ShutdownState::default(),
                    orpc_internal,
                });

                spawn_thread(server.clone(), {
                    let server = server.clone();
                    let observer = queue
                        .attach_strong_observer()
                        .whatever_context("attach strong observer")?;
                    let shutdown_consumer = server
                        .shutdown_state
                        .shutdown_oqueue
                        .attach_consumer()
                        .whatever_context("attach shutdown")?;
                    move || {
                        loop {
                            server.shutdown_state.check()?;
                            select!(
                                if let _ = observer.try_strong_observe() {
                                    server.processed_count.fetch_add(1, Ordering::SeqCst);
                                },
                                if let _ = shutdown_consumer.try_consume() {}
                            );
                        }
                    }
                });

                Ok(server)
            }
        }

        impl Drop for MessageCounterServer {
            fn drop(&mut self) {
                SERVER_DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }

        SERVER_DROPS.store(0, Ordering::SeqCst);

        let queue: OQueueRef<usize> = ObservableLockingQueue::new(8, 1);
        let server1 = MessageCounterServer::spawn_with_queue(queue.clone()).unwrap();

        queue.produce(42).unwrap();
        assert_eq_eventually!(server1.get_processed_count().unwrap(), 1);

        let _ = server1.shutdown();
        assert_matches_eventually!(server1.get_processed_count(), Err(RPCError::ServerMissing));

        drop(server1);
        assert_eq_eventually!(SERVER_DROPS.load(Ordering::SeqCst), 1);

        let server2 = MessageCounterServer::spawn_with_queue(queue.clone()).unwrap();
        assert_eq!(server2.get_processed_count().unwrap(), 0);

        queue.produce(42).unwrap();
        assert_eq_eventually!(server2.get_processed_count().unwrap(), 1);

        let _ = server2.shutdown();
        drop(server2);
        assert_eq_eventually!(SERVER_DROPS.load(Ordering::SeqCst), 2);
    }
}
