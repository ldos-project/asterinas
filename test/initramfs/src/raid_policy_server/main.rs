// SPDX-License-Identifier: MPL-2.0

//! `raid_policy_server`: a userspace RAID-1 read-selection policy over OQFS (`/oqueues`).
//!
//! In the background, this drains each RAID-1 member's `bio_completion` OQueue (exposed at
//! `/oqueues/raid1/bio_completion/<index>/strong_observe` as a fixed 5-element CBOR array —
//! `[latency_us, outstanding_pages, queue_len, request_size_pages, device_id]`, see
//! `BioCompletionStatsWire` in `kernel/src/device/registry/raid.rs`), keeping the four most recent
//! latency samples per member.
//!
//! Selection itself is kernel-triggered request/reply, run one at a time on the main thread: block
//! reading one selection request (the admitted candidate indices, a fixed CBOR array; see
//! `SelectionRequestWire` in `kernel/comps/raid/src/selection_policies.rs`) from
//! `/oqueues/raid1/selection_request/strong_observe`, choose the candidate with the lowest recent
//! average latency (round-robin among candidates with no data yet), and write exactly one reply
//! (that member's index) to `/oqueues/raid1/decision/produce`. The kernel's `UserspacePolicy`
//! (`kernel/comps/raid/src/selection_policies.rs`) produces one request and blocks for this reply
//! on every read it needs to route.
//!
//! The values on both OQueues are plain CBOR arrays/unsigned integers (produced via
//! `minicbor_serde` on the kernel side), so this program only needs to encode and decode CBOR
//! unsigned integers and definite-length array headers -- no CBOR crate dependency is needed.

use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

const BIO_COMPLETION_DIR: &str = "/oqueues/raid1/bio_completion";
const SELECTION_REQUEST_PATH: &str = "/oqueues/raid1/selection_request/strong_observe";
const DECISION_PRODUCE_PATH: &str = "/oqueues/raid1/decision/produce";

/// Number of most recent completion latencies kept per member device.
const HISTORY_LEN: usize = 4;

/// Size of the read buffer used when draining an OQueue stream file.
const READ_CHUNK_SIZE: usize = 4096;

/// Decodes one CBOR unsigned-integer-encoded header (an unsigned integer itself, major type 0; or
/// a definite-length array's element count, major type 4) from the front of `bytes`, checking that
/// its major type matches `expected_major`.
///
/// Returns `(value, bytes_consumed)`, or `None` if `bytes` does not yet hold a complete encoding,
/// or if the major type doesn't match (not a shape this stream ever produces).
fn decode_cbor_major_uint(bytes: &[u8], expected_major: u8) -> Option<(u64, usize)> {
    let first = *bytes.first()?;
    if first >> 5 != expected_major {
        return None;
    }
    match first & 0x1F {
        info @ 0..=23 => Some((info as u64, 1)),
        24 => Some((*bytes.get(1)? as u64, 2)),
        25 => {
            let b: [u8; 2] = bytes.get(1..3)?.try_into().ok()?;
            Some((u16::from_be_bytes(b) as u64, 3))
        }
        26 => {
            let b: [u8; 4] = bytes.get(1..5)?.try_into().ok()?;
            Some((u32::from_be_bytes(b) as u64, 5))
        }
        27 => {
            let b: [u8; 8] = bytes.get(1..9)?.try_into().ok()?;
            Some((u64::from_be_bytes(b), 9))
        }
        _ => None,
    }
}

/// Decodes one CBOR unsigned integer (major type 0) from the front of `bytes`.
///
/// Returns `(value, bytes_consumed)`, or `None` if `bytes` does not yet hold a complete encoding.
fn decode_cbor_uint(bytes: &[u8]) -> Option<(u64, usize)> {
    decode_cbor_major_uint(bytes, 0)
}

/// Decodes a CBOR definite-length array header (major type 4) from the front of `bytes`.
///
/// Returns `(element_count, bytes_consumed)`, or `None` if `bytes` does not yet hold a complete
/// header.
fn decode_cbor_array_header(bytes: &[u8]) -> Option<(u64, usize)> {
    decode_cbor_major_uint(bytes, 4)
}

/// Number of elements in a `bio_completion` record: `[latency_us, outstanding_pages, queue_len,
/// request_size_pages, device_id]` (see `BioCompletionStatsWire` in
/// `kernel/src/device/registry/raid.rs`).
const BIO_COMPLETION_RECORD_LEN: usize = 5;

/// Decodes one `bio_completion` record (a fixed 5-element CBOR array of unsigned integers) from
/// the front of `bytes`.
///
/// Returns `(elements, bytes_consumed)`, or `None` if `bytes` does not yet hold a complete record.
/// Only `elements[0]` (the latency, in microseconds) is used by [`choose_member`]'s average-latency
/// policy today; the other fields are decoded and available for a future policy to use.
fn decode_bio_completion_record(bytes: &[u8]) -> Option<([u64; BIO_COMPLETION_RECORD_LEN], usize)> {
    let (len, mut consumed) = decode_cbor_array_header(bytes)?;
    if len != BIO_COMPLETION_RECORD_LEN as u64 {
        return None;
    }
    let mut elements = [0u64; BIO_COMPLETION_RECORD_LEN];
    for element in &mut elements {
        let (value, n) = decode_cbor_uint(&bytes[consumed..])?;
        *element = value;
        consumed += n;
    }
    Some((elements, consumed))
}

/// Maximum admitted candidate indices carried in one selection request; must match
/// `MAX_REQUEST_CANDIDATES` in `kernel/comps/raid/src/selection_policies.rs`.
const MAX_REQUEST_CANDIDATES: usize = 8;

/// Elements in a `selection_request` record: a candidate count followed by
/// [`MAX_REQUEST_CANDIDATES`] fixed slots.
const SELECTION_REQUEST_RECORD_LEN: usize = 1 + MAX_REQUEST_CANDIDATES;

/// Decodes one selection request record (a candidate count followed by fixed slots, all CBOR
/// unsigned integers) from the front of `bytes`.
///
/// Returns `(candidates, bytes_consumed)`, or `None` if `bytes` does not yet hold a complete
/// record.
fn decode_selection_request(bytes: &[u8]) -> Option<(Vec<u32>, usize)> {
    let (len, mut consumed) = decode_cbor_array_header(bytes)?;
    if len != SELECTION_REQUEST_RECORD_LEN as u64 {
        return None;
    }
    let mut elements = [0u64; SELECTION_REQUEST_RECORD_LEN];
    for element in &mut elements {
        let (value, n) = decode_cbor_uint(&bytes[consumed..])?;
        *element = value;
        consumed += n;
    }
    let count = (elements[0] as usize).min(MAX_REQUEST_CANDIDATES);
    let candidates = elements[1..1 + count].iter().map(|&v| v as u32).collect();
    Some((candidates, consumed))
}

/// Encodes `value` as a CBOR unsigned integer (major type 0), appending it to `out`.
fn encode_cbor_uint(value: u64, out: &mut Vec<u8>) {
    if value < 24 {
        out.push(value as u8);
    } else if value <= u8::MAX as u64 {
        out.push(0x18);
        out.push(value as u8);
    } else if value <= u16::MAX as u64 {
        out.push(0x19);
        out.extend_from_slice(&(value as u16).to_be_bytes());
    } else if value <= u32::MAX as u64 {
        out.push(0x1A);
        out.extend_from_slice(&(value as u32).to_be_bytes());
    } else {
        out.push(0x1B);
        out.extend_from_slice(&value.to_be_bytes());
    }
}

/// Max attempts before giving up waiting for an OQueue path or directory to appear.
const MAX_RETRY_ATTEMPTS: u32 = 50;

/// Delay between retries when waiting for an OQueue path or directory to appear.
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Opens `path`, retrying for a few seconds: the OQueue registry is populated during kernel boot,
/// slightly before this program's `open` calls, but this guards against any ordering slop.
fn open_with_retry(path: &str, write: bool) -> File {
    let mut attempt = 0;
    loop {
        let opened = if write {
            fs::OpenOptions::new().write(true).open(path)
        } else {
            File::open(path)
        };
        match opened {
            Ok(file) => return file,
            Err(err) if attempt < MAX_RETRY_ATTEMPTS => {
                attempt += 1;
                eprintln!("raid_policy_server: waiting for {path} ({err}), retrying...");
                thread::sleep(RETRY_INTERVAL);
            }
            Err(err) => {
                panic!("raid_policy_server: failed to open {path}: {err}");
            }
        }
    }
}

/// Discovers the member indices currently exposed under `/oqueues/raid1/bio_completion`.
fn discover_member_indices() -> Vec<usize> {
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
            panic!(
                "raid_policy_server: no RAID-1 members found under {BIO_COMPLETION_DIR} \
                 after waiting"
            );
        }
        attempt += 1;
        eprintln!("raid_policy_server: waiting for {BIO_COMPLETION_DIR}, retrying...");
        thread::sleep(RETRY_INTERVAL);
    }
}

/// Continuously drains one member's `bio_completion` stream, keeping the last [`HISTORY_LEN`]
/// latencies (in microseconds) in `history`.
fn drain_bio_completion(index: usize, history: Arc<Mutex<VecDeque<u64>>>) {
    let path = format!("{BIO_COMPLETION_DIR}/{index}/strong_observe");
    let mut file = open_with_retry(&path, false);
    eprintln!("raid_policy_server: attached to member {index}'s bio_completion stream");

    let mut pending = Vec::new();
    let mut chunk = [0u8; READ_CHUNK_SIZE];
    loop {
        let read = match file.read(&mut chunk) {
            Ok(0) => {
                eprintln!("raid_policy_server: member {index}'s bio_completion stream ended");
                return;
            }
            Ok(n) => n,
            Err(err) => {
                eprintln!("raid_policy_server: error reading member {index}: {err}");
                return;
            }
        };
        pending.extend_from_slice(&chunk[..read]);

        let mut consumed_total = 0;
        while let Some((record, consumed)) = decode_bio_completion_record(&pending[consumed_total..])
        {
            consumed_total += consumed;
            let latency_us = record[0];

            let mut history = history.lock().unwrap();
            if history.len() >= HISTORY_LEN {
                history.pop_front();
            }
            history.push_back(latency_us);
        }
        pending.drain(..consumed_total);
    }
}

// TODO: Migrate the kernel-space selection policies (LinnOS, LinnOS+, DecisionTree; see
// kernel/comps/raid/src/selection_policies.rs) into this userspace server, so all policies can
// eventually run here behind the kernel's `UserspacePolicy` shim instead of only this one.

/// Picks the admitted `candidates` member with the lowest recent average latency, breaking ties
/// (including "no data yet") by round-robin. The feature is each member's average of its last
/// [`HISTORY_LEN`] completion latencies.
fn choose_member(
    histories: &[Arc<Mutex<VecDeque<u64>>>],
    round_robin: &AtomicUsize,
    candidates: &[u32],
) -> u32 {
    let mut best: Option<(u32, u64)> = None;
    for &candidate in candidates {
        let Some(history) = histories.get(candidate as usize) else {
            continue;
        };
        let history = history.lock().unwrap();
        if history.is_empty() {
            continue;
        }
        let average = history.iter().sum::<u64>() / history.len() as u64;
        let is_better = match best {
            Some((_, best_average)) => average < best_average,
            None => true,
        };
        if is_better {
            best = Some((candidate, average));
        }
    }

    match best {
        Some((candidate, _)) => candidate,
        None => candidates[round_robin.fetch_add(1, Ordering::Relaxed) % candidates.len()],
    }
}

fn main() {
    eprintln!("raid_policy_server: starting");

    let member_indices = discover_member_indices();
    eprintln!("raid_policy_server: found members {member_indices:?}");

    let histories: Vec<Arc<Mutex<VecDeque<u64>>>> = member_indices
        .iter()
        .map(|_| Arc::new(Mutex::new(VecDeque::with_capacity(HISTORY_LEN))))
        .collect();

    for (slot, &index) in member_indices.iter().enumerate() {
        let history = histories[slot].clone();
        thread::spawn(move || drain_bio_completion(index, history));
    }
    eprintln!("raid_policy_server: observer threads spawned");

    let mut request_file = open_with_retry(SELECTION_REQUEST_PATH, false);
    eprintln!("raid_policy_server: attached to the selection request stream");
    let mut decision_file = open_with_retry(DECISION_PRODUCE_PATH, true);
    eprintln!("raid_policy_server: attached to the decision produce file");

    let round_robin = AtomicUsize::new(0);
    let mut replied_first = false;
    let mut pending = Vec::new();
    let mut chunk = [0u8; READ_CHUNK_SIZE];
    let mut record = Vec::new();
    loop {
        // Block for the next kernel-triggered selection request.
        let candidates = loop {
            if let Some((candidates, consumed)) = decode_selection_request(&pending) {
                pending.drain(..consumed);
                break candidates;
            }
            let read = match request_file.read(&mut chunk) {
                Ok(0) => {
                    eprintln!("raid_policy_server: selection request stream ended");
                    return;
                }
                Ok(n) => n,
                Err(err) => {
                    eprintln!("raid_policy_server: error reading a selection request: {err}");
                    return;
                }
            };
            pending.extend_from_slice(&chunk[..read]);
        };

        let chosen = choose_member(&histories, &round_robin, &candidates);

        record.clear();
        encode_cbor_uint(chosen as u64, &mut record);
        if let Err(err) = decision_file.write_all(&record) {
            eprintln!("raid_policy_server: failed to write a decision: {err}");
            return;
        }

        if !replied_first {
            eprintln!("raid_policy_server: replied to first selection request (member {chosen})");
            replied_first = true;
        }
    }
}
