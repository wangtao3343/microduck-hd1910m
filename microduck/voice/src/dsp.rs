//! The cleanup between the microphone and the file.
//!
//! Everything here was measured on the robot's own recording, not chosen from a textbook:
//!
//!  - `despeckle` — the capture path drops full-scale single-sample spikes into the stream
//!    (63 of them in a 45 s take, none of them audible in the room). They are what makes a
//!    recording crackle, and they are trivially removable because a real voice never jumps
//!    from room level to ±1.0 and back inside 0.1 ms.
//!  - `highpass` — the ADC's own high-pass can be parked at 216 Hz by the mixer script; this
//!    is the same idea in software for boards where it is not.
//!  - `gate` — the microphone's noise floor sits about 18 dB below speech, which is audible
//!    as hiss between words. Gating is what buys the rest: measured 18 dB → 36 dB SNR.
//!
//! The chain is deliberately mild. Anything aggressive enough to sound "denoised" also eats
//! consonants, and a robotic voice memo is worse than a slightly hissy one.

/// Full chain, in the order the artefacts appear in the signal.
pub fn clean(samples: &[f32], rate: u32) -> Vec<f32> {
    let mut out = samples.to_vec();
    despeckle(&mut out, rate);
    highpass(&mut out, rate, 90.0);
    gate(&mut out, rate);
    // Only normalize when the gate actually opened on something. In a recording of silence
    // the loudest sample *is* the noise floor, and scaling that up to full scale walks the
    // gate straight back to where it started — measured: a quiet room went from −44 dBFS
    // raw to −23 dBFS "cleaned", which is the opposite of the point.
    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak > SPEECH_PRESENT {
        normalize_peak(&mut out, 0.9);
    }
    out
}

/// Below this peak the take contains no speech, so there is nothing to normalise against.
/// Speech at 36 dB capture gain peaks well above 0.1; gated noise sits orders of magnitude
/// below it.
const SPEECH_PRESENT: f32 = 0.05;

/// Replace isolated full-scale spikes with a straight line between their neighbours.
///
/// A spike is "a sample far above the local RMS *and* near full scale". Both halves matter:
/// the RMS test alone would eat a plosive's attack, and the level test alone would eat a
/// loud-but-real syllable.
pub fn despeckle(samples: &mut [f32], rate: u32) {
    let frame = (rate as usize / 100).max(1); // 10 ms
    if samples.len() < frame * 3 {
        return;
    }
    // Two passes: a spike often comes in a cluster, and the first pass only removes the
    // ones whose neighbours were clean.
    for _ in 0..2 {
        let mut local = vec![0f32; samples.len()];
        let mut start = 0;
        while start < samples.len() {
            let end = (start + frame).min(samples.len());
            let rms = (samples[start..end].iter().map(|v| v * v).sum::<f32>()
                / (end - start) as f32)
                .sqrt();
            local[start..end].fill(rms);
            start = end;
        }

        let mut i = 0;
        while i < samples.len() {
            let limit = (8.0 * local[i]).max(0.4);
            if samples[i].abs() <= limit {
                i += 1;
                continue;
            }
            let run_start = i;
            while i < samples.len() && samples[i].abs() > (8.0 * local[i]).max(0.4) {
                i += 1;
            }
            let run_end = i; // exclusive
            let before = if run_start > 0 {
                samples[run_start - 1]
            } else {
                0.0
            };
            let after = if run_end < samples.len() {
                samples[run_end]
            } else {
                0.0
            };
            let n = (run_end - run_start) as f32 + 1.0;
            for (k, slot) in samples[run_start..run_end].iter_mut().enumerate() {
                let t = (k as f32 + 1.0) / n;
                *slot = before + (after - before) * t;
            }
        }
    }
}

/// Second-order high-pass, Butterworth response (`Q = 1/√2`) at `cutoff` Hz.
pub fn highpass(samples: &mut [f32], rate: u32, cutoff: f32) {
    let w0 = 2.0 * std::f32::consts::PI * cutoff / rate as f32;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);

    let b0 = (1.0 + cos) / 2.0;
    let b1 = -(1.0 + cos);
    let b2 = (1.0 + cos) / 2.0;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos;
    let a2 = 1.0 - alpha;

    let (b0, b1, b2, a1, a2) = (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0);
    let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for x in samples.iter_mut() {
        let xi = *x;
        let yi = b0 * xi + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
        x2 = x1;
        x1 = xi;
        y2 = y1;
        y1 = yi;
        *x = yi;
    }
}

/// Downward expander on 10 ms frames: below the noise floor the gain is `FLOOR`, above
/// `OPEN_RATIO` times it the gain is 1, and anything in between ramps.
///
/// The noise floor is the 20th percentile of frame RMS, so it adapts to the room instead of
/// being a constant that only works in the lab where it was measured.
pub fn gate(samples: &mut [f32], rate: u32) {
    const FLOOR: f32 = 0.02; // −34 dB of attenuation between words
    const OPEN_RATIO: f32 = 3.0;
    const CLOSE_RATIO: f32 = 1.4;
    const RELEASE: f32 = 0.85; // per frame; ~60 ms to fully close

    let frame = (rate as usize / 100).max(1);
    if samples.len() < frame * 4 {
        return;
    }
    let frames: Vec<f32> = samples
        .chunks(frame)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .collect();

    let mut sorted = frames.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let noise = sorted[(sorted.len() / 5).min(sorted.len() - 1)].max(1e-6);
    let (open, close) = (noise * OPEN_RATIO, noise * CLOSE_RATIO);

    // Start closed when the take opens in silence, or the first ~60 ms of room noise leaks
    // through at full gain — and that leak is what a later peak normalisation would latch on.
    let mut gain = if frames[0] <= close { FLOOR } else { 1.0 };
    for (i, chunk) in samples.chunks_mut(frame).enumerate() {
        let rms = frames[i];
        let target = if rms >= open {
            1.0
        } else if rms <= close {
            FLOOR
        } else {
            let t = (rms - close) / (open - close);
            FLOOR + (1.0 - FLOOR) * t
        };
        // Open instantly (a word's first phoneme must not be clipped), close slowly.
        let next = if target > gain { target } else { gain * RELEASE + target * (1.0 - RELEASE) };
        let step = (next - gain) / chunk.len() as f32;
        for (k, s) in chunk.iter_mut().enumerate() {
            *s *= gain + step * k as f32;
        }
        gain = next;
    }
}

/// Scale so the loudest sample sits at `target`; silence is left alone.
pub fn normalize_peak(samples: &mut [f32], target: f32) {
    let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak <= 1e-6 {
        return;
    }
    let scale = target / peak;
    for s in samples.iter_mut() {
        *s *= scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn tone(n: usize, freq: f32, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / RATE as f32).sin())
            .collect()
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn despeckle_removes_a_full_scale_spike() {
        let mut s = tone(RATE as usize, 220.0, 0.1);
        s[10_000] = 1.0;
        despeckle(&mut s, RATE);
        assert!(s[10_000].abs() < 0.2, "spike survived: {}", s[10_000]);
    }

    #[test]
    fn despeckle_keeps_a_loud_syllable() {
        let mut s = tone(RATE as usize, 220.0, 0.8);
        let before = s.clone();
        despeckle(&mut s, RATE);
        assert_eq!(s, before, "a loud but real signal must pass untouched");
    }

    #[test]
    fn gate_quiets_the_pauses_and_keeps_the_speech() {
        // 0.5 s of hiss, 1 s of hiss + speech, 0.5 s of hiss.
        let n = RATE as usize;
        let mut s = vec![0f32; n * 2];
        let mut seed = 12345u32;
        for v in s.iter_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            *v = ((seed >> 8) as f32 / 8_388_608.0 - 1.0) * 0.01; // ~-40 dBFS hiss
        }
        let speech = 0.3;
        for i in 0..n {
            s[n / 2 + i] += speech * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / RATE as f32).sin();
        }
        gate(&mut s, RATE);
        let pause = rms(&s[..n / 2 - 2_000]);
        let talk = rms(&s[n / 2 + 2_000..n / 2 + n - 2_000]);
        assert!(pause < 0.002, "pause still loud: {pause}");
        assert!(talk > 0.1, "speech was gated away: {talk}");
    }

    #[test]
    fn silence_stays_silent() {
        // A take with no speech must come out quieter than it went in, never louder: the
        // peak normaliser used to amplify a gated silence back to full scale.
        let n = RATE as usize * 2;
        let mut seed = 99u32;
        let raw: Vec<f32> = (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                ((seed >> 8) as f32 / 8_388_608.0 - 1.0) * 0.006 // ~-44 dBFS, the measured floor
            })
            .collect();
        let cleaned = clean(&raw, RATE);
        assert!(
            rms(&cleaned) < rms(&raw) * 0.1,
            "silence was amplified: {:.5} -> {:.5}",
            rms(&raw),
            rms(&cleaned)
        );
    }

    #[test]
    fn highpass_kills_dc() {
        let mut s = vec![0.3f32; RATE as usize];
        highpass(&mut s, RATE, 90.0);
        assert!(s[RATE as usize - 1].abs() < 0.01, "DC survived: {}", s[RATE as usize - 1]);
    }
}
