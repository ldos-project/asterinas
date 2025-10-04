struct OsInterface {}

/*
impl ostd::interface::OsInterface for OsInterface {
    fn current() -> ostd::interface::TaskHandle {
        ostd::interface::TaskHandle::Real(ostd::task::Task::current())
    }

    fn park(handle: ostd::interface::TaskHandle) {}

    fn unpark(handle: ostd::interface::TaskHandle) {}

    fn set_blocking(handle: ostd::interface::TaskHandle) {}
    fn add_task(handle: ostd::interface::TaskHandle) {}
    fn remove_task(handle: ostd::interface::TaskHandle) {}
    fn wake_all() {}

    fn lock(m: ostd::interface::MutexHandle) -> Result<(), ostd::interface::LockError> {
        match m {
            ostd::interface::MutexHandle::Real(m) => {
                // TODO :- don't lock and then drop the lock!
                m.lock();
                Ok(())
            }
            _ => ostd::interface::LockError,
        }
    }
    fn try_lock(m: ostd::interface::MutexHandle) -> Result<(), ostd::interface::LockError> {
        match m {
            ostd::interface::MutexHandle::Real(m) => {
                // TODO :- don't lock and then drop the lock!
                m.try_lock()
                    .map_err(|| ostd::interface::LockError::WouldBlock)?;
                Ok(())
            }
            _ => ostd::interface::LockError,
        }
    }
    fn unlock(m: ostd::interface::MutexHandle) -> Result<(), ostd::interface::LockError> {
        Ok(())
    }
}
*/
