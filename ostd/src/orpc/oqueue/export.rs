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
use ostd_macros::ostd_error;
use serde::{Serialize, de::DeserializeOwned};
use snafu::Snafu;

use super::{
    AnyOQueueRef, ConsumableOQueue as _, ConsumableOQueueRef, OQueueBase as _, OQueueError,
    ObservationQuery, RevokedSnafu, StrongObserver, UnsupportedSnafu, ValueProducer,
    WeakAnyOQueueRef,
};

/// A message-type-erased handle to an OQueue that has been exported for userspace consumption.
///
/// Stored in the export registry so consumers can enumerate and read queues by
/// [`Path`](crate::orpc::path::Path) without naming the message type. An export can independently
/// carry an observe attachment, a produce attachment, or both: [`register`](super::registry::register)
/// / [`register_with`](super::registry::register_with) add an observe attachment, and
/// [`register_producible`](super::registry::register_producible) adds a produce attachment. This is
/// a factory: attaching mints a fresh per-reader (or per-writer) handle.
pub trait OQueueExport: Send + Sync {
    /// Returns the name of the message type, for use in file metadata.
    fn type_name(&self) -> &'static str;

    /// Returns whether the underlying OQueue still exists.
    fn is_alive(&self) -> bool;

    /// Returns whether this export carries an observe attachment, i.e. whether
    /// [`attach_strong_observer`](Self::attach_strong_observer) can succeed.
    fn supports_observe(&self) -> bool {
        false
    }

    /// Returns whether this export carries a produce attachment, i.e. whether
    /// [`attach_producer`](Self::attach_producer) can succeed.
    fn supports_produce(&self) -> bool {
        false
    }

    /// Attaches a fresh observer and returns it as a CBOR record source.
    ///
    /// The default implementation reports [`OQueueError::Unsupported`], which is correct for every
    /// export that does not [`supports_observe`](Self::supports_observe).
    fn attach_strong_observer(&self) -> Result<Box<dyn CborStrongObserve>, OQueueError> {
        Err(UnsupportedSnafu.build())
    }

    /// Attaches a fresh producer that accepts CBOR-encoded values from userspace.
    ///
    /// The default implementation reports [`OQueueError::Unsupported`], which is correct for every
    /// export that does not [`supports_produce`](Self::supports_produce).
    fn attach_producer(&self) -> Result<Box<dyn CborProducer>, OQueueError> {
        Err(UnsupportedSnafu.build())
    }
}

/// Error produced by [`CborProducer::produce_cbor`] when the bytes at the front of the buffer
/// cannot be produced into the OQueue as-is.
#[non_exhaustive]
#[ostd_error]
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub enum ProduceCborError {
    /// The underlying OQueue is gone.
    #[snafu(transparent)]
    #[ostd(context(source))]
    OQueue {
        /// The underlying OQueue error.
        source: OQueueError,
    },
    /// The bytes at the front of the buffer are not well-formed CBOR.
    #[snafu(display("Malformed CBOR record ({context})"))]
    Malformed,
    /// The bytes at the front of the buffer are well-formed CBOR but do not decode as the expected
    /// message type.
    #[snafu(display("CBOR record does not match the expected message schema ({context})"))]
    SchemaMismatch,
}

/// A per-reader handle that accepts CBOR-encoded bytes from userspace and produces the decoded
/// values into an OQueue, with the message type erased.
pub trait CborProducer: Send {
    /// Attempts to decode one CBOR-encoded record from the front of `bytes` and, if complete,
    /// blocks (if necessary) until there is space to produce it into the OQueue.
    ///
    /// Returns the number of bytes consumed from the front of `bytes` on success. Returns `Ok(0)`
    /// if `bytes` does not yet contain a complete record (the caller should append more bytes and
    /// retry). Returns `Err` if `bytes` starts with malformed CBOR, if the CBOR item does not
    /// decode as the expected message type, or once the underlying OQueue is gone.
    fn produce_cbor(&self, bytes: &[u8]) -> Result<usize, ProduceCborError>;
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

/// A closure that attaches a fresh producer to an OQueue and wraps it as a [`CborProducer`]. Only
/// present on exports registered via [`super::registry::register_producible`].
type AttachProducerFn<T> =
    Box<dyn Fn(&AnyOQueueRef<T>) -> Result<Box<dyn CborProducer>, OQueueError> + Send + Sync>;

/// The concrete [`OQueueExport`] for an OQueue with message type `T`.
///
/// `observe` and `produce` are held separately (rather than through a direction discriminant) so
/// an export can carry either, or both, independently of one another.
pub(super) struct OQueueExportHandle<T: 'static> {
    weak: WeakAnyOQueueRef<T>,
    type_name: &'static str,
    observe: Option<AttachStrongObserveFn<T>>,
    produce: Option<AttachProducerFn<T>>,
}

impl<T: Send + 'static> OQueueExport for OQueueExportHandle<T> {
    fn type_name(&self) -> &'static str {
        self.type_name
    }

    fn is_alive(&self) -> bool {
        self.weak.upgrade().is_some()
    }

    fn supports_observe(&self) -> bool {
        self.observe.is_some()
    }

    fn supports_produce(&self) -> bool {
        self.produce.is_some()
    }

    fn attach_strong_observer(&self) -> Result<Box<dyn CborStrongObserve>, OQueueError> {
        let Some(attach_fn) = &self.observe else {
            return Err(UnsupportedSnafu.build());
        };
        let oqueue = self.weak.upgrade().ok_or_else(|| RevokedSnafu.build())?;
        attach_fn(&oqueue)
    }

    fn attach_producer(&self) -> Result<Box<dyn CborProducer>, OQueueError> {
        let Some(attach_fn) = &self.produce else {
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

/// Builds a type-erased export handle carrying only an observe attachment for an OQueue whose
/// whole message is streamed via the identity projection (so the message type's derived
/// `Serialize` is used).
pub(super) fn make_export<T: Copy + Send + Serialize + 'static>(
    oqueue: &AnyOQueueRef<T>,
) -> OQueueExportHandle<T> {
    OQueueExportHandle {
        weak: oqueue.downgrade(),
        type_name: core::any::type_name::<T>(),
        observe: Some(Box::new(|oqueue| {
            attach_cbor_observer(oqueue, ObservationQuery::<T, T>::identity())
        })),
        produce: None,
    }
}

/// Builds a type-erased export handle carrying only an observe attachment for an OQueue whose
/// messages are streamed through a caller-supplied projection `project: Fn(&T) -> U`, where `U` is
/// the `Copy + Serialize` value placed in the stream.
pub(super) fn make_export_with<T, U, F>(
    oqueue: &AnyOQueueRef<T>,
    project: F,
) -> OQueueExportHandle<T>
where
    T: Send + 'static,
    U: Copy + Send + Serialize + 'static,
    F: Fn(&T) -> U + Send + Sync + 'static,
{
    let project = Arc::new(project);
    OQueueExportHandle {
        weak: oqueue.downgrade(),
        type_name: core::any::type_name::<U>(),
        observe: Some(Box::new(move |oqueue| {
            let project = project.clone();
            attach_cbor_observer(
                oqueue,
                ObservationQuery::new(move |msg: &T| (*project)(msg)),
            )
        })),
        produce: None,
    }
}

/// A [`CborProducer`] backed by a [`ValueProducer<T>`], decoding each CBOR record as a `T` and
/// producing it into the OQueue.
struct CborValueProducer<T> {
    producer: ValueProducer<T>,
}

impl<T: Send + DeserializeOwned + 'static> CborProducer for CborValueProducer<T> {
    fn produce_cbor(&self, bytes: &[u8]) -> Result<usize, ProduceCborError> {
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
                return Err(MalformedSnafu.build());
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
                return Err(SchemaMismatchSnafu.build());
            }
        };

        self.producer.produce(value);
        Ok(item_len)
    }
}

/// Builds a type-erased export handle carrying only a produce attachment for a
/// [`super::ConsumableOQueue`]: it accepts values produced from userspace (see
/// [`super::registry::register_producible`]).
///
/// Requiring `&ConsumableOQueueRef<T>` (rather than the type-erased `&AnyOQueueRef<T>`) enforces at
/// compile time that only a `ConsumableOQueue` can be made producible.
pub(super) fn make_produce_export<T: Copy + Send + Serialize + DeserializeOwned + 'static>(
    oqueue: &ConsumableOQueueRef<T>,
) -> OQueueExportHandle<T> {
    OQueueExportHandle {
        weak: oqueue.downgrade(),
        type_name: core::any::type_name::<T>(),
        observe: None,
        produce: Some(Box::new(|oqueue| {
            let producer = oqueue.attach_value_producer()?;
            Ok(Box::new(CborValueProducer { producer }) as Box<dyn CborProducer>)
        })),
    }
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::{orpc::oqueue::ConsumableOQueueRef, path, prelude::*};

    /// Decode a self-delimiting CBOR stream of records, as produced by the observer.
    fn decode_records(buf: &[u8]) -> Vec<u64> {
        let mut de = minicbor_serde::Deserializer::new(buf);
        let mut records = Vec::new();
        while de.decoder().position() < buf.len() {
            records.push(serde::Deserialize::deserialize(&mut de).unwrap());
        }
        records
    }

    #[ktest]
    fn handle_serves_observe_and_produce_independently() {
        let path = path!(export.bidirectional[1]);
        let queue = ConsumableOQueueRef::<usize>::new(16, path.clone());

        // A single `OQueueExportHandle` can carry both attachments at once; this is representable
        // even though the registry no longer lets two separate registrations at the same path
        // merge into one (see `registry::insert_export`). Combine the observe half from
        // `make_export` and the produce half from `make_produce_export` into one handle, exactly
        // as a caller who needs both would build it directly.
        let observe_only = make_export(&queue.as_any_oqueue());
        let produce_only = make_produce_export(&queue);
        let handle = OQueueExportHandle {
            weak: observe_only.weak,
            type_name: observe_only.type_name,
            observe: observe_only.observe,
            produce: produce_only.produce,
        };
        assert!(handle.supports_observe());
        assert!(handle.supports_produce());

        let observer = handle.attach_strong_observer().unwrap();
        let producer = handle.attach_producer().unwrap();
        let consumer = queue.attach_consumer().unwrap();

        // A single userspace write through the produce attachment reaches both the consumer and
        // the independently attached observer.
        let mut record = Vec::new();
        serde::Serialize::serialize(&7usize, &mut minicbor_serde::Serializer::new(&mut record))
            .unwrap();
        producer.produce_cbor(&record).unwrap();

        assert_eq!(consumer.consume(), 7);

        let mut buf = Vec::new();
        assert!(observer.try_strong_observe_into(&mut buf).unwrap());
        assert_eq!(decode_records(&buf), [7]);
    }
}
