//! Shared server state: the editing session behind a lock.

use std::sync::Mutex;

use crate::error::CyberError;
use crate::session::Session;

/// Application state owned by one server connection and shared by its tools.
///
/// All session access goes through [`AppState::with_session`], which keeps the
/// lock discipline in one place.
#[derive(Debug, Default)]
pub struct AppState {
    /// The editing session, guarded for interior mutability.
    session: Mutex<Session>,
}

impl AppState {
    /// Run `f` with exclusive access to the editing session.
    ///
    /// The lock is held only for the duration of the synchronous closure:
    /// never hold a session across an `.await`, and do the asynchronous work
    /// (if any) before or after calling this.
    ///
    /// # Errors
    ///
    /// [`CyberError::Internal`] with the message `"session lock poisoned"` if
    /// the session lock was poisoned by a panicking thread; otherwise the
    /// closure's own result is returned unchanged.
    #[inline]
    pub fn with_session<T, F>(&self, f: F) -> Result<T, CyberError>
    where
        F: FnOnce(&mut Session) -> Result<T, CyberError>,
    {
        // `.ok()` discards the PoisonError payload on purpose: the payload
        // carries no extra information here, a poisoned session is fatal for
        // this connection either way.
        let Some(mut session) = self.session.lock().ok() else {
            return Err(CyberError::Internal("session lock poisoned".to_owned()));
        };
        f(&mut session)
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use std::sync::MutexGuard;

    use super::AppState;
    use crate::error::CyberError;
    use crate::session::Session;

    /// Lock the session and panic while the guard is borrowed, so the
    /// unwinding drop poisons the mutex.
    fn poison_holding_lock(holder: &AppState) {
        let guard: MutexGuard<'_, Session> = holder.session.lock().expect("fresh lock");
        poison_while_held(&guard)
    }

    /// Panic while a reference into the locked session is alive.
    fn poison_while_held(held: &Session) -> ! {
        panic!("poisoning the session lock while holding {held:?}")
    }

    #[test]
    fn with_session_runs_the_closure_on_the_default_session() {
        let state = AppState::default();
        let shown = state
            .with_session(|session: &mut Session| Ok(format!("{session:?}")))
            .expect("a fresh lock is never poisoned");
        assert_eq!(
            shown, "Session",
            "a fresh connection gets a default session"
        );
    }

    #[test]
    fn with_session_propagates_the_closure_error() {
        let state = AppState::default();
        let error = state
            .with_session(|_| {
                Err::<(), CyberError>(CyberError::NotFound("nothing here".to_owned()))
            })
            .expect_err("the closure error must surface unchanged");
        assert_eq!(error.code(), "not_found", "closure error kind preserved");
        assert_eq!(error.to_string(), "nothing here", "closure error message");
    }

    #[test]
    fn with_session_reports_a_poisoned_lock_as_internal() {
        let state = Arc::new(AppState::default());
        let holder = Arc::clone(&state);
        let poisoning = std::thread::spawn(move || poison_holding_lock(&holder));
        assert!(
            poisoning.join().is_err(),
            "the holder thread must have panicked"
        );
        let error = state
            .with_session(|_| Ok(()))
            .expect_err("a poisoned lock must not hand out a session");
        assert!(
            matches!(&error, CyberError::Internal(message) if message.as_str() == "session lock poisoned"),
            "fixed poison message expected, got {error:?}"
        );
        assert_eq!(error.code(), "internal", "poison maps to the internal code");
    }
}
