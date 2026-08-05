// SPDX-License-Identifier: MPL-2.0

//! `raid_policy_common`: shared plumbing for the standalone userspace RAID-1 read-selection policy
//! programs.
//!
//! Every policy is its own crate with its own `main`, and talks to the kernel ONLY through OQFS
//! files under `/oqueues/raid1`. What every policy needs — and nothing policy-specific — lives here:
//!
//!   - OQFS attach helpers with open-retry ([`oqfs`]),
//!   - the CBOR codec for both wire formats and `SelectionRequest` decoding ([`cbor`],
//!     [`SelectionRequest`]),
//!   - the per-member completion history and its draining thread ([`SelectionContext`]),
//!   - the 31-element `feature_digits` vector and its float conversion ([`features`]),
//!   - the shared candidate-loop control flow (round-robin read_cursor, first-predicted-fast wins,
//!     round-robin fallback when all predict slow) ([`SelectionContext::run_candidate_loop`]).
//!
//! A policy crate contains only its own decision logic (and, where applicable, its own weights),
//! implements [`SelectionPolicy`], and calls [`run`] from `main`.

pub mod cbor;
pub mod features;
pub mod oqfs;

use std::{
    collections::VecDeque,
    fs::File,
    io::Write,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
};

pub use features::{feature_digits_f32, feature_digits_u8, BlockDeviceCompletionStats, FEATURE_LEN, HISTORY_LEN};

/// Maximum admitted candidate indices carried in one selection request; must match
/// `MAX_REQUEST_CANDIDATES` in `kernel/comps/raid/src/selection_policies.rs`.
pub const MAX_REQUEST_CANDIDATES: usize = 8;

/// One kernel-triggered selection request, decoded from the request stream.
///
/// The kernel packs, besides the admitted candidate indices, the two features a userspace program
/// cannot derive from the completion stream on its own: `request_size_pages` (the size of the
/// still-pending bio) and each candidate's *live* `outstanding_pages` page count (`outstanding_pages` is
/// parallel to `candidates`). See `SelectionRequestMessage` in the kernel.
pub struct SelectionRequest {
    /// Admitted member indices to select_block_device among (never empty).
    pub candidates: Vec<u32>,
    /// Size, in 4KB pages, of the pending bio being routed.
    pub request_size_pages: u32,
    /// Live outstanding_pages-page count for each candidate, in the same order as `candidates`.
    pub outstanding_pages: Vec<u32>,
}

impl SelectionRequest {
    /// The live outstanding pages of `device_idx`, or 0 if it is not among the candidates.
    fn num_outstanding_pages(&self, device_idx: u32) -> u32 {
        self.candidates
            .iter()
            .position(|&c| c == device_idx)
            .map(|slot| self.outstanding_pages[slot])
            .unwrap_or(0)
    }

    /// The "current outstanding pages" feature the kernel builds as
    /// `num_pages + members[device_idx].num_outstanding_pages()`.
    pub fn current_outstanding(&self, device_idx: u32) -> u32 {
        self.request_size_pages + self.num_outstanding_pages(device_idx)
    }
}

/// Per-member completion history plus the shared round-robin read_cursor: everything a policy's decision
/// logic reads. `observers` is indexed by member index (matching the kernel's candidate indices).
pub struct SelectionContext {
    observers: Vec<Arc<Mutex<VecDeque<BlockDeviceCompletionStats>>>>,
    read_cursor: AtomicUsize,
}

impl SelectionContext {
    /// Discovers the RAID-1 members and spawns one background thread per member to drain its
    /// `bio_completion` stream into the shared history.
    fn attach() -> Self {
        let member_indices = oqfs::discover_member_indices();
        eprintln!("raid_policy: found members {member_indices:?}");

        // Index observers by member index directly (candidates are member indices), so a policy can
        // look up `observers[device_idx]` exactly as the kernel policies index their own `observers`.
        let capacity = member_indices.iter().copied().max().unwrap_or(0) + 1;
        let observers: Vec<Arc<Mutex<VecDeque<BlockDeviceCompletionStats>>>> = (0..capacity)
            .map(|_| Arc::new(Mutex::new(VecDeque::with_capacity(HISTORY_LEN))))
            .collect();

        for &index in &member_indices {
            let history = observers[index].clone();
            thread::spawn(move || drain_bio_completion(index, history));
        }
        eprintln!("raid_policy: observer threads spawned");

        Self {
            observers,
            read_cursor: AtomicUsize::new(0),
        }
    }

    /// A snapshot of `device_idx`'s completion history, oldest first (empty if unknown / no data).
    pub fn weak_observe_recent(&self, device_idx: u32) -> Vec<BlockDeviceCompletionStats> {
        match self.observers.get(device_idx as usize) {
            Some(history) => history.lock().unwrap().iter().copied().collect(),
            None => Vec::new(),
        }
    }

    /// The shared round-robin read_cursor. Policies with their own control flow (round-robin, lowest
    /// average latency, …) use it for tie-breaking / fallback.
    pub fn read_cursor(&self) -> &AtomicUsize {
        &self.read_cursor
    }

    /// The kernel's 31-element `f32` feature vector for `device_idx`.
    pub fn feature_digits_f32(&self, req: &SelectionRequest, device_idx: u32) -> [f32; FEATURE_LEN] {
        feature_digits_f32(req.current_outstanding(device_idx), &self.weak_observe_recent(device_idx))
    }

    /// The kernel's 31-element `u8` feature vector for `device_idx` (decision-tree input).
    pub fn feature_digits_u8(&self, req: &SelectionRequest, device_idx: u32) -> [u8; FEATURE_LEN] {
        feature_digits_u8(req.current_outstanding(device_idx), &self.weak_observe_recent(device_idx))
    }

    /// The shared candidate-loop control flow used by every ML policy: walk the candidates from the
    /// round-robin read_cursor, return the first one `predict` calls fast; if all predict slow, fall back
    /// to the next round-robin candidate. Mirrors the kernel LinnOS / LinnOS+ / DecisionTreePolicy loop
    /// exactly.
    pub fn run_candidate_loop(
        &self,
        req: &SelectionRequest,
        predict: impl Fn(u32) -> bool,
    ) -> u32 {
        let candidates = &req.candidates;
        let num_candidates = candidates.len();
        let mut fail_cnt = 0;
        loop {
            let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
            let device_idx = candidates[idx % num_candidates];
            if predict(device_idx) {
                return device_idx;
            }
            fail_cnt += 1;
            // All candidates predicted slow -- fall back to round-robin among them.
            if fail_cnt >= num_candidates {
                return candidates[self.read_cursor.fetch_add(1, Ordering::Relaxed) % num_candidates];
            }
        }
    }
}

/// Continuously drains one member's `bio_completion` stream into `history`, keeping the last
/// [`HISTORY_LEN`] completions (oldest first). Each record is the fixed 5-element CBOR array
/// `[latency_us, outstanding_pages, queue_len, request_size_pages, device_id]`.
fn drain_bio_completion(index: usize, history: Arc<Mutex<VecDeque<BlockDeviceCompletionStats>>>) {
    let path = format!("{}/{index}/strong_observe", oqfs::BIO_COMPLETION_DIR);
    let mut file = oqfs::open_with_retry(&path, false);
    eprintln!("raid_policy: attached to member {index}'s bio_completion stream");

    let mut pending = Vec::new();
    let mut chunk = [0u8; oqfs::READ_CHUNK_SIZE];
    loop {
        let read = match oqfs::read_stream_polling(&mut file, &mut chunk) {
            Ok(0) => {
                eprintln!("raid_policy: member {index}'s bio_completion stream ended");
                return;
            }
            Ok(n) => n,
            Err(err) => {
                eprintln!("raid_policy: error reading member {index}: {err}");
                return;
            }
        };
        pending.extend_from_slice(&chunk[..read]);

        let mut consumed_total = 0;
        while let Some((record, consumed)) =
            cbor::decode_bio_completion_record(&pending[consumed_total..])
        {
            consumed_total += consumed;
            let completion = BlockDeviceCompletionStats {
                // Element order matches `BioCompletionStatsMessage` in the kernel.
                latency_us: record[0],
                outstanding_pages: record[1] as u32,
            };
            let mut history = history.lock().unwrap();
            if history.len() >= HISTORY_LEN {
                history.pop_front();
            }
            history.push_back(completion);
        }
        pending.drain(..consumed_total);
    }
}

/// A userspace RAID-1 read-selection policy. Each policy crate implements this and calls [`run`].
pub trait SelectionPolicy {
    /// The policy name (matches the `raid_policy_<NAME>` binary and the supervisor's swap protocol).
    const NAME: &'static str;

    /// Chooses one member index among `req.candidates`, using the per-member completion history and
    /// round-robin read_cursor in `ctx`. The returned index must be one of `req.candidates`.
    fn select_block_device(&self, ctx: &SelectionContext, req: &SelectionRequest) -> u32;

    /// Offline parity / sanity checks run by `--self-test` (no OQueues needed). The default checks
    /// that this crate's `feature_digits` is bit-identical to the kernel's, including the
    /// fewer-than-4-history case. ML policies override to additionally exercise their inference.
    fn self_test(&self) -> Result<(), String> {
        features::feature_vector_parity_self_test()
    }
}

/// The entry point every policy's `main` calls. With `--self-test`, runs [`SelectionPolicy::self_test`]
/// offline and exits 0/non-zero. Otherwise attaches to OQFS and serves kernel-triggered selection
/// requests one at a time on the main thread, forever.
pub fn run<P: SelectionPolicy>(policy: P) -> ! {
    if std::env::args().any(|arg| arg == "--self-test") {
        match policy.self_test() {
            Ok(()) => {
                eprintln!("raid_policy_{}: self-test OK", P::NAME);
                std::process::exit(0);
            }
            Err(err) => {
                eprintln!("raid_policy_{}: self-test FAILED: {err}", P::NAME);
                std::process::exit(1);
            }
        }
    }

    eprintln!("raid_policy_{}: starting", P::NAME);
    let ctx = SelectionContext::attach();

    let mut request_file = oqfs::open_with_retry(oqfs::SELECTION_REQUEST_PATH, false);
    eprintln!("raid_policy_{}: attached to the selection request stream", P::NAME);
    let mut decision_file = oqfs::open_with_retry(oqfs::DECISION_PRODUCE_PATH, true);
    eprintln!("raid_policy_{}: attached to the decision produce file", P::NAME);

    let mut replied_first = false;
    let mut pending = Vec::new();
    let mut chunk = [0u8; oqfs::READ_CHUNK_SIZE];
    let mut record = Vec::new();
    loop {
        // Block for the next kernel-triggered selection request.
        let request = loop {
            if let Some((request, consumed)) = cbor::decode_selection_request(&pending) {
                pending.drain(..consumed);
                break request;
            }
            let read = match oqfs::read_stream_polling(&mut request_file, &mut chunk) {
                Ok(0) => {
                    eprintln!("raid_policy_{}: selection request stream ended", P::NAME);
                    std::process::exit(0);
                }
                Ok(n) => n,
                Err(err) => {
                    eprintln!("raid_policy_{}: error reading a selection request: {err}", P::NAME);
                    std::process::exit(1);
                }
            };
            pending.extend_from_slice(&chunk[..read]);
        };

        let chosen = policy.select_block_device(&ctx, &request);

        record.clear();
        minicbor::encode(chosen as u64, &mut record).expect("encoding a u64 cannot fail");
        if let Err(err) = write_decision(&mut decision_file, &record) {
            eprintln!("raid_policy_{}: failed to write a decision: {err}", P::NAME);
            std::process::exit(1);
        }

        if !replied_first {
            eprintln!(
                "raid_policy_{}: replied to first selection request (member {chosen})",
                P::NAME
            );
            replied_first = true;
        }
    }
}

fn write_decision(file: &mut File, record: &[u8]) -> std::io::Result<()> {
    file.write_all(record)
}
