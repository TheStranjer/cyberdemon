//! `Doom` `DP*` `PC` speaker lumps: the tick-based tone-list format the
//! engine plays through the `PC` speaker driver, synthesised into a mono
//! 16-bit square-wave [`Pcm`].
//!
//! The lump is a `u16` format word (`0`), a `u16` tick count and then one
//! byte per `1/140` s tick: `0` is silence, anything else is a tone index.
//! The vanilla driver divides a fixed table down by the index; that table
//! is not reproduced here, so the pitch of [`synthesize_pc_speaker`]'s
//! output is approximate.

use crate::audio::pcm::Pcm;
use crate::error::CyberError;
use crate::wad::bytes::Reader;

/// The only `PC` speaker format `Doom` ships: format word `0`.
const PC_FORMAT: u16 = 0_u16;
/// Sample rate of the synthesized audio, the engine's own sound rate.
const PC_SAMPLE_RATE: u32 = 11_025;
/// Ticks one second of lump data lasts.
const TICKS_PER_SECOND: u32 = 140;
/// Frequency of the virtual tone `0`: every tone sits this far above it.
const TONE_BASE_HZ: u32 = 110;
/// Frequency step between consecutive tone indices.
const TONE_STEP_HZ: u32 = 13;
/// Square-wave peak: the synthesized samples are `±` this or `0`.
const WAVE_CEIL: i16 = 8_000;
/// Square-wave trough, [`WAVE_CEIL`] negated.
const WAVE_FLOOR: i16 = -8_000;
/// Silence, the sample value of tone `0`.
const WAVE_SILENCE: i16 = 0;
/// Mono output: one sample per frame.
const MONO_CHANNELS: u16 = 1_u16;
/// A square wave has two half-periods per cycle.
const HALF_PERIODS_PER_CYCLE: u64 = 2_u64;
/// Odd half-period count means the wave is in its negative half.
const ODD_HALF_PERIOD: u64 = 1_u64;

/// Synthesizes a `PC` speaker `DP*` tone list into a mono 16-bit [`Pcm`]
/// at `11025` Hz.
///
/// Each tick lasts `11025 / 140` samples with the remainder carried to the
/// next one, so the whole lump keeps the exact `1/140` s tick grid. Tone
/// `n` plays a square wave of amplitude `±8000` at `110 + 13 · n` Hz
/// (approximate, see the module docs); tone `0` is silence.
///
/// # Errors
///
/// [`CyberError::Format`] when the header is truncated, the format word is
/// not `0`, or the declared tick count runs past the end of the lump.
#[inline]
pub fn synthesize_pc_speaker(bytes: &[u8]) -> Result<Pcm, CyberError> {
    let mut reader = Reader::new(bytes);
    let format = reader.read_u16()?;
    if format != PC_FORMAT {
        return Err(CyberError::Format(format!(
            "PC speaker lump declares format {format}, only format {PC_FORMAT} (the vanilla \
             tone list) synthesizes; check the lump is a DP* sound"
        )));
    }
    let ticks = usize::from(reader.read_u16()?);
    let tones = reader.read_bytes(ticks)?;
    let mut samples = Vec::with_capacity(tick_frame(ticks));
    for (tick, &tone) in tones.iter().enumerate() {
        let start = tick_frame(tick);
        let end = tick_frame(tick.saturating_add(1));
        samples.extend((start..end).map(|frame| tone_sample(tone, frame)));
    }
    Ok(Pcm {
        sample_rate: PC_SAMPLE_RATE,
        channels: MONO_CHANNELS,
        samples,
    })
}

/// The first sample frame of tick `ticks`: `ticks * 11025 / 140` floored.
/// Deriving every boundary from the tick number carries the `78.75`-sample
/// tick length's remainder instead of losing `0.25` samples per tick.
fn tick_frame(ticks: usize) -> usize {
    u64::try_from(ticks)
        .unwrap_or(0)
        .checked_mul(u64::from(PC_SAMPLE_RATE))
        .and_then(|scaled| scaled.checked_div(u64::from(TICKS_PER_SECOND)))
        .and_then(|frames| usize::try_from(frames).ok())
        .unwrap_or(0_usize)
}

/// The approximate frequency of tone index `tone` in Hz.
fn tone_frequency(tone: u8) -> u32 {
    u32::from(tone)
        .checked_mul(TONE_STEP_HZ)
        .and_then(|scaled| scaled.checked_add(TONE_BASE_HZ))
        .unwrap_or(TONE_BASE_HZ)
}

/// The square-wave sample of tone `tone` at frame `frame`: the sign flips
/// every half period, computed exactly as `frame · 2 · frequency / rate`.
fn tone_sample(tone: u8, frame: usize) -> i16 {
    if tone == 0_u8 {
        return WAVE_SILENCE;
    }
    let half_periods = u64::try_from(frame)
        .unwrap_or(0)
        .checked_mul(HALF_PERIODS_PER_CYCLE)
        .and_then(|doubled| doubled.checked_mul(u64::from(tone_frequency(tone))))
        .and_then(|scaled| scaled.checked_div(u64::from(PC_SAMPLE_RATE)))
        .unwrap_or(0);
    let negative = half_periods.checked_rem(HALF_PERIODS_PER_CYCLE) == Some(ODD_HALF_PERIOD);
    if negative { WAVE_FLOOR } else { WAVE_CEIL }
}

#[cfg(test)]
mod tests {
    use super::{PC_SAMPLE_RATE, synthesize_pc_speaker, tick_frame, tone_frequency, tone_sample};

    /// A tone-list lump: the header plus the given tick bytes.
    fn lump(tones: &[u8]) -> Vec<u8> {
        let count = u16::try_from(tones.len()).expect("test tone counts fit u16");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(tones);
        bytes
    }

    #[test]
    fn tick_grid_carries_the_remainder_without_drift() {
        assert_eq!(tick_frame(0), 0, "the lump starts at frame 0");
        assert_eq!(tick_frame(1), 78, "the first tick is 78 whole samples");
        assert_eq!(tick_frame(2), 157, "the second tick gained the 0.5 carry");
        assert_eq!(tick_frame(4), 315, "no 0.25-sample drift after four ticks");
        assert_eq!(
            tick_frame(140),
            11_025,
            "one second of ticks is exactly one second of frames"
        );
    }

    #[test]
    fn tone_frequencies_start_at_110_hz_and_step_by_13() {
        assert_eq!(tone_frequency(0), 110, "the documented base");
        assert_eq!(tone_frequency(20), 370, "the ticket's example tone 20");
        assert_eq!(tone_frequency(127), 1761, "the top vanilla index");
    }

    #[test]
    fn tone_zero_is_silence_and_other_tones_are_square() {
        assert_eq!(tone_sample(0, 0), 0, "tone 0 never sounds");
        assert_eq!(tone_sample(0, 1000), 0, "tone 0 never sounds, late either");
        let tone1 = Vec::from_iter((0..200).map(|frame| tone_sample(1, frame)));
        assert_eq!(tone1.first().copied(), Some(8000), "the wave starts high");
        assert!(
            tone1.windows(2).any(|pair| pair.first() != pair.get(1)),
            "a tone-1 half period is 44 frames, so 200 frames flip"
        );
        assert!(
            tone1
                .iter()
                .all(|&sample| sample == 8000 || sample == -8000),
            "the wave only ever sits at +8000 or -8000"
        );
    }

    #[test]
    fn synthesizes_three_ticks_onto_the_exact_grid() {
        let pcm = synthesize_pc_speaker(&lump(&[0_u8, 20, 40])).expect("a valid tone list");
        assert_eq!(pcm.sample_rate, PC_SAMPLE_RATE, "the engine's rate");
        assert_eq!(pcm.channels, 1, "the driver is mono");
        assert_eq!(pcm.frames(), 236, "3 * 11025 / 140 floored");
        assert!(
            pcm.samples.iter().take(78).all(|&sample| sample == 0),
            "the first tick is silence"
        );
        assert!(
            pcm.samples
                .get(78..157)
                .is_some_and(|tick| tick.iter().any(|&sample| sample == 8000 || sample == -8000)),
            "the second tick sounds"
        );
        assert!(
            pcm.samples
                .iter()
                .all(|&sample| sample == 0 || sample == 8000 || sample == -8000),
            "every sample is silence or the square-wave peak"
        );
    }

    #[test]
    fn an_empty_tone_list_synthesizes_silence() {
        let pcm = synthesize_pc_speaker(&lump(&[])).expect("zero ticks is valid");
        assert!(pcm.samples.is_empty(), "nothing to play");
        assert_eq!(
            pcm.sample_rate, PC_SAMPLE_RATE,
            "the rate is still declared"
        );
    }

    #[test]
    fn a_nonzero_format_word_is_a_format_error() {
        let mut bytes = lump(&[0_u8]);
        bytes[0..2].copy_from_slice(&1_u16.to_le_bytes());
        let error = synthesize_pc_speaker(&bytes).expect_err("format 1 is not a tone list");
        assert_eq!(error.code(), "format_error", "bad format code");
        assert!(
            error.to_string().contains("format 1"),
            "the message names the format: {error}"
        );
    }

    #[test]
    fn short_lumps_are_format_errors() {
        let truncated = synthesize_pc_speaker(&[0_u8]).expect_err("one byte holds no header");
        assert_eq!(truncated.code(), "format_error", "short header code");
        let mut over = lump(&[0_u8]);
        over[2..4].copy_from_slice(&10_u16.to_le_bytes());
        let declared =
            synthesize_pc_speaker(&over).expect_err("ten ticks declared, one byte present");
        assert_eq!(
            declared.code(),
            "format_error",
            "tone list past the end code"
        );
    }
}
