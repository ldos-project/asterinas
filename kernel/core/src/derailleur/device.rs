// SPDX-License-Identifier: MPL-2.0

//! The two ivshmem devices derailleur gives a VM, and the cassette client's
//! transport over them.
//!
//! - The mailbox (16 MiB): the cassette session (request batches) and the
//!   page-request slots.
//! - The peer's RAM (VM1's guest RAM, as a BAR): page contents, at the
//!   guest-physical addresses VM1 reports.
//!
//! Both are ivshmem-plain PCI devices (1af4:1110); BAR 2 is the shared memory.
//! They are told apart by size: the mailbox is the 16 MiB one.

use alloc::vec;
use core::{
    fmt::Debug,
    hint::spin_loop,
    sync::atomic::{Ordering, fence},
};

use aster_pci::{
    PciDeviceId,
    bus::{PciDevice, PciDriver},
    cfg_space::Bar,
    common_device::PciCommonDevice,
};
use ostd::{
    bus::BusProbeError,
    io::IoMem,
    mm::{CachePolicy, VmIo, VmIoOnce},
};
use spin::Once;

use super::wire::{self, session as s, slot as sl};
use crate::{prelude::*, process::signal::HandlePendingSignal};

const MAILBOX_SIZE: u64 = 16 << 20;

pub struct Shared {
    pub mailbox: IoMem,
    pub peer: IoMem,
}

static MAILBOX: Once<IoMem> = Once::new();
static PEER: Once<IoMem> = Once::new();
static PEER_MEM: Once<ostd::mm::frame::peer::PeerMemory> = Once::new();

/// The peer's RAM as lendable frames (when it could be registered).
pub fn peer_memory() -> Option<&'static ostd::mm::frame::peer::PeerMemory> {
    PEER_MEM.get()
}

/// The mailbox and the peer's RAM, once both devices were found.
pub fn shared() -> Result<Shared> {
    match (MAILBOX.get(), PEER.get()) {
        (Some(m), Some(p)) => Ok(Shared {
            mailbox: m.clone(),
            peer: p.clone(),
        }),
        _ => return_errno_with_message!(Errno::ENODEV, "no derailleur ivshmem devices"),
    }
}

#[derive(Debug)]
struct Ivshmem {
    id: PciDeviceId,
}

impl PciDevice for Ivshmem {
    fn device_id(&self) -> PciDeviceId {
        self.id
    }
}

#[derive(Debug)]
struct IvshmemDriver;

impl PciDriver for IvshmemDriver {
    fn probe(
        &self,
        device: PciCommonDevice,
    ) -> core::result::Result<Arc<dyn PciDevice>, (BusProbeError, PciCommonDevice)> {
        let id = *device.device_id();
        if id.vendor_id != 0x1af4 || id.device_id != 0x1110 {
            return Err((BusProbeError::DeviceNotMatch, device));
        }
        let (base, size) = match device.bar_manager().bar(2) {
            Some(Bar::Memory(b)) => (b.base(), b.size()),
            _ => return Err((BusProbeError::ConfigurationSpaceError, device)),
        };
        // Shared RAM on the host: map it write-back, like RAM.
        let io_mem = match IoMem::acquire_with_cache_policy(
            (base as usize)..((base + size) as usize),
            CachePolicy::Writeback,
        ) {
            Ok(m) => m,
            Err(e) => {
                ostd::early_println!("derailleur: ivshmem BAR {:#x}+{:#x} not mapped: {:?}", base, size, e);
                return Err((BusProbeError::ConfigurationSpaceError, device));
            }
        };
        if size == MAILBOX_SIZE {
            MAILBOX.call_once(|| io_mem);
        } else {
            // Its frames can then be mapped in place (lent, not copied).
            match ostd::mm::frame::peer::PeerMemory::register(io_mem.clone()) {
                Ok(pm) => {
                    PEER_MEM.call_once(|| pm);
                }
                Err(e) => ostd::early_println!("derailleur: peer RAM not lendable: {:?}", e),
            }
            PEER.call_once(|| io_mem);
        }
        ostd::early_println!(
            "derailleur: ivshmem {} at {:#x} ({} MiB)",
            if size == MAILBOX_SIZE { "mailbox" } else { "peer RAM" },
            base,
            size >> 20
        );
        Ok(Arc::new(Ivshmem { id }))
    }
}

pub fn init() {
    aster_pci::PCI_BUS
        .lock()
        .register_driver(Arc::new(IvshmemDriver));
}

/* ------------------------------------------------------------ the client */

/// One request of a batch.
pub struct Req<'a> {
    pub op: u32,
    pub key: &'a str,
}

/// One answer: status, and the value copied out of the heap.
pub struct Resp {
    pub status: i64,
    pub value: Vec<u8>,
}

impl Shared {
    fn r32(&self, off: usize) -> u32 {
        self.mailbox.read_once::<u32>(off).unwrap()
    }
    fn r64(&self, off: usize) -> u64 {
        self.mailbox.read_once::<u64>(off).unwrap()
    }
    fn w32(&self, off: usize, v: u32) {
        self.mailbox.write_once::<u32>(off, &v).unwrap()
    }
    fn w64(&self, off: usize, v: u64) {
        self.mailbox.write_once::<u64>(off, &v).unwrap()
    }

    /// Waits for an offer; returns the offered pid.
    pub fn wait_offer(&self, ctx: &Context) -> Result<i32> {
        loop {
            fence(Ordering::Acquire);
            if self.r32(wire::CAS_OFF + s::MAGIC) == wire::CAS_MAGIC {
                let pid = self.r32(wire::CAS_OFF + s::PID) as i32;
                if pid > 0 {
                    return Ok(pid);
                }
            }
            if ctx.has_pending_sigkill() {
                return_errno!(Errno::EINTR);
            }
            ostd::task::Task::yield_now();
        }
    }

    /// Opens the session on the posted offer.
    pub fn open(&self) -> Result<()> {
        if self.r32(wire::CAS_OFF + s::MAGIC) != wire::CAS_MAGIC {
            return_errno_with_message!(Errno::EAGAIN, "no offer");
        }
        let v = self.r32(wire::CAS_OFF + s::VERSION);
        if v != wire::CAS_VERSION {
            ostd::early_println!("derailleur: cassette version {}, we speak {}", v, wire::CAS_VERSION);
            return_errno!(Errno::EPROTONOSUPPORT);
        }
        fence(Ordering::Release);
        self.w32(wire::CAS_OFF + s::SESSION, 1);
        Ok(())
    }

    /// Sends one batch and waits for the answers.
    pub fn call(&self, reqs: &[Req]) -> Result<Vec<Resp>> {
        let base = wire::CAS_OFF;
        for (i, r) in reqs.iter().enumerate() {
            let at = base + s::REQ + i * s::REQ_SIZE;
            let mut key = [0u8; wire::CAS_KEY];
            key[..r.key.len()].copy_from_slice(r.key.as_bytes());
            self.w32(at, r.op);
            self.w32(at + 4, 0);
            self.w64(at + 8, 0);
            self.mailbox.write_bytes(at + 16, &key).unwrap();
        }
        self.w32(base + s::NREQ, reqs.len() as u32);
        let seq = self.r64(base + s::BATCH) + 1;
        fence(Ordering::Release);
        self.w64(base + s::BATCH, seq);
        let mut spins = 0u64;
        while self.r64(base + s::DONE) != seq {
            spin_loop();
            spins += 1;
            if spins % 4096 == 0 {
                ostd::task::Task::yield_now();
            }
        }
        fence(Ordering::Acquire);
        let mut out = Vec::with_capacity(reqs.len());
        for i in 0..reqs.len() {
            let at = base + s::RESP + i * s::RESP_SIZE;
            let status = self.r64(at) as i64;
            let (off, len) = (self.r64(at + 8) as usize, self.r64(at + 16) as usize);
            let mut value = Vec::new();
            if status >= 0 && len > 0 && off <= wire::CAS_HEAP && len <= wire::CAS_HEAP - off {
                value = vec![0u8; len];
                self.mailbox
                    .read_bytes(base + s::HEAP + off, &mut value)
                    .unwrap();
            }
            out.push(Resp { status, value });
        }
        Ok(out)
    }

    /// Resolves `n` (<= 512) pages of the process at `addr` on slot `i`:
    /// VM1's guest-physical address of each, with flags, or GPA_NONE.
    pub fn resolve(&self, i: usize, addr: u64, n: usize) -> Result<Vec<u64>> {
        let at = sl::SLOTS_OFF + i * sl::STRIDE;
        self.w64(at + sl::ADDR, addr);
        self.w32(at + sl::NPAGES, n as u32);
        self.w32(at + sl::WRITE, sl::RW_FORCE);
        self.w32(at + sl::OP, sl::OP_VA);
        self.w32(at + sl::OBJ, 0);
        let seq = self.r64(at + sl::REQ_SEQ) + 1;
        fence(Ordering::Release);
        self.w64(at + sl::REQ_SEQ, seq);
        while self.r64(at + sl::RESP_SEQ) != seq {
            spin_loop();
        }
        fence(Ordering::Acquire);
        let status = self.r64(at + sl::STATUS) as i64;
        if status < 0 {
            return_errno_with_message!(Errno::EIO, "page request failed on the server");
        }
        let mut gpa = vec![0u64; n];
        for (k, g) in gpa.iter_mut().enumerate() {
            *g = self.r64(at + sl::GPA + k * 8);
        }
        Ok(gpa)
    }
}
