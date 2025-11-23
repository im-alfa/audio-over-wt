use crate::{
    constants::FRAME_SIZE_DURATION,
    errors::{DecoderErrors, EncoderErrors},
};
use audiopus::{
    Application, Bitrate, Channels, Error as OpusError, ErrorCode as OpusErrorCode, MutSignals,
    SampleRate, packet::Packet,
};
use dasp::{Signal, interpolate::linear::Linear, signal};
use tracing::warn;

pub type AudioFrame = Vec<f32>;
pub type EncodedFrame = Vec<u8>;

pub fn resample(samples: AudioFrame, sample_rate: i32, target_sample_rate: i32) -> Vec<f32> {
    if sample_rate == target_sample_rate {
        return samples;
    }

    let mut signal = signal::from_iter(samples.iter().copied());
    let a = signal.next();
    let b = signal.next();

    let linear = Linear::new(a, b);

    signal
        .from_hz_to_hz(linear, sample_rate as f64, target_sample_rate as f64)
        .take(samples.len() * (target_sample_rate as usize) / (sample_rate as usize))
        .collect()
}

pub trait Encoder: Sized {
    fn new(bitrate: i32) -> Result<Self, EncoderErrors>;
    fn encode(&self, frame: AudioFrame, sample_rate: i32) -> Result<EncodedFrame, EncoderErrors>;
}

pub trait Decoder: Sized {
    fn new() -> Result<Self, DecoderErrors>;
    fn decode(&mut self, encoded_frame: EncodedFrame) -> Result<AudioFrame, DecoderErrors>;
}

pub struct OpusEncoder {
    encoder: audiopus::coder::Encoder,
}

impl Encoder for OpusEncoder {
    fn new(bitrate: i32) -> Result<Self, EncoderErrors> {
        let mut encoder =
            audiopus::coder::Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)
                .map_err(|e| EncoderErrors::InitializationError(e.to_string()))?;

        encoder
            .set_bitrate(Bitrate::BitsPerSecond(bitrate))
            .map_err(|e| EncoderErrors::BitrateError(e.to_string()))?;

        Ok(Self { encoder })
    }

    fn encode(&self, frame: AudioFrame, sample_rate: i32) -> Result<EncodedFrame, EncoderErrors> {
        // Resample if needed
        let samples = match sample_rate {
            48000 => frame,
            _ => resample(frame, sample_rate, 48000i32),
        };

        let mut output = vec![0u8; samples.len().max(128)];
        match self.encoder.encode_float(&samples, &mut output) {
            Err(OpusError::Opus(OpusErrorCode::BufferTooSmall)) => {
                warn!(
                    "Needed to increase buffer size, opus is compressing less well than expected."
                );
                output.resize(output.len() * 2, 0u8);
            }
            Err(e) => {
                return Err(EncoderErrors::EncodingError(e.to_string()));
            }
            _ => {}
        }

        Ok(output)
    }
}

pub struct OpusDecoder {
    decoder: audiopus::coder::Decoder,
}

impl Decoder for OpusDecoder {
    fn new() -> Result<Self, DecoderErrors> {
        let decoder = audiopus::coder::Decoder::new(SampleRate::Hz48000, Channels::Mono)
            .map_err(|e| DecoderErrors::InitializationError(e.to_string()))?;

        Ok(Self { decoder })
    }

    fn decode(&mut self, data: EncodedFrame) -> Result<AudioFrame, DecoderErrors> {
        let mut output = vec![0f32; 20 * 48];

        let actual_size = self
            .decoder
            .decode_float(
                Some(Packet::try_from(data.as_slice()).map_err(|e| {
                    DecoderErrors::PacketFormatError(format!("Error parsing frame: {e:?}"))
                })?),
                MutSignals::try_from(output.as_mut_slice()).map_err(|e| {
                    DecoderErrors::PacketFormatError(format!("Error parsing samples: {e:?}"))
                })?,
                false, // FEC
            )
            .map_err(|e| DecoderErrors::DecodingError(e.to_string()))?;

        // Check if the frame size is correct.
        // 48 samples per frame * 20 ms per frame
        if actual_size != (48 * FRAME_SIZE_DURATION) as usize {
            return Err(DecoderErrors::PacketFormatError(
                "Invalid frame size".to_string(),
            ));
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_new() {
        let encoder = OpusEncoder::new(24000);
        assert!(encoder.is_ok());
    }

    #[test]
    fn test_encode_basic() {
        let encoder = OpusEncoder::new(24000).unwrap();
        let frame_size = 48 * FRAME_SIZE_DURATION;
        let frame = vec![0.0f32; frame_size as usize];

        let result = encoder.encode(frame, 48000);
        assert!(result.is_ok());
    }

    #[test]
    fn test_resample_same_rate() {
        let samples = vec![0.1, 0.2, 0.3];
        let result = resample(samples.clone(), 48000, 48000);
        assert_eq!(samples, result);
    }

    #[test]
    fn test_decoder_new() {
        let decoder = OpusDecoder::new();
        assert!(decoder.is_ok());
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let encoder = OpusEncoder::new(24000).unwrap();
        let mut decoder = OpusDecoder::new().unwrap();

        let frame_size = 48 * FRAME_SIZE_DURATION;
        let original = vec![0.5f32; frame_size as usize];

        let encoded = encoder.encode(original.clone(), 48000).unwrap();
        let decoded = decoder.decode(encoded).unwrap();

        assert_eq!(decoded.len(), original.len());

        let max_delta = original
            .iter()
            .zip(decoded.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);

        assert!(
            max_delta < 1.0,
            "Max delta should be less than 1.0, got: {}",
            max_delta
        );

        let max_amplitude = decoded.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);
        assert!(max_amplitude > 0.01, "Decoded audio should not be silent");
    }
}
