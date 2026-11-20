// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::time::Duration;

use ostd::orpc::{
    oqueue::{OQueueBase as _, projection::Projection},
    sync::select,
};

use super::{promote_hugepages, vmar::PageFaultMessage};
use crate::{
    process::Process,
    util::timer::TimerServer,
    vm::{PageFaultRecord, vmar},
};

/// Spawns a daemon that periodically attempts to promote pages to huge pages.
pub(crate) fn spawn(initproc: Arc<Process>) {
    ostd::task::spawn_thread(move || run(initproc));
}

fn run(initproc: Arc<Process>) -> Result<(), Box<dyn core::error::Error>> {
    let notify_server = TimerServer::spawn(Duration::from_secs(1));

    let pagefault_oq = vmar::oqueues::get_page_fault_oqueue();
    let pagefault_observer =
        pagefault_oq.attach_strong_observer(Projection::new(|e: &PageFaultMessage| {
            PageFaultRecord {
                vm_space_id: e.vm_space.id(),
                fault_info: e.fault_info,
            }
        }))?;
    let notify_observer = notify_server
        .notification_oqueue()
        .attach_strong_observer(Projection::identity())?;
    loop {
        let mut value: Option<PageFaultRecord> = None;
        loop {
            select!(
                if let msg = pagefault_observer.try_strong_observe() {
                    value = Some(msg);
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
