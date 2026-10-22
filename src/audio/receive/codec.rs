use opus::{Channels, Decoder, Encoder};

pub struct OpusDecoder {
    decoder: Decoder,
    pcm_buffer: Vec<i16>,
    sample_rate: u32,
    channels: u16,
}

impl OpusDecoder {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, String> {
        let ch = match channels {
            1 => Channels::Mono,
            2 => Channels::Stereo,
            _ => return Err(format!("Unsupported channels: {}", channels)),
        };

        let decoder = Decoder::new(sample_rate, ch)
            .map_err(|e| format!("Failed to create Opus decoder: {}", e))?;

        // Opus permits up to 120 ms per packet, counted per channel.
        let pcm_buffer = vec![0i16; (sample_rate as usize * 120 / 1000) * channels as usize];

        Ok(Self {
            decoder,
            pcm_buffer,
            sample_rate,
            channels,
        })
    }

    pub fn decode(&mut self, opus_data: &[u8]) -> Result<DecodedAudio, String> {
        if opus_data.is_empty() {
            return Ok(DecodedAudio {
                data: Vec::new(),
                sample_rate: self.sample_rate,
                channels: self.channels,
            });
        }

        // Decode Opus packet to PCM
        // Returns per-channel sample count (e.g., 960 for stereo 20ms frame)
        let decoded_samples = self
            .decoder
            .decode(opus_data, &mut self.pcm_buffer, false)
            .map_err(|e| format!("Opus decode error: {}", e))?;

        let actual_i16_count = decoded_samples * self.channels as usize;
        let data = self.pcm_buffer[..actual_i16_count].to_vec();

        Ok(DecodedAudio {
            data,
            sample_rate: self.sample_rate,
            channels: self.channels,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_preserves_supported_rates_channels_and_sample_count() {
        for sample_rate in [8000, 12000, 16000, 24000, 48000] {
            for channels in [1, 2] {
                let sample_count = sample_rate as usize / 50 * channels as usize;
                let mut encoder = OpusEncoder::new(sample_rate, channels).unwrap();
                let packet = encoder.encode(&vec![0; sample_count]).unwrap();
                let mut decoder = OpusDecoder::new(sample_rate, channels).unwrap();
                let decoded = decoder.decode(&packet).unwrap();
                assert_eq!(
                    (decoded.sample_rate, decoded.channels),
                    (sample_rate, channels)
                );
                assert_eq!(decoded.data.len(), sample_count);
                let empty = decoder.decode(&[]).unwrap();
                assert_eq!((empty.sample_rate, empty.channels), (sample_rate, channels));
            }
        }
    }
}
