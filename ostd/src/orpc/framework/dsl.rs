// SPDX-License-Identifier: MPL-2.0

//! A DSL for defining server usage of OQueues
//!
//! This DSL provides a way to define a servers attachment to OQueues.
//!
//! ```no_compile
//!
//! ```

// Monitors are a separate unit from a Server. Servers contain them.
// 
// A monitor has:
// * handlers (methods on the state struct)
// * a lock
// * state
// * attach methods for each handler to configure the monitor to run that handle when a specific
//   attachment provides a value. This can be either consumers or strong observers (interchangably).
// * direct call methods for each handler. These are used to implement server methods using the
//   monitor methods.
// 
// Monitors are not associated with a server by default. But they can carry a reference to one in
// their state.


// Servers are recommended to have a single monitor containing all their state.
//  * The server spawn will attach the monitor to OQueues
//  * The server methods will forward to the monitor methods
//  * The monitor will often contain a weak reference to the server to allow method implementations
//    to materialize a reference to the server when it needs to pass one to some other server.
//    (Built-in support to make this something we can replace later.)
 

// How to handle "Shutdown"? Does it just need to be built-in? Probably.

use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
    vec::{self, Vec},
};
use core::marker::PhantomData;

use orpc_macros::{orpc_impl, orpc_server, orpc_trait};
use spin::Mutex;

use crate::{
    orpc::{
        framework::{Server as _, errors::RPCError, spawn_thread},
        oqueue::{
            AnyConsumer, AnyOQueueRef, AttachmentError, ConsumableOQueue, Consumer,
            InlineStrongObserver, OQueueBase, ObservationError, ObservationQuery, StrongObserver,
            WeakObserver,
        },
    },
    sync::SpinLock,
};

trait MonitorState {
    type Handles;
}

// Build a type specific struct instead of the generic one. That allows it to more easily implement
// methods and have varied representation at compile time. It can still implement a `Monitor` trait
// to expose a generic interface if needed.

struct Monitor<T: MonitorState> {
    handles: Mutex<T::Handles>,
    state: Mutex<T>,
}

struct State {}
struct SimpleState {
    x: usize,
    z_observer: WeakObserver<i32>,
    // GENERATED
    _server_this: Weak<Server>,
}

// #[monitor(Server)]
impl SimpleState {
    // Targetable "handlers" or "methods". All are the same and they are attached to OQueues or
    // methods via generated methods and macros on the server trait impl.

    // #[strong_observer]?
    fn handle_a(&mut self, x: &i32) -> Result<(), RPCError> {
        let mut history = [None; 4];
        self.z_observer.weak_observe_recent_into(&mut history);
        self.x = *x as usize;
        Ok(())
    }

    fn f(&mut self, x: u64) -> Result<u32, RPCError> {
        Ok((x / 2) as u32)
    }

    // GENERATED
    fn get_ref(&self) -> Arc<Server> {
        self._server_this
            .upgrade()
            .expect("monitor still exists, so server must as well")
    }
}

// GENERATED: how? (from impl SimpleState)
#[derive(Default)]
struct SimpleStateHandles {
    handle_a: Option<InlineStrongObserver>, // Any attachment
}
// Generate attachment methods onto SimpleStateMonitor (Monitor<StateMonitor>)

// GENERATED
impl MonitorState for SimpleState {
    type Handles = SimpleStateHandles;
}

#[orpc_server()]
struct Server {
    // #[state]
    // state: State,
    // #[monitor]
    monitor: Monitor<SimpleState>,
}

impl Server {
    fn spawn(oq: AnyOQueueRef<i32>) -> Result<Arc<Self>, AttachmentError> {
        let mut server = Self::new_with(|orpc_internal, weak_self| Self {
            orpc_internal,
            monitor: Monitor {
                handles: Mutex::new(Default::default()),
                state: Mutex::new(SimpleState {
                    x: todo!(),
                    z_observer: todo!(),
                    _server_this: weak_self.clone(),
                }),
            },
            // state: todo!(),
        });
        server.monitor.state.lock().start_monitor();

        server.monitor.state.lock().attach_handle_a(oq.attach_strong_observer(ObservationQuery::new(|x| *x))?)?;

        Ok(server)
    }
}

// GENERATED
impl SimpleState {
    fn start_monitor(&self) {
        let server = self.get_ref();
        spawn_thread(server.clone(), {
            let this = server.clone();
            move || {
                let mut handles = this.monitor.handles.lock();
                let mut monitor_state = this.monitor.state.lock();
                loop {
                    // select over all handles
                    // handles.handle_a ..., *and* an attachment change OQueue which has commands that cause this to swap attachments.
                    monitor_state.handle_a(x);
                    {
                        // Attach command
                        let old_handle = handles.handle_a.take();
                        drop(old_handle);
                        handles.handle_a = Some(handle);
                        reply_producer.produce(());
                    }
                }
                Ok(())
            }
        });
    }

    // GENERATED: from SimpleState decl
    fn attach_handle_a_inline(
        &self,
        handle: StrongObserver<i32>,
    ) -> Result<(), AttachmentError> {
        let server = self.get_ref();
        let mut handles = server.monitor.handles.lock();
        // Drop the existing attachment and *then* create the new attachment. This makes sure that
        // there is never two attachments.
        let old_handle = handles.handle_a.take();
        drop(old_handle);
        handles.handle_a = Some(handle.strong_observe_inline({
            let server = server.clone();
            move |x| {
                server.monitor.state.lock().handle_a(x);
            }
        })?);
        Ok(())
    }

    // GENERATED: from SimpleState decl
    fn attach_handle_a_thread(
        &self,
        handle: StrongObserver<i32>,
    ) -> Result<(), AttachmentError> {
        let server = self.get_ref();
        // Need a reply to make sure the attachment has completed when this returns. This is
        // important for ordering of attachments.
        let reply_queue = ReplyOQueue::new();
        let reply_consumer = reply_queue.attach_consumer()?;
        server.monitor
            .attach_oqueue
            .attach_producer()?
            .produce(ReplyableCommand(
                AttachCommand::AttachHandleA(handle),
                reply_queue.attach_producer()?,
            ));
        reply_consumer.consume();

        Ok(())
    }
}

#[orpc_trait]
trait TTTT {
    fn f(&self, x: u64) -> Result<u32, RPCError>;
}

#[orpc_impl]
impl TTTT for Server {
    fn f(&self, x: u64) -> Result<u32, RPCError> {
        // GENERATED and wrapped like current generation
        self.monitor.state.lock().f(x)
    }
}
