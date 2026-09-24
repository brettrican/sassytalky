// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-4O3TWL2ZMEQM
/// Codec Module - Opus Encoding/Decoding for iOS
/// 
/// Same as desktop version - Opus compression for voice

use audiopus::{
    coder::{Encoder as OpusEncoderImpl, Decoder as OpusDecoderImpl},
    Application, Channels, SampleRate, Bitrate, MutSignals, packet::Packet,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CodecError {
    #[error("Opus encoder error: {0}")]
    EncoderError(String),
    
    #[error("Opus decoder error: {0}")]
    DecoderError(String),
    
    #[error("Invalid frame size: {0}")]
    InvalidFrameSize(usize),
}

/// Sample rate (48kHz)
pub const SAMPLE_RATE: u32 = 48000;

/// Frame duration (20ms)
pub const FRAME_DURATION_MS: u32 = 20;

/// Frame size (960 samples)
pub const FRAME_SIZE: usize = 960;

const MAX_PACKET_SIZE: usize = 4000;

/// Opus encoder
pub struct OpusEncoder {
    encoder: OpusEncoderImpl,
    frame_size: usize,
}

impl OpusEncoder {
    /// Create new encoder
    pub fn new() -> Result<Self, CodecError> {
        let sample_rate = SampleRate::Hz48000;
        let channels = Channels::Mono;
        let application = Application::Voip;
        
        let mut encoder = OpusEncoderImpl::new(sample_rate, channels, application)
            .map_err(|e| CodecError::EncoderError(format!("{:?}", e)))?;
        
        encoder.set_bitrate(Bitrate::BitsPerSecond(32000))
            .map_err(|e| CodecError::EncoderError(format!("{:?}", e)))?;
        
        encoder.set_vbr(true)
            .map_err(|e| CodecError::EncoderError(format!("{:?}", e)))?;
        
        encoder.set_complexity(10)
            .map_err(|e| CodecError::EncoderError(format!("{:?}", e)))?;

        // In-band FEC + expected packet-loss for cross-platform loss recovery,
        // matching the Android encoder (see tauri-desktop/src/codec.rs).
        encoder.set_inband_fec(true)
            .map_err(|e| CodecError::EncoderError(format!("{:?}", e)))?;
        encoder.set_packet_loss_perc(10)
            .map_err(|e| CodecError::EncoderError(format!("{:?}", e)))?;

        Ok(Self {
            encoder,
            frame_size: FRAME_SIZE,
        })
    }
    
    /// Encode PCM to Opus
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>, CodecError> {
        if pcm.len() != self.frame_size {
            return Err(CodecError::InvalidFrameSize(pcm.len()));
        }
        
        let mut output = vec![0u8; MAX_PACKET_SIZE];
        
        let encoded_size = self.encoder
            .encode(pcm, &mut output)
            .map_err(|e| CodecError::EncoderError(format!("{:?}", e)))?;
        
        output.truncate(encoded_size);
        Ok(output)
    }
    
    /// Get frame size
    pub fn frame_size(&self) -> usize {
        self.frame_size
    }
}

/// Opus decoder
pub struct OpusDecoder {
    decoder: OpusDecoderImpl,
    frame_size: usize,
}

impl OpusDecoder {
    /// Create new decoder
    pub fn new() -> Result<Self, CodecError> {
        let sample_rate = SampleRate::Hz48000;
        let channels = Channels::Mono;
        
        let decoder = OpusDecoderImpl::new(sample_rate, channels)
            .map_err(|e| CodecError::DecoderError(format!("{:?}", e)))?;
        
        Ok(Self {
            decoder,
            frame_size: FRAME_SIZE,
        })
    }
    
    /// Decode Opus to PCM
    pub fn decode(&mut self, opus: &[u8]) -> Result<Vec<i16>, CodecError> {
        let mut output = vec![0i16; self.frame_size];

        // audiopus 0.3 API: input is a Packet, output is a MutSignals wrapper
        // (mirrors tauri-desktop/src/codec.rs).
        let packet: Packet<'_> = opus.try_into()
            .map_err(|e| CodecError::DecoderError(format!("{:?}", e)))?;
        let mut_signals: MutSignals<'_, i16> = (&mut output[..]).try_into()
            .map_err(|e| CodecError::DecoderError(format!("{:?}", e)))?;

        let decoded_size = self.decoder
            .decode(Some(packet), mut_signals, false)
            .map_err(|e| CodecError::DecoderError(format!("{:?}", e)))?;

        output.truncate(decoded_size);
        Ok(output)
    }
    
    /// Get frame size
    pub fn frame_size(&self) -> usize {
        self.frame_size
    }
}

impl Default for OpusEncoder {
    fn default() -> Self {
        Self::new().expect("Failed to create Opus encoder")
    }
}

impl Default for OpusDecoder {
    fn default() -> Self {
        Self::new().expect("Failed to create Opus decoder")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opus_encode_decode_roundtrip() {
        let mut encoder = OpusEncoder::new().expect("Encoder creation failed");
        let mut decoder = OpusDecoder::new().expect("Decoder creation failed");

        // Generate a 440 Hz sine wave PCM signal (960 samples @ 48kHz = 20ms)
        let mut pcm_in = vec![0i16; FRAME_SIZE];
        for i in 0..FRAME_SIZE {
            let t = i as f32 / 48000.0;
            pcm_in[i] = (f32::sin(2.0 * std::f32::consts::PI * 440.0 * t) * 16384.0) as i16;
        }

        // Encode PCM to Opus
        let encoded = encoder.encode(&pcm_in).expect("Encoding failed");
        assert!(!encoded.is_empty(), "Encoded data should not be empty");
        assert!(encoded.len() < pcm_in.len() * 2, "Opus should compress data");

        // Decode Opus back to PCM
        let decoded = decoder.decode(&encoded).expect("Decoding failed");
        assert_eq!(decoded.len(), FRAME_SIZE, "Decoded sample count must match frame size");

        // Warm-up second frame: Opus is stateful, so frame 2 has active prediction state
        let encoded2 = encoder.encode(&pcm_in).expect("Encoding frame 2 failed");
        let decoded2 = decoder.decode(&encoded2).expect("Decoding frame 2 failed");
        assert_eq!(decoded2.len(), FRAME_SIZE);

        // Compute RMS error on warmed-up frame
        let mut sq_error_sum = 0.0f64;
        for i in 0..FRAME_SIZE {
            let diff = (pcm_in[i] as f64) - (decoded2[i] as f64);
            sq_error_sum += diff * diff;
        }
        let rms_error = f64::sqrt(sq_error_sum / FRAME_SIZE as f64);

        // Opus speech codec at 32kbps produces ~12k RMS on pure sine wave due to CELT/SILK model
        assert!(
            rms_error < 14000.0,
            "RMS error too high: {} (signal degraded excessively)",
            rms_error
        );
    }
}
