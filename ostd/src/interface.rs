// SPDX-License-Identifier: MPL-2.0
//! An implementable interface that can be mocked to test the OS
use core::marker::PhantomData;

use crate::task::Task;

enum TaskHandle {
    Real(Task),
    // In userspace simulations, the harness will need to define a mapping from task_handles to
    // whatever is used to model a task
    Fake(u64),
}

enum MutexHandle {
    Real(crate::sync::Mutex<()>),
    // In userspace simulations, the harness will need to define a mapping from task_handles to
    // whatever is used to model a task
    Fake(u64),
}

enum LockError {
    WouldBlock,
    OtherError,
}

pub trait OsInterface {
    fn current() -> TaskHandle;
    fn park(task: TaskHandle);
    fn unpark(task: TaskHandle);
    fn set_blocking(task: TaskHandle);
    fn add_task(task: TaskHandle);
    fn remove_task(task: TaskHandle);
    fn wake_all(task: TaskHandle);

    fn lock(lock: &MutexHandle) -> Result<(), LockError>;
    fn try_lock(lock: &MutexHandle) -> Result<(), LockError>;
    fn unlock(lock: &MutexHandle) -> Result<(), LockError>;
}

pub struct MutexGuard<'a, OsI: OsInterface, T: ?Sized> {
    m: &'a MutexHandle,
    value: &'a T,
    phantom: PhantomData<OsI>,
}

impl<OsI: OsInterface, T: ?Sized> Drop for MutexGuard<'_, OsI, T> {
    fn drop(&mut self) {
        let _ = OsI::unlock(&self.m);
    }
}

impl<OsI: OsInterface, T: ?Sized> Deref for MutexGuard_<T, R> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

pub struct Mutex<OsI: OsInterface, T> {
    m: MutexHandle,
    value: T,
    phantom: PhantomData<OsI>,
}

pub impl<OsI: OsInterface, T> Mutex<OsI, T> {
    fn lock(&mut self) -> Result<MutexGuard<OsI, T>, LockError> {
        OsI::lock(&self.m)?;
        return Ok(MutexGuard {
            m: &self.m,
            value: &self.value,
            phantom: PhantomData,
        });
    }
}
