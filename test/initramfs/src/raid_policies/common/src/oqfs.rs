// SPDX-License-Identifier: MPL-2.0

//! OQFS (`/oqueues`) attach helpers with open-retry.

use std::{
    fs,
    fs::File,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    thread,
    time::Duration,
};

/// `O_NONBLOCK` (Linux). We open the OQueue *stream* files non-blocking on purpose: a blocking
/// OQueue read parks the thread in a raw kernel wait queue that is NOT signal-interruptible, so a
/// policy blocked in one could never be terminated by the supervisor (its `wait()` would hang
/// forever). Reading non-blocking and sleeping briefly while idle keeps every policy thread returning
/// to userspace, where a pending `SIGKILL` is actually delivered — which is what makes the process
/// swap work.
const O_NONBLOCK: i32 = 0o4000;

/// How long to sleep between non-blocking read attempts while an OQueue stream is idle. This bounds
/// both the CPU cost of idling and the extra latency before a produced value is noticed; well under
/// the kernel's 200ms selection reply timeout.
const READ_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Directory under which each RAID-1 member's `bio_completion` stream is exposed.
pub const BIO_COMPLETION_DIR: &str = "/oqueues/raid1/bio_completion";
/// The kernel -> user selection request stream.
pub const SELECTION_REQUEST_PATH: &str = "/oqueues/raid1/selection_request/strong_observe";
/// The user -> kernel decision file. Opening this `produce` file attaches a fresh producer, so at
/// most one policy process may hold it at a time; this is why the supervisor terminates the outgoing
/// process before spawning the incoming one.
pub const DECISION_PRODUCE_PATH: &str = "/oqueues/raid1/decision/produce";

/// Size of the read buffer used when draining an OQueue stream file.
pub const READ_CHUNK_SIZE: usize = 4096;

/// Max attempts before giving up waiting for an OQueue path or directory to appear.
const MAX_RETRY_ATTEMPTS: u32 = 50;

/// Delay between retries when waiting for an OQueue path or directory to appear.
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Opens `path`, retrying for a few seconds: the OQueue registry is populated during kernel boot,
/// slightly before this program's `open` calls, and a freshly-spawned policy process (after a swap)
/// must wait for the outgoing producer's attachment to be released. This guards both cases.
///
/// Read opens are non-blocking (see [`O_NONBLOCK`]); the write (produce) open stays blocking.
pub fn open_with_retry(path: &str, write: bool) -> File {
    let mut attempt = 0;
    loop {
        let opened = if write {
            fs::OpenOptions::new().write(true).open(path)
        } else {
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(O_NONBLOCK)
                .open(path)
        };
        match opened {
            Ok(file) => return file,
            Err(err) if attempt < MAX_RETRY_ATTEMPTS => {
                attempt += 1;
                eprintln!("raid_policy: waiting for {path} ({err}), retrying...");
                thread::sleep(RETRY_INTERVAL);
            }
            Err(err) => {
                panic!("raid_policy: failed to open {path}: {err}");
            }
        }
    }
}

/// Discovers the member indices currently exposed under `/oqueues/raid1/bio_completion`.
pub fn discover_member_indices() -> Vec<usize> {
    let mut attempt = 0;
    loop {
        if let Ok(entries) = fs::read_dir(BIO_COMPLETION_DIR) {
            let mut indices: Vec<usize> = entries
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok()?.parse().ok())
                .collect();
            if !indices.is_empty() {
                indices.sort_unstable();
                return indices;
            }
        }
        if attempt >= MAX_RETRY_ATTEMPTS {
            panic!("raid_policy: no RAID-1 members found under {BIO_COMPLETION_DIR} after waiting");
        }
        attempt += 1;
        eprintln!("raid_policy: waiting for {BIO_COMPLETION_DIR}, retrying...");
        thread::sleep(RETRY_INTERVAL);
    }
}

/// Reads from a non-blocking OQueue stream file, sleeping [`READ_POLL_INTERVAL`] while idle instead
/// of blocking in the kernel. Returns like a normal blocking read (`Ok(0)` on end of stream), but
/// keeps the calling thread returning to userspace so a pending `SIGKILL` can terminate the policy
/// during a swap (see [`O_NONBLOCK`]).
pub fn read_stream_polling(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    loop {
        match file.read(buf) {
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(READ_POLL_INTERVAL);
                continue;
            }
            other => return other,
        }
    }
}
