//! An implementation of table based IPC for benchmarking, testing, and discussion. This *VERY* experimental.
// mod callable_service;
pub(crate) mod generic_benchmarks;
pub(crate) mod generic_test;
pub mod spsc;
pub mod locking;

use alloc::{borrow::ToOwned, boxed::Box, format, string::String};
use core::{
    any::Any, error::Error, fmt::Display, iter::Step, ops::{Add, Sub}
};

/// A reference to a specific row in a table. This refers to an element over the full history of a table, not based on
/// some implementation defined buffer.
///
/// See [`WeakObserver`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cursor(usize);

impl Step for Cursor {
    fn steps_between(start: &Self, end: &Self) -> (usize, Option<usize>) {
        Step::steps_between(&start.0, &end.0)
    }

    fn forward_checked(start: Self, count: usize) -> Option<Self> {
        Step::forward_checked(start.0, count).map(Cursor)
    }

    fn backward_checked(start: Self, count: usize) -> Option<Self> {
        Step::backward_checked(start.0, count).map(Cursor)
    }
}

impl Cursor {
    /// Get the global index of the cursor. I.e., the index of the row the cursor points to in a hypothetical infinite
    /// buffer containing all rows ever added to the table.
    pub fn index(&self) -> usize {
        self.0
    }
}

impl Add<isize> for Cursor {
    type Output = Self;

    fn add(self, rhs: isize) -> Self::Output {
        let Cursor(i) = self;
        Cursor(i.checked_add_signed(rhs).unwrap())
    }
}

impl Sub<isize> for Cursor {
    type Output = Self;
    fn sub(self, rhs: isize) -> Self::Output {
        self + -rhs
    }
}

/// A producer handle to a table. This allows inserting or sending values to the table.
pub trait Producer<T>: Send {
    /// Append/enqueue an element
    fn put(&self, data: T);

    /// Append/enqueue an element if there is space immediately, otherwise return it to the caller. If this returns
    /// `None`, the put succeeded.
    fn try_put(&self, data: T) -> Option<T>;
}

/// A consumer handle to a table. This allows taking or receiving values from the table such that no other consumer will
/// receive the same value ("exactly once to exactly one" semantics).
pub trait Consumer<T>: Send {
    /// Take/dequeue some data. The caller must be subscribed as a consumer.
    ///
    /// This has "exactly once to exactly one consumer" semantics.
    fn take(&self) -> T;

    /// Take/dequeue an element from the table if it is immediately available.
    fn try_take(&self) -> Option<T>;
}

/// A strong-observer handle to a table. This allows receiving every value from a table without preventing other
/// consumers or observers from seeing the same value ("exactly once to each" semantics). If a strong observer falls
/// behind on observing elements it will cause the table to block producers, so strong observers must make sure they
/// process data promptly.
pub trait StrongObserver<T>: Send {
    /// Observe some data. The caller must be subscribed as a strict observer.
    ///
    /// This has "exactly once to each observer" semantics.
    fn strong_observe(&self) -> T;

    /// Observe an element from the table if it is immediately available.
    fn try_strong_observe(&self) -> Option<T>;
}

/// A weak-observer handle to a table. This allows looking at the history of the table without affecting any other
/// producers, consumers, or observers. Weak-observers are not guaranteed to observe every element, so they never block
/// producers (which can simply overwrite data). However, weak-observers are guaranteed to alway get either nothing or
/// the data at the cursor requested.
pub trait WeakObserver<T>: Send {
    /// Observe the data at the given index in the full history of the table. If the data has already been discarded
    /// this will return `None`. This is guaranteed to always return either `None` or the actual value that existed at
    /// the given index.
    fn weak_observe(&self, index: Cursor) -> Option<T>;

    /// Return a cursor pointing to the most recent value in the table. This has very relaxed consistency, the element
    /// may no longer be the most recent or even no longer be available.
    fn recent_cursor(&self) -> Cursor;

    /// Return a cursor pointing to the oldest value still in the table. This has very relaxed consistency, the element
    /// may no longer be the oldest or even no longer be available.
    fn oldest_cursor(&self) -> Cursor;
}

/// An error for attaching a handle to a [`Table`].
#[derive(Debug)]
pub enum TableAttachError {
    /// The type of table doesn't support attachment of this type.
    Unsupported {
        /// The name of the type which does not support the attachment.
        table_type: String,
    },

    /// An attachment slot of the given kind could not be allocated.
    AllocationFailed {
        /// The name of the type which does not support the attachment.
        table_type: String,
        /// The reason the allocation failed, for example, "not enough allocated weak-observer slots".
        reason: String,
    },

    /// Unknown error.
    /// 
    /// TODO: This will be setup to integrate with snafu eventually.
    Whatever {
        /// A message describing the error.
        message: String,
        /// The cause of this error, if it exists.
        source: Option<Box<dyn Error>>,
    },
}

impl Display for TableAttachError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TableAttachError::Unsupported { table_type } => {
                write!(
                    f,
                    "table of type {table_type} does not support this kind of attachment",
                )
            }
            TableAttachError::AllocationFailed { table_type, reason } => {
                write!(f, "table of type {table_type} could not allocate attachment to table, because {reason}")
            }
            TableAttachError::Whatever { message, source } => {
                write!(
                    f,
                    "{message} {}",
                    if let Some(e) = source {
                        format!("{e}")
                    } else {
                        "()".to_owned()
                    }
                )
            }
        }
    }
}

impl Error for TableAttachError {}

// /// The type of producer handles for this table. It provides methods to put values into this table.
// type Producer: Producer<T> + ?Sized = dyn Producer<T>;
// /// The type of consumer handles for this table. It provides methods to take values from this table with "exact
// /// once to exactly one" semantics. I.e., only one consumer will get each value.
// type Consumer: Consumer<T> + ?Sized = dyn Consumer<T>;
// /// The type of strong-observer handles for this table. It provides methods to get values from this table without
// /// affecting consumers or other observers. This provides "exactly once to each" semantics.
// type StrongObserver: StrongObserver<T> + ?Sized = dyn StrongObserver<T>;
// /// The type of weak-observer handles for this table. It provides methods to access values from the history of the
// /// table on a best effort basis.
// type WeakObserver: WeakObserver<T> + ?Sized = dyn WeakObserver<T>;

/// The interface to all table implementations. This is extended by a few other traits to add features.
///
/// TODO: An improved implementation should probably split up these operations into several separate types and traits
/// and reduce the risk of misuse.
pub trait Table<T>: Any + Sync + Send {
    // TODO: It will make sense to split these out into different traits since some implementations will only implement
    // come for a given set of traits on T. Specifically, sometimes `Copy + Send` is needed for some, but `Clone + Sync`
    // are needed for others. (Interestingly, you can replace the latter with the former for some implementations, but
    // you need one or the other.)

    /// Attach to the table as a producer. An error represents either that producers are not supported or that producers
    /// are supported but all supported producers are already attached (for instance, if a second producer tries to
    /// attach to a single-producer table implementation).
    fn attach_producer(&self) -> Result<Box<dyn Producer<T>>, TableAttachError>;
    /// Attach to the table as a consumer. An error represents either that producers are not supported or that no more
    /// consumers are allowed on this specific table (for example, for a single-consumer table implementation).
    fn attach_consumer(&self) -> Result<Box<dyn Consumer<T>>, TableAttachError>;
    /// Attach to the table as a strong observer. An error represents either that strong observers are not supported or that no more
    /// strong-observers are allowed on this specific table (for example, if the table as a limited number of strong-observer slots).
    fn attach_strong_observer(&self) -> Result<Box<dyn StrongObserver<T>>, TableAttachError>;
    /// Attach to the table as a weak-observer. An error represents either that weak-observer are not supported or that
    /// no more weak-observer are allowed on this specific table (for example, if there are a limited number of
    /// weak-observer slots on the table.).
    fn attach_weak_observer(&self) -> Result<Box<dyn WeakObserver<T>>, TableAttachError>;
}

// struct AnyTable {
//     table: Arc<dyn Any + Sync + Send + 'static>
// }

// impl AnyTable {
//     fn downcast<U: Sync + Send>(&self) -> Option<Arc<dyn Table<U>>> {
//         self.table.clone().downcast().ok()
//     }
// }

