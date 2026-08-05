// SPDX-License-Identifier: MPL-2.0

//! The CBOR codec for both OQFS wire formats.
//!
//! Both streams carry plain CBOR arrays of unsigned integers (produced via `minicbor_serde` on the
//! kernel side), so we decode/encode them with the `minicbor` crate -- the same CBOR library the
//! kernel side is built on.

use crate::{SelectionRequest, MAX_REQUEST_CANDIDATES};

/// Decodes a definite-length CBOR array of exactly `out.len()` unsigned integers from the front of
/// `bytes` into `out`.
///
/// Returns the number of bytes consumed, or `None` if `bytes` does not yet hold a complete record,
/// or the record isn't an `out.len()`-element array of unsigned integers (not a shape either stream
/// ever produces). On `None`, `out` may have been partially written; callers must not read it.
fn decode_uint_array(bytes: &[u8], out: &mut [u64]) -> Option<usize> {
    let mut decoder = minicbor::decode::Decoder::new(bytes);
    if decoder.array().ok()? != Some(out.len() as u64) {
        return None;
    }
    for element in out.iter_mut() {
        *element = decoder.u64().ok()?;
    }
    Some(decoder.position())
}

/// Number of elements in a `bio_completion` record: `[latency_us, outstanding_pages, queue_len,
/// request_size_pages, device_id]` (see `BioCompletionStatsMessage` in
/// `kernel/src/device/registry/raid.rs`).
const BIO_COMPLETION_RECORD_LEN: usize = 5;

/// Decodes one `bio_completion` record (a fixed 5-element CBOR array of unsigned integers) from the
/// front of `bytes`. Returns `(elements, bytes_consumed)`, or `None` if `bytes` does not yet hold a
/// complete record.
pub fn decode_bio_completion_record(bytes: &[u8]) -> Option<([u64; BIO_COMPLETION_RECORD_LEN], usize)> {
    let mut elements = [0u64; BIO_COMPLETION_RECORD_LEN];
    let consumed = decode_uint_array(bytes, &mut elements)?;
    Some((elements, consumed))
}

/// Elements in a `selection_request` record: `candidate_count`, `request_size_pages`, then
/// [`MAX_REQUEST_CANDIDATES`] candidate slots, then [`MAX_REQUEST_CANDIDATES`] outstanding_pages-page
/// slots. Must match `SelectionRequestMessage::serialize` in the kernel exactly.
const SELECTION_REQUEST_RECORD_LEN: usize = 2 + 2 * MAX_REQUEST_CANDIDATES;

/// Decodes one selection request record from the front of `bytes`.
///
/// Returns `(request, bytes_consumed)`, or `None` if `bytes` does not yet hold a complete record.
pub fn decode_selection_request(bytes: &[u8]) -> Option<(SelectionRequest, usize)> {
    let mut elements = [0u64; SELECTION_REQUEST_RECORD_LEN];
    let consumed = decode_uint_array(bytes, &mut elements)?;

    let count = (elements[0] as usize).min(MAX_REQUEST_CANDIDATES);
    let request_size_pages = elements[1] as u32;
    // Candidate slots start at index 2; the parallel outstanding_pages slots start after all
    // MAX_REQUEST_CANDIDATES candidate slots.
    let candidates = elements[2..2 + count].iter().map(|&v| v as u32).collect();
    let outstanding_base = 2 + MAX_REQUEST_CANDIDATES;
    let outstanding_pages = elements[outstanding_base..outstanding_base + count]
        .iter()
        .map(|&v| v as u32)
        .collect();

    Some((
        SelectionRequest {
            candidates,
            request_size_pages,
            outstanding_pages,
        },
        consumed,
    ))
}
