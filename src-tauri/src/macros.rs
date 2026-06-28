//! Crate-wide macros.

/// Recover from a poisoned Mutex lock instead of panicking.
///
/// When a thread panics while holding a Mutex, the lock becomes "poisoned".
/// By default, `.lock()` on a poisoned Mutex returns `Err(PoisonError)`.
/// Using `.expect("X poisoned")` panics on poison, which causes cascade crashes
/// if one slot's panic poisons a lock shared across the app.
///
/// This macro recovers the inner guard from the poison error, logs a warning,
/// and continues operation. The data inside the Mutex may be in an inconsistent
/// state, but that is preferable to crashing the entire application.
///
/// # Usage
/// ```ignore
/// let guard = recover_lock!(self.label.lock(), "label");
/// ```
#[macro_export]
macro_rules! recover_lock {
    ($lock:expr, $name:literal) => {
        $lock.unwrap_or_else(|e| {
            log::error!("[lock] {} poisoned, recovering", $name);
            e.into_inner()
        })
    };
}
