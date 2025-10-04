use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::LazyLock,
};

use snafu::Snafu;

use crate::{oqueue::OQueueRef, sync::Mutex};

type Registry = Mutex<HashMap<(String, TypeId), Box<dyn Any + Send + Sync + 'static>>>;

static REGISTRY: LazyLock<Registry> = LazyLock::new(|| Registry::default());

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
