// SPDX-License-Identifier: MPL-2.0
//! Registry of named OQueues

use alloc::{boxed::Box, collections::BTreeMap, sync::Arc, vec::Vec};
use core::any::{Any, TypeId};

use hashbrown::{
    HashMap,
    hash_map::{DefaultHashBuilder, EntryRef},
};
use log::warn;
use serde::Serialize;

use super::{
    AnyOQueueRef,
    export::{OQueueExport, make_export, make_export_with},
};
use crate::{
    orpc::{
        oqueue::WeakAnyOQueueRef,
        path::{Path, PathPattern},
    },
    sync::{Mutex, MutexGuard},
};

type RegistryMap = HashMap<TypeId, PathMap>;

/// A mapping from Paths to OQueues for a specific type. The type is erased and calling methods at
/// the wrong type will panic.
struct PathMap {
    clean_fn: fn(&mut PathMap),
    map: HashMap<Path, Box<dyn Any + Send + Sync + 'static>>,
}

impl PathMap {
    fn new<T: ?Sized + 'static>() -> Self {
        Self {
            clean_fn: |type_map| {
                type_map
                    .map
                    .retain(|_, queue| downcast_oqueue::<T>(queue).upgrade().is_some())
            },
            map: Default::default(),
        }
    }

    /// Remove any entries for OQueues which no longer exist (the weak reference is invalidated).
    fn clean(&mut self) {
        (self.clean_fn)(self)
    }
}

static REGISTRY: Mutex<Option<RegistryMap>> = Mutex::new(None);

/// Get a reference to the global registry object, initializing it if needed.
fn registry() -> MutexGuard<'static, Option<RegistryMap>> {
    let mut guard = REGISTRY.lock();
    if guard.is_none() {
        guard.replace(HashMap::new());
    }

    guard
}

/// Register a OQueue with `path` in [`REGISTRY`] only, without exporting it to userspace via [`OQUEUES`].
///
/// If an existing OQueue is registered with the same type and path, this will replace that OQueue
/// and emit a warning.
pub fn register_no_export<T: ?Sized + 'static>(path: &Path, v: &AnyOQueueRef<T>) {
    let mut map = registry();
    let entry = get_entry::<T>(&mut map, path);
    if let EntryRef::Occupied(entry) = &entry
        && downcast_oqueue::<T>(entry.get()).upgrade().is_some()
    {
        warn!("Overwriting OQueue registry entry with path {}", path);
    }
    entry.insert(Box::<WeakAnyOQueueRef<T>>::new(v.downgrade()));
}

/// Ensure `path` is present in [`REGISTRY`], inserting it if absent.
fn ensure_registered<T: ?Sized + 'static>(path: &Path, v: &AnyOQueueRef<T>) {
    let mut map = registry();
    let entry = get_entry::<T>(&mut map, path);
    if let EntryRef::Occupied(entry) = &entry
        && downcast_oqueue::<T>(entry.get()).upgrade().is_some()
    {
        return;
    }
    entry.insert(Box::<WeakAnyOQueueRef<T>>::new(v.downgrade()));
}

/// Get the OQueue from a box stored in the OQueue registry.
fn downcast_oqueue<'a, T: ?Sized + 'static>(
    v: &'a Box<dyn Any + Send + Sync + 'static>,
) -> &'a WeakAnyOQueueRef<T> {
    v.downcast_ref::<WeakAnyOQueueRef<T>>()
        .expect("The entry has the type T")
}

/// Remove registry entries for OQueues which no longer exist.
pub fn clean_registry() {
    let mut map = registry();
    for type_map in map.as_mut().unwrap().values_mut() {
        type_map.clean();
    }
}

/// A registry of OQueues that have been exported for OQueue FileSystem, keyed by `Path`.
///
/// This is separate from [`REGISTRY`]: it is keyed by `Path` only (not by type) and holds
/// type-erased [`OQueueExport`] handles, so a generic consumer such as the OQueue filesystem can
/// enumerate and read queues without naming the message type. A `BTreeMap` gives stable, sorted
/// iteration for directory listings. At most one export is kept per path.
static OQUEUES: Mutex<Option<BTreeMap<Path, Arc<dyn OQueueExport>>>> = Mutex::new(None);

/// Get a reference to the global export registry, initializing it if needed.
fn exports() -> MutexGuard<'static, Option<BTreeMap<Path, Arc<dyn OQueueExport>>>> {
    let mut guard = OQUEUES.lock();
    if guard.is_none() {
        guard.replace(BTreeMap::new());
    }

    guard
}

/// Register a OQueue at `path` in both [`REGISTRY`] and [`OQUEUES`], exporting it to userspace with
/// the message type's derived `Serialize` implementation (the whole message is streamed).
///
/// This is the common case for exportable queues: the message type just needs
/// `#[derive(Serialize)]` (plus `Copy`, since the whole value is observed). Use [`register_with`]
/// to stream a projected summary instead, or [`register_no_export`] for queues that must not be
/// exported.
pub fn register<T: Copy + Send + Serialize + 'static>(path: &Path, oqueue: &AnyOQueueRef<T>) {
    ensure_registered(path, oqueue);
    insert_export(path, make_export(oqueue));
}

/// Register a OQueue at `path` in both [`REGISTRY`] and [`OQUEUES`], exporting it to userspace via a
/// caller-supplied projection `project: Fn(&T) -> U`.
///
/// The projected value `U` (which must be `Copy + Serialize`) is what appears in the stream. Use
/// this when the whole message is not `Copy`/`Serialize` but a small serializable summary can be
/// extracted from it.
pub fn register_with<T, U, F>(path: &Path, oqueue: &AnyOQueueRef<T>, project: F)
where
    T: Send + 'static,
    U: Copy + Send + Serialize + 'static,
    F: Fn(&T) -> U + Send + Sync + 'static,
{
    ensure_registered(path, oqueue);
    insert_export(path, make_export_with(oqueue, project));
}

/// Insert an export handle into [`OQUEUES`], enforcing one export per path.
///
/// A path identifies a single stream, so a second export at an already-occupied live path is a
/// programming error: it is ignored with a warning. A stale entry (its OQueue already dropped) is
/// replaced.
fn insert_export(path: &Path, handle: Arc<dyn OQueueExport>) {
    let mut map = exports();
    let map = map.as_mut().unwrap();
    if let Some(existing) = map.get(path)
        && existing.is_alive()
    {
        warn!("Ignoring duplicate OQueue export for path {}", path);
        return;
    }
    map.insert(path.clone(), handle);
}

/// Get a handle to the OQueue exported at `path`, if any.
pub fn lookup_export(path: &Path) -> Option<Arc<dyn OQueueExport>> {
    let map = exports();
    map.as_ref().unwrap().get(path).cloned()
}

/// List the paths of all currently-exported OQueues, in sorted order.
pub fn list_export_paths() -> Vec<Path> {
    let map = exports();
    map.as_ref().unwrap().keys().cloned().collect()
}

/// Remove export registry entries for OQueues which no longer exist.
pub fn clean_exports() {
    let mut map = exports();
    map.as_mut().unwrap().retain(|_, handle| handle.is_alive());
}

/// Get the OQueue with the given type and name.
pub fn lookup_by_path<T: ?Sized + 'static>(path: &Path) -> Option<AnyOQueueRef<T>> {
    let mut map = registry();
    let entry = get_entry::<T>(&mut map, path);
    if let EntryRef::Occupied(entry) = entry {
        downcast_oqueue::<T>(entry.get()).upgrade()
    } else {
        None
    }
}

/// Get all OQueues whose names match a given pattern.
pub fn lookup_by_path_pattern<T: ?Sized + 'static>(pat: &PathPattern) -> Vec<AnyOQueueRef<T>> {
    let mut map = registry();
    let type_map = get_type_map::<T>(&mut map);
    type_map
        .map
        .iter()
        .filter_map(|(path, queue)| {
            if pat.matches(path) {
                queue
                    .downcast_ref::<WeakAnyOQueueRef<T>>()
                    .expect("The entry has the type T")
                    .upgrade()
            } else {
                None
            }
        })
        .collect()
}

/// Get all OQueues of a given type.
pub fn lookup_by_type<T: ?Sized + 'static>() -> Vec<AnyOQueueRef<T>> {
    let mut map = registry();
    let type_map = get_type_map::<T>(&mut map);

    type_map
        .map
        .values()
        .filter_map(|value| {
            value
                .downcast_ref::<WeakAnyOQueueRef<T>>()
                .unwrap()
                .upgrade()
        })
        .collect()
}

/// Get the entry for a specific type and name out of `map`.
fn get_entry<'a, 'b, T: ?Sized + 'static>(
    map: &'a mut MutexGuard<'static, Option<RegistryMap>>,
    path: &'b Path,
) -> EntryRef<'a, 'b, Path, Path, Box<dyn Any + Send + Sync>, DefaultHashBuilder> {
    get_type_map::<T>(map).map.entry_ref(path)
}

/// Get the name--OQueue map associated with the given `T`.
fn get_type_map<'a, T: ?Sized + 'static>(
    map: &'a mut MutexGuard<'static, Option<RegistryMap>>,
) -> &'a mut PathMap {
    map.as_mut()
        .unwrap()
        .entry(TypeId::of::<T>())
        .or_insert_with(|| PathMap::new::<T>())
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::{
        orpc::oqueue::{ConsumableOQueue as _, ConsumableOQueueRef, OQueueBase as _},
        path, path_pattern,
        prelude::*,
    };

    fn new_oqueue(path: &Path) -> ConsumableOQueueRef<usize> {
        ConsumableOQueueRef::<usize>::new(4, path.clone())
    }

    #[ktest(expect_redundant_test_prefix)]
    fn test_lookup_by_path() {
        let path = path!(test.queue[1]);
        let _queue = new_oqueue(&path);

        assert!(lookup_by_path::<usize>(&path).is_some());
        assert!(lookup_by_path::<i32>(&path).is_none());
    }

    #[ktest(expect_redundant_test_prefix)]
    fn test_lookup_by_path_pattern() {
        let path1 = path!(a.b[1]);
        let path2 = path!(a.b[2]);
        let path3 = path!(a.c[1]);

        let _queue1 = new_oqueue(&path1);
        let _queue2 = new_oqueue(&path2);
        let _queue3 = new_oqueue(&path3);

        let results = lookup_by_path_pattern::<usize>(&path_pattern!(a.b[*]));
        assert_eq!(results.len(), 2);

        let results = lookup_by_path_pattern::<usize>(&path_pattern!(a.c[*]));
        assert_eq!(results.len(), 1);

        let results = lookup_by_path_pattern::<usize>(&path_pattern!(a.*[*]));
        assert_eq!(results.len(), 3);

        let results = lookup_by_path_pattern::<usize>(&path_pattern!(a.*[1]));
        assert_eq!(results.len(), 2);

        let results = lookup_by_path_pattern::<usize>(&path_pattern!(a.*[2]));
        assert_eq!(results.len(), 1);
    }

    #[ktest(expect_redundant_test_prefix)]
    fn test_lookup_by_type() {
        let path1 = path!(x.y[1]);
        let path2 = path!(z.w[2]);

        let _queue1 = new_oqueue(&path1);
        let _queue2 = new_oqueue(&path2);

        let results = lookup_by_type::<usize>();
        assert_eq!(results.len(), 2);
    }

    #[ktest]
    fn nonexistent_lookup() {
        assert!(lookup_by_path::<usize>(&path!(nonexistent.path[1])).is_none());
        assert!(lookup_by_path_pattern::<usize>(&path_pattern!(nonexistent[*])).is_empty());
        assert!(lookup_by_type::<usize>().is_empty());
    }

    #[derive(serde::Deserialize)]
    struct DecodedRecord {
        seq: u64,
        value: u64,
    }

    /// Decode a self-delimiting CBOR stream of records, as produced by the observer.
    fn decode_records(buf: &[u8]) -> Vec<DecodedRecord> {
        let mut de = minicbor_serde::Deserializer::new(buf);
        let mut records = Vec::new();
        while de.decoder().position() < buf.len() {
            records.push(serde::Deserialize::deserialize(&mut de).unwrap());
        }
        records
    }

    #[ktest]
    fn export_registration_and_streaming() {
        let path = path!(observe.test[1]);
        let queue = ConsumableOQueueRef::<usize>::new(16, path.clone());
        register(&path, &queue.as_any_oqueue());

        // The queue is discoverable by path alone, without naming its message type.
        assert!(list_export_paths().contains(&path));
        let handle = lookup_export(&path).expect("exported OQueue should be found");
        assert!(handle.is_alive());
        assert_eq!(handle.type_name(), core::any::type_name::<usize>());

        // A strong observer sees every value produced after it attaches.
        let observer = handle.attach_strong_observer().unwrap();
        let producer = queue.attach_value_producer().unwrap();
        for value in [10usize, 20, 30] {
            producer.produce(value);
        }

        // Draining yields exactly three records, then reports nothing available.
        let mut buf = Vec::new();
        let mut count = 0;
        while observer.try_next(&mut buf).unwrap() {
            count += 1;
        }
        assert_eq!(count, 3);
        assert!(!observer.try_next(&mut buf).unwrap());

        // The bytes decode as an ordered CBOR record stream with sequence numbers.
        let records = decode_records(&buf);
        assert_eq!(records.len(), 3);
        assert_eq!((records[0].seq, records[0].value), (0, 10));
        assert_eq!((records[1].seq, records[1].value), (1, 20));
        assert_eq!((records[2].seq, records[2].value), (2, 30));
    }

    #[ktest]
    fn export_is_independent_per_open() {
        let path = path!(observe.multi[1]);
        let queue = ConsumableOQueueRef::<usize>::new(16, path.clone());
        register(&path, &queue.as_any_oqueue());
        let handle = lookup_export(&path).unwrap();

        // Two independent observers each receive the full stream.
        let observer_a = handle.attach_strong_observer().unwrap();
        let observer_b = handle.attach_strong_observer().unwrap();
        let producer = queue.attach_value_producer().unwrap();
        for value in [1usize, 2, 3, 4] {
            producer.produce(value);
        }

        let mut buf_a = Vec::new();
        while observer_a.try_next(&mut buf_a).unwrap() {}
        let mut buf_b = Vec::new();
        while observer_b.try_next(&mut buf_b).unwrap() {}

        let records_a = decode_records(&buf_a);
        let records_b = decode_records(&buf_b);
        assert_eq!(records_a.len(), 4);
        assert_eq!(records_b.len(), 4);
        assert_eq!(
            records_a.iter().map(|r| r.value).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert_eq!(
            records_b.iter().map(|r| r.value).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
    }

    #[ktest]
    fn export_one_per_path_and_cleanup() {
        let path = path!(observe.dup[1]);
        let queue1 = ConsumableOQueueRef::<usize>::new(4, path.clone());
        register(&path, &queue1.as_any_oqueue());

        // A second, live export at the same path is rejected (one stream per path).
        let queue2 = ConsumableOQueueRef::<usize>::new(4, path.clone());
        register(&path, &queue2.as_any_oqueue());
        assert_eq!(
            list_export_paths().iter().filter(|p| **p == path).count(),
            1
        );

        // Once the underlying queue is gone the handle is dead and `clean_exports` reaps it.
        drop(queue1);
        drop(queue2);
        assert!(!lookup_export(&path).unwrap().is_alive());
        clean_exports();
        assert!(lookup_export(&path).is_none());
    }

    #[ktest]
    fn register_populates_both_maps() {
        let path = path!(observe.both[1]);
        // The constructor registers to REGISTRY only; the queue is not exported yet.
        let queue = ConsumableOQueueRef::<usize>::new(4, path.clone());
        assert!(lookup_by_path::<usize>(&path).is_some());
        assert!(lookup_export(&path).is_none());

        // `register` adds it to OQUEUES while keeping the REGISTRY entry.
        register(&path, &queue.as_any_oqueue());
        assert!(lookup_by_path::<usize>(&path).is_some());
        assert!(lookup_export(&path).is_some());
    }

    #[ktest]
    fn register_covers_anonymous_queue() {
        let path = path!(observe.anon[1]);
        // An anonymous queue is not registered by its constructor.
        let queue = ConsumableOQueueRef::<usize>::new_anonymous(4);
        assert!(lookup_by_path::<usize>(&path).is_none());

        // `register` puts it in both maps: `ensure_registered` covers REGISTRY.
        register(&path, &queue.as_any_oqueue());
        assert!(lookup_by_path::<usize>(&path).is_some());
        assert!(lookup_export(&path).is_some());
    }

    #[ktest]
    fn export_with_projection() {
        // A message type that is `Copy` but deliberately not `Serialize`.
        #[derive(Clone, Copy)]
        struct Sample(u64);

        let path = path!(observe.proj[1]);
        let queue = ConsumableOQueueRef::<Sample>::new(16, path.clone());
        // Export only the inner value via a projection; `Sample` itself need not be `Serialize`.
        register_with(&path, &queue.as_any_oqueue(), |sample: &Sample| sample.0);

        let handle = lookup_export(&path).unwrap();
        let observer = handle.attach_strong_observer().unwrap();
        let producer = queue.attach_value_producer().unwrap();
        for value in [100u64, 200, 300] {
            producer.produce(Sample(value));
        }

        let mut buf = Vec::new();
        while observer.try_next(&mut buf).unwrap() {}

        let records = decode_records(&buf);
        assert_eq!(
            records.iter().map(|r| r.value).collect::<Vec<_>>(),
            [100, 200, 300]
        );
        assert_eq!(records.iter().map(|r| r.seq).collect::<Vec<_>>(), [0, 1, 2]);
    }
}
