/// Simple H264 RTP depacketizer for FU-A fragmentation
/// Ref: RFC 6184
const FU_A_TYPE: u8 = 28;

pub struct H264RtpReassembler {
    buffer: Vec<u8>,
    in_progress: bool,
}

impl Default for H264RtpReassembler {
    fn default() -> Self {
        Self::new()
    }
}

impl H264RtpReassembler {
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(65536),
            in_progress: false,
        }
    }

    pub fn process_packet(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.is_empty() {
            return None;
        }

        let nal_type = payload[0] & 0x1F;

        if nal_type == FU_A_TYPE {
            if payload.len() < 2 {
                return None;
            }

            let fu_header = payload[1];
            let start_bit = (fu_header & 0x80) != 0;
            let end_bit = (fu_header & 0x40) != 0;
            let nal_unit_type = fu_header & 0x1F;

            if start_bit {
                self.buffer.clear();
                let reconstructed_nal_header = (payload[0] & 0xE0) | nal_unit_type;
                self.buffer.push(reconstructed_nal_header);
                self.buffer.extend_from_slice(&payload[2..]);
                self.in_progress = true;
                None
            } else if self.in_progress {
                self.buffer.extend_from_slice(&payload[2..]);

                if end_bit {
                    self.in_progress = false;
                    Some(self.buffer.clone())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            Some(payload.to_vec())
        }
    }
}
