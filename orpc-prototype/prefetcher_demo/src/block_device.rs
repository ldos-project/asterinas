//! Traits for block devices.

use std::usize;

use orpc::{
    oqueue::{
        OQueueAttachError, OQueueRef, Sender,
        locking::{LockingQueue, ObservableLockingQueue},
    },
    orpc_impl::errors::RPCError,
    orpc_trait,
};
use snafu::Snafu;

pub const BLOCK_SIZE: usize = 4096;

/// The block data returned from a read.
#[derive(Clone)]
pub(crate) struct Block {
    /// The block number.
    pub block_id: usize,
    /// The block data.
    pub data: Box<[u8]>,
}

/// A read request
pub(crate) struct ReadRequest {
    /// The block number.
    pub block_id: usize,
    /// A handle to the OQueue to send the result of the read to.
    pub reply_to: Box<dyn Sender<Block>>,
}

/// The trait for block devices which support reading blocks of data both synchronously and asynchronously.
#[orpc_trait]
pub(crate) trait BlockDevice {
    /// Read a block.
    fn read(&self, block_id: usize) -> Result<Block, IOError>;

    /// An asynchronous read request. ReadRequest includes a handle to allow replying to the message.
    fn read_request_oqueue(&self) -> OQueueRef<ReadRequest> {
        LockingQueue::new(16)
    }

    /// An OQueue which is sent every block_id as it is read.
    ///
    /// NOTE: In the future, this will be automatically created in at least some cases.
    fn read_request_observation_oqueue(&self) -> OQueueRef<usize> {
        ObservableLockingQueue::new(16, 4)
    }
}

/// The trait for block caches (generally on top of some other [`BlockDevice`]) which support the block interface for
/// reading, but also interfaces for manipulating the cache.
#[orpc_trait]
pub(crate) trait BlockCache: BlockDevice {
    fn prefetch_request_oqueue(&self) -> OQueueRef<usize> {
        LockingQueue::new(16)
    }
}

/// An error that can be returned from block device operations.
#[derive(Snafu, Debug)]
pub(crate) enum IOError {
    /// An RPC failure.
    #[snafu(transparent)]
    RPCError { source: RPCError },
    /// An OQueue handling failure.
    #[snafu(transparent)]
    OQueueAttachError { source: OQueueAttachError },
    /// An error in the IO operation.
    #[snafu(display("{message}"))]
    IOFailure { message: String },
}

#[orpc_trait]
pub(crate) trait EvictionPolicy {
    fn select_victim(&self) -> Result<usize, IOError>;
    fn inserted(&self, block_id: usize) -> Result<(), RPCError>;
}