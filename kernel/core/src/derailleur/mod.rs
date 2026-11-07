// SPDX-License-Identifier: MPL-2.0

//! derailleur: take over a process from a Linux VM on the same host
//! (cross-OS process migration, prototype).  This kernel is the cassette
//! client; the Linux VM offering the process is the server.  See the
//! derailleur repository's docs/cassette.md and docs/asterinas.md.

mod device;
pub mod restore;
mod wire;

pub fn init() {
    device::init();
}
