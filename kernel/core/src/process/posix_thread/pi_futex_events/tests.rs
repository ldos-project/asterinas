// SPDX-License-Identifier: MPL-2.0

use minicbor_serde::{Deserializer, Serializer};
use ostd::prelude::ktest;
use serde::{Deserialize, de::IgnoredAny};

use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventView {
    time_ns: u64,
    pid: Pid,
    futex_addr: u64,
    owner: Option<OwnerView>,
    details: IgnoredAny,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerView {
    tid: Tid,
    policy: IgnoredAny,
}

fn round_trip(owner: Option<PiFutexOwner>, details: PiFutexEventDetails) -> EventView {
    let event = PiFutexEvent {
        time_ns: 123,
        pid: 1,
        futex_addr: 0x1000,
        owner,
        details,
    };
    let mut bytes = Vec::new();
    event.serialize(&mut Serializer::new(&mut bytes)).unwrap();
    let view = EventView::deserialize(&mut Deserializer::new(&bytes)).unwrap();
    assert_eq!(view.time_ns, event.time_ns);
    assert_eq!(view.pid, event.pid);
    assert_eq!(view.futex_addr, event.futex_addr);
    let IgnoredAny = view.details;
    view
}

#[ktest]
fn owner_metadata_is_common_to_all_event_details() {
    let policy = SchedPolicyView::Fair { nice: 0 };
    let details = [
        PiFutexEventDetails::Block {
            waiter: 3,
            waiter_policy: policy,
        },
        PiFutexEventDetails::Handoff {
            from: 2,
            to: 3,
            waiters_left: 0,
        },
        PiFutexEventDetails::WaitEnd {
            waiter: 3,
            waited_ns: 10,
            outcome: PiWaitOutcome::Acquired,
        },
    ];
    for details in details {
        let view = round_trip(Some(PiFutexOwner { tid: 3, policy }), details);
        let owner = view.owner.unwrap();
        assert_eq!(owner.tid, 3);
        let IgnoredAny = owner.policy;
    }
}

#[ktest]
fn wait_end_can_report_no_current_owner() {
    let view = round_trip(
        None,
        PiFutexEventDetails::WaitEnd {
            waiter: 3,
            waited_ns: 10,
            outcome: PiWaitOutcome::Retry,
        },
    );
    assert!(view.owner.is_none());
}
