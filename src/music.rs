//! Music core: `Doom` `MUS` scores and the `Standard MIDI File` they convert
//! to.
//!
//! Pure data processing on owned buffers, like [`crate::audio`]:
//! [`mus2mid`] decodes a `Doom` `D_*` music lump and [`midi_write`] writes
//! the `SMF` bytes it produces (and the reverse converter will). The
//! `MIDI` side is the interchange format every music tool publishes, so the
//! writer is shared rather than inlined into the converter. Nothing here
//! knows about MCP, lumps or the filesystem, and all arithmetic is
//! integer-only (see [`crate::fixed`]).

pub mod midi_write;
pub mod mus2mid;

pub use midi_write::{MidiWriter, read_vlq, write_vlq};
pub use mus2mid::mus_to_midi;
