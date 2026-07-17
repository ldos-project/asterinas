// SPDX-License-Identifier: MPL-2.0

//! Exporting OQueues to userspace as type-erased CBOR byte streams.
//!
//! Every OQueue is already observable in-kernel; this module is about making a queue's contents
//! readable from *userspace* (for example, via the OQueue filesystem) so operating-system policies
//! can be driven by data collected outside the kernel.
//!
//! The OQueue [`registry`](super::registry) is keyed by the message type `T`, which callers must
//! name statically. A generic consumer such as the OQueue filesystem holds only a
//! [`Path`](crate::orpc::path::Path) and cannot recover `T` — Rust has no runtime reflection. To
//! bridge this gap, [`register`](super::registry::register) (and its projecting sibling
//! [`register_with`](super::registry::register_with)) capture — at registration time, when `T` is
//! known — a type-erased [`OQueueExport`] handle that knows how to attach an observer and encode
//! the observed values as CBOR. The queue can then be streamed by path alone, without the reader
//! naming `T`.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::cell::Cell;

use minicbor_serde::Serializer;
use serde::Serialize;

use super::{
    AnyOQueueRef, RevokedSnafu, OQueueBase as _, OQueueError, ObservationQuery, StrongObserver,
    WeakAnyOQueueRef,
};

/// A message-type-erased handle to an OQueue that has been exported for userspace consumption.
///
/// Stored in the export registry so consumers can enumerate and read queues by
/// [`Path`](crate::orpc::path::Path) without naming the message type. This is a factory: it mints
/// a fresh [`CborStrongObserve`] per reader, so multiple readers each receive the full stream.
pub trait OQueueExport: Send + Sync {
    /// Returns the name of the message type, for use in file metadata.
    fn type_name(&self) -> &'static str;

    /// Returns whether the underlying OQueue still exists.
    fn is_alive(&self) -> bool;

    /// Attaches a fresh observer and returns it as a CBOR record source.
    fn attach_strong_observer(&self) -> Result<Box<dyn CborStrongObserve>, OQueueError>;
}

/// A per-reader observer that yields CBOR-encoded records, with the message type erased.
pub trait CborStrongObserve: Send {
    /// Drains the next observed value without blocking and appends its CBOR record to `out`.
    ///
    /// Returns `Ok(true)` if a record was written, `Ok(false)` if nothing is currently available,
    /// and `Err` (typically [`OQueueError::Revoked`]) once the OQueue is gone.
    fn try_strong_observe_into(&self, out: &mut Vec<u8>) -> Result<bool, OQueueError>;

    /// Blocks until the next observed value is available, then appends its CBOR record to `out`.
    ///
    /// Returns `Err` (typically [`OQueueError::Revoked`]) once the OQueue is gone, which a reader
    /// treats as end-of-stream.
    fn strong_observe_into(&self, out: &mut Vec<u8>) -> Result<(), OQueueError>;
}

/// A CBOR record, as described in the OQueue FS record format.
#[derive(Serialize)]
struct Record<U> {
    seq: u64,
    value: U,
}

/// A closure that attaches a fresh observer to an OQueue and wraps it as a [`CborStrongObserve`].
///
/// The observed value type `U` is erased inside the closure, so a single [`OQueueExportHandle`]
/// can serve both the identity case (whole message) and the projection case (a `Copy + Serialize`
/// summary). The closure is built where the relevant bounds are known — in
/// [`make_export`]/[`make_export_with`] — and captures whatever projection it needs.
type AttachFn<T> =
    Box<dyn Fn(&AnyOQueueRef<T>) -> Result<Box<dyn CborStrongObserve>, OQueueError> + Send + Sync>;

/// The concrete [`OQueueExport`] for an OQueue with message type `T`.
///
/// Both `register` (whole message via the identity projection) and `register_with` (a
/// caller-supplied projection) produce this one handle; they differ only in the
/// `attach_strong_observer_fn` closure
/// baked in at construction, since the observed type `U` and its `Copy + Serialize` bound cannot
/// survive to this erased handle.
struct OQueueExportHandle<T: 'static> {
    weak: WeakAnyOQueueRef<T>,
    type_name: &'static str,
    attach_strong_observer_fn: AttachFn<T>,
}

impl<T: Send + 'static> OQueueExport for OQueueExportHandle<T> {
    fn type_name(&self) -> &'static str {
        self.type_name
    }

    fn is_alive(&self) -> bool {
        self.weak.upgrade().is_some()
    }

    fn attach_strong_observer(&self) -> Result<Box<dyn CborStrongObserve>, OQueueError> {
        let oqueue = self.weak.upgrade().ok_or_else(|| RevokedSnafu.build())?;
        (self.attach_strong_observer_fn)(&oqueue)
    }
}

/// A [`CborStrongObserve`] backed by a [`StrongObserver<U>`], encoding each observed value as a CBOR
/// [`Record`].
struct CborStrongObserver<U> {
    observer: StrongObserver<U>,
    seq: Cell<u64>,
}

impl<U: Copy + Send + Serialize + 'static> CborStrongObserver<U> {
    /// Appends the CBOR record `{seq, value}` for `value` to `out`, bumping the sequence number.
    fn encode(&self, value: U, out: &mut Vec<u8>) {
        let seq = self.seq.get();
        self.seq.set(seq.wrapping_add(1));

        // Encoding into a `Vec` writer is infallible, so the record is always appended whole.
        Record { seq, value }
            .serialize(&mut Serializer::new(&mut *out))
            .expect("CBOR encoding of an OQueue record into a Vec cannot fail");
    }
}

impl<U: Copy + Send + Serialize + 'static> CborStrongObserve for CborStrongObserver<U> {
    fn try_strong_observe_into(&self, out: &mut Vec<u8>) -> Result<bool, OQueueError> {
        let Some(value) = self.observer.try_strong_observe()? else {
            return Ok(false);
        };
        self.encode(value, out);
        Ok(true)
    }

    fn strong_observe_into(&self, out: &mut Vec<u8>) -> Result<(), OQueueError> {
        let value = self.observer.strong_observe()?;
        self.encode(value, out);
        Ok(())
    }
}

/// Attaches a strong observer with the given query and wraps it as a CBOR record source.
fn attach_cbor_observer<T, U>(
    oqueue: &AnyOQueueRef<T>,
    query: ObservationQuery<T, U>,
) -> Result<Box<dyn CborStrongObserve>, OQueueError>
where
    T: Send + 'static,
    U: Copy + Send + Serialize + 'static,
{
    // Revocable: a stalled userspace reader must never block the producing kernel component.
    let observer = oqueue.attach_revocable_strong_observer(query)?;
    Ok(Box::new(CborStrongObserver {
        observer,
        seq: Cell::new(0),
    }))
}

/// Builds a type-erased export handle for an OQueue whose whole message is streamed via the
/// identity projection (so the message type's derived `Serialize` is used).
pub(super) fn make_export<T: Copy + Send + Serialize + 'static>(
    oqueue: &AnyOQueueRef<T>,
) -> Arc<dyn OQueueExport> {
    Arc::new(OQueueExportHandle {
        weak: oqueue.downgrade(),
        type_name: core::any::type_name::<T>(),
        attach_strong_observer_fn: Box::new(|oqueue| {
            attach_cbor_observer(oqueue, ObservationQuery::<T, T>::identity())
        }),
    })
}

/// Builds a type-erased export handle for an OQueue whose messages are streamed through a
/// caller-supplied projection `project: Fn(&T) -> U`, where `U` is the `Copy + Serialize` value
/// placed in the stream.
pub(super) fn make_export_with<T, U, F>(
    oqueue: &AnyOQueueRef<T>,
    project: F,
) -> Arc<dyn OQueueExport>
where
    T: Send + 'static,
    U: Copy + Send + Serialize + 'static,
    F: Fn(&T) -> U + Send + Sync + 'static,
{
    let project = Arc::new(project);
    Arc::new(OQueueExportHandle {
        weak: oqueue.downgrade(),
        type_name: core::any::type_name::<U>(),
        attach_strong_observer_fn: Box::new(move |oqueue| {
            let project = project.clone();
            attach_cbor_observer(
                oqueue,
                ObservationQuery::new(move |msg: &T| (*project)(msg)),
            )
        }),
    })
}
