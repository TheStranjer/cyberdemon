//! Audio building blocks shared by every WAD sound tool.
//!
//! Pure data processing on owned buffers: [`pcm::Pcm`] is the in-memory
//! sample container, [`dmx`] decodes and encodes `Doom` `DS*` sound lumps
//! and [`wav`] reads and writes canonical `RIFF`/`WAVE` files. Nothing here
//! knows about MCP, lumps or the filesystem; later sound tools compose
//! these pieces. All arithmetic is integer-only, like the rest of the
//! crate (see [`crate::fixed`]).

pub mod dmx;
pub mod pcm;
pub mod wav;

pub use dmx::{decode_dmx, encode_dmx};
pub use pcm::Pcm;
pub use wav::{decode_wav, encode_wav};

/// Smallest sample rate [`Pcm::resample`](pcm::Pcm::resample) accepts.
pub const MIN_SAMPLE_RATE: u32 = 1000;
/// Largest sample rate [`Pcm::resample`](pcm::Pcm::resample) accepts.
pub const MAX_SAMPLE_RATE: u32 = 192_000;

/// Shift from an `i16` sample to the top byte used by unsigned `8`-bit
/// storage (`DMX` lumps and `8`-bit `WAV`).
pub(crate) const U8_SHIFT: u32 = 8;

/// Bias added to the shifted sample so silence sits at `128`.
pub(crate) const U8_BIAS: i32 = 128;

/// Unsigned `8`-bit sample to `i16`: `(b − 128) << 8`, the mapping every
/// `Doom` sound uses (`DMX` lumps, `8`-bit `WAV`).
pub(crate) fn u8_unsigned_to_i16(byte: u8) -> i16 {
    let centered = i32::from(byte).checked_sub(U8_BIAS).unwrap_or(0_i32);
    i16::try_from(centered.checked_shl(U8_SHIFT).unwrap_or(0_i32)).unwrap_or(i16::MIN)
}

/// `i16` sample to unsigned `8`-bit storage: `(s >> 8) + 128`, clamped.
pub(crate) fn i16_to_u8_unsigned(sample: i16) -> u8 {
    let shifted = i32::from(sample)
        .checked_shr(U8_SHIFT)
        .unwrap_or(0_i32)
        .checked_add(U8_BIAS)
        .unwrap_or(U8_BIAS);
    u8::try_from(shifted).unwrap_or(if shifted > U8_BIAS { u8::MAX } else { 0_u8 })
}

#[cfg(test)]
mod tests {
    use super::{i16_to_u8_unsigned, u8_unsigned_to_i16};

    #[test]
    fn u8_to_i16_spans_the_unsigned_range() {
        assert_eq!(u8_unsigned_to_i16(0_u8), i16::MIN, "floor byte");
        assert_eq!(u8_unsigned_to_i16(128_u8), 0_i16, "silence byte");
        assert_eq!(u8_unsigned_to_i16(255_u8), 0x7F00_i16, "top byte");
    }

    #[test]
    fn i16_to_u8_spans_the_signed_range() {
        assert_eq!(i16_to_u8_unsigned(i16::MIN), 0, "floor sample");
        assert_eq!(i16_to_u8_unsigned(0), 128, "silence sample");
        assert_eq!(i16_to_u8_unsigned(i16::MAX), 255, "top sample");
        assert_eq!(i16_to_u8_unsigned(-1), 127, "one below silence");
    }

    #[test]
    fn u8_round_trip_is_exact_on_representable_samples() {
        for byte in 0_u8..=255 {
            let sample = u8_unsigned_to_i16(byte);
            assert_eq!(i16_to_u8_unsigned(sample), byte, "byte {byte}");
        }
    }
}
