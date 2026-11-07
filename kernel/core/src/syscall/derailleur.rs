// SPDX-License-Identifier: MPL-2.0

//! derailleur's syscalls (Asterinas only; Linux uses ioctls on /dev/derailleur).
//! The `derailleur restore` tool calls them in this order.

use ostd::{arch::cpu::context::UserContext, mm::VmIo};

use super::SyscallReturn;
use crate::{derailleur::restore, prelude::*};

/// Waits for VM1 to offer a process; stores its pid at `pid_ptr`.
pub fn sys_derailleur_offered(pid_ptr: Vaddr, ctx: &Context) -> Result<SyscallReturn> {
    let pid = restore::offered_pid(ctx)?;
    ctx.user_space().write_val(pid_ptr, &pid)?;
    Ok(SyscallReturn::Return(0))
}

/// Becomes the offered process (the caller must be single-threaded).
/// Returns to user mode as that process.
pub fn sys_derailleur_restore(
    _flags: u64,
    ctx: &Context,
    user_ctx: &mut UserContext,
) -> Result<SyscallReturn> {
    restore::restore(ctx, user_ctx)?;
    Ok(SyscallReturn::NoReturn)
}

/// Ends the session (the restore already released VM1 in this stage).
pub fn sys_derailleur_done(_ctx: &Context) -> Result<SyscallReturn> {
    Ok(SyscallReturn::Return(0))
}
