//! Audio coming in from a device, for tracks that read `input(1)`.

use niminal_engine::{BLOCK, MAX_CHANNELS};
use rtrb::{Consumer, Producer, RingBuffer};

/// The receiving end of the audio input: frames of every channel, interleaved, put there by whatever
/// listens to the device.
pub struct AudioInput {
    rx: Consumer<f32>,
    channels: usize,
}

impl AudioInput {
    /// A ring that holds `frames` frames of `channels` channels. The producer takes interleaved samples.
    pub fn ring(channels: usize, frames: usize) -> (Producer<f32>, AudioInput) {
        let channels = channels.clamp(1, MAX_CHANNELS);
        let (tx, rx) = RingBuffer::new(channels * frames.max(BLOCK));
        (tx, AudioInput { rx, channels })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The next `n` frames, a buffer for each channel. A frame the device hasn't delivered yet is silence;
    /// a frame is never split across channels.
    pub fn read(&mut self, n: usize, out: &mut [[f32; BLOCK]; MAX_CHANNELS]) {
        for i in 0..n.min(BLOCK) {
            let ready = self.rx.slots() >= self.channels;
            for channel in out.iter_mut().take(self.channels) {
                channel[i] = if ready { self.rx.pop().unwrap_or(0.0) } else { 0.0 };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_come_out_in_order_and_silence_fills_a_shortfall() {
        let (mut tx, mut input) = AudioInput::ring(2, 64);
        for i in 0..3 {
            tx.push(i as f32).unwrap();
            tx.push(10.0 + i as f32).unwrap();
        }
        let mut out = [[1.0; BLOCK]; MAX_CHANNELS];
        input.read(5, &mut out);
        assert_eq!(&out[0][..5], &[0.0, 1.0, 2.0, 0.0, 0.0]);
        assert_eq!(&out[1][..5], &[10.0, 11.0, 12.0, 0.0, 0.0]);
        // a half frame waits for its other half
        tx.push(7.0).unwrap();
        input.read(1, &mut out);
        assert_eq!((out[0][0], out[1][0]), (0.0, 0.0));
        tx.push(8.0).unwrap();
        input.read(1, &mut out);
        assert_eq!((out[0][0], out[1][0]), (7.0, 8.0));
    }
}
