// SPDX-License-Identifier: MPL-2.0

use alloc::sync::Arc;

use ostd::orpc::framework::{Server, threads};

use crate::thread::kernel_thread::ThreadOptions;

// NOTE: This cannot be ktested, because this is only meaningful within the full kernel environment.

/// Start a new server thread. This should only be called while spawning a server.
fn spawn_thread(server: Arc<dyn Server + Send + Sync + 'static>, body: threads::ThreadMain) {
    let _ = ThreadOptions::new(ostd::orpc::framework::threads::wrap_server_thread_body(
        server, body,
    ))
    // TODO(arthurp): This sets server threads to be real-time threads with a medium priority. This
    // prevents them from being blocked by user threads, but is probably not the right solution in
    // general.
    .sched_policy(crate::sched::SchedPolicy::RealTime {
        rt_prio: 50.try_into().unwrap(),
        rt_policy: Default::default(),
    })
    .spawn();
}

pub fn init() {
    threads::inject_spawn_thread(spawn_thread);
}
