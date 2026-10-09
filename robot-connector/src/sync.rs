use std::sync::{Mutex, MutexGuard};

/// Recovers the owned value after a thread panic while keeping call sites
/// explicit about the synchronization primitive they are locking.
pub(crate) trait MutexRecovery<T> {
    fn recover_lock(&self) -> MutexGuard<'_, T>;
}

impl<T> MutexRecovery<T> for Mutex<T> {
    fn recover_lock(&self) -> MutexGuard<'_, T> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
