//! A set of utilities for concurrency and synchronization which act similarly to those that will be available in the
//! Asterinas kernel.
//! 
//! NOTE: These are testing stubs and will be replaced by real implementations in most cases. These implementations or
//! similar may be useful for user-space testing of kernel code. Related:
//! https://github.com/ldos-project/asterinas/issues/26

use std::sync::TryLockError;

use alloc::fmt::Debug;

pub mod blocker;
pub mod task;

/// A wrapper around std::sync::Mutex with panicking poisoning to match Mariposa.
#[derive(Default)]
pub struct Mutex<T>(std::sync::Mutex<T>);

impl<T> Mutex<T> {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }

    pub fn try_lock(&self) -> Option<std::sync::MutexGuard<'_, T>> {
        match self.0.try_lock() {
            Ok(g) => Some(g),
            Err(TryLockError::WouldBlock) => None,
            Err(_) => panic!("Mutex poisoned"),
        }
    }

    pub fn new(v: T) -> Self {
        Self(std::sync::Mutex::new(v))
    }
}

impl<T: Debug> Debug for Mutex<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
