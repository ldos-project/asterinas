// SPDX-License-Identifier: MPL-2.0

#![expect(dead_code)]

use core::{
    iter,
    ops::{DerefMut, Range},
    sync::atomic::{AtomicU8, Ordering},
};

use align_ext::AlignExt;
use aster_rights::Full;
use lru::LruCache;
use ostd::{
    impl_untyped_frame_meta_for,
    mm::{Frame, FrameAllocOptions, UFrame, VmIo},
    orpc::{
        oqueue::{Consumer, OQueueRef, reply::ReplyQueue},
        orpc_impl, orpc_server,
    },
};

use crate::{
    fs::server_traits::{
        self, AsyncReadRequest, AsyncWriteRequest, PageHandle, PageIOObservable, PageStore,
    },
    prelude::*,
    vm::vmo::{Pager, Vmo, VmoFlags, VmoOptions, get_page_idx_range},
};

pub struct PageCache {
    pages: Vmo<Full>,
    manager: Arc<PageCacheManager>,
}

const USE_BUILTIN_POLICY: bool = true;

impl PageCache {
    /// Creates an empty size page cache associated with a new backend.
    pub fn new(backend: Weak<dyn PageStore>) -> Result<Self> {
        let manager = PageCacheManager::spawn(backend, USE_BUILTIN_POLICY);
        let pages = VmoOptions::<Full>::new(0)
            .flags(VmoFlags::RESIZABLE)
            .pager(manager.clone())
            .alloc()?;
        Ok(Self { pages, manager })
    }

    /// Creates a page cache associated with an existing backend.
    ///
    /// The `capacity` is the initial cache size required by the backend.
    /// This size usually corresponds to the size of the backend.
    pub fn with_capacity(capacity: usize, backend: Weak<dyn PageStore>) -> Result<Self> {
        let manager = PageCacheManager::spawn(backend, USE_BUILTIN_POLICY);
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
        self.manager.writeback_range(range)
    }

    /// Evict the data within a specified range from the page cache without persisting
    /// them to the backend.
    pub fn discard_range(&self, range: Range<usize>) {
        self.manager.discard_range(range)
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
struct BuiltinPrefetchPolicy {
    /// Current readahead window.
    ra_window: Option<ReadaheadWindow>,
    /// Maximum window size.
    max_size: usize,
    /// The last page visited, used to determine sequential I/O.
    prev_page: Option<usize>,
}

impl BuiltinPrefetchPolicy {
    const INIT_WINDOW_SIZE: usize = 4;
    const DEFAULT_MAX_SIZE: usize = 32;

    pub fn new() -> Self {
        Self {
            ra_window: None,
            max_size: Self::DEFAULT_MAX_SIZE,
            prev_page: None,
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

    /// Determines whether a new readahead should be performed. We only consider readahead for
    /// sequential I/O now. There should be at most one in-progress readahead.
    pub fn should_readahead(&self, idx: usize, max_page: usize) -> bool {
        if self.is_sequential(idx) {
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

    /// Setup the new readahead window.
    pub fn setup_window(&mut self, idx: usize, max_page: usize) {
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

    /// Sets the last page visited.
    pub fn set_prev_page(&mut self, idx: usize) {
        self.prev_page = Some(idx);
    }
}

/// Manager for outstanding read requests. This is used for managing prefetch requests.
#[derive(Default)]
struct OutstandingRequests {
    /// Outstanding requests in the form of the consumers that receive the reply. Each one is
    /// expected to receive exactly one reply.
    outstanding: Vec<Box<dyn Consumer<PageHandle>>>,
}

impl Debug for OutstandingRequests {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OutstandingRequests")
            .field("n_outstanding", &self.outstanding.len())
            .finish()
    }
}

impl OutstandingRequests {
    /// Waits for outstanding requests and updates `pages` based on result of the readahead.
    pub fn wait_for_requests(&mut self, pages: &mut LruCache<usize, CachePage>) {
        for consumer in self.outstanding.drain(..) {
            let PageHandle { idx, frame } = consumer.consume();
            Self::store_uptodate(pages, idx, frame);
        }
    }

    /// Check for completed requests, but do not wait.
    pub fn check_requests(&mut self, pages: &mut LruCache<usize, CachePage>) {
        self.outstanding
            .retain_mut(|c| !Self::check_single_request(pages, c));
    }

    /// Handle any response for a request and return true iff it has been fully processed.
    fn check_single_request(
        pages: &mut LruCache<usize, Frame<CachePageMeta>>,
        c: &mut Box<dyn Consumer<PageHandle>>,
    ) -> bool {
        if let Some(PageHandle { idx, frame }) = c.try_consume() {
            Self::store_uptodate(pages, idx, frame);
            true
        } else {
            false
        }
    }

    /// Set the page to up-to-date. This exists as a function to include some assertions.
    fn store_uptodate(
        pages: &mut LruCache<usize, Frame<CachePageMeta>>,
        idx: usize,
        frame: Frame<CachePageMeta>,
    ) {
        frame.store_state(PageState::UpToDate);
        // These two pages should be the same. Assert that they are as a sanity check.
        #[cfg(debug_assertions)]
        if let Some(page) = pages.get_mut(&idx) {
            assert_eq!(frame.start_paddr(), page.start_paddr());
            assert_eq!(page.load_state(), PageState::UpToDate);
        }
    }

    /// True iff there are outstanding requests.
    pub fn has_requests(&self) -> bool {
        !self.outstanding.is_empty()
    }

    /// Submit an async read request.
    pub fn request_async(
        &mut self,
        pages: &mut LruCache<usize, CachePage>,
        backend: &Arc<dyn PageStore>,
        async_idx: usize,
    ) -> Result<()> {
        let async_page = CachePage::alloc_uninit()?;
        pages.put(async_idx, async_page.clone());
        let (reply_producer, mut reply_consumer) = ReplyQueue::new_pair()?;
        backend.read_page_async(AsyncReadRequest {
            handle: PageHandle {
                idx: async_idx,
                frame: async_page,
            },
            reply_handle: reply_producer,
        })?;

        if !Self::check_single_request(pages, &mut reply_consumer) {
            self.outstanding.push(reply_consumer);
        }

        Ok(())
    }
}

/// The page cache including both the cached pages and the readahead state and a reference to the
/// backend to perform the actual loads. This references the backend weakly.
#[orpc_server(
    crate::vm::vmo::Pager,
    server_traits::PageCache,
    server_traits::PageIOObservable
)]
struct PageCacheManager {
    backend: Weak<dyn PageStore>,
    inner: Mutex<PageCacheManagerInner>,
}

/// The synchronized state of [`PageCacheManager`]. This holds the state and behavior that is
/// protected by the mutex.
struct PageCacheManagerInner {
    // XXX: The cache never actually uses the "LRU-ness" to evict pages.
    pages: LruCache<usize, CachePage>,
    builtin_prefetch_policy: Option<BuiltinPrefetchPolicy>,
    outstanding_requests: OutstandingRequests,
}

impl PageCacheManagerInner {
    /// Conducts the new readahead. Sends the relevant read request and sets the relevant page in
    /// the page cache to `Uninit`.
    pub fn conduct_readahead(&mut self, backend: &Arc<dyn PageStore>) -> Result<()> {
        let Some(policy) = &mut self.builtin_prefetch_policy else {
            return Err(Error::unreachable());
        };
        let Some(window) = &policy.ra_window else {
            return_errno!(Errno::EINVAL)
        };
        for async_idx in window.readahead_range() {
            println!("Prefetching: {async_idx}");
            self.outstanding_requests
                .request_async(&mut self.pages, &backend, async_idx)?;
        }
        Ok(())
    }

    /// Issue any prefetches that the built-in prefetcher requests. This is a no-op if the built-in
    /// prefetcher is disabled.
    pub fn maybe_builtin_prefetch(
        &mut self,
        idx: usize,
        backend: &Arc<dyn PageStore>,
    ) -> Result<()> {
        if let Some(policy) = &mut self.builtin_prefetch_policy {
            // Read ahead if there are no outstanding requests and the policy has determined it
            // should read ahead.
            if !self.outstanding_requests.has_requests()
                && policy.should_readahead(idx, backend.npages()?)
            {
                policy.setup_window(idx, backend.npages()?);
                self.conduct_readahead(backend)?;
            }
            // Need to reborrow because of call to conduct_readahead.
            self.builtin_prefetch_policy
                .as_mut()
                .unwrap()
                .set_prev_page(idx);
        }
        Ok(())
    }
}

impl PageCacheManager {
    pub fn spawn(backend: Weak<dyn PageStore>, use_builtin_policy: bool) -> Arc<Self> {
        Self::new_with(|orpc_internal, _| Self {
            backend,
            inner: Mutex::new(PageCacheManagerInner {
                // Using a bounded LRU cache would cause data loss because automatic evictions are not caught and written back.
                pages: LruCache::unbounded(),
                builtin_prefetch_policy: if use_builtin_policy {
                    Some(BuiltinPrefetchPolicy::new())
                } else {
                    None
                },
                outstanding_requests: Default::default(),
            }),
            orpc_internal,
        })
    }

    pub fn backend(&self) -> Result<Arc<dyn PageStore>> {
        // TODO: This assumes the backend is still available which is not locally guaranteed. It is
        // only true because PageCacheManagers never outlive the backend used to create them. For
        // example, see kernel/src/fs/ext2/block_group.rs:BlockGroup.
        self.backend.upgrade().ok_or_else(Error::unknown)
    }

    /// Discard pages without writing them back to disk.
    pub fn discard_range(&self, range: Range<usize>) {
        let page_idx_range = get_page_idx_range(&range);
        let mut inner = self.inner.lock();
        let pages = &mut inner.pages;
        for idx in page_idx_range {
            pages.pop(&idx);
        }
    }

    /// Write a range of pages to the backend synchronously. They remain in the cache.
    pub fn writeback_range(&self, range: Range<usize>) -> Result<()> {
        let page_idx_range = get_page_idx_range(&range);

        let mut consumers = Vec::with_capacity(range.len());
        // TODO(arthurp): This locks the entire cache. That's probably a performance problem.
        let mut inner = self.inner.lock();
        let pages = &mut inner.pages;
        let backend = self.backend()?;
        let backend_npages = backend.npages()?;
        for idx in page_idx_range.start..page_idx_range.end {
            if let Some(page) = pages.peek(&idx) {
                if page.load_state() == PageState::Dirty && idx < backend_npages {
                    let (reply_handle, reply_consumer) = ReplyQueue::new_pair()?;
                    backend.write_page_async(AsyncWriteRequest {
                        handle: PageHandle {
                            idx,
                            frame: page.clone(),
                        },
                        reply_handle: Some(reply_handle),
                    })?;
                    consumers.push(reply_consumer);
                }
            }
        }

        for consumer in consumers {
            let PageHandle { idx: _, frame } = consumer.consume();
            frame.store_state(PageState::UpToDate);
        }

        Ok(())
    }

    /// Load a page. The page may be loaded from the page cache or read synchronously as part of
    /// this call. If the built-in prefetch policy is enabled, this will trigger readaheads as
    /// needed.
    fn read_page(&self, idx: usize) -> Result<UFrame> {
        let mut inner = self.inner.lock();
        let inner = inner.deref_mut();
        let backend = self.backend()?;

        // Handle any requests that have already completed.
        inner.outstanding_requests.check_requests(&mut inner.pages);

        // There are three possible conditions that could be encountered upon reaching here:
        // 1. The requested page is ready for read in page cache.
        // 2. The requested page is currently being read (generally due to a prefetch).
        // 3. The requested page is on disk, need a sync read operation here.
        let frame = if let Some(page) = inner.pages.get(&idx) {
            println!("Cache hit: {idx}");
            // Cond 1 & 2.
            if let PageState::Uninit = page.load_state() {
                // Cond 2: We should wait for the previous readahead.
                // If there is no previous readahead, an error must have occurred somewhere.
                assert!(inner.outstanding_requests.has_requests());
                inner
                    .outstanding_requests
                    .wait_for_requests(&mut inner.pages);
                inner
                    .pages
                    .get(&idx)
                    .ok_or_else(Error::unreachable)?
                    .clone()
            } else {
                // Cond 1.
                page.clone()
            }
        } else {
            // Cond 3.
            // Conducts the sync read operation.
            let page = if idx < backend.npages()? {
                let page = CachePage::alloc_uninit()?;
                backend.read_page(PageHandle {
                    idx,
                    frame: page.clone(),
                })?;
                page.store_state(PageState::UpToDate);
                page
            } else {
                CachePage::alloc_zero(PageState::Uninit)?
            };
            let frame = page.clone();
            inner.pages.put(idx, page);
            frame
        };

        // Invoke built-in policy.
        inner.maybe_builtin_prefetch(idx, &backend)?;

        self.page_reads_oqueue().produce(PageHandle {
            idx,
            frame: frame.clone(),
        })?;

        Ok(frame.into())
    }
}

impl Debug for PageCacheManager {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.debug_struct("PageCacheManager")
            .field("pages", &self.inner.lock().pages)
            .field(
                "outstanding_requests",
                &self.inner.lock().outstanding_requests,
            )
            .finish()
    }
}

#[orpc_impl]
impl PageIOObservable for PageCacheManager {
    fn page_reads_oqueue(&self) -> OQueueRef<PageHandle>;
    fn page_writes_oqueue(&self) -> OQueueRef<PageHandle>;
}

// XXX: How is Pager handled in ORPC? Do I also need to refactor that?
#[orpc_impl]
impl Pager for PageCacheManager {
    fn commit_page(&self, idx: usize) -> Result<UFrame> {
        self.read_page(idx)
    }

    fn update_page(&self, idx: usize) -> Result<()> {
        let pages = &mut self.inner.lock().pages;
        if let Some(page) = pages.get_mut(&idx) {
            self.page_writes_oqueue().produce(PageHandle {
                idx,
                frame: page.clone(),
            })?;
            page.store_state(PageState::Dirty);
        } else {
            warn!("The page {} is not in page cache", idx);
        }

        Ok(())
    }

    fn decommit_page(&self, idx: usize) -> Result<()> {
        let page_result = self.inner.lock().pages.pop(&idx);
        if let Some(page) = page_result {
            if let PageState::Dirty = page.load_state() {
                let Some(backend) = self.backend.upgrade() else {
                    return Ok(());
                };
                if idx < backend.npages()? {
                    backend.write_page(PageHandle { idx, frame: page })?;
                }
            }
        }

        Ok(())
    }

    fn commit_overwrite(&self, idx: usize) -> Result<UFrame> {
        if let Some(page) = self.inner.lock().pages.get(&idx) {
            return Ok(page.clone().into());
        }

        let page = CachePage::alloc_uninit()?;
        Ok(self
            .inner
            .lock()
            .pages
            .get_or_insert(idx, || page)
            .clone()
            .into())
    }
}

#[orpc_impl]
impl server_traits::PageCache for PageCacheManager {
    fn prefetch(&self, idx: usize) -> Result<()> {
        let mut inner = self.inner.lock();
        let inner = inner.deref_mut();
        inner
            .outstanding_requests
            .request_async(&mut inner.pages, &self.backend()?, idx)?;
        Ok(())
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
    fn store_state(&self, new_state: PageState) {
        self.metadata().state.store(new_state, Ordering::Relaxed);
    }
}

impl CachePageExt for CachePage {
    fn metadata(&self) -> &CachePageMeta {
        self.meta()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// A page state with atomic operations.
#[derive(Debug)]
pub struct AtomicPageState {
    state: AtomicU8,
}

impl AtomicPageState {
    pub fn new(state: PageState) -> Self {
        Self {
            state: AtomicU8::new(state as _),
        }
    }

    pub fn load(&self, order: Ordering) -> PageState {
        let val = self.state.load(order);
        match val {
            0 => PageState::Uninit,
            1 => PageState::UpToDate,
            2 => PageState::Dirty,
            _ => unreachable!(),
        }
    }

    pub fn store(&self, val: PageState, order: Ordering) {
        self.state.store(val as u8, order);
    }
}
