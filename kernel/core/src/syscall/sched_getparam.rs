// SPDX-License-Identifier: MPL-2.0

use ostd::mm::VmIo;

use super::{
    SyscallReturn, sched_get_priority_max::rt_to_static, sched_getattr::access_sched_attr_with,
};
use crate::{prelude::*, sched::SchedPolicy, thread::Tid};

pub fn sys_sched_getparam(tid: Tid, addr: Vaddr, ctx: &Context) -> Result<SyscallReturn> {
    if addr == 0 {
        return_errno_with_message!(Errno::EINVAL, "invalid user space address");
    }

    let policy = access_sched_attr_with(tid, ctx, |attr| Ok(attr.policy()))?;
    let rt_prio = match policy {
        SchedPolicy::RealTime { rt_prio, .. } => rt_to_static(rt_prio),
        _ => 0,
    };

    ctx.user_space().write_val(addr, &rt_prio)?;

    Ok(SyscallReturn::Return(0))
}
