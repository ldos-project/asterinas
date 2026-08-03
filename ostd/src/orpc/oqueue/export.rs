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

use log::warn;
use minicbor_serde::Serializer;
use serde::{Serialize, de::DeserializeOwned};

use super::{
    AnyOQueueRef, ConsumableOQueue as _, ConsumableOQueueRef, OQueueBase as _, OQueueError,
    ObservationQuery, RevokedSnafu, StrongObserver, UnsupportedSnafu, ValueProducer,
    WeakAnyOQueueRef,
};

/// The single direction of an exported OQueue's data flow.
///
/// Every OQueue export is a one-way tunnel: either the kernel produces values that userspace
/// observes (`Observe`, via the `strong_observe` file), or userspace produces values that the
/// kernel consumes (`Produce`, via the `produce` file). Never both — see [`OQueueExport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportDirection {
    /// Kernel -> user: userspace observes values produced in the kernel.
    Observe,
    /// User -> kernel: userspace produces values consumed in the kernel.
    Produce,
}

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

    /// Returns this export's single data-flow direction.
    fn direction(&self) -> ExportDirection;

    /// Attaches a fresh observer and returns it as a CBOR record source.
    ///
    /// The default implementation reports [`OQueueError::Unsupported`], which is correct for every
    /// export whose [`direction`](Self::direction) is [`ExportDirection::Produce`].
    fn attach_strong_observer(&self) -> Result<Box<dyn CborStrongObserve>, OQueueError> {
        Err(UnsupportedSnafu.build())
    }

    /// Attaches a fresh producer that accepts CBOR-encoded values from userspace.
    ///
    /// The default implementation reports [`OQueueError::Unsupported`], which is correct for every
    /// export whose [`direction`](Self::direction) is [`ExportDirection::Observe`].
    fn attach_producer(&self) -> Result<Box<dyn CborProduce>, OQueueError> {
        Err(UnsupportedSnafu.build())
    }
}

/// A per-reader handle that accepts CBOR-encoded bytes from userspace and produces the decoded
/// values into an OQueue, with the message type erased.
pub trait CborProduce: Send {
    /// Attempts to decode one CBOR-encoded record from the front of `bytes` and, if complete,
    /// blocks (if necessary) until there is space to produce it into the OQueue.
    ///
    /// Returns the number of bytes consumed from the front of `bytes` on success. Returns `Ok(0)`
    /// if `bytes` does not yet contain a complete record (the caller should append more bytes and
    /// retry). Returns `Err` (typically [`OQueueError::Revoked`]) once the underlying OQueue is
    /// gone.
    fn produce_cbor(&self, bytes: &[u8]) -> Result<usize, OQueueError>;
}

/// A per-reader observer that yields CBOR-encoded records, with the message type erased.
pub trait CborStrongObserve: Send {
    /// Drains the next observed value without blocking and appends its CBOR record to `out`.
    ///
    /// Returns `Ok(true)` if a record was written, `Ok(false)` if nothing is currently available,
    /// and `Err` (typically [`OQueueError::Revoked`]) once the observer has been revoked (for
    /// example, because the reader fell too far behind).
    fn try_strong_observe_into(&self, out: &mut Vec<u8>) -> Result<bool, OQueueError>;

    /// Blocks until the next observed value is available, then appends its CBOR record to `out`.
    ///
    /// Returns `Err` (typically [`OQueueError::Revoked`]) once the observer has been revoked (for
    /// example, because the reader fell too far behind), which a reader treats as end-of-stream.
    fn strong_observe_into(&self, out: &mut Vec<u8>) -> Result<(), OQueueError>;
}

/// A closure that attaches a fresh observer to an OQueue and wraps it as a [`CborStrongObserve`].
/// The observed type `U` (identity or a projection) is erased inside the closure.
type AttachStrongObserveFn<T> =
    Box<dyn Fn(&AnyOQueueRef<T>) -> Result<Box<dyn CborStrongObserve>, OQueueError> + Send + Sync>;

/// A closure that attaches a fresh producer to an OQueue and wraps it as a [`CborProduce`]. Only
/// present on exports registered via [`super::registry::register_producible`].
type AttachProducerFn<T> =
    Box<dyn Fn(&AnyOQueueRef<T>) -> Result<Box<dyn CborProduce>, OQueueError> + Send + Sync>;

/// The single attach closure an [`OQueueExportHandle`] holds, tying its [`ExportDirection`] to the
/// one closure it may call — holding both would let a handle serve both directions, which
/// [`OQueueExport`] forbids.
enum Attachment<T: 'static> {
    Observe(AttachStrongObserveFn<T>),
    Produce(AttachProducerFn<T>),
}

/// The concrete [`OQueueExport`] for an OQueue with message type `T`.
struct OQueueExportHandle<T: 'static> {
    weak: WeakAnyOQueueRef<T>,
    type_name: &'static str,
    attachment: Attachment<T>,
}

impl<T: Send + 'static> OQueueExport for OQueueExportHandle<T> {
    fn type_name(&self) -> &'static str {
        self.type_name
    }

    fn is_alive(&self) -> bool {
        self.weak.upgrade().is_some()
    }

    fn direction(&self) -> ExportDirection {
        match &self.attachment {
            Attachment::Observe(_) => ExportDirection::Observe,
            Attachment::Produce(_) => ExportDirection::Produce,
        }
    }

    fn attach_strong_observer(&self) -> Result<Box<dyn CborStrongObserve>, OQueueError> {
        let Attachment::Observe(attach_fn) = &self.attachment else {
            return Err(UnsupportedSnafu.build());
        };
        let oqueue = self.weak.upgrade().ok_or_else(|| RevokedSnafu.build())?;
        attach_fn(&oqueue)
    }

    fn attach_producer(&self) -> Result<Box<dyn CborProduce>, OQueueError> {
        let Attachment::Produce(attach_fn) = &self.attachment else {
            return Err(UnsupportedSnafu.build());
        };
        let oqueue = self.weak.upgrade().ok_or_else(|| RevokedSnafu.build())?;
        attach_fn(&oqueue)
    }
}

/// A [`CborStrongObserve`] backed by a [`StrongObserver<U>`], encoding each observed value as a CBOR
/// record.
struct CborStrongObserver<U> {
    observer: StrongObserver<U>,
}

impl<U: Copy + Send + Serialize + 'static> CborStrongObserver<U> {
    /// Appends the CBOR record for `value` to `out`.
    fn encode(&self, value: U, out: &mut Vec<u8>) {
        // Encoding into a `Vec` writer is infallible, so the record is always appended whole.
        value
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
    let observer = oqueue.attach_revocable_strong_observer(query)?;
    Ok(Box::new(CborStrongObserver { observer }))
}

/// Builds a type-erased, observe-direction export handle for an OQueue whose whole message is
/// streamed via the identity projection (so the message type's derived `Serialize` is used).
pub(super) fn make_export<T: Copy + Send + Serialize + 'static>(
    oqueue: &AnyOQueueRef<T>,
) -> Arc<dyn OQueueExport> {
    Arc::new(OQueueExportHandle {
        weak: oqueue.downgrade(),
        type_name: core::any::type_name::<T>(),
        attachment: Attachment::Observe(Box::new(|oqueue| {
            attach_cbor_observer(oqueue, ObservationQuery::<T, T>::identity())
        })),
    })
}

/// Builds a type-erased, observe-direction export handle for an OQueue whose messages are streamed
/// through a caller-supplied projection `project: Fn(&T) -> U`, where `U` is the `Copy + Serialize`
/// value placed in the stream.
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
        attachment: Attachment::Observe(Box::new(move |oqueue| {
            let project = project.clone();
            attach_cbor_observer(
                oqueue,
                ObservationQuery::new(move |msg: &T| (*project)(msg)),
            )
        })),
    })
}

/// A [`CborProduce`] backed by a [`ValueProducer<T>`], decoding each CBOR record as a `T` and
/// producing it into the OQueue.
struct CborValueProducer<T> {
    producer: ValueProducer<T>,
}

impl<T: Send + DeserializeOwned + 'static> CborProduce for CborValueProducer<T> {
    fn produce_cbor(&self, bytes: &[u8]) -> Result<usize, OQueueError> {
        if bytes.is_empty() {
            return Ok(0);
        }

        // Probe for one complete, well-formed CBOR item independently of `T`: this distinguishes
        // "not enough bytes yet" (retry) from "not valid CBOR at all" (malformed), which
        // `T::deserialize` alone cannot tell apart.
        let mut probe = minicbor::decode::Decoder::new(bytes);
        match probe.skip() {
            Err(err) if err.is_end_of_input() => return Ok(0),
            Err(err) => {
                warn!("OQueue produce file received a malformed CBOR record: {err}");
                // Drop one byte so a bad record cannot wedge the stream.
                return Ok(1);
            }
            Ok(()) => {}
        }
        let item_len = probe.position();

        let mut deserializer = minicbor_serde::Deserializer::new(&bytes[..item_len]);
        let value = match T::deserialize(&mut deserializer) {
            Ok(value) => value,
            Err(err) => {
                // Well-formed CBOR (per the probe above) that doesn't decode as `T`: a schema
                // mismatch, not an incomplete record.
                warn!(
                    "OQueue produce file received a CBOR record that does not decode as {}: {err}",
                    core::any::type_name::<T>()
                );
                return Ok(item_len);
            }
        };

        self.producer.produce(value);
        Ok(item_len)
    }
}

/// Builds a type-erased, produce-direction export handle for a [`super::ConsumableOQueue`]: it
/// accepts values produced from userspace (see [`super::registry::register_producible`]) but,
/// unlike [`make_export`]/[`make_export_with`], does not also support `strong_observe`.
///
/// Requiring `&ConsumableOQueueRef<T>` (rather than the type-erased `&AnyOQueueRef<T>`) enforces at
/// compile time that only a `ConsumableOQueue` can be made producible.
pub(super) fn make_produce_export<T: Copy + Send + Serialize + DeserializeOwned + 'static>(
    oqueue: &ConsumableOQueueRef<T>,
) -> Arc<dyn OQueueExport> {
    Arc::new(OQueueExportHandle {
        weak: oqueue.downgrade(),
        type_name: core::any::type_name::<T>(),
        attachment: Attachment::Produce(Box::new(|oqueue| {
            let producer = oqueue.attach_value_producer()?;
            Ok(Box::new(CborValueProducer { producer }) as Box<dyn CborProduce>)
        })),
    })
}
