//! Music file identification from the first bytes of a file.
//!
//! [`sniff_music`] answers a [`MusicKind`] purely from magic numbers, the
//! same way [`crate::audio::decode::sniff_format`] names sound containers.
//! Tracker modules and `MUS` are recognised by their signatures; no parsing
//! happens here, so a recognised kind can still be a corrupt file - the
//! import tools validate it after sniffing.

use serde::Serialize;

/// Offset of the `SCRM` signature in an `S3M` module.
const S3M_TAG_OFFSET: usize = 44;
/// Offset of the four-byte channel tag in a `ProTracker` `MOD` module.
const MOD_TAG_OFFSET: usize = 1080;
/// `RIFF` form-type field offset in a `WAV` file.
const RIFF_FORM_OFFSET: usize = 8;
/// High byte of an `MPEG` audio frame sync.
const SYNC_HIGH: u8 = 0xFF_u8;
/// Mask of the three sync bits in the second byte of an `MPEG` frame.
const SYNC_MASK: u8 = 0xE0_u8;

/// What kind of music file a buffer holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MusicKind {
    /// A `Doom` `MUS` score: starts with `MUS` and `0x1A`.
    Mus,
    /// A `Standard MIDI File`: starts with `MThd`.
    Midi,
    /// An `Ogg` container: starts with `OggS`.
    Ogg,
    /// An `MP3`: an `ID3` tag or a bare `MPEG` frame sync.
    Mp3,
    /// A `FLAC` stream: starts with `fLaC`.
    Flac,
    /// A `RIFF`/`WAVE` file.
    Wav,
    /// A `FastTracker` II module: starts with `Extended Module:`.
    Xm,
    /// An Impulse Tracker module: starts with `IMPM`.
    It,
    /// A Scream Tracker 3 module: `SCRM` at offset 44.
    S3m,
    /// A `ProTracker` module: a channel tag at offset 1080.
    Mod,
    /// None of the recognised music formats.
    Unknown,
}

impl MusicKind {
    /// Whether a file of this kind can be stored as a `MUS` lump: only
    /// `MUS` itself and `MIDI`, which converts to it, qualify.
    #[must_use]
    #[inline]
    pub const fn convertible_to_mus(self) -> bool {
        matches!(self, Self::Mus | Self::Midi)
    }
}

/// A magic at byte 0 paired with the kind it identifies, checked in order.
const PREFIX_MAGICS: [(&[u8], MusicKind); 7] = [
    (b"MUS\x1A", MusicKind::Mus),
    (b"MThd", MusicKind::Midi),
    (b"OggS", MusicKind::Ogg),
    (b"ID3", MusicKind::Mp3),
    (b"fLaC", MusicKind::Flac),
    (b"Extended Module:", MusicKind::Xm),
    (b"IMPM", MusicKind::It),
];

/// The four-byte channel tags `ProTracker` modules carry at [`MOD_TAG_OFFSET`].
const MOD_TAGS: [[u8; 4]; 6] = [*b"M.K.", *b"M!K!", *b"FLT4", *b"4CHN", *b"6CHN", *b"8CHN"];

/// Whether `bytes` carry `marker` exactly at `offset`.
fn has_marker(bytes: &[u8], offset: usize, marker: &[u8]) -> bool {
    let Some(end) = offset.checked_add(marker.len()) else {
        return false;
    };
    bytes.get(offset..end) == Some(marker)
}

/// Whether `bytes` start with a `RIFF` header carrying the `WAVE` form type.
fn is_wave(bytes: &[u8]) -> bool {
    bytes.starts_with(b"RIFF") && has_marker(bytes, RIFF_FORM_OFFSET, b"WAVE")
}

/// Whether `bytes` open with an `MPEG` audio frame sync (11 set bits).
fn is_mpeg_sync(bytes: &[u8]) -> bool {
    bytes
        .first_chunk::<2>()
        .is_some_and(|&[first, second]| first == SYNC_HIGH && second & SYNC_MASK == SYNC_MASK)
}

/// Whether `bytes` carry a `ProTracker` channel tag at [`MOD_TAG_OFFSET`].
fn is_mod(bytes: &[u8]) -> bool {
    MOD_TAGS
        .iter()
        .any(|tag| has_marker(bytes, MOD_TAG_OFFSET, tag))
}

/// Identifies a music file from its first bytes.
///
/// Checked in order: the prefix magics (`MUS`+0x1A, `MThd`, `OggS`, `ID3`,
/// `fLaC`, `Extended Module:`, `IMPM`) at offset 0, then `RIFF`/`WAVE`,
/// then a bare `MPEG` frame sync, then the `S3M` and `MOD` tags at their
/// fixed offsets. Anything else is [`MusicKind::Unknown`]; an empty or very
/// short buffer is never mistaken for a magic it does not hold.
#[must_use]
#[inline]
pub fn sniff_music(bytes: &[u8]) -> MusicKind {
    if let Some(kind) = PREFIX_MAGICS
        .iter()
        .find_map(|&(magic, kind)| bytes.starts_with(magic).then_some(kind))
    {
        return kind;
    }
    if is_wave(bytes) {
        return MusicKind::Wav;
    }
    if is_mpeg_sync(bytes) {
        return MusicKind::Mp3;
    }
    if has_marker(bytes, S3M_TAG_OFFSET, b"SCRM") {
        return MusicKind::S3m;
    }
    if is_mod(bytes) {
        return MusicKind::Mod;
    }
    MusicKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::MusicKind;
    use crate::music::sniff::sniff_music;

    /// Bytes of `head` followed by `filler` zero bytes.
    fn padded(head: &[u8], len: usize) -> Vec<u8> {
        let mut bytes = head.to_vec();
        bytes.resize(len, 0_u8);
        bytes
    }

    /// A buffer whose bytes at `offset` are `tag`, everywhere else zero.
    fn with_tag(offset: usize, tag: &[u8]) -> Vec<u8> {
        let end = offset.checked_add(tag.len()).expect("test offsets fit");
        let mut bytes = vec![0_u8; end];
        bytes[offset..end].copy_from_slice(tag);
        bytes
    }

    #[test]
    fn every_prefix_magic_names_its_kind() {
        let cases: [(&[u8], MusicKind); 7] = [
            (b"MUS\x1A", MusicKind::Mus),
            (b"MThd", MusicKind::Midi),
            (b"OggS", MusicKind::Ogg),
            (b"ID3\x04\0", MusicKind::Mp3),
            (b"fLaC", MusicKind::Flac),
            (b"Extended Module: ", MusicKind::Xm),
            (b"IMPM", MusicKind::It),
        ];
        for (magic, want) in cases {
            assert_eq!(sniff_music(magic), want, "bare {magic:?}");
            assert_eq!(sniff_music(&padded(magic, 64)), want, "padded");
        }
    }

    #[test]
    fn wave_needs_the_form_type_at_eight() {
        assert_eq!(
            sniff_music(b"RIFF\0\0\0\0WAVE"),
            MusicKind::Wav,
            "full head"
        );
        assert_eq!(
            sniff_music(b"RIFF\0\0\0\0AVI "),
            MusicKind::Unknown,
            "not a WAVE form"
        );
        assert_eq!(sniff_music(b"RIFF"), MusicKind::Unknown, "no form field");
    }

    #[test]
    fn mpeg_frame_sync_is_mp3_when_the_three_sync_bits_are_set() {
        assert_eq!(sniff_music(&[0xFF, 0xFB, 0x90]), MusicKind::Mp3, "0xFB");
        assert_eq!(sniff_music(&[0xFF, 0xE0]), MusicKind::Mp3, "0xE0");
        assert_eq!(
            sniff_music(&[0xFF, 0x1F]),
            MusicKind::Unknown,
            "sync bits missing"
        );
        assert_eq!(sniff_music(&[0xFF]), MusicKind::Unknown, "one byte alone");
    }

    #[test]
    fn scream_tracker_tag_lives_at_forty_four() {
        assert_eq!(sniff_music(&with_tag(44, b"SCRM")), MusicKind::S3m, "at 44");
        assert_eq!(
            sniff_music(&with_tag(43, b"SCRM")),
            MusicKind::Unknown,
            "one byte early"
        );
        assert_eq!(
            sniff_music(&padded(b"SCRM", 48)),
            MusicKind::Unknown,
            "at 0"
        );
    }

    #[test]
    fn protracker_tags_live_at_one_thousand_eighty() {
        for tag in [b"M.K.", b"M!K!", b"FLT4", b"4CHN", b"6CHN", b"8CHN"] {
            assert_eq!(sniff_music(&with_tag(1080, tag)), MusicKind::Mod, "{tag:?}");
        }
        assert_eq!(
            sniff_music(&padded(b"M.K.", 8)),
            MusicKind::Unknown,
            "the tag is not a prefix"
        );
        assert_eq!(sniff_music(&vec![0_u8; 1083]), MusicKind::Unknown, "short");
    }

    #[test]
    fn empty_and_junk_buffers_are_unknown() {
        assert_eq!(sniff_music(&[]), MusicKind::Unknown, "empty");
        assert_eq!(
            sniff_music(b"total nonsense"),
            MusicKind::Unknown,
            "no magic"
        );
    }

    #[test]
    fn only_mus_and_midi_convert_to_mus() {
        assert!(MusicKind::Mus.convertible_to_mus(), "MUS");
        assert!(MusicKind::Midi.convertible_to_mus(), "MIDI");
        for kind in [
            MusicKind::Ogg,
            MusicKind::Mp3,
            MusicKind::Flac,
            MusicKind::Wav,
            MusicKind::Xm,
            MusicKind::It,
            MusicKind::S3m,
            MusicKind::Mod,
            MusicKind::Unknown,
        ] {
            assert!(!kind.convertible_to_mus(), "{kind:?} must not convert");
        }
    }

    #[test]
    fn kinds_serialize_to_their_lowercase_names() {
        let cases = [
            (MusicKind::Mus, "\"mus\""),
            (MusicKind::Midi, "\"midi\""),
            (MusicKind::Ogg, "\"ogg\""),
            (MusicKind::Mp3, "\"mp3\""),
            (MusicKind::Flac, "\"flac\""),
            (MusicKind::Wav, "\"wav\""),
            (MusicKind::Xm, "\"xm\""),
            (MusicKind::It, "\"it\""),
            (MusicKind::S3m, "\"s3m\""),
            (MusicKind::Mod, "\"mod\""),
            (MusicKind::Unknown, "\"unknown\""),
        ];
        for (kind, expected) in cases {
            let text = serde_json::to_string(&kind).expect("a string serializes");
            assert_eq!(text, expected, "serialized {kind:?}");
        }
    }
}
