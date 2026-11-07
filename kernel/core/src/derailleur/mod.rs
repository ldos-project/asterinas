// SPDX-License-Identifier: MPL-2.0

//! derailleur: take over a process from a Linux VM on the same host
//! (cross-OS process migration, prototype).  This kernel is the cassette
//! client; the Linux VM offering the process is the server.  See the
//! derailleur repository's docs/cassette.md and docs/asterinas.md.

mod device;
pub mod restore;
mod wire;

use core::sync::atomic::AtomicBool;

use ostd::sync::Mutex;

/// The current session lent frames (VM1 keeps them pinned until `done`).
static LENT: Mutex<bool> = Mutex::new(false);
/// `derailleur.copy` (kernel command line): copy pages instead of lending
/// them (for comparison).
static NO_LEND: AtomicBool = AtomicBool::new(false);
aster_cmdline::define_flag_param!("derailleur.copy", NO_LEND);

pub fn init() {
    device::init();
}
