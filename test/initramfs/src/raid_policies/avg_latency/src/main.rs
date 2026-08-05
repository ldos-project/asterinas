// SPDX-License-Identifier: MPL-2.0

//! `raid_policy_avg_latency`: select_block_device the admitted candidate with the lowest recent average
//! completion latency, breaking ties (including "no data yet") by round-robin. This is the default
//! policy the supervisor starts at boot.

use raid_policy_common::{SelectionPolicy, SelectionContext, SelectionRequest};

struct AvgLatencyPolicy;

impl SelectionPolicy for AvgLatencyPolicy {
    const NAME: &'static str = "avg_latency";

    fn select_block_device(&self, ctx: &SelectionContext, req: &SelectionRequest) -> u32 {
        let mut best: Option<(u32, u64)> = None;
        for &candidate in &req.candidates {
            let completion_trace = ctx.weak_observe_recent(candidate);
            if completion_trace.is_empty() {
                continue;
            }
            let average =
                completion_trace.iter().map(|c| c.latency_us).sum::<u64>() / completion_trace.len() as u64;
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
            // No data yet for any candidate: round-robin among them.
            None => {
                use std::sync::atomic::Ordering;
                req.candidates[ctx.read_cursor().fetch_add(1, Ordering::Relaxed) % req.candidates.len()]
            }
        }
    }
}

fn main() {
    raid_policy_common::run(AvgLatencyPolicy);
}
