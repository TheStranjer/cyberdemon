//! Shared server state: the editing session and the asset store behind locks.

use std::sync::Mutex;

use crate::assets::store::AssetStore;
use crate::error::CyberError;
use crate::session::Session;

/// Application state owned by one server connection and shared by its tools.
///
/// All state access goes through [`AppState::with_session`] and
/// [`AppState::with_assets`], which keep the lock discipline in one place.
#[derive(Debug)]
pub struct AppState {
    /// The editing session, guarded for interior mutability.
    session: Mutex<Session>,
    /// The generated-asset store, guarded for interior mutability.
    assets: Mutex<AssetStore>,
}

impl Default for AppState {
    /// Builds the per-connection state: a fresh session and an asset store
    /// sized from `CYBERDEMON_ASSET_CAPACITY` (see [`AssetStore::from_env`]).
    #[inline]
    fn default() -> Self {
        Self {
            session: Mutex::new(Session),
            assets: Mutex::new(AssetStore::from_env()),
        }
    }
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

    /// Run `f` with exclusive access to the generated-asset store.
    ///
    /// The lock is held only for the duration of the synchronous closure:
    /// never hold the store across an `.await`, and do the asynchronous work
    /// (if any) before or after calling this.
    ///
    /// # Errors
    ///
    /// [`CyberError::Internal`] with the message `"assets lock poisoned"` if
    /// the assets lock was poisoned by a panicking thread; otherwise the
    /// closure's own result is returned unchanged.
    #[inline]
    pub fn with_assets<T, F>(&self, f: F) -> Result<T, CyberError>
    where
        F: FnOnce(&mut AssetStore) -> Result<T, CyberError>,
    {
        // The same poison-payload reasoning as [`AppState::with_session`].
        let Some(mut assets) = self.assets.lock().ok() else {
            return Err(CyberError::Internal("assets lock poisoned".to_owned()));
        };
        f(&mut assets)
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use std::sync::MutexGuard;

    use super::AppState;
    use crate::assets::store::AssetStore;
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

    /// Lock the assets store and panic while the guard is borrowed, so the
    /// unwinding drop poisons that mutex.
    fn poison_assets_holding_lock(holder: &AppState) {
        let guard: MutexGuard<'_, AssetStore> = holder.assets.lock().expect("fresh lock");
        poison_assets_while_held(&guard)
    }

    /// Panic while a reference into the locked asset store is alive.
    fn poison_assets_while_held(held: &AssetStore) -> ! {
        panic!("poisoning the assets lock while holding {held:?}")
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

    #[test]
    fn with_assets_runs_the_closure_on_the_default_store() {
        let state = AppState::default();
        let report = state
            .with_assets(|store: &mut AssetStore| Ok((store.len(), store.capacity())))
            .expect("a fresh lock is never poisoned");
        assert_eq!(report.1, 50, "an unset variable gives the default capacity");
        assert_eq!(report.0, 0, "a fresh connection starts with an empty store");
    }

    #[test]
    fn with_assets_mutates_the_shared_store() {
        let state = AppState::default();
        let id = state
            .with_assets(|store: &mut AssetStore| {
                Ok(store
                    .insert("note.txt", "text/plain", vec![b'x'])
                    .expect("a one-byte insert succeeds")
                    .id)
            })
            .expect("a fresh lock is never poisoned");
        let seen = state
            .with_assets(|store: &mut AssetStore| {
                Ok(store.peek(&id).map(|asset| asset.filename.clone()))
            })
            .expect("the second call takes the lock again");
        assert_eq!(seen, Some("note.txt".to_owned()), "the store is shared");
    }

    #[test]
    fn with_assets_propagates_the_closure_error() {
        let state = AppState::default();
        let error = state
            .with_assets(|_| {
                Err::<(), CyberError>(CyberError::NotFound("no such asset".to_owned()))
            })
            .expect_err("the closure error must surface unchanged");
        assert_eq!(error.code(), "not_found", "closure error kind preserved");
        assert_eq!(error.to_string(), "no such asset", "closure error message");
    }

    #[test]
    fn with_assets_reports_a_poisoned_lock_as_internal() {
        let state = Arc::new(AppState::default());
        let holder = Arc::clone(&state);
        let poisoning = std::thread::spawn(move || poison_assets_holding_lock(&holder));
        assert!(
            poisoning.join().is_err(),
            "the holder thread must have panicked"
        );
        let error = state
            .with_assets(|_| Ok(()))
            .expect_err("a poisoned lock must not hand out the store");
        assert!(
            matches!(&error, CyberError::Internal(message) if message.as_str() == "assets lock poisoned"),
            "fixed poison message expected, got {error:?}"
        );
        assert_eq!(error.code(), "internal", "poison maps to the internal code");
    }
}
