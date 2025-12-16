// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, sync::Arc};
use core::marker::Copy;

use ostd::orpc::{
    oqueue::{OQueue as _, OQueueRef, Producer, reply::ReplyQueue},
    orpc_trait,
};

use crate::{Result, fs::utils::CachePage};

/// A reference to a page in a [`PageStore`]. It contains the page index and the frame that holds
/// the page data (if available).
#[derive(Clone)]
pub struct PageHandle {
    /// The ID of the page (as an offset in the store).
    pub idx: usize,
    /// The page to read data from, or put data into.
    pub frame: CachePage,
}

pub struct AsyncReadRequest {
    pub handle: PageHandle,
    /// A producer handle into an OQueue to send the reply to.
    pub reply_handle: Box<dyn Producer<PageHandle>>,
}

pub struct AsyncWriteRequest {
    pub handle: PageHandle,
    /// A producer handle into an OQueue to send the reply to. If this is [`None`] no reply is sent.
    pub reply_handle: Option<Box<dyn Producer<PageHandle>>>,
}

impl From<PageHandle> for AsyncWriteRequest {
    fn from(handle: PageHandle) -> Self {
        Self {
            handle,
            reply_handle: None,
        }
    }
}

// Constructor for a new OQueue for the prefetcher. This is to make testing easier to switch between
// oqueue implementations.
fn new_oqueue<T: Copy + Send + 'static>() -> OQueueRef<T> {
    ostd::orpc::oqueue::locking::ObservableLockingQueue::new(8, 8)
}

// Constructor for a new OQueue for the prefetcher which needs a specific length. This is needed for
// cases where a long queue is required to avoid deadlocks.
fn new_oqueue_with_len<T: Copy + Send + 'static>(len: usize) -> OQueueRef<T> {
    ostd::orpc::oqueue::locking::ObservableLockingQueue::new(len, 8)
}

#[orpc_trait]
pub trait PageIOObservable {
    /// The OQueue containing every read request. This includes both sync and async reads on this
    /// trait and any other read operations on other traits (for instance,
    /// [`crate::vm::vmo::Pager::commit_page`]).
    fn page_reads_oqueue(&self) -> OQueueRef<usize> {
        new_oqueue()
    }

    /// The OQueue containing every reply for read requests.
    fn page_reads_reply_oqueue(&self) -> OQueueRef<usize> {
        // TODO: This must be longer than the largest number of IO that can be outstanding in the
        // system. Otherwise a produce into this OQueue in the interrupt handler will block panicing
        // the kernel.
        new_oqueue_with_len(32)
    }

    /// The OQueue containing every write request. This includes both sync and async writes and any
    /// other write operations on other traits
    fn page_writes_oqueue(&self) -> OQueueRef<usize> {
        new_oqueue()
    }

    /// The OQueue containing every reply for write requests.
    fn page_writes_reply_oqueue(&self) -> OQueueRef<usize> {
        // TODO: as page_reads_reply_oqueue
        new_oqueue_with_len(32)
    }
}

/// A data store full of pages. This can be used to represent either a file or a block device.
///
/// NOTE: Read and write have both sync and async variants. Because of this, the implicit OQueues
/// may not include all actual read operations. Use the explicit OQueues instead. The implementation
/// should make sure to populate those OQueues.
///
/// TODO(arthurp, https://github.com/ldos-project/asterinas/issues/122): The trait takes an object
/// oriented approach to match the existing file system implementation. It would be better to use a
/// single server per filesystem and pass around richer page IDs than an index.
#[orpc_trait]
pub trait PageStore: PageIOObservable {
    // TODO(arthurp, https://github.com/ldos-project/asterinas/issues/121): read_page_async and
    // write_page_async should be OQueues, but doing so would make implementing them in the existing
    // monolithic kernel code is tricky and not worth it at the moment.

    // TODO(arthurp, https://github.com/ldos-project/asterinas/issues/121): Handle errors on async
    // requests. They probably need to reply in the error case.

    /// Reads a page asynchronously. The reply will be sent to [`AsyncReadRequest::reply_handle`].
    fn read_page_async(&self, handle: AsyncReadRequest) -> Result<()>;

    /// Writes a page asynchronously. The reply will be sent to [`AsyncWriteRequest::reply_handle`]
    /// if it is available.
    fn write_page_async(&self, handle: AsyncWriteRequest) -> Result<()>;

    /// Reads a page synchronously.
    fn read_page(&self, handle: PageHandle) -> Result<()> {
        let reply_oqueue = ReplyQueue::new(2, 0);
        let consumer = reply_oqueue.attach_consumer()?;
        self.read_page_async(AsyncReadRequest {
            handle,
            reply_handle: reply_oqueue.attach_producer()?,
        })?;
        consumer.consume();
        Ok(())
    }

    /// Writes a page synchronously.
    fn write_page(&self, handle: PageHandle) -> Result<()> {
        let reply_oqueue = ReplyQueue::new(2, 0);
        let consumer = reply_oqueue.attach_consumer()?;
        self.write_page_async(AsyncWriteRequest {
            handle,
            reply_handle: Some(reply_oqueue.attach_producer()?),
        })?;
        consumer.consume();
        Ok(())
    }

    /// Returns the number of pages in this store.
    fn npages(&self) -> Result<usize>;
}

/// The state of a page in the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    /// The page was in the cache.
    Hit,
    /// The page was not in the cache.
    Miss,
    /// The page was currently being read into the cache.
    Pending,
}

/// Information about a read request on the page cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageCacheReadInfo {
    /// The index of the page.
    pub idx: usize,
    /// The state of the cached page when the request was made.
    pub cache_state: CacheState,
}

#[orpc_trait]
pub trait PageCache {
    /// Request that the cache prefetch a page. This is asynchronous and advisory, so the page may
    /// appear in the cache at a later time or never.
    fn prefetch_oqueue(&self) -> OQueueRef<usize> {
        new_oqueue()
    }

    fn underlying_page_store(&self) -> Result<Arc<dyn PageStore>>;

    /// The OQueue containing every reply for write requests.
    fn page_cache_read_info_oqueue(&self) -> OQueueRef<PageCacheReadInfo> {
        new_oqueue_with_len(32)
    }
}

#[orpc_trait]
pub trait PageEvictionPolicy {
    /// Get the page index to evict. The policy cannot decide, for whatever reason, this should
    /// return an error. The caller will use some fallback. If the returned index does not refer to
    /// a valid page in the cache, then the caller will also fallback.
    fn select_victim(&self) -> Result<usize>;
}
