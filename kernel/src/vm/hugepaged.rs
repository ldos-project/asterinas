// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::time::Duration;

use ostd::{
    new_server,
    orpc::{
        framework::{notifier::Notifier, spawn_thread},
        oqueue::{OQueueBase as _, ObservationQuery},
        orpc_server, orpc_trait,
        sync::select,
    },
    path,
};
use snafu::Whatever;

use super::{promote_hugepages, vmar::PageFaultOQueueMessage};
use crate::{process::Process, util::timer::TimerServer, vm::vmar};

#[orpc_trait]
pub(crate) trait HugePageD {}

/// HugePage daemon that periodically attempts to promote pages to huge pages
#[orpc_server(HugePageD)]
pub struct HugepagedServer {}

impl HugepagedServer {
    /// Create and spawn a new HugepagedServer.
    pub fn spawn(initproc: Arc<Process>) -> Arc<Self> {
        let hugepaged = Self::new().unwrap();
        spawn_thread(hugepaged.clone(), {
            let hugepaged = hugepaged.clone();
            move || hugepaged.main(initproc)
        });
        hugepaged
    }
    pub fn new() -> Result<Arc<Self>, Whatever> {
        let server = new_server!(path!(hugepaged[unique]), |_| Self {});
        Ok(server)
    }

    pub fn main(&self, initproc: Arc<Process>) -> Result<(), Box<dyn core::error::Error>> {
        let notify_server = TimerServer::spawn(Duration::from_secs(1));

        let pagefault_oq = vmar::oqueues::get_page_fault_oqueue();
        let pagefault_observer =
            pagefault_oq.attach_strong_observer(ObservationQuery::identity())?;
        let notify_observer = notify_server
            .notification_oqueue()
            .attach_strong_observer(ObservationQuery::unit())?;
        loop {
            let mut value: Option<PageFaultOQueueMessage> = None;
            loop {
                select!(
                    if let msg = pagefault_observer.try_strong_observe() {
                        value = Some(msg.event);
                        break;
                    },
                    if let _ = notify_observer.try_strong_observe() {
                        break;
                    }
                );
                ostd::task::Task::yield_now();
            }

            let mut procs: Vec<Arc<Process>> = Vec::new();
            procs.push(initproc.clone());
            while let Some(proc) = procs.pop() {
                proc.current_children()
                    .iter()
                    .for_each(|c| procs.push(c.clone()));

                if promote_hugepages(&proc, value).is_err() {
                    break;
                }
            }
        }
    }
}
