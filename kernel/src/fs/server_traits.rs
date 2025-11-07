// SPDX-License-Identifier: MPL-2.0

use alloc::boxed::Box;

use ostd::orpc::{
    oqueue::{
        OQueue as _, OQueueRef, Producer, locking::ObservableLockingQueue, reply::ReplyQueue,
    },
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

#[orpc_trait]
pub trait PageIOObservable {
    /// The OQueue containing every read request. This includes both sync and async reads on this
    /// trait and any other read operations on other traits (for instance,
    /// [`crate::vm::vmo::Pager::commit_page`]).
    fn page_reads_oqueue(&self) -> OQueueRef<usize> {
        // TODO: Use lock-free implementation
        ObservableLockingQueue::new(8, 8)
    }

    /// The OQueue containing every write request. This includes both sync and async writes and any
    /// other write operations on other traits
    fn page_writes_oqueue(&self) -> OQueueRef<usize> {
        // TODO: Use lock-free implementation
        ObservableLockingQueue::new(8, 8)
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
pub trait PageStore {
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
        let reply_oqueue = ReplyQueue::new(2);
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
        let reply_oqueue = ReplyQueue::new(2);
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

#[orpc_trait]
pub trait PageCache {
    /// Request that the cache prefetch a page. This is asynchronous and advisory, so the page may
    /// appear in the cache at a later time or never.
    fn prefetch(&self, idx: usize) -> Result<()>;

    // TODO(arthurp): Make this an OQueue. Provide a prefetch handling server which watches all the
    // OQueues and then makes calls to the actual prefetch method. Effectively, this is a thread
    // that donates it's time in place of the sender without requiring an thread per PageCache.
}

#[orpc_trait]
pub trait PageEvictionPolicy {
    /// Get the page index to evict. The policy cannot decide, for whatever reason, this should
    /// return an error. The caller will use some fallback. If the returned index does not refer to
    /// a valid page in the cache, then the caller will also fallback.
    fn select_victim(&self) -> Result<usize>;
}
