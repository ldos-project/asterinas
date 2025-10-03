use alloc::boxed::Box;
use core::{default::Default, fmt::Debug, panic, prelude::rust_2024::derive};

use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(display("Mutex would have blocked"))]
struct WouldBlockError {}

pub trait MutexImpl<T> {
    type MutexGuard;

    fn lock<R>(&mut self) -> Result<Self::MutexGuard, Box<dyn snafu::Error>>;
    fn try_lock<R>(&mut self) -> Result<Self::MutexGuard, Box<dyn snafu::Error>>;
    fn new(v: T) -> Self;
}

/// A wrapper around std::sync::Mutex with panicking poisoning to match Mariposa.
#[derive(Default)]
pub struct Mutex<T, M: MutexImpl<T>>(M);

impl<T, M: MutexImpl<T>> Mutex<T, M> {
    pub fn lock(&self) -> M::MutexGuard {
        self.0.lock().unwrap()
    }

    pub fn try_lock(&self) -> Option<M::MutexGuard> {
        match self.0.try_lock() {
            Ok(g) => Some(g),
            Err(WouldBlockError {}) => None,
            Err(_) => panic!("Mutex poisoned"),
        }
    }

    pub fn new(v: T) -> Self {
        Self(M::new(v))
    }
}

impl<T: Debug, M: Debug + MutexImpl<T>> Debug for Mutex<T, M> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}
