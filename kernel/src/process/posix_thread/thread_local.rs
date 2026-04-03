// SPDX-License-Identifier: MPL-2.0

use core::cell::{Cell, Ref, RefCell, RefMut};

use aster_rights::Full;
use ostd::{mm::Vaddr, sync::RwArc, task::CurrentTask};

use super::RobustListHead;
use crate::{
    error::{Errno, Error},
    fs::file_table::FileTable,
    process::signal::SigStack,
    vm::vmar::Vmar,
};

type Result<T> = core::result::Result<T, Error>;

/// Local data for a POSIX thread.
pub struct ThreadLocal {
    // TID pointers.
    // https://man7.org/linux/man-pages/man2/set_tid_address.2.html
    set_child_tid: Cell<Vaddr>,
    clear_child_tid: Cell<Vaddr>,

    // Virtual memory address regions.
    root_vmar: RefCell<Option<Vmar<Full>>>,
    page_fault_disabled: Cell<bool>,

    // Robust futexes.
    // https://man7.org/linux/man-pages/man2/get_robust_list.2.html
    robust_list: RefCell<Option<RobustListHead>>,

    // Files.
    file_table: RefCell<Option<RwArc<FileTable>>>,

    // Signal.
    /// `ucontext` address for the signal handler.
    // FIXME: This field may be removed. For glibc applications with RESTORER flag set, the
    // `sig_context` is always equals with RSP.
    sig_context: Cell<Option<Vaddr>>,
    /// Stack address, size, and flags for the signal handler.
    sig_stack: RefCell<Option<SigStack>>,
}

impl ThreadLocal {
    pub(super) fn new(
        set_child_tid: Vaddr,
        clear_child_tid: Vaddr,
        root_vmar: Vmar<Full>,
        file_table: RwArc<FileTable>,
    ) -> Self {
        Self {
            set_child_tid: Cell::new(set_child_tid),
            clear_child_tid: Cell::new(clear_child_tid),
            root_vmar: RefCell::new(Some(root_vmar)),
            page_fault_disabled: Cell::new(false),
            robust_list: RefCell::new(None),
            file_table: RefCell::new(Some(file_table)),
            sig_context: Cell::new(None),
            sig_stack: RefCell::new(None),
        }
    }

    pub fn set_child_tid(&self) -> &Cell<Vaddr> {
        &self.set_child_tid
    }

    pub fn clear_child_tid(&self) -> &Cell<Vaddr> {
        &self.clear_child_tid
    }

    pub fn root_vmar(&self) -> &RefCell<Option<Vmar<Full>>> {
        &self.root_vmar
    }

    /// Executes the closure with the page fault handler diasabled.
    ///
    /// When page faults occur, the handler may attempt to load the page from the disk, which can break
    /// the atomic mode. By using this method, the page fault handler will fail immediately, so
    /// fallible memory operation will return [`Errno::EFAULT`] once it triggers a page fault.
    ///
    /// Usually, we should _not_ try to access the userspace memory while being in the atomic mode
    /// (e.g., when holding a spin lock). If we must do so, this method is a last resort that disables
    /// the handler instead.
    ///
    /// Note that the closure runs with different semantics of the fallible memory operation.
    /// Therefore, if it fails with a [`Errno::EFAULT`], this method will return [`None`] and it is
    /// the caller's responsibility to exit the atomic mode, handle the page fault, and retry. Do
    /// _not_ use this method without adding code that explicitly handles the page fault!
    pub fn with_page_fault_disabled<F, T>(&self, func: F) -> Option<Result<T>>
    where
        F: FnOnce() -> Result<T>,
    {
        let is_disabled = self.is_page_fault_disabled();
        self.page_fault_disabled.set(true);

        let result = func();

        self.page_fault_disabled.set(is_disabled);

        if result
            .as_ref()
            .is_err_and(|err| err.error() == Errno::EFAULT)
        {
            None
        } else {
            Some(result)
        }
    }

    pub fn is_page_fault_disabled(&self) -> bool {
        self.page_fault_disabled.get()
    }

    pub fn robust_list(&self) -> &RefCell<Option<RobustListHead>> {
        &self.robust_list
    }

    pub fn borrow_file_table(&self) -> FileTableRef {
        FileTableRef(self.file_table.borrow())
    }

    pub fn borrow_file_table_mut(&self) -> FileTableRefMut {
        FileTableRefMut(self.file_table.borrow_mut())
    }

    pub fn sig_context(&self) -> &Cell<Option<Vaddr>> {
        &self.sig_context
    }

    pub fn sig_stack(&self) -> &RefCell<Option<SigStack>> {
        &self.sig_stack
    }
}

/// An immutable, shared reference to the file table in [`ThreadLocal`].
pub struct FileTableRef<'a>(Ref<'a, Option<RwArc<FileTable>>>);

impl FileTableRef<'_> {
    /// Unwraps and returns a reference to the file table.
    ///
    /// # Panics
    ///
    /// This method will panic if the thread has exited and the file table has been dropped.
    pub fn unwrap(&self) -> &RwArc<FileTable> {
        self.0.as_ref().unwrap()
    }
}

/// A mutable, exclusive reference to the file table in [`ThreadLocal`].
pub struct FileTableRefMut<'a>(RefMut<'a, Option<RwArc<FileTable>>>);

impl FileTableRefMut<'_> {
    /// Unwraps and returns a reference to the file table.
    ///
    /// # Panics
    ///
    /// This method will panic if the thread has exited and the file table has been dropped.
    pub fn unwrap(&mut self) -> &mut RwArc<FileTable> {
        self.0.as_mut().unwrap()
    }

    /// Removes the file table and drops it.
    pub(super) fn remove(&mut self) {
        *self.0 = None;
    }

    /// Replaces the file table with a new one, returning the old one.
    pub fn replace(&mut self, new_table: Option<RwArc<FileTable>>) -> Option<RwArc<FileTable>> {
        core::mem::replace(&mut *self.0, new_table)
    }
}

/// A trait to provide the `as_thread_local` method for tasks.
pub trait AsThreadLocal {
    /// Returns the associated [`ThreadLocal`].
    fn as_thread_local(&self) -> Option<&ThreadLocal>;
}

impl AsThreadLocal for CurrentTask {
    fn as_thread_local(&self) -> Option<&ThreadLocal> {
        self.local_data().downcast_ref()
    }
}
