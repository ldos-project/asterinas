#![cfg(test)]

use assert_matches::assert_matches;
use orpc::{orpc_impl, orpc_server, orpc_trait};

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    thread::sleep,
    time::Duration,
};

use snafu::{ResultExt as _, Whatever};

use crate::{
    oqueue::{locking::LockingQueue, OQueueRef, Receiver},
    orpc_impl::{errors::RPCError, framework::CurrentServer, ServerRef}, spawn_thread,
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
        // collect!(addend, addend);
        let v = self.atomic_count.fetch_add(addend, Ordering::Relaxed);
        Ok(v + addend)
    }

    fn incr_oqueue(&self) -> OQueueRef<AdditionalAmount>;
}

impl ServerAState {
    fn main_thread(
        &self,
        incr_oqueue_receiver: Box<dyn Receiver<AdditionalAmount>>,
    ) -> Result<(), Whatever> {
        let mut _count = 0;
        loop {
            let AdditionalAmount { n, trigger_panic } = incr_oqueue_receiver.receive();
            if trigger_panic {
                panic!("Asked to panic by message");
            }
            let addend = self.increment + n;
            // collect!(addend, addend);
            _count += addend;
            CurrentServer::abort_point();
        }
    }

    fn spawn(increment: usize, atomic_count: AtomicUsize) -> Result<ServerRef<Self>, Whatever> {
        let server = Self::new_with(|orpc_internal| Self {
            increment,
            atomic_count,
            orpc_internal,
        });
        spawn_thread(server.clone(), {
            let server = server.clone();
            let incr_oqueue_receiver = server
                .incr_oqueue()
                .attach_receiver()
                .whatever_context("incr_oqueue_receiver")?;
            move || Ok(server.main_thread(incr_oqueue_receiver)?)
        });
        Ok(server)
    }
}

#[test]
fn start_server() {
    let _server_ref = ServerAState::spawn(2, AtomicUsize::new(0)).unwrap();
}

#[test]
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

    let server_ref: ServerRef<dyn Counter> = server_ref;
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

#[test]
fn message() {
    let server_ref = ServerAState::spawn(2, AtomicUsize::new(0)).unwrap();

    server_ref
        .incr_oqueue()
        .attach_sender()
        .unwrap()
        .send(AdditionalAmount {
            n: 1,
            trigger_panic: false,
        });

    let server_ref: ServerRef<dyn Counter> = server_ref;

    server_ref
        .incr_oqueue()
        .attach_sender()
        .unwrap()
        .send(AdditionalAmount {
            n: 1,
            trigger_panic: false,
        });

    server_ref
        .incr_oqueue()
        .attach_sender()
        .unwrap()
        .send(AdditionalAmount {
            n: 1,
            trigger_panic: true,
        });

    // This is fundamentally racy, but it's very hard to avoid because any reply from the message send above will, by
    // definition, be sent before the panic.
    sleep(Duration::from_millis(250));

    assert_matches!(
        server_ref.atomic_incr(AdditionalAmount {
            n: 1,
            trigger_panic: false,
        }),
        Err(RPCError::ServerMissing)
    );
}
