use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard};

static LOCK: Mutex<()> = Mutex::new(());

/// A test's exclusive hold on the process environment: every variable it
/// sets or removes goes back to its prior value when the guard drops.
///
/// Take it before the code under test reads the environment, and before the
/// test spawns any thread that could.
#[must_use = "the lock and every restore end when the guard drops"]
pub struct EnvGuard {
    saved: Vec<(OsString, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Waits for every other guard in this process to drop.
    pub fn lock() -> Self {
        Self {
            saved: Vec::new(),
            // A poisoned lock is still consistent: the panicking test's guard
            // restored its variables while unwinding.
            _lock: LOCK.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }

    /// Sets `key` to `value` until the guard drops.
    pub fn set(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
        let key = key.as_ref();
        self.save(key);
        write(key, Some(value.as_ref()));
    }

    /// Unsets `key` until the guard drops.
    pub fn remove(&mut self, key: impl AsRef<OsStr>) {
        let key = key.as_ref();
        self.save(key);
        write(key, None);
    }

    /// Only a key's first change records its prior value, which is the one
    /// the drop restores.
    fn save(&mut self, key: &OsStr) {
        if !self.saved.iter().any(|(k, _)| k == key) {
            self.saved.push((key.to_owned(), std::env::var_os(key)));
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prior) in std::mem::take(&mut self.saved) {
            write(&key, prior.as_deref());
        }
    }
}

fn write(key: &OsStr, value: Option<&OsStr>) {
    // SAFETY: only an `EnvGuard` calls this, so `LOCK` is held and no other
    // guard writes concurrently; and `just test` runs under nextest, which gives
    // every test its own process, so no other test's thread reads the
    // environment meanwhile.
    unsafe {
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set in every test process, so it has a prior value to restore.
    const SET: &str = "PATH";
    const UNSET: &str = "PIXTUOID_TEST_ENV_GUARD_UNSET";

    #[test]
    fn the_drop_restores_each_key_to_its_value_before_the_first_change() {
        let mut env = EnvGuard::lock();
        let prior = std::env::var_os(SET).expect("PATH is set");
        env.remove(SET);
        env.set(SET, "/first");
        env.set(UNSET, "now set");
        env.remove(UNSET);
        env.set(UNSET, "set again");
        drop(env);

        let _env = EnvGuard::lock();
        assert_eq!(std::env::var_os(SET), Some(prior));
        assert_eq!(std::env::var_os(UNSET), None);
    }

    #[test]
    fn a_panicking_holder_restores_while_unwinding_and_frees_the_lock() {
        let held = std::panic::catch_unwind(|| {
            let mut env = EnvGuard::lock();
            env.set(UNSET, "leaked?");
            panic!("the test under the guard fails");
        });
        assert!(held.is_err());

        let _env = EnvGuard::lock();
        assert_eq!(std::env::var_os(UNSET), None);
    }
}
