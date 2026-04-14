// SPDX-License-Identifier: MPL-2.0
//! Registry of named OQueues

use alloc::{boxed::Box, vec::Vec};
use core::any::{Any, TypeId};

use hashbrown::{
    HashMap,
    hash_map::{DefaultHashBuilder, EntryRef},
};
use log::warn;

use super::AnyOQueueRef;
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

/// Register a OQueue with `path`.
///
/// If an existing OQueue is registered with the same type and path, this will replace that OQueue
/// and emit a warning.
pub fn register<T: ?Sized + 'static>(path: &Path, v: AnyOQueueRef<T>) {
    let mut map = registry();
    let entry = get_entry::<T>(&mut map, path);
    if let EntryRef::Occupied(entry) = &entry
        && downcast_oqueue::<T>(entry.get()).upgrade().is_some()
    {
        warn!("Overwriting OQueue registry entry with path {}", path);
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
pub fn lookup_by_type<T: ?Sized + 'static>() -> Vec<(Path, AnyOQueueRef<T>)> {
    let mut map = registry();
    let type_map = get_type_map::<T>(&mut map);

    type_map
        .map
        .iter()
        .filter_map(|(path, value)| {
            Some((
                path.clone(),
                value
                    .downcast_ref::<WeakAnyOQueueRef<T>>()
                    .unwrap()
                    .upgrade()?,
            ))
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
    use crate::{orpc::oqueue::ConsumableOQueueRef, path, path_pattern, prelude::*};

    fn new_oqueue(path: &Path) -> ConsumableOQueueRef<usize> {
        ConsumableOQueueRef::<usize>::new(4, path.clone())
    }

    #[ktest]
    fn test_lookup_by_path() {
        let path = path!(test.queue[1]);
        let _queue = new_oqueue(&path);

        assert!(lookup_by_path::<usize>(&path).is_some());
        assert!(lookup_by_path::<i32>(&path).is_none());
    }

    #[ktest]
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

    #[ktest]
    fn test_lookup_by_type() {
        let path1 = path!(x.y[1]);
        let path2 = path!(z.w[2]);

        let _queue1 = new_oqueue(&path1);
        let _queue2 = new_oqueue(&path2);

        let results = lookup_by_type::<usize>();
        assert_eq!(results.len(), 2);
    }

    #[ktest]
    fn test_nonexistent_lookup() {
        assert!(lookup_by_path::<usize>(&path!(nonexistent.path[1])).is_none());
        assert!(lookup_by_path_pattern::<usize>(&path_pattern!(nonexistent[*])).is_empty());
        assert!(lookup_by_type::<usize>().is_empty());
    }
}
