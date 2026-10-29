// SPDX-License-Identifier: MPL-2.0

use super::SyscallReturn;
use crate::{prelude::*, process::posix_thread::RobustListHead};

pub fn sys_set_robust_list(
    robust_list_head_ptr: Vaddr,
    len: usize,
    ctx: &Context,
) -> Result<SyscallReturn> {
    debug!(
        "robust list head ptr: 0x{:x}, len = {}",
        robust_list_head_ptr, len
    );

    if len != size_of::<RobustListHead>() {
        return_errno_with_message!(
            Errno::EINVAL,
            "the length is not equal to the size of the robust list head"
        );
    }

    // Like Linux, only remember the address. User space keeps updating the list, so it is read
    // when the thread exits.
    ctx.thread_local.robust_list().set(robust_list_head_ptr);

    Ok(SyscallReturn::Return(0))
}
