use alloc::string::{String, ToString};
use core::any::{Any, TypeId};

use hashbrown::HashMap;
use snafu::Snafu;
use spin::Once;

use super::OQueueRef;
use crate::{
    prelude::{Arc, Box},
    sync::Mutex,
};

type Registry = Mutex<HashMap<(String, TypeId), Box<dyn Any + Send + Sync + 'static>>>;

static REGISTRY: Once<Registry> = Once::new();

pub fn init() {
    REGISTRY.call_once(|| Registry::default());
}

pub fn register<T: 'static>(name: &str, v: OQueueRef<T>) {
    let key = (name.to_string(), TypeId::of::<T>());
    let mut map = REGISTRY.get().unwrap().lock();
    map.insert(key, Box::new(v));
}

pub fn lookup<T: 'static>(name: &str) -> Option<OQueueRef<T>> {
    let key = (name.to_string(), TypeId::of::<T>());
    let map = REGISTRY.get().unwrap().lock();
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
