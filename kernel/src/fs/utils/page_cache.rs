// SPDX-License-Identifier: MPL-2.0

#![expect(dead_code)]

// TODO: This file contains an entire, hardcoded, kind of bad, prefetcher ("readahead" as they call it). This should be
// entirely removed once we have a reliable prefetcher of our own. Currently it is left in place, mostly in hopes of
// providing a fall back in case of new bugs.

use core::{
    cell::OnceCell,
    iter,
    ops::Range,
    sync::atomic::{AtomicU8, Ordering},
};

use align_ext::AlignExt;
use aster_block::bio::{BioStatus, BioWaiter};
use aster_rights::Full;
use atomic_integer_wrapper::define_atomic_version_of_integer_like_type;
use lru::LruCache;
use ostd::{
    impl_untyped_frame_meta_for,
    mm::{Frame, FrameAllocOptions, UFrame, VmIo},
    path,
    sync::WaitQueue,
    tables::{
        locking::ObservableLockingTable, registry::get_global_table_registry,
        spsc::SpscTableCustom, Producer, Table,
    },
};

use crate::{
    fs::utils::page_prefetch_policy::{
        AccessType, PageAccessEvent, PageCacheRegistrationCommand, PrefetchCommand,
    },
    prelude::*,
    process::posix_thread::PosixThread,
    sched::{RealTimePolicy, SchedPolicy},
    thread::kernel_thread::ThreadOptions,
    time::clocks::{BootTimeClock, MonotonicClock},
    vm::vmo::{get_page_idx_range, Pager, Vmo, VmoFlags, VmoOptions},
};

/// A cache of pages used by file-systems. In most cases, a separate `PageCache` is created for each file.
pub struct PageCache {
    pages: Vmo<Full>,
    manager: Arc<PageCacheManager>,
}

impl PageCache {
    /// Creates an empty size page cache associated with a new backend.
    pub fn new(backend: Weak<dyn PageCacheBackend>, prefetch: bool) -> Result<Self> {
        Self::with_capacity(0, backend, prefetch)
    }

    /// Creates a page cache associated with an existing backend.
    ///
    /// The `capacity` is the initial cache size required by the backend.
    /// This size usually corresponds to the size of the backend.
    pub fn with_capacity(
        capacity: usize,
        backend: Weak<dyn PageCacheBackend>,
        prefetch: bool,
    ) -> Result<Self> {
        let manager = PageCacheManager::new(
            backend,
            if prefetch {
                PrefetcherMode::Builtin
            } else {
                PrefetcherMode::None
            },
        );
        let pages = VmoOptions::<Full>::new(capacity)
            .flags(VmoFlags::RESIZABLE)
            .pager(manager.clone())
            .alloc()?;
        Ok(Self { pages, manager })
    }

    /// Returns the Vmo object.
    // TODO: The capability is too high，restrict it to eliminate the possibility of misuse.
    //       For example, the `resize` api should be forbidden.
    pub fn pages(&self) -> &Vmo<Full> {
        &self.pages
    }

    /// Evict the data within a specified range from the page cache and persist
    /// them to the backend.
    pub fn evict_range(&self, range: Range<usize>) -> Result<()> {
        self.manager.evict_range(range)
    }

    /// Evict the data within a specified range from the page cache without persisting
    /// them to the backend.
    pub fn discard_range(&self, range: Range<usize>) {
        self.manager.discard_range(range)
    }

    /// Returns the backend.
    pub fn backend(&self) -> Arc<dyn PageCacheBackend> {
        self.manager.backend()
    }

    /// Resizes the current page cache to a target size.
    pub fn resize(&self, new_size: usize) -> Result<()> {
        // If the new size is smaller and not page-aligned,
        // first zero the gap between the new size and the
        // next page boundary (or the old size), if such a gap exists.
        let old_size = self.pages.size();
        if old_size > new_size && new_size % PAGE_SIZE != 0 {
            let gap_size = old_size.min(new_size.align_up(PAGE_SIZE)) - new_size;
            if gap_size > 0 {
                self.fill_zeros(new_size..new_size + gap_size)?;
            }
        }
        self.pages.resize(new_size)
    }

    /// Fill the specified range with zeros in the page cache.
    pub fn fill_zeros(&self, range: Range<usize>) -> Result<()> {
        if range.is_empty() {
            return Ok(());
        }
        let (start, end) = (range.start, range.end);

        // Write zeros to the first partial page if any
        let first_page_end = start.align_up(PAGE_SIZE);
        if first_page_end > start {
            let zero_len = first_page_end.min(end) - start;
            self.pages()
                .write_vals(start, iter::repeat_n(&0, zero_len), 0)?;
        }

        // Write zeros to the last partial page if any
        let last_page_start = end.align_down(PAGE_SIZE);
        if last_page_start < end && last_page_start >= start {
            let zero_len = end - last_page_start;
            self.pages()
                .write_vals(last_page_start, iter::repeat_n(&0, zero_len), 0)?;
        }

        for offset in (first_page_end..last_page_start).step_by(PAGE_SIZE) {
            self.pages()
                .write_vals(offset, iter::repeat_n(&0, PAGE_SIZE), 0)?;
        }
        Ok(())
    }
}

impl Drop for PageCache {
    fn drop(&mut self) {
        // TODO:
        // The default destruction procedure exhibits slow performance.
        // In contrast, resizing the `VMO` to zero greatly accelerates the process.
        // We need to find out the underlying cause of this discrepancy.
        let _ = self.pages.resize(0);
    }
}

impl Debug for PageCache {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.debug_struct("PageCache")
            .field("size", &self.pages.size())
            .field("mamager", &self.manager)
            .finish()
    }
}

struct ReadaheadWindow {
    /// The window.
    window: Range<usize>,
    /// Look ahead position in the current window, where the readahead is triggered.
    /// TODO: We set the `lookahead_index` to the start of the window for now.
    /// This should be adjustable by the user.
    lookahead_index: usize,
}

impl ReadaheadWindow {
    pub fn new(window: Range<usize>) -> Self {
        let lookahead_index = window.start;
        Self {
            window,
            lookahead_index,
        }
    }

    /// Gets the next readahead window.
    /// Most of the time, we push the window forward and double its size.
    ///
    /// The `max_size` is the maximum size of the window.
    /// The `max_page` is the total page number of the file, and the window should not
    /// exceed the scope of the file.
    pub fn next(&self, max_size: usize, max_page: usize) -> Self {
        let new_start = self.window.end;
        let cur_size = self.window.end - self.window.start;
        let new_size = (cur_size * 2).min(max_size).min(max_page - new_start);
        Self {
            window: new_start..(new_start + new_size),
            lookahead_index: new_start,
        }
    }

    pub fn lookahead_index(&self) -> usize {
        self.lookahead_index
    }

    pub fn readahead_index(&self) -> usize {
        self.window.end
    }

    pub fn readahead_range(&self) -> Range<usize> {
        self.window.clone()
    }
}

/// A management object which tracks the state of the prefetcher, including asynchronous read handles
/// ([`waiter`](`ReadaheadState::waiter`)).
///
/// This implements a simple policy where pages are prefetched if there are sequential reads. The number of pages to
/// prefetch increases as more sequential reads happen.
struct ReadaheadState {
    /// Current readahead window.
    ra_window: Option<ReadaheadWindow>,
    /// The outstanding requests which are not related to the window.
    outstanding_requests: Vec<usize>,
    /// Maximum window size.
    max_size: usize,
    /// The last page visited, used to determine sequential I/O.
    prev_page: Option<usize>,
    /// Readahead requests waiter.
    waiter: BioWaiter,
    /// The kind of prefetcher we are
    mode: PrefetcherMode,
}

impl ReadaheadState {
    const INIT_WINDOW_SIZE: usize = 4;
    const DEFAULT_MAX_SIZE: usize = 32;

    pub fn new(mode: PrefetcherMode) -> Self {
        Self {
            ra_window: None,
            outstanding_requests: Vec::new(),
            max_size: Self::DEFAULT_MAX_SIZE,
            prev_page: None,
            waiter: BioWaiter::new(),
            mode,
        }
    }

    /// Sets the maximum readahead window size.
    pub fn set_max_window_size(&mut self, size: usize) {
        self.max_size = size;
    }

    fn is_sequential(&self, idx: usize) -> bool {
        if let Some(prev) = self.prev_page {
            idx == prev || idx == prev + 1
        } else {
            false
        }
    }

    /// The number of bio requests in waiter.
    /// This number will be zero if there are no previous readahead.
    pub fn n_ongoing_requests(&self) -> usize {
        self.waiter.nreqs()
    }

    /// Checks for the previous readahead.
    /// Returns true if the previous readahead has been completed.
    pub fn prev_readahead_is_completed(&self) -> bool {
        let nreqs = self.n_ongoing_requests();
        if nreqs == 0 {
            return false;
        }

        for i in 0..nreqs {
            if self.waiter.status(i) == BioStatus::Submit {
                return false;
            }
        }
        true
    }

    fn get_outstanding_requests(&self) -> Vec<usize> {
        let mut res: Vec<usize> = self
            .ra_window
            .as_ref()
            .map(|window| window.readahead_range().collect())
            .unwrap_or_default();
        res.extend(self.outstanding_requests.iter());
        res
    }

    /// Waits for the previous readahead.
    pub fn wait_for_prev_readahead(
        &mut self,
        pages: &mut MutexGuard<LruCache<usize, CachePage>>,
    ) -> Result<()> {
        if matches!(self.waiter.wait(), Some(BioStatus::Complete)) {
            for idx in self.get_outstanding_requests() {
                if let Some(page) = pages.get_mut(&idx) {
                    page.store_state(PageState::UpToDate);
                }
            }
            self.outstanding_requests.clear();
            self.waiter.clear();
        } else {
            return_errno!(Errno::EIO)
        }

        Ok(())
    }

    pub fn maybe_readahead(
        &mut self,
        idx: usize,
        pages: &mut MutexGuard<LruCache<usize, CachePage>>,
        backend: Arc<dyn PageCacheBackend>,
    ) -> Result<()> {
        if self.should_readahead(idx, backend.npages()) {
            self.setup_window(idx, backend.npages());
            self.conduct_readahead(pages, backend)?;
        };
        Ok(())
    }

    /// Determines whether a new readahead should be performed.
    /// We only consider readahead for sequential I/O now.
    /// There should be at most one in-progress readahead.
    pub fn should_readahead(&self, idx: usize, max_page: usize) -> bool {
        if self.mode == PrefetcherMode::Builtin
            && self.n_ongoing_requests() == 0
            && self.is_sequential(idx)
        {
            if let Some(cur_window) = &self.ra_window {
                let trigger_readahead =
                    idx == cur_window.lookahead_index() || idx == cur_window.readahead_index();
                let next_window_exist = cur_window.readahead_range().end < max_page;
                trigger_readahead && next_window_exist
            } else {
                let new_window_start = idx + 1;
                new_window_start < max_page
            }
        } else {
            false
        }
    }

    /// Setup the new readahead window. This expands the readahead window of creates a new one with size
    /// [`Self::INIT_WINDOW_SIZE`].
    pub fn setup_window(&mut self, idx: usize, max_page: usize) {
        if self.mode == PrefetcherMode::Builtin {
            let new_window = if let Some(cur_window) = &self.ra_window {
                cur_window.next(self.max_size, max_page)
            } else {
                let start_idx = idx + 1;
                let init_size = Self::INIT_WINDOW_SIZE.min(self.max_size);
                let end_idx = (start_idx + init_size).min(max_page);
                ReadaheadWindow::new(start_idx..end_idx)
            };
            self.ra_window = Some(new_window);
        }
    }

    /// Conducts the new readahead.
    /// Sends the relevant read request and sets the relevant page in the page cache to `Uninit`.
    pub fn conduct_readahead(
        &mut self,
        pages: &mut MutexGuard<LruCache<usize, CachePage>>,
        backend: Arc<dyn PageCacheBackend>,
    ) -> Result<()> {
        if self.mode == PrefetcherMode::Builtin {
            let Some(window) = &self.ra_window else {
                return_errno!(Errno::EINVAL)
            };
            for async_idx in window.readahead_range() {
                self.readahead_page(pages, &backend, async_idx)?;
            }
        }
        Ok(())
    }

    pub fn force_readahead_page(
        &mut self,
        pages: &mut MutexGuard<LruCache<usize, CachePage>>,
        backend: &Arc<dyn PageCacheBackend>,
        idx: usize,
    ) -> Result<()> {
        self.readahead_page(pages, backend, idx)?;
        self.outstanding_requests.push(idx);
        Ok(())
    }

    /// Readahead (prefetch) a page. This will cause the page to be asynchronously be loaded. Future loads will use the
    /// results if it is available, or wait for this load to complete to use it.
    fn readahead_page(
        &mut self,
        pages: &mut MutexGuard<LruCache<usize, CachePage>>,
        backend: &Arc<dyn PageCacheBackend>,
        idx: usize,
    ) -> Result<()> {
        let mut async_page = CachePage::alloc_uninit()?;
        let pg_waiter = backend.read_page_async(idx, &async_page)?;
        if pg_waiter.nreqs() > 0 {
            self.waiter.concat(pg_waiter);
        } else {
            // Some backends (e.g. RamFS) do not issue requests, but fill the page directly.
            async_page.store_state(PageState::UpToDate);
        }
        pages.put(idx, async_page);
        Ok(())
    }

    /// Sets the last page visited.
    pub fn set_prev_page(&mut self, idx: usize) {
        self.prev_page = Some(idx);
    }
}

/// The page cache including both the cached pages and the readahead state and a reference to the backend to perform the
/// actual loads. This references the backend weakly.
///
/// ## Locking Discipline
///
/// The lock ordering is: `pages`, `ra_state`.
struct PageCacheManager {
    pages: Mutex<LruCache<usize, CachePage>>,
    backend: Weak<dyn PageCacheBackend>,
    // TODO: This seems to be locked only when `pages` is also locked. So we could combine them into a struct within a
    // single `Mutex`. This would eliminate the need for the locking discipline listed above.
    ra_state: Mutex<ReadaheadState>,
    access_producer: Mutex<OnceCell<Box<dyn Producer<PageAccessEvent>>>>,
    prefetcher_mode: PrefetcherMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefetcherMode {
    None,
    Builtin,
    Server,
}

static_assertions::assert_impl_all!(PageCacheManager: Sync, Send);

impl PageCacheManager {
    pub fn new(backend: Weak<dyn PageCacheBackend>, prefetcher_mode: PrefetcherMode) -> Arc<Self> {
        let ret = Arc::new(Self {
            pages: Mutex::new(LruCache::unbounded()),
            backend,
            ra_state: Mutex::new(ReadaheadState::new(prefetcher_mode)),
            access_producer: Mutex::new(OnceCell::new()),
            prefetcher_mode,
        });

        if prefetcher_mode == PrefetcherMode::Server {
            error_result!(
                crate::fs::utils::page_prefetch_policy::start_prefetch_policy_subsystem()
            );
            error_result!(
                ret.setup_prefetcher(),
                "prefetcher could not be started; this page cache will not prefetch"
            );
        }

        ret
    }

    fn setup_prefetcher(self: &Arc<PageCacheManager>) -> Result<()> {
        let registry: &'static ostd::tables::registry::TableRegistry = get_global_table_registry();

        // Create a page cache specific prefetching table. This will accept `PrefetchCommand`s which will trigger
        // prefetches in this cache.
        let prefetch_command_table = SpscTableCustom::<PrefetchCommand, WaitQueue>::new(8, 4, 40);
        registry.register(
            path!(pagecache.prefetch.{?}),
            prefetch_command_table.clone(),
        );
        let access_table = ObservableLockingTable::<PageAccessEvent>::new(64, 4);
        registry.register(path!(pagecache.access.{?}), access_table.clone());

        {
            let table = registry
                .lookup::<PageCacheRegistrationCommand>(&path!(pagecache.policy.registration))
                .ok_or_else(|| {
                    Error::with_message(
                        Errno::ENOENT,
                        "could not find prefetcher registration table",
                    )
                })?;
            // Now, temporarily attach to the page cache policy manager and notify it that we exist. It will start sending
            // prefetches to us.
            let producer = table.attach_producer()?;
            producer.put(PageCacheRegistrationCommand {
                prefetch_command_table: prefetch_command_table.clone(),
                access_table: access_table.clone(),
            });
        }

        let Ok(()) = self
            .access_producer
            .lock()
            .set(access_table.attach_producer()?)
        else {
            return Err(Error::with_message(
                Errno::EALREADY,
                "access table producer already set",
            ));
        };

        let prefetch_consumer = prefetch_command_table.attach_consumer()?;
        let weak_this = Arc::downgrade(self);

        ThreadOptions::new(move || loop {
            let Some(this) = weak_this.upgrade() else {
                break;
            };
            let cmd = prefetch_consumer.take();
            info!("Received prefetch {cmd:?}. doing it.");
            error_result!(
                this.force_readahead_page(cmd.page),
                "readahead failed of page {}, continuing",
                cmd.page
            );
        })
        .sched_policy(SchedPolicy::RealTime {
            rt_prio: 2.try_into().unwrap(),
            rt_policy: RealTimePolicy::default(),
        })
        .spawn();

        Ok(())
    }

    pub fn backend(&self) -> Arc<dyn PageCacheBackend> {
        // TODO: This assumes the backend is still available which is not locally guaranteed. It is only true because
        // PageCacheManagers never outlive the backend used to create them. For example, see
        // kernel/src/fs/ext2/block_group.rs BlockGroup. Users of this should probably no-op and log if there is no
        // backend.
        self.backend.upgrade().unwrap()
    }

    /// Discard pages without writing them back to disk.
    pub fn discard_range(&self, range: Range<usize>) {
        let page_idx_range = get_page_idx_range(&range);
        let mut pages = self.pages.lock();
        for idx in page_idx_range {
            pages.pop(&idx);
        }
    }

    pub fn evict_range(&self, range: Range<usize>) -> Result<()> {
        let page_idx_range = get_page_idx_range(&range);

        let mut bio_waiter = BioWaiter::new();
        let mut pages = self.pages.lock();
        let backend = self.backend();
        let backend_n_pages = backend.npages();
        for idx in page_idx_range.start..page_idx_range.end {
            if let Some(page) = pages.peek(&idx) {
                if page.load_state() == PageState::Dirty && idx < backend_n_pages {
                    let waiter = backend.write_page_async(idx, page)?;
                    bio_waiter.concat(waiter);
                }
            }
        }

        if !matches!(bio_waiter.wait(), Some(BioStatus::Complete)) {
            // Do not allow partial failure
            return_errno!(Errno::EIO);
        }

        for (_, page) in pages
            .iter_mut()
            .filter(|(idx, _)| page_idx_range.contains(*idx))
        {
            page.store_state(PageState::UpToDate);
        }
        Ok(())
    }

    /// Load a page performing read ahead if appropriate. The page may be loaded from the page cache or read
    /// synchronously as part of this call.
    fn ondemand_readahead(&self, idx: usize) -> Result<UFrame> {
        {
            let access_producer = self.access_producer.lock();
            if let Some(access_producer) = access_producer.get() {
                let event = PageAccessEvent {
                    timestamp: BootTimeClock::get().read_time(),
                    thread: PosixThread::current().map(|t| t.tid()),
                    page: idx,
                    access_type: AccessType::Read,
                };
                info!("Sending access event: {event:?}");
                if access_producer.try_put(event).is_some() {
                    warn!("couldn't put access event");
                }
            }
        }
        
        let mut pages = self.pages.lock();
        let mut ra_state = self.ra_state.lock();
        let backend = self.backend();
        // Checks for the previous readahead.
        if ra_state.prev_readahead_is_completed() {
            ra_state.wait_for_prev_readahead(&mut pages)?;
        }
        // There are three possible conditions that could be encountered upon reaching here.
        // 1. The requested page is ready for read in page cache.
        // 2. The requested page is in previous readahead range, not ready for now.
        // 3. The requested page is on disk, need a sync read operation here.
        let frame = if let Some(page) = pages.get(&idx) {
            // Cond 1 & 2.
            if let PageState::Uninit = page.load_state() {
                // Cond 2: We should wait for the previous readahead.
                // If there is no previous readahead, an error must have occurred somewhere.
                assert!(ra_state.n_ongoing_requests() != 0);
                ra_state.wait_for_prev_readahead(&mut pages)?;
                pages.get(&idx).unwrap().clone()
            } else {
                // Cond 1.
                info!("Cache hit for {idx}");
                page.clone()
            }
        } else {
            // Cond 3.
            // Conducts the sync read operation.
            let page = if idx < backend.npages() {
                let mut page = CachePage::alloc_uninit()?;
                backend.read_page(idx, &page)?;
                page.store_state(PageState::UpToDate);
                page
            } else {
                CachePage::alloc_zero(PageState::Uninit)?
            };
            let frame = page.clone();
            pages.put(idx, page);
            frame
        };
        ra_state.maybe_readahead(idx, &mut pages, backend)?;

        // Notify prefetcher of read.
        ra_state.set_prev_page(idx);

        Ok(frame.into())
    }

    /// Readahead (prefetch) a page. This will cause the page to be asynchronously be loaded. Future loads will use the
    /// results if it is available, or wait for this load to complete to use it.
    pub fn force_readahead_page(&self, idx: usize) -> Result<()> {
        let mut pages = self.pages.lock();
        let mut ra_state = self.ra_state.lock();
        ra_state.force_readahead_page(&mut pages, &self.backend(), idx)
    }
}

impl Debug for PageCacheManager {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.debug_struct("PageCacheManager")
            .field("pages", &self.pages.lock())
            .finish()
    }
}

impl Pager for PageCacheManager {
    fn commit_page(&self, idx: usize) -> Result<UFrame> {
        self.ondemand_readahead(idx)
    }

    fn update_page(&self, idx: usize) -> Result<()> {
        let mut pages = self.pages.lock();
        if let Some(page) = pages.get_mut(&idx) {
            page.store_state(PageState::Dirty);
        } else {
            warn!("The page {} is not in page cache", idx);
        }

        Ok(())
    }

    fn decommit_page(&self, idx: usize) -> Result<()> {
        let page_result = self.pages.lock().pop(&idx);
        if let Some(page) = page_result {
            if let PageState::Dirty = page.load_state() {
                let Some(backend) = self.backend.upgrade() else {
                    return Ok(());
                };
                if idx < backend.npages() {
                    backend.write_page(idx, &page)?;
                }
            }
        }

        Ok(())
    }

    fn commit_overwrite(&self, idx: usize) -> Result<UFrame> {
        if let Some(page) = self.pages.lock().get(&idx) {
            return Ok(page.clone().into());
        }

        let page = CachePage::alloc_uninit()?;
        Ok(self.pages.lock().get_or_insert(idx, || page).clone().into())
    }
}

/// A page in the page cache.
pub type CachePage = Frame<CachePageMeta>;

/// Metadata for a page in the page cache.
#[derive(Debug)]
pub struct CachePageMeta {
    pub state: AtomicPageState,
    // TODO: Add a reverse mapping from the page to VMO for eviction.
}

impl_untyped_frame_meta_for!(CachePageMeta);

pub trait CachePageExt {
    /// Gets the metadata associated with the cache page.
    fn metadata(&self) -> &CachePageMeta;

    /// Allocates a new cache page which content and state are uninitialized.
    fn alloc_uninit() -> Result<CachePage> {
        let meta = CachePageMeta {
            state: AtomicPageState::new(PageState::Uninit),
        };
        let page = FrameAllocOptions::new()
            .zeroed(false)
            .alloc_frame_with(meta)?;
        Ok(page)
    }

    /// Allocates a new zeroed cache page with the wanted state.
    fn alloc_zero(state: PageState) -> Result<CachePage> {
        let meta = CachePageMeta {
            state: AtomicPageState::new(state),
        };
        let page = FrameAllocOptions::new()
            .zeroed(true)
            .alloc_frame_with(meta)?;
        Ok(page)
    }

    /// Loads the current state of the cache page.
    fn load_state(&self) -> PageState {
        self.metadata().state.load(Ordering::Relaxed)
    }

    /// Stores a new state for the cache page.
    fn store_state(&mut self, new_state: PageState) {
        self.metadata().state.store(new_state, Ordering::Relaxed);
    }
}

impl CachePageExt for CachePage {
    fn metadata(&self) -> &CachePageMeta {
        self.meta()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromInt)]
#[repr(u8)]
pub enum PageState {
    /// `Uninit` indicates a new allocated page which content has not been initialized.
    /// The page is available to write, not available to read.
    Uninit = 0,
    /// `UpToDate` indicates a page which content is consistent with corresponding disk content.
    /// The page is available to read and write.
    UpToDate = 1,
    /// `Dirty` indicates a page which content has been updated and not written back to underlying disk.
    /// The page is available to read and write.
    Dirty = 2,
}

define_atomic_version_of_integer_like_type!(PageState, try_from = true, {
    #[derive(Debug)]
    pub struct AtomicPageState(AtomicU8);
});

impl From<PageState> for u8 {
    fn from(value: PageState) -> Self {
        value as _
    }
}

/// This trait represents the backend for the page cache.
pub trait PageCacheBackend: Sync + Send {
    /// Reads a page from the backend asynchronously.
    fn read_page_async(&self, idx: usize, frame: &CachePage) -> Result<BioWaiter>;
    /// Writes a page to the backend asynchronously.
    fn write_page_async(&self, idx: usize, frame: &CachePage) -> Result<BioWaiter>;
    /// Returns the number of pages in the backend.
    fn npages(&self) -> usize;
}

impl dyn PageCacheBackend {
    /// Reads a page from the backend synchronously.
    fn read_page(&self, idx: usize, frame: &CachePage) -> Result<()> {
        let waiter = self.read_page_async(idx, frame)?;
        match waiter.wait() {
            Some(BioStatus::Complete) => Ok(()),
            _ => return_errno!(Errno::EIO),
        }
    }
    /// Writes a page to the backend synchronously.
    fn write_page(&self, idx: usize, frame: &CachePage) -> Result<()> {
        let waiter = self.write_page_async(idx, frame)?;
        match waiter.wait() {
            Some(BioStatus::Complete) => Ok(()),
            _ => return_errno!(Errno::EIO),
        }
    }
}
