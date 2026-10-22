/// Simple H264 RTP depacketizer for FU-A fragmentation
/// Ref: RFC 6184
const FU_A_TYPE: u8 = 28;
// Resource policy: cap one pending NAL; this is not a robot/codec maximum.
const MAX_NAL_BYTES: usize = 8 * 1024 * 1024;

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

    fn reset(&mut self) {
        self.buffer.clear();
        self.in_progress = false;
    }

    pub fn process_packet(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.is_empty() || payload.len() > MAX_NAL_BYTES || payload[0] & 0x80 != 0 {
            self.reset();
            return None;
        }

        let nal_type = payload[0] & 0x1f;
        if nal_type != FU_A_TYPE {
            self.reset();
            // Aggregation packets are not single NAL units; do not send them to the decoder.
            return (1..=23).contains(&nal_type).then(|| payload.to_vec());
        }
        if payload.len() < 2 {
            self.reset();
            return None;
        }

        let fu_header = payload[1];
        let start = fu_header & 0x80 != 0;
        let end = fu_header & 0x40 != 0;
        let unit_type = fu_header & 0x1f;
        if (start && end) || fu_header & 0x20 != 0 || !(1..=23).contains(&unit_type) {
            self.reset();
            return None;
        }
        let header = (payload[0] & 0x60) | unit_type;
        if start {
            self.reset();
            self.buffer.push(header);
            self.in_progress = true;
        } else if !self.in_progress || self.buffer.first() != Some(&header) {
            self.reset();
            return None;
        }
        if payload.len() - 2 > MAX_NAL_BYTES - self.buffer.len() {
            self.reset();
            return None;
        }
        self.buffer.extend_from_slice(&payload[2..]);
        if !end {
            return None;
        }
        self.in_progress = false;
        Some(std::mem::take(&mut self.buffer))
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

#[cfg(test)]
mod bounds_tests {
    use super::*;

    #[test]
    fn malformed_fragment_discards_pending_nal() {
        let mut reassembler = H264RtpReassembler::new();
        reassembler.process_packet(&[0x7c, 0x85, 1]);
        assert!(reassembler.process_packet(&[0x7c]).is_none());
        assert!(reassembler.process_packet(&[0x7c, 0x45, 2]).is_none());
    }

    #[test]
    fn mismatched_fragment_discards_pending_nal() {
        let mut reassembler = H264RtpReassembler::new();
        reassembler.process_packet(&[0x7c, 0x85, 1]);
        assert!(reassembler.process_packet(&[0x7c, 0x41, 2]).is_none());
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;

    #[test]
    fn oversized_assembly_is_discarded_and_can_restart() {
        let mut reassembler = H264RtpReassembler::new();
        reassembler.process_packet(&[0x7c, 0x85, 1]);
        let mut fragment = vec![0; MAX_NAL_BYTES];
        fragment[..2].copy_from_slice(&[0x7c, 0x05]);
        reassembler.process_packet(&fragment);
        assert!(reassembler.process_packet(&[0x7c, 0x45, 2]).is_none());
        assert!(reassembler.buffer.is_empty());
        reassembler.process_packet(&[0x7c, 0x85, 3]);
        assert_eq!(
            reassembler.process_packet(&[0x7c, 0x45, 4]),
            Some(vec![0x65, 3, 4])
        );
    }

    #[test]
    fn interruption_and_invalid_headers_reset_assembly() {
        for interruption in [
            &[][..],
            &[0x65, 3],
            &[0x7c, 0xc5, 3],
            &[0xfc, 0x45, 3],
            &[0x7c, 0x65, 3],
        ] {
            let mut reassembler = H264RtpReassembler::new();
            reassembler.process_packet(&[0x7c, 0x85, 1]);
            reassembler.process_packet(interruption);
            assert!(reassembler.process_packet(&[0x7c, 0x45, 2]).is_none());
        }
    }
}
