// SPDX-License-Identifier: MPL-2.0

use ostd::ostd_error;
use snafu::Snafu;

#[non_exhaustive]
#[ostd_error]
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum ProcfsError {
    #[snafu(display("Unknown OQueue path ({context})"))]
    UnknownPath {},
    #[snafu(display("OQueue lookup returned no live queue ({context})"))]
    QueueGone {},
    #[snafu(display("OQueue observer attachment failed ({context})"))]
    ObserveFailed {
        source: ostd::orpc::oqueue::OQueueError,
    },
}
