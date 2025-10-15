//! Registry of named OQueues

use alloc::string::{String, ToString};
use core::any::{Any, TypeId};

use hashbrown::HashMap;

use super::OQueueRef;
use crate::{
    prelude::Box,
    sync::{Mutex, MutexGuard},
};

type Registry = Mutex<Option<HashMap<(String, TypeId), Box<dyn Any + Send + Sync + 'static>>>>;

static REGISTRY: Registry = Mutex::new(None);

/// Get a reference to the global registry object, initializing it if needed.
pub fn registry()
-> MutexGuard<'static, Option<HashMap<(String, TypeId), Box<dyn Any + Send + Sync + 'static>>>> {
    let mut guard = REGISTRY.lock();
    if let None = *guard {
        guard.replace(HashMap::new());
    }

    guard
}

/// Register a OQueue with name `name`
pub fn register<T: 'static>(name: &str, v: OQueueRef<T>) {
    let key = (name.to_string(), TypeId::of::<T>());
    let mut map = registry();
    map.as_mut().unwrap().insert(key, Box::new(v));
}

/// CHeck the registry for an OQueue with name `name`
pub fn lookup<T: 'static>(name: &str) -> Option<OQueueRef<T>> {
    let key = (name.to_string(), TypeId::of::<T>());
    let mut map = registry();
    let value = map.as_mut().unwrap().get(&key)?;
    let value: &OQueueRef<T> = value.downcast_ref()?;
    Some(value.clone())
}
