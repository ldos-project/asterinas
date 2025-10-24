// SPDX-License-Identifier: MPL-2.0
#[cfg(ktest)]
mod test {
    use core::{
        assert_matches::assert_matches,
        cell::Cell,
        panic::{RefUnwindSafe, UnwindSafe},
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use orpc_macros::{orpc_impl, orpc_server, orpc_trait};
    use ostd_macros::ktest;
    use snafu::{ResultExt as _, Whatever};

    use crate::{
        orpc::{
            framework::{CurrentServer, errors::RPCError, spawn_thread},
            oqueue::{Consumer, OQueueRef, generic_test, locking::LockingQueue},
        },
        prelude::{Arc, Box},
    };

    // #[observable]
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
            let server = Self::new_with(|orpc_internal| Self {
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
                let server = Self::new_with(|orpc_internal| Self { orpc_internal });
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
                let server = Self::new_with(|orpc_internal| Self {
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
}
