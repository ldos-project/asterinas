// SPDX-License-Identifier: MPL-2.0

mod clone;
pub mod credentials;
mod exit;
mod kill;
pub mod posix_thread;
#[expect(clippy::module_inception)]
mod process;
mod process_filter;
pub mod process_table;
mod process_vm;
mod program_loader;
pub mod rlimit;
pub mod signal;
mod status;
pub mod sync;
mod task_set;
mod term_status;
mod wait;

pub use clone::{CloneArgs, CloneFlags, clone_child};
pub use credentials::{Credentials, Gid, Uid};
pub use kill::{kill, kill_all, kill_group, tgkill};
pub use process::{
    ExitCode, JobControl, PauseProcGuard, Pgid, Pid, Process, ProcessGroup, Session, Sid, Terminal,
    broadcast_signal_async, enqueue_signal_async, spawn_init_process,
};
pub use process_filter::ProcessFilter;
pub use process_vm::{
    MAX_ARG_LEN, MAX_ARGV_NUMBER, MAX_ENV_LEN, MAX_ENVP_NUMBER, renew_vm_and_map,
};
pub use program_loader::{ProgramToLoad, check_executable_file};
pub use rlimit::ResourceType;
pub use term_status::TermStatus;
pub use wait::{WaitOptions, do_wait};

pub(super) fn init() {
    process::init();
    posix_thread::futex::init();
}
