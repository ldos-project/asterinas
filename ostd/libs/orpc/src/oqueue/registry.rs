use alloc::{boxed::Box, string::String};
use core::any::{Any, TypeId};

use hashbrown::HashMap;
use snafu::Snafu;
use spin::Once;

use crate::{oqueue::OQueueRef, sync::mutex::Mutex};

type RegistryInner = HashMap<(String, TypeId), Box<dyn Any + Send + Sync + 'static>>;
type Registry<M: MutexImpl<RegistryInner>> = Mutex<RegistryInner, M>;

static REGISTRY: Once<Registry<Box<dyn MutexImpl<RegistryInner>>>> = Once::new();

pub fn initialize_registry() {
    REGISTRY.call_once(|| Registry::default());
}

pub fn register<T: 'static>(name: &str, v: OQueueRef<T>) {
    let key = (name.to_string(), TypeId::of::<T>());
    let mut map = REGISTRY.lock();
    map.insert(key, Box::new(v));
}

pub fn lookup<T: 'static>(name: &str) -> Option<OQueueRef<T>> {
    let key = (name.to_string(), TypeId::of::<T>());
    let map = REGISTRY.lock();
    let value = map.get(&key)?;
    let value: &OQueueRef<T> = value.downcast_ref()?;
    Some(value.clone())
}

// #[derive(Debug, Snafu)]
// pub enum RegistryError {
//     #[snafu(display("Key not found: {}", name))]
//     KeyNotFound { name: String },

//     #[snafu(display("Failed to downcast value: {}", type_name))]
//     DowncastError { type_name: String },
// }
