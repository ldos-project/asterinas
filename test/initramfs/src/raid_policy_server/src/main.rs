// SPDX-License-Identifier: MPL-2.0

//! `raid_policy_server`: a userspace RAID-1 read-selection policy over OQFS (`/oqueues`).
//!
//! In the background, this drains each RAID-1 member's `bio_completion` OQueue (exposed at
//! `/oqueues/raid1/bio_completion/<index>/strong_observe` as a CBOR map keyed by field name —
//! `{latency_us, outstanding_pages, queue_len, request_size_pages, device_id}`, see
//! `BioCompletionStatsMessage` in `kernel/src/device/registry/raid.rs`), keeping the four most
//! recent latency samples per member.
//!
//! Selection itself is kernel-triggered request/reply, run one at a time on the main thread: block
//! reading one selection request (a CBOR map of the admitted candidate indices; see
//! `SelectionRequestMessage` in `kernel/comps/raid/src/selection_policies.rs`) from
//! `/oqueues/raid1/selection_request/strong_observe`, choose the candidate with the lowest recent
//! average latency (round-robin among candidates with no data yet), and write exactly one reply
//! (that member's index) to `/oqueues/raid1/decision/produce`. The kernel's `UserspacePolicy`
//! (`kernel/comps/raid/src/selection_policies.rs`) produces one request and blocks for this reply
//! on every read it needs to route.
//!
//! Both ends declare their messages with `serde` derive and encode/decode through `minicbor-serde`:
//! the kernel serializes each record as a CBOR map keyed by field-name strings (with `candidates` a
//! nested CBOR array), and this program deserializes it into a mirror struct. Each decision reply is
//! a bare CBOR unsigned integer.

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

/// Maximum admitted candidate indices carried in one selection request; must match
/// `MAX_REQUEST_CANDIDATES` in `kernel/comps/raid/src/selection_policies.rs`.
const MAX_REQUEST_CANDIDATES: usize = 8;

#[derive(serde::Deserialize)]
#[allow(dead_code)] // fields other than `latency_us` mirror the wire format but are unused today
struct BioCompletionStats {
    latency_us: u64,
    outstanding_pages: u32,
    queue_len: u32,
    request_size_pages: u32,
    device_id: u32,
}

#[derive(serde::Deserialize)]
struct SelectionRequest {
    candidate_count: u32,
    candidates: [u32; MAX_REQUEST_CANDIDATES],
}

/// Frames and decodes one CBOR record of type `T` from the front of `bytes`, mirroring the
/// `produce_cbor` method in `ostd/src/orpc/oqueue/export.rs`.
///
/// `minicbor` probes for one complete, well-formed CBOR item — the only job kept at the low
/// `minicbor` level, because it distinguishes "not enough bytes yet" from "malformed", which the
/// `minicbor-serde` layer alone cannot — and then `minicbor-serde` structurally decodes that item
/// into `T`.
///
/// Returns `Ok(None)` when `bytes` does not yet hold a complete record (read more and retry from the
/// same offset), `Ok(Some((value, bytes_consumed)))` on success, and `Err(..)` when the bytes form a
/// complete record that is malformed or does not decode as `T` (a loud failure, not an infinite
/// retry).
fn decode_record<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
) -> Result<Option<(T, usize)>, Box<dyn std::error::Error>> {
    if bytes.is_empty() {
        return Ok(None);
    }

    let mut probe = minicbor::decode::Decoder::new(bytes);
    match probe.skip() {
        Err(err) if err.is_end_of_input() => return Ok(None),
        Err(err) => return Err(err.into()),
        Ok(()) => {}
    }
    let item_len = probe.position();

    let mut deserializer = minicbor_serde::Deserializer::new(&bytes[..item_len]);
    let value = T::deserialize(&mut deserializer)?;
    Ok(Some((value, item_len)))
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
        loop {
            let stats = match decode_record::<BioCompletionStats>(&pending[consumed_total..]) {
                Ok(Some((stats, consumed))) => {
                    consumed_total += consumed;
                    stats
                }
                Ok(None) => break,
                Err(err) => {
                    eprintln!(
                        "raid_policy_server: member {index}'s bio_completion stream is malformed: {err}"
                    );
                    return;
                }
            };

            let mut history = history.lock().unwrap();
            if history.len() >= HISTORY_LEN {
                history.pop_front();
            }
            history.push_back(stats.latency_us);
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
            match decode_record::<SelectionRequest>(&pending) {
                Ok(Some((request, consumed))) => {
                    pending.drain(..consumed);
                    let count = (request.candidate_count as usize).min(MAX_REQUEST_CANDIDATES);
                    break request.candidates[..count].to_vec();
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("raid_policy_server: selection request stream is malformed: {err}");
                    return;
                }
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
        serde::Serialize::serialize(
            &(chosen as u64),
            &mut minicbor_serde::Serializer::new(&mut record),
        )
        .expect("serializing a u64 cannot fail");
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
