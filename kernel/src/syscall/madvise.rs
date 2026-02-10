// SPDX-License-Identifier: MPL-2.0

use core::ops::Range;

use align_ext::AlignExt;
use aster_rights::Full;
use ostd::{
    mm::{PagingConsts, page_size},
    task::disable_preempt,
};

use super::SyscallReturn;
use crate::{
    prelude::*,
    vm::vmar::{Vmar, huge_mapping_preserve_on_dontneed},
};

pub fn sys_madvise(
    start: Vaddr,
    len: usize,
    behavior: i32,
    ctx: &Context,
) -> Result<SyscallReturn> {
    let behavior = MadviseBehavior::try_from(behavior)?;
    debug!(
        "start = 0x{:x}, len = 0x{:x}, behavior = {:?}",
        start, len, behavior
    );

    if start % PAGE_SIZE != 0 {
        return_errno_with_message!(Errno::EINVAL, "the start address should be page aligned");
    }
    if len > isize::MAX as usize {
        return_errno_with_message!(Errno::EINVAL, "len align overflow");
    }
    if len == 0 {
        return Ok(SyscallReturn::Return(0));
    }

    let len = len.align_up(PAGE_SIZE);
    let end = start.checked_add(len).ok_or(Error::with_message(
        Errno::EINVAL,
        "integer overflow when (start + len)",
    ))?;
    match behavior {
        MadviseBehavior::MADV_NORMAL
        | MadviseBehavior::MADV_SEQUENTIAL
        | MadviseBehavior::MADV_WILLNEED => {
            // perform a read at first
            let mut buffer = vec![0u8; len];
            ctx.user_space()
                .read_bytes(start, &mut VmWriter::from(buffer.as_mut_slice()))?;
        }
        // TODO(aneesh): MADV_DONTNEED and MADV_FREE are not exactly the same - MADV_FREE is
        // supposed to lazily reclaim pages when there is pressure, while MADV_DONTNEED is eager and
        // should immediately free pages - i.e. MADV_DONTNEED can impact application performance by
        // stalling until the resources are released. However, the implementation of madv_free below
        // is already eager. In the future we should implement a more sophisticated DONTNEED and
        // FREE.
        MadviseBehavior::MADV_DONTNEED => {
            let user_space = ctx.user_space();
            let root_vmar = user_space.root_vmar();
            madv_free(root_vmar, start, end)?
        }
        MadviseBehavior::MADV_FREE => {
            let user_space = ctx.user_space();
            let root_vmar = user_space.root_vmar();
            madv_free(root_vmar, start, end)?
        }
        _ => todo!(),
    }
    Ok(SyscallReturn::Return(0))
}

fn madv_free(root_vmar: &Vmar<Full>, start: Vaddr, end: Vaddr) -> Result<()> {
    let advised_range = start..end;

    let mut mappings_to_remove: Vec<Range<usize>> = vec![];
    {
        let vm_space = root_vmar.vm_space();
        let preempt_guard = disable_preempt();
        let Ok(mut cursor) = vm_space.cursor_mut(&preempt_guard, &advised_range) else {
            return Ok(());
        };

        cursor
            .do_for_each_submapping(start, end, |range, _, _| {
                let size = range.end - range.start;
                if huge_mapping_preserve_on_dontneed() && size > page_size::<PagingConsts>(1) {
                    // TODO(aneesh): zero out the intersection of start..end and
                    // range.start..range.end
                } else {
                    let intersection =
                        core::cmp::max(start, range.start)..core::cmp::min(end, range.end);
                    mappings_to_remove.push(intersection);
                }

                Ok(())
            })
            .unwrap();
    }

    for range in mappings_to_remove {
        // This is advise, so failing silently is acceptable.
        let _ = root_vmar.remove_mapping(range);
    }

    Ok(())
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, TryFromInt)]
#[expect(non_camel_case_types)]
/// This definition is the same from linux
pub enum MadviseBehavior {
    MADV_NORMAL = 0,     /* no further special treatment */
    MADV_RANDOM = 1,     /* expect random page references */
    MADV_SEQUENTIAL = 2, /* expect sequential page references */
    MADV_WILLNEED = 3,   /* will need these pages */
    MADV_DONTNEED = 4,   /* don't need these pages */

    /* common parameters: try to keep these consistent across architectures */
    MADV_FREE = 8,           /* free pages only if memory pressure */
    MADV_REMOVE = 9,         /* remove these pages & resources */
    MADV_DONTFORK = 10,      /* don't inherit across fork */
    MADV_DOFORK = 11,        /* do inherit across fork */
    MADV_HWPOISON = 100,     /* poison a page for testing */
    MADV_SOFT_OFFLINE = 101, /* soft offline page for testing */

    MADV_MERGEABLE = 12,   /* KSM may merge identical pages */
    MADV_UNMERGEABLE = 13, /* KSM may not merge identical pages */

    MADV_HUGEPAGE = 14,   /* Worth backing with hugepages */
    MADV_NOHUGEPAGE = 15, /* Not worth backing with hugepages */

    MADV_DONTDUMP = 16, /* Explicitly exclude from the core dump,
                        overrides the coredump filter bits */
    MADV_DODUMP = 17, /* Clear the MADV_DONTDUMP flag */

    MADV_WIPEONFORK = 18, /* Zero memory on fork, child only */
    MADV_KEEPONFORK = 19, /* Undo MADV_WIPEONFORK */

    MADV_COLD = 20,    /* deactivate these pages */
    MADV_PAGEOUT = 21, /* reclaim these pages */

    MADV_POPULATE_READ = 22,  /* populate (prefault) page tables readable */
    MADV_POPULATE_WRITE = 23, /* populate (prefault) page tables writable */

    MADV_DONTNEED_LOCKED = 24, /* like DONTNEED, but drop locked pages too */
}

#[cfg(ktest)]
mod test {
    use ostd::prelude::*;

    use super::*;
    use crate::{
        thread::exception::PageFaultInfo,
        vm::{
            page_fault_handler::PageFaultHandler,
            perms::VmPerms,
            vmar::{Vmar, set_huge_mapping_enabled, set_huge_mapping_preserve_on_dontneed},
        },
    };

    const HUGE_PAGE_SIZE: usize = page_size::<PagingConsts>(2);

    fn count_huge_pages(vmar: &Vmar<Full>, start: Vaddr, end: Vaddr) -> usize {
        let mut n_hugepages = 0;
        let vm_space = vmar.vm_space();
        let preempt_guard = disable_preempt();
        let range = start..end;
        let Ok(mut cursor) = vm_space.cursor_mut(&preempt_guard, &range) else {
            return 0;
        };

        cursor
            .do_for_each_submapping(start, end, |range, _, _| {
                let size = range.end - range.start;
                if size > page_size::<PagingConsts>(1) {
                    n_hugepages += 1;
                }

                Ok(())
            })
            .unwrap();
        n_hugepages
    }

    fn map_huge_page(vmar: &Vmar<Full>) -> Vaddr {
        let opts = vmar
            .new_map(HUGE_PAGE_SIZE, VmPerms::READ | VmPerms::WRITE)
            .unwrap();
        let vaddr = opts.align(HUGE_PAGE_SIZE).build().unwrap();
        let info_ = PageFaultInfo {
            address: vaddr,
            required_perms: VmPerms::READ | VmPerms::WRITE,
        };

        // Trigger a "page fault" to force the actual page to be mapped in
        vmar.handle_page_fault(&info_).unwrap();
        vaddr
    }

    #[ktest]
    fn huge_mappings_are_split() {
        set_huge_mapping_enabled(true);
        crate::vm::vmar::init();
        let vmar = Vmar::<Full>::new_root();

        let start = map_huge_page(&vmar);
        let end = start + HUGE_PAGE_SIZE;
        assert_eq!(count_huge_pages(&vmar, start, end), 1);
        let _ = madv_free(&vmar, start, end);
        assert_eq!(count_huge_pages(&vmar, start, end), 0);
    }

    #[ktest]
    fn huge_mappings_are_not_split() {
        set_huge_mapping_enabled(true);
        set_huge_mapping_preserve_on_dontneed(true);
        crate::vm::vmar::init();
        let vmar = Vmar::<Full>::new_root();

        let start = map_huge_page(&vmar);
        let end = start + HUGE_PAGE_SIZE;
        assert_eq!(count_huge_pages(&vmar, start, end), 1);
        let _ = madv_free(&vmar, start, end);
        assert_eq!(count_huge_pages(&vmar, start, end), 1);
    }
}
