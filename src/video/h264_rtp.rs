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

#[cfg(test)]
mod tests {
    use super::*;

    /// Single NAL unit (non-FU-A) is returned immediately.
    #[test]
    fn single_nal_unit_returned_immediately() {
        let mut reassembler = H264RtpReassembler::new();
        let payload = vec![0x65, 0x88, 0x84, 0x00]; // NAL type 5 (IDR)

        let result = reassembler.process_packet(&payload);

        assert!(result.is_some());
        assert_eq!(result.unwrap(), payload);
    }

    /// FU-A fragmented NAL: start fragment returns None, middle returns None, end returns complete NAL.
    #[test]
    fn fragmented_nal_reassembles_correctly() {
        let mut reassembler = H264RtpReassembler::new();

        // FU-A start: NAL header 0x7C, FU header 0x85 (start=1, type=5)
        let start_fragment = vec![0x7C, 0x85, 0x01, 0x02, 0x03];
        let result1 = reassembler.process_packet(&start_fragment);
        assert!(result1.is_none(), "start fragment should return None");

        // FU-A middle: NAL header 0x7C, FU header 0x05 (start=0, end=0, type=5)
        let middle_fragment = vec![0x7C, 0x05, 0x04, 0x05, 0x06];
        let result2 = reassembler.process_packet(&middle_fragment);
        assert!(result2.is_none(), "middle fragment should return None");

        // FU-A end: NAL header 0x7C, FU header 0x45 (end=1, type=5)
        let end_fragment = vec![0x7C, 0x45, 0x07, 0x08];
        let result3 = reassembler.process_packet(&end_fragment);

        assert!(result3.is_some(), "end fragment should return complete NAL");
        let nal = result3.unwrap();
        // Reconstructed NAL header: (0x7C & 0xE0) | 0x05 = 0x60 | 0x05 = 0x65
        assert_eq!(nal[0], 0x65, "reconstructed NAL header");
        // Payload: [0x01, 0x02, 0x03] + [0x04, 0x05, 0x06] + [0x07, 0x08]
        assert_eq!(
            nal,
            vec![0x65, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]
        );
    }

    /// Incomplete fragment (middle without start) returns None.
    #[test]
    fn incomplete_fragment_returns_none() {
        let mut reassembler = H264RtpReassembler::new();

        // FU-A middle without prior start
        let middle_fragment = vec![0x7C, 0x05, 0x01, 0x02];
        let result = reassembler.process_packet(&middle_fragment);

        assert!(result.is_none(), "middle without start should return None");
    }

    /// Empty payload returns None.
    #[test]
    fn empty_payload_returns_none() {
        let mut reassembler = H264RtpReassembler::new();
        let result = reassembler.process_packet(&[]);
        assert!(result.is_none());
    }

    /// FU-A packet with insufficient length (< 2 bytes) returns None.
    #[test]
    fn fu_a_insufficient_length_returns_none() {
        let mut reassembler = H264RtpReassembler::new();
        let short_payload = vec![FU_A_TYPE]; // Only 1 byte
        let result = reassembler.process_packet(&short_payload);
        assert!(result.is_none());
    }
}
