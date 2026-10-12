//! Generated-file assets kept out of tool results.
//!
//! Returning base64 blobs through a model is expensive, so tools will place
//! generated files (`PNG`, `WAV`, `MIDI`, raw lumps, big JSON) into an
//! in-memory store and hand the client a short `URL` instead; a localhost
//! `HTTP` server (next ticket) serves these entries. This module is only
//! the store.

pub mod store;
