//! The session's read cache: the merged directory, its classification, the
//! layers-only view and the session definitions, each held in a [`Slot`]
//! keyed by the working revision and the layer generation it was built at.
//!
//! Every read view of [`Session`](crate::session) is a pure function of the
//! layer stack and the working archive, and both change only through the
//! session's own `&mut self` steps: an edit (or batch fold) moves
//! [`History::revision`](super::history::History::revision), a base swap or
//! a resource push bumps the [generation](ViewCache::bump_generation). A
//! [`Key`] therefore names one exact session state: the slots serve while
//! the key holds and rebuild on the first read at a new one, so
//! [`Session::edit`], [`Session::undo`], [`Session::redo`],
//! [`Session::set_iwad`], [`Session::add_resource`] and
//! [`Session::replace_working`] need nothing beyond the revision/generation
//! bookkeeping the mutators already do - the mutators additionally call
//! [`ViewCache::invalidate`] so a forgotten bookkeeping change could never
//! serve a stale view.
//!
//! The slots fill through `&self` reads (the session's whole read API), so
//! the filled value lives in a [`core::cell::Cell`]: a slot is moved out,
//! read by clone and moved back, never mutably borrowed, so the fill cannot
//! panic on a borrow conflict the way a `RefCell` can, and no `OnceCell`
//! (single-shot, wrong for a re-keyed cache) is needed. Revision counters
//! never rewind, so a key is never reused for different content.
//!
//! The cache is session machinery, never session state: cloning a session
//! starts cold, a session's `Debug` shows only warm/cold, and equality
//! ignores the slots entirely, so a warm cache cannot make two sessions
//! with the same archives compare unequal.

use core::cell::Cell;
use core::fmt;

use crate::defs::Definitions;
use crate::session::MergedLump;
use crate::wad::classify::Analysis;

/// The session state a cached read belongs to: the working archive's
/// revision and the layer-stack generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session) struct Key {
    /// The [`History::revision`](super::history::History::revision) the
    /// working archive stood at.
    pub(in crate::session) revision: u64,
    /// The layer generation the stack stood at.
    pub(in crate::session) generation: u64,
}

/// One cached value with the key it was built at, in a `Cell`.
///
/// A slot serves a hit by moving the value out, cloning it out and moving
/// it back, so reads take `&self` and nothing here can panic on a borrow
/// conflict.
pub(in crate::session) struct Slot<T> {
    /// The key [`Self::value`] was built at, `None` while the slot is cold.
    key: Cell<Option<Key>>,
    /// The cached value, moved out only inside a clone or a fill.
    value: Cell<Option<T>>,
}

impl<T> Slot<T> {
    /// An empty slot.
    const fn empty() -> Self {
        Self {
            key: Cell::new(None),
            value: Cell::new(None),
        }
    }

    /// Whether `key` matches the slot's key and a value stands behind it.
    fn warm_at(&self, key: Key) -> bool {
        matches!(self.key.get(), Some(held) if held == key)
    }

    /// The held value when the slot is warm at `key`, leaving the slot as
    /// it was.
    fn peek(&self, key: Key) -> Option<T>
    where
        T: Clone,
    {
        if !self.warm_at(key) {
            return None;
        }
        let held = self.value.take();
        let found = held.clone();
        self.value.set(held);
        found
    }

    /// Stores `value` as the slot's answer for `key`.
    fn put(&self, key: Key, value: &T)
    where
        T: Clone,
    {
        self.key.set(Some(key));
        self.value.set(Some(value.clone()));
    }

    /// Drops whatever the slot holds.
    pub(in crate::session) fn invalidate(&self) {
        self.key.set(None);
        self.value.set(None);
    }

    /// The cached value at `key`; `init` builds it on a miss and the slot
    /// keeps it for every later read at the same key.
    pub(in crate::session) fn get_or_init<F>(&self, key: Key, init: F) -> T
    where
        F: FnOnce() -> T,
        T: Clone,
    {
        if let Some(found) = self.peek(key) {
            return found;
        }
        let fresh = init();
        self.put(key, &fresh);
        fresh
    }

    /// [`Self::get_or_init`] for a fallible `init`: an error is reported
    /// unchanged, nothing is stored and the next read runs `init` again.
    pub(in crate::session) fn get_or_try_init<F, E>(&self, key: Key, init: F) -> Result<T, E>
    where
        F: FnOnce() -> Result<T, E>,
        T: Clone,
    {
        if let Some(found) = self.peek(key) {
            return Ok(found);
        }
        let fresh = init()?;
        self.put(key, &fresh);
        Ok(fresh)
    }
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> Clone for Slot<T> {
    /// Cloning a slot hands out a fresh cold one: a cache is never shared
    /// through a session clone, and the clone's first read rebuilds.
    fn clone(&self) -> Self {
        Self::empty()
    }
}

impl<T> fmt::Debug for Slot<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let warm = self.key.get().is_some();
        write!(f, "Slot({})", if warm { "warm" } else { "cold" })
    }
}

impl<T> PartialEq for Slot<T> {
    /// Slots carry no session state: any two of them compare equal, so a
    /// warm cache never makes two equal sessions compare unequal.
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<T> Eq for Slot<T> {}

/// The session's four keyed reads plus the layer generation they are keyed
/// with. See the [module](self) documentation.
#[derive(Default)]
pub(in crate::session) struct ViewCache {
    /// Bumped by every base swap and resource push; part of every key.
    generation: Cell<u64>,
    /// The merged directory: layers in load order plus the working archive.
    pub(in crate::session) merged: Slot<Vec<MergedLump>>,
    /// The classification of [`Self::merged`]'s directory.
    pub(in crate::session) analysis: Slot<Analysis>,
    /// The merged view of the read-only layers alone.
    pub(in crate::session) base: Slot<Vec<MergedLump>>,
    /// The session's definitions at the current key.
    pub(in crate::session) definitions: Slot<Definitions>,
    /// How many times a merged directory was (re)built through this cache;
    /// the cache tests read it.
    #[cfg(test)]
    rebuilds: Cell<u64>,
    /// How many times the definitions were (re)built through this cache;
    /// the cache tests read it.
    #[cfg(test)]
    definition_rebuilds: Cell<u64>,
}

impl ViewCache {
    /// The key a read taken while the working archive stood at `revision`
    /// belongs to.
    pub(in crate::session) fn key(&self, revision: u64) -> Key {
        Key {
            revision,
            generation: self.generation.get(),
        }
    }

    /// Records that the layer stack changed: the generation moves past
    /// every key built from the old stack and everything cached is dropped.
    pub(in crate::session) fn bump_generation(&self) {
        self.generation.set(self.generation.get().saturating_add(1));
        self.invalidate();
    }

    /// Drops every cached read; the next read at the current key rebuilds.
    pub(in crate::session) fn invalidate(&self) {
        self.merged.invalidate();
        self.analysis.invalidate();
        self.base.invalidate();
        self.definitions.invalidate();
    }

    /// Counts one merged-directory rebuild (cache tests only).
    #[cfg(test)]
    pub(in crate::session) fn count_rebuild(&self) {
        self.rebuilds.set(self.rebuilds.get().saturating_add(1));
    }

    /// Counts one definitions rebuild (cache tests only).
    #[cfg(test)]
    pub(in crate::session) fn count_definition_rebuild(&self) {
        self.definition_rebuilds
            .set(self.definition_rebuilds.get().saturating_add(1));
    }

    /// How many merged directories this cache has built (cache tests only).
    #[cfg(test)]
    pub(in crate::session) fn rebuild_count(&self) -> u64 {
        self.rebuilds.get()
    }

    /// How many definition sets this cache has built (cache tests only).
    #[cfg(test)]
    pub(in crate::session) fn definition_rebuild_count(&self) -> u64 {
        self.definition_rebuilds.get()
    }
}

impl Clone for ViewCache {
    /// A cloned cache is cold: it keeps the layer generation (a key must
    /// not repeat for different content) and holds no built value.
    fn clone(&self) -> Self {
        Self {
            generation: Cell::new(self.generation.get()),
            merged: Slot::default(),
            analysis: Slot::default(),
            base: Slot::default(),
            definitions: Slot::default(),
            #[cfg(test)]
            rebuilds: Cell::new(0_u64),
            #[cfg(test)]
            definition_rebuilds: Cell::new(0_u64),
        }
    }
}

impl fmt::Debug for ViewCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut built = f.debug_struct("ViewCache");
        built
            .field("generation", &self.generation.get())
            .field("merged", &self.merged)
            .field("analysis", &self.analysis)
            .field("base", &self.base)
            .field("definitions", &self.definitions);
        #[cfg(test)]
        built
            .field("rebuilds", &self.rebuilds.get())
            .field("definition_rebuilds", &self.definition_rebuilds.get());
        built.finish()
    }
}

impl PartialEq for ViewCache {
    /// The cache mirrors other state and never carries its own: any two
    /// caches of sessions whose archives compare equal are equal.
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for ViewCache {}
