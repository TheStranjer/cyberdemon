//! The in-memory sample container: interleaved `i16` audio with
//! integer-only downmix, resampling and `8`-bit conversions.

use crate::audio::{MAX_SAMPLE_RATE, MIN_SAMPLE_RATE, i16_to_u8_unsigned, u8_unsigned_to_i16};
use crate::error::CyberError;
use crate::fixed::mul_div_round;

/// Samples per frame, the smallest channel count the codecs accept.
const MIN_CHANNELS: u16 = 1;

/// A chunk of decoded audio: a sample rate, a channel count and
/// interleaved signed `16`-bit samples.
///
/// Frame `f`, channel `c` is `samples[f * channels + c]`; a trailing
/// partial frame is ignored by [`Pcm::frames`] and the conversions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Pcm {
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// Interleaved channels per frame (`1` for mono).
    pub channels: u16,
    /// Interleaved samples, channel-major by frame.
    pub samples: Vec<i16>,
}

impl Pcm {
    /// Mono [`Pcm`] from unsigned `8`-bit bytes: `(b − 128) << 8`.
    #[must_use]
    #[inline]
    pub fn from_u8_unsigned(bytes: &[u8], sample_rate: u32) -> Self {
        Self {
            sample_rate,
            channels: MIN_CHANNELS,
            samples: bytes.iter().copied().map(u8_unsigned_to_i16).collect(),
        }
    }

    /// Whole frames present: the sample count divided by the channel
    /// count, floored (`0` when there are no channels).
    #[must_use]
    #[inline]
    pub fn frames(&self) -> usize {
        let channels = usize::from(self.channels);
        self.samples.len().checked_div(channels).unwrap_or(0)
    }

    /// Length of the audio in whole milliseconds (floored); `0` when the
    /// sample rate is unusable.
    #[must_use]
    #[inline]
    pub fn duration_ms(&self) -> u64 {
        let frames = u64::try_from(self.frames()).unwrap_or(u64::MAX);
        frames
            .checked_mul(1000_u64)
            .and_then(|scaled| scaled.checked_div(u64::from(self.sample_rate)))
            .unwrap_or(0)
    }

    /// A copy averaged to one channel: each frame's samples are summed in
    /// `i32` and divided by the channel count, rounded to nearest
    /// (halves away from zero). Mono input is returned unchanged.
    #[must_use]
    #[inline]
    pub fn to_mono(&self) -> Self {
        let frames = self.frames();
        let mut samples = Vec::with_capacity(frames);
        for frame in 0..frames {
            self.mix_frame(frame, &mut samples);
        }
        Self {
            sample_rate: self.sample_rate,
            channels: MIN_CHANNELS,
            samples,
        }
    }

    /// Average one frame into `out`; does nothing at a partial frame.
    fn mix_frame(&self, frame: usize, out: &mut Vec<i16>) {
        let Some(channel_samples) = self.frame_slice(frame) else {
            return;
        };
        let sum = channel_samples
            .iter()
            .fold(0_i32, |acc, sample| acc.saturating_add(i32::from(*sample)));
        out.push(average_channel(sum, usize::from(self.channels)));
    }

    /// Linearly resampled audio at `target_rate` frames per second.
    ///
    /// Output frame `k` reads source position `k * src_rate / dst_rate`
    /// as an exact fraction and interpolates between the two frames
    /// around it; the frame count is `frames * dst_rate / src_rate`
    /// rounded to nearest. Both rates must be within `1000..=192000`.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when either rate leaves
    /// `1000..=192000`; [`CyberError::LimitExceeded`] when the frame
    /// arithmetic cannot be represented.
    #[inline]
    pub fn resample(&self, target_rate: u32) -> Result<Self, CyberError> {
        check_rate("source sample rate", self.sample_rate)?;
        check_rate("target sample rate", target_rate)?;
        let frames = resampled_frame_count(self.frames(), self.sample_rate, target_rate)?;
        let capacity = frames
            .checked_mul(usize::from(self.channels))
            .ok_or_else(|| {
                CyberError::LimitExceeded(format!(
                    "resampling to {target_rate} Hz would need more samples than memory can hold"
                ))
            })?;
        let mut samples = Vec::with_capacity(capacity);
        for frame in 0..frames {
            self.resample_frame(frame, target_rate, &mut samples)?;
        }
        Ok(Self {
            sample_rate: target_rate,
            channels: self.channels,
            samples,
        })
    }

    /// Interpolate one output frame into `out`.
    fn resample_frame(
        &self,
        frame: usize,
        target_rate: u32,
        out: &mut Vec<i16>,
    ) -> Result<(), CyberError> {
        let (whole, fraction) =
            source_position(frame, self.sample_rate, target_rate).ok_or_else(|| {
                CyberError::LimitExceeded(format!("resample position overflowed at frame {frame}"))
            })?;
        let channels = usize::from(self.channels);
        for channel in 0..channels {
            let start = whole
                .checked_mul(channels)
                .and_then(|base| base.checked_add(channel));
            let first = start
                .and_then(|index| self.samples.get(index))
                .copied()
                .unwrap_or(0_i16);
            let second = start
                .and_then(|index| index.checked_add(channels))
                .and_then(|index| self.samples.get(index))
                .copied()
                .unwrap_or(first);
            out.push(interpolate(first, second, fraction, target_rate));
        }
        Ok(())
    }

    /// Every sample as unsigned `8`-bit bytes: `(s >> 8) + 128`, clamped.
    #[must_use]
    #[inline]
    pub fn to_u8_unsigned(&self) -> Vec<u8> {
        self.samples
            .iter()
            .copied()
            .map(i16_to_u8_unsigned)
            .collect()
    }

    /// The interleaved samples of one frame, when they are all present.
    fn frame_slice(&self, frame: usize) -> Option<&[i16]> {
        let channels = usize::from(self.channels);
        let start = frame.checked_mul(channels)?;
        let end = start.checked_add(channels)?;
        self.samples.get(start..end)
    }
}

/// Rejects sample rates outside the resampling range.
fn check_rate(what: &str, rate: u32) -> Result<(), CyberError> {
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&rate) {
        return Err(CyberError::InvalidArgument(format!(
            "{what} {rate} is outside {MIN_SAMPLE_RATE}..={MAX_SAMPLE_RATE}; pass a rate in that range"
        )));
    }
    Ok(())
}

/// The frame average: `sum / channels` rounded to nearest (halves away
/// from zero); the average of `i16` values always fits `i16`.
fn average_channel(sum: i32, channels: usize) -> i16 {
    let divisor = i64::try_from(channels).unwrap_or(1);
    let average = mul_div_round(i64::from(sum), 1_i64, divisor).unwrap_or(0);
    i16::try_from(average).unwrap_or(if average >= 0 { i16::MAX } else { i16::MIN })
}

/// The output frame count for `frames` input frames, `frames *
/// target / source` rounded to nearest.
fn resampled_frame_count(
    frames: usize,
    source_rate: u32,
    target_rate: u32,
) -> Result<usize, CyberError> {
    let count = i64::try_from(frames).map_err(|error| {
        CyberError::LimitExceeded(format!(
            "frame count {frames} does not fit 64 bits: {error}"
        ))
    })?;
    let rounded =
        mul_div_round(count, i64::from(target_rate), i64::from(source_rate)).ok_or_else(|| {
            CyberError::LimitExceeded(format!(
                "resampling {frames} frames from {source_rate} to {target_rate} Hz leaves 64 bits"
            ))
        })?;
    usize::try_from(rounded).map_err(|error| {
        CyberError::LimitExceeded(format!("resampled frame count does not fit: {error}"))
    })
}

/// The source frame index and the fraction toward the next frame for
/// output frame `frame`, exactly `frame * source_rate / target_rate`
/// split at the integer part. `None` when it cannot be represented.
fn source_position(frame: usize, source_rate: u32, target_rate: u32) -> Option<(usize, u64)> {
    let numerator = u128::try_from(frame)
        .ok()?
        .checked_mul(u128::from(source_rate))?;
    let denominator = u128::from(target_rate);
    let whole = usize::try_from(numerator.checked_div(denominator)?).ok()?;
    let fraction = u64::try_from(numerator.checked_rem(denominator)?).ok()?;
    Some((whole, fraction))
}

/// `first + (second − first) * fraction / denominator`, rounded to
/// nearest; the result always sits between the two samples.
fn interpolate(first: i16, second: i16, fraction: u64, denominator: u32) -> i16 {
    let delta = i64::from(second).checked_sub(i64::from(first)).unwrap_or(0);
    let step = mul_div_round(
        delta,
        i64::try_from(fraction).unwrap_or(0),
        i64::from(denominator),
    )
    .unwrap_or(0);
    let value = i64::from(first).checked_add(step).unwrap_or(0);
    i16::try_from(value).unwrap_or(if value >= 0 { i16::MAX } else { i16::MIN })
}

#[cfg(test)]
mod tests {
    use super::Pcm;
    use crate::audio::MAX_SAMPLE_RATE;

    /// A mono test signal at `rate` with the given samples.
    fn mono(rate: u32, samples: Vec<i16>) -> Pcm {
        Pcm {
            sample_rate: rate,
            channels: 1,
            samples,
        }
    }

    #[test]
    fn frames_counts_whole_interleaved_frames() {
        let stereo = Pcm {
            sample_rate: 44100,
            channels: 2,
            samples: vec![0; 5],
        };
        assert_eq!(stereo.frames(), 2, "five samples are two stereo frames");
        assert_eq!(
            mono(8000, vec![0, 1, 2]).frames(),
            3,
            "mono counts every sample"
        );
        let no_channels = Pcm {
            sample_rate: 8000,
            channels: 0,
            samples: vec![0, 1],
        };
        assert_eq!(no_channels.frames(), 0, "zero channels yields no frames");
    }

    #[test]
    fn duration_ms_is_the_floored_millisecond_length() {
        let pcm = mono(22050, vec![0; 22050]);
        assert_eq!(pcm.duration_ms(), 1000, "one second at 22050 Hz");
        assert_eq!(mono(1000, vec![0]).duration_ms(), 1, "one frame at 1000 Hz");
        assert_eq!(
            mono(22050, vec![0]).duration_ms(),
            0,
            "sub-millisecond floors"
        );
        assert_eq!(
            mono(0, vec![0; 10]).duration_ms(),
            0,
            "a zero rate reports zero"
        );
        assert_eq!(
            mono(8000, Vec::new()).duration_ms(),
            0,
            "empty audio is silent"
        );
    }

    #[test]
    fn to_mono_averages_channels_with_i32_sums() {
        let stereo = Pcm {
            sample_rate: 44100,
            channels: 2,
            samples: vec![100, 200, -100, -200, 10, 11],
        };
        let down = stereo.to_mono();
        assert_eq!(down.channels, 1, "output is mono");
        assert_eq!(down.sample_rate, 44100, "rate survives");
        assert_eq!(down.samples, vec![150, -150, 11], "averaged frames");
        let surround = Pcm {
            sample_rate: 48000,
            channels: 3,
            samples: vec![-30000, 30000, 15000],
        };
        assert_eq!(
            surround.to_mono().samples,
            vec![5000],
            "three-channel sum stays exact in i32"
        );
        let already = mono(8000, vec![7, -7]);
        assert_eq!(
            already.to_mono().samples,
            vec![7, -7],
            "mono passes through"
        );
    }

    #[test]
    fn resample_halves_frames_and_keeps_constants_constant() {
        let pcm = mono(22050, vec![12345; 100]);
        let down = pcm.resample(11025).expect("valid rates");
        assert_eq!(down.sample_rate, 11025, "rate applied");
        assert_eq!(down.frames(), 50, "halving the rate halves the frames");
        assert!(
            down.samples.iter().all(|&sample| sample == 12345),
            "a constant signal resamples to itself"
        );
    }

    #[test]
    fn resample_upsampling_interpolates_linearly() {
        let pcm = mono(11025, vec![0, 2000]);
        let up = pcm.resample(22050).expect("valid rates");
        assert_eq!(up.frames(), 4, "doubling the rate doubles the frames");
        assert_eq!(
            up.samples,
            vec![0, 1000, 2000, 2000],
            "midpoint interpolated"
        );
    }

    #[test]
    fn resample_keeps_every_channel_of_a_stereo_frame() {
        let stereo = Pcm {
            sample_rate: 11025,
            channels: 2,
            samples: vec![0, -1000, 2000, 3000],
        };
        let up = stereo.resample(22050).expect("valid rates");
        assert_eq!(up.channels, 2, "channel count survives");
        assert_eq!(
            up.samples,
            vec![0, -1000, 1000, 1000, 2000, 3000, 2000, 3000],
            "each channel interpolates on its own"
        );
    }

    #[test]
    fn resample_rejects_rates_outside_the_range() {
        let low = mono(999, vec![0; 4]);
        let high = mono(11025, vec![0; 4]);
        assert_eq!(
            low.resample(8000).expect_err("999 Hz source").code(),
            "invalid_argument",
            "source rate below the range"
        );
        assert_eq!(
            high.resample(999).expect_err("999 Hz target").code(),
            "invalid_argument",
            "target rate below the range"
        );
        assert_eq!(
            high.resample(MAX_SAMPLE_RATE.saturating_add(1))
                .expect_err("above the range")
                .code(),
            "invalid_argument",
            "target rate above the range"
        );
        high.resample(1000).expect("the low bound is allowed");
        high.resample(MAX_SAMPLE_RATE)
            .expect("the high bound is allowed");
    }

    #[test]
    fn resample_of_empty_audio_is_empty() {
        let empty = mono(22050, Vec::new());
        let down = empty.resample(8000).expect("valid rates");
        assert!(down.samples.is_empty(), "no frames in, no frames out");
        assert_eq!(down.sample_rate, 8000, "the rate is still applied");
    }

    #[test]
    fn u8_conversions_hit_the_extremes_and_round_trip() {
        let pcm = mono(22050, vec![i16::MIN, -1, 0, 255, i16::MAX]);
        assert_eq!(
            pcm.to_u8_unsigned(),
            vec![0, 127, 128, 128, 255],
            "shifted bytes"
        );
        let bytes: Vec<u8> = (0_u8..=255).collect();
        let round = Pcm::from_u8_unsigned(&bytes, 8000);
        assert_eq!(round.channels, 1, "from bytes is mono");
        assert_eq!(round.sample_rate, 8000, "rate stored");
        assert_eq!(
            round.samples.first().copied(),
            Some(i16::MIN),
            "byte 0 is the floor"
        );
        assert_eq!(
            round.samples.last().copied(),
            Some(0x7F00),
            "byte 255 is the top"
        );
        assert_eq!(round.to_u8_unsigned(), bytes, "8-bit audio round-trips");
    }
}
