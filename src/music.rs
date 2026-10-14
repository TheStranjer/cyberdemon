//! Music core: `Doom` `MUS` scores and the `Standard MIDI File` that
//! converts to and from them.
//!
//! Pure data processing on owned buffers, like [`crate::audio`]:
//! [`sniff`] names the container a buffer of music bytes came from;
//! [`mus2mid`] decodes a `Doom` `D_*` music lump and [`midi_write`] writes
//! the `SMF` bytes it produces; [`midi_read`] parses a `Standard MIDI File`
//! and [`mid2mus`] writes the `MUS` lump equivalent. The `MIDI` side is the
//! interchange format every music tool publishes, so the writer, the parser
//! and the `VLQ` helpers are shared rather than inlined. Nothing here knows
//! about MCP, lumps or the filesystem, and all arithmetic is integer-only
//! (see [`crate::fixed`]).

pub mod mid2mus;
pub mod midi_read;
pub mod midi_write;
pub mod mus2mid;
pub mod sniff;

pub use mid2mus::{MusConversion, midi_to_mus};
pub use midi_read::{MidiEvent, MidiFile, TimedEvent, parse_midi};
pub use midi_write::{MidiWriter, read_vlq, write_vlq};
pub use mus2mid::mus_to_midi;
pub use sniff::{MusicKind, sniff_music};
