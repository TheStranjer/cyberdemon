//! Generated-file assets kept out of tool results.
//!
//! Returning base64 blobs through a model is expensive, so tools place
//! generated files (`PNG`, `WAV`, `MIDI`, raw lumps, big JSON) into an
//! in-memory store and hand the client a short `URL` instead. [`store`]
//! holds the bytes; [`server`] is a localhost `HTTP` server that serves
//! them to the client and accepts uploads through [`AppState::publish`];
//! [`fetch`] does the reverse, turning client-supplied `source` strings
//! into bytes for import tools.
//!
//! [`AppState::publish`]: crate::state::AppState::publish

pub mod fetch;
pub mod server;
pub mod store;
