//! The sounds as 16-bit mono PCM in a WAV: a few short triangle tones each, with the dashboard's
//! old `WebAudio` envelope (an exponential 20 ms attack, then an exponential decay to silence
//! over the tone).

use super::Sound;

pub const RATE: u32 = 48_000;

/// One tone: start offset (s), frequency (Hz), length (s).
type Tone = (f64, f64, f64);

const fn tones(sound: Sound) -> &'static [Tone] {
    match sound {
        Sound::FarmDrop => &[(0.0, 1318.5, 0.12), (0.08, 1661.2, 0.12), (0.16, 1975.5, 0.3)],
        Sound::MarkedDrop => &[(0.0, 880.0, 0.08), (0.12, 1174.66, 0.08)],
    }
}

const ATTACK: f64 = 0.02;
/// The envelope's floor (an exponential ramp can't reach 0).
const FLOOR: f64 = 0.0001;
/// The oscillators ran this much past their tone, at the floor.
const TAIL: f64 = 0.02;

/// The sound's samples at [`RATE`].
#[must_use]
pub fn pcm(sound: Sound) -> Vec<i16> {
    let peak = if sound.loud() { 0.32 } else { 0.12 };
    let tones = tones(sound);
    let seconds = tones.iter().map(|t| t.0 + t.2).fold(0.0, f64::max) + TAIL;
    let rate = f64::from(RATE);
    let mut mix = vec![0.0_f64; samples(seconds * rate)];
    for &(start, freq, len) in tones {
        let first = samples(start * rate);
        let mut phase = 0.0_f64;
        for (i, out) in mix.iter_mut().skip(first).enumerate() {
            let t = f64::from(u32::try_from(i).unwrap_or(u32::MAX)) / rate;
            if t >= len + TAIL {
                break;
            }
            *out = envelope(t, len, peak).mul_add(triangle(phase), *out);
            phase = (phase + freq / rate).fract();
        }
    }
    mix.into_iter().map(to_i16).collect()
}

/// The gain `t` s into a tone of `len` s.
fn envelope(t: f64, len: f64, peak: f64) -> f64 {
    if t < ATTACK {
        FLOOR * (peak / FLOOR).powf(t / ATTACK)
    } else if t < len {
        peak * (FLOOR / peak).powf((t - ATTACK) / (len - ATTACK))
    } else {
        FLOOR
    }
}

/// One triangle cycle's value at `phase` (0..1), between -1 and 1.
fn triangle(phase: f64) -> f64 {
    4.0f64.mul_add(-(phase - 0.5).abs(), 1.0)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a few seconds of samples, clamped to be non-negative"
)]
const fn samples(count: f64) -> usize {
    count.max(0.0).round() as usize
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "clamped to -1..=1 first, so the product fits an i16"
)]
fn to_i16(x: f64) -> i16 {
    (x.clamp(-1.0, 1.0) * f64::from(i16::MAX)).round() as i16
}

/// A mono 16-bit PCM WAV of `samples` at [`RATE`].
#[must_use]
pub fn wav(samples: &[i16]) -> Vec<u8> {
    let data = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // bytes per frame
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(samples: &[i16]) -> i32 {
        samples.iter().map(|&s| i32::from(s).abs()).max().unwrap_or(0)
    }

    #[test]
    fn every_sound_is_short_and_audible() {
        for sound in Sound::ALL {
            let samples = pcm(sound);
            let seconds = f64::from(u32::try_from(samples.len()).unwrap_or(u32::MAX)) / f64::from(RATE);
            assert!(
                (0.1..=0.6).contains(&seconds),
                "{sound:?}: {seconds} s (tones under 0.6 s, 20 ms tail)"
            );
            let level = if sound.loud() { 0.32 } else { 0.12 };
            let p = f64::from(peak(&samples)) / f64::from(i16::MAX);
            assert!(p > level * 0.5 && p < level * 1.6, "{sound:?}: peak {p}");
            assert!(
                peak(samples.get(samples.len() - 200..).unwrap_or_default()) < 20,
                "{sound:?} ends in silence"
            );
        }
    }

    #[test]
    fn lengths_follow_the_tones() {
        let len = |s| pcm(s).len();
        assert_eq!(len(Sound::FarmDrop), 48_000 * 48 / 100, "0.16 + 0.3 + 0.02 s");
        assert_eq!(len(Sound::MarkedDrop), 48_000 * 22 / 100, "0.12 + 0.08 + 0.02 s");
    }

    #[test]
    fn the_loud_sounds_are_louder() {
        assert!(peak(&pcm(Sound::FarmDrop)) > 2 * peak(&pcm(Sound::MarkedDrop)));
    }

    #[test]
    fn the_wav_header_describes_the_samples() {
        let samples = pcm(Sound::MarkedDrop);
        let wav = wav(&samples);
        assert_eq!(wav.len(), 44 + 2 * samples.len());
        assert_eq!(wav.get(..4), Some(&b"RIFF"[..]));
        assert_eq!(wav.get(8..16), Some(&b"WAVEfmt "[..]));
        let u32_at = |i: usize| {
            wav.get(i..i + 4)
                .and_then(|b| b.try_into().ok())
                .map(u32::from_le_bytes)
        };
        assert_eq!(u32_at(4), u32::try_from(wav.len() - 8).ok());
        assert_eq!(u32_at(24), Some(RATE));
        assert_eq!(wav.get(36..40), Some(&b"data"[..]));
        assert_eq!(u32_at(40), u32::try_from(2 * samples.len()).ok());
        let first = samples.iter().position(|&s| s != 0).unwrap_or(0);
        let at = 44 + 2 * first;
        assert_eq!(
            wav.get(at..at + 2)
                .and_then(|b| b.try_into().ok())
                .map(i16::from_le_bytes),
            samples.get(first).copied()
        );
    }
}
