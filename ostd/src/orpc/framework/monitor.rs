// SPDX-License-Identifier: MPL-2.0

//! ORPC monitors are objects which hold state and have a set of methods which operate on that
//! state. They are **not** servers, and are orthogonal to them. As such, monitors must be inside a
//! server. The standard usage is to have a single monitor holding all the state of the server and
//! forwarding all server methods into that monitor.
//!
//! Monitors are created using the [`orpc_monitor`] macro.

pub use orpc_macros::orpc_monitor;

use crate::orpc::oqueue::{ValueProducer, new_reply_pair};

/// **INTERNAL FOR MACRO USE ONLY**
///
/// Make a synchronous request over an OQueue. `make_request` is called with the reply producer
/// attachment to create the request message. The request is sent over `producer`. The consumer of
/// that message should guarantee exactly one reply will be published.
#[doc(hidden)]
pub fn synchronous_request<C: Send + 'static, R: Send + 'static>(
    producer: &ValueProducer<C>,
    make_request: impl FnOnce(ValueProducer<R>) -> C,
) -> R {
    let (reply_producer, reply_consumer) = new_reply_pair::<R>();
    producer.produce(make_request(reply_producer));
    reply_consumer.consume()
}

#[cfg(ktest)]
mod tests {
    use orpc_macros::{orpc_monitor, orpc_server};

    use crate::{
        assert_eq_eventually, new_server,
        orpc::{
            errors::RPCError,
            oqueue::{
                ConsumableOQueue, ConsumableOQueueRef, OQueue, OQueueBase, OQueueRef,
                ObservationQuery,
            },
            path::Path,
        },
        prelude::{Arc, ktest},
    };

    #[orpc_server()]
    struct TestServer {
        monitor: TestStateMonitor,
    }

    pub struct TestState {
        x: i32,
    }

    #[orpc_monitor(pub)]
    impl TestState {
        /// A method which can be attached to an OQueue as an observer so it is called everytime
        /// something is produced. This attachment is done with a generated method called
        /// `attach_update`.
        #[strong_observer]
        pub fn update(&mut self, x: i32) -> Result<(), RPCError> {
            self.x = (self.x + x * 3) / 4;
            Ok(())
        }

        /// A method which can be attached as a consumer. This is analogous to the observer
        /// capabilities of `update`.
        #[consumer]
        pub fn next(&mut self, _: ()) -> Result<(), RPCError> {
            self.x += 1;
            Ok(())
        }

        pub fn get(&mut self) -> Result<i32, RPCError> {
            Ok(self.x)
        }
    }

    fn spawn_server() -> Arc<TestServer> {
        let server = new_server!(Path::test(), |_| TestServer {
            monitor: TestStateMonitor::new(Path::test()),
        });
        server.monitor.start(server.clone(), TestState { x: 0 });
        server
    }

    #[ktest]
    fn monitor_updates_from_strong_observer() {
        let values = OQueueRef::new(2, Path::test());
        let server = spawn_server();
        server
            .monitor
            .attach_update(
                values
                    .attach_strong_observer(ObservationQuery::identity())
                    .unwrap(),
            )
            .unwrap();

        let producer = values.attach_ref_producer().unwrap();

        producer.produce_ref(&0);
        producer.produce_ref(&100);
        assert_eq_eventually!(server.monitor.get().unwrap(), 75);

        producer.produce_ref(&100);
        assert_eq_eventually!(server.monitor.get().unwrap(), 93);
    }

    #[ktest]
    fn monitor_call() {
        let server = spawn_server();

        server.monitor.update(0).unwrap();
        server.monitor.update(100).unwrap();
        assert_eq_eventually!(server.monitor.get().unwrap(), 75);

        server.monitor.update(100).unwrap();
        assert_eq_eventually!(server.monitor.get().unwrap(), 93);
    }

    #[ktest]
    fn monitor_updates_from_consumer() {
        let values = ConsumableOQueueRef::new(2, Path::test());
        let server = spawn_server();
        server
            .monitor
            .attach_next(values.attach_consumer().unwrap())
            .unwrap();
        let producer = values.attach_value_producer().unwrap();

        producer.produce(());
        producer.produce(());
        assert_eq_eventually!(server.monitor.get().unwrap(), 2);

        producer.produce(());
        assert_eq_eventually!(server.monitor.get().unwrap(), 3);
    }
}
