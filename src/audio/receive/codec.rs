use opus::{Channels, Decoder, Encoder};

pub struct OpusDecoder {
    decoder: Decoder,
    pcm_buffer: Vec<i16>,
}

// SAFETY: OpusDecoder is Send because:
// 1. The opus::Decoder is a stateful decoder that maintains internal state
// 2. This decoder is ONLY used within a single tokio task (spawn_audio_handler) and never
//    shared across threads - it's a single-owner pattern
// 3. The pcm_buffer is an owned Vec<i16> which is Send
// 4. No interior mutability or shared references exist that would violate Send
unsafe impl Send for OpusDecoder {}

impl OpusDecoder {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, String> {
        let ch = match channels {
            1 => Channels::Mono,
            2 => Channels::Stereo,
            _ => return Err(format!("Unsupported channels: {}", channels)),
        };

        let decoder = Decoder::new(sample_rate, ch)
            .map_err(|e| format!("Failed to create Opus decoder: {}", e))?;

        // Pre-allocate buffer for 960 samples stereo (typical Opus frame @ 48kHz)
        let pcm_buffer = vec![0i16; 960 * 2];

        Ok(Self {
            decoder,
            pcm_buffer,
        })
    }

    pub fn decode(&mut self, opus_data: &[u8]) -> Result<DecodedAudio, String> {
        if opus_data.is_empty() {
            return Ok(DecodedAudio {
                data: Vec::new(),
                sample_rate: 48000,
                channels: 2,
            });
        }

        // Resize buffer if needed (max 120ms @ 48kHz stereo)
        let max_samples = 5760 * 2;
        if self.pcm_buffer.len() < max_samples {
            self.pcm_buffer.resize(max_samples, 0);
        }

        // Decode Opus packet to PCM
        // Returns per-channel sample count (e.g., 960 for stereo 20ms frame)
        let decoded_samples = self
            .decoder
            .decode(opus_data, &mut self.pcm_buffer, false)
            .map_err(|e| format!("Opus decode error: {}", e))?;

        // For stereo, actual i16 count = samples * 2 (interleaved L/R)
        let actual_i16_count = decoded_samples * 2;
        let data = self.pcm_buffer[..actual_i16_count].to_vec();

        Ok(DecodedAudio {
            data,
            sample_rate: 48000,
            channels: 2,
        })
    }
}

pub struct DecodedAudio {
    pub data: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
}

pub struct OpusEncoder {
    encoder: Encoder,
    opus_buffer: Vec<u8>,
}

// SAFETY: OpusEncoder is Send because:
// 1. The opus::Encoder is a stateful encoder that maintains internal state
// 2. This encoder is currently unused but designed for single-owner pattern usage
// 3. The opus_buffer is an owned Vec<u8> which is Send
// 4. No interior mutability or shared references exist that would violate Send
unsafe impl Send for OpusEncoder {}

impl OpusEncoder {
    #[allow(dead_code)]
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, String> {
        let ch = match channels {
            1 => Channels::Mono,
            2 => Channels::Stereo,
            _ => return Err(format!("Unsupported channels: {}", channels)),
        };

        let encoder = Encoder::new(sample_rate, ch, opus::Application::Audio)
            .map_err(|e| format!("Failed to create Opus encoder: {}", e))?;

        // Pre-allocate buffer for max Opus packet size (4000 bytes)
        let opus_buffer = vec![0u8; 4000];

        Ok(Self {
            encoder,
            opus_buffer,
        })
    }

    #[allow(dead_code)]
    pub fn encode(&mut self, pcm_data: &[i16]) -> Result<Vec<u8>, String> {
        if pcm_data.is_empty() {
            return Ok(Vec::new());
        }

        // Encode PCM to Opus
        let encoded_len = self
            .encoder
            .encode(pcm_data, &mut self.opus_buffer)
            .map_err(|e| format!("Opus encode error: {}", e))?;

        // Extract only the encoded bytes
        Ok(self.opus_buffer[..encoded_len].to_vec())
    }
}
