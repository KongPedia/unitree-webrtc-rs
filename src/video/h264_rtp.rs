/// Simple H264 RTP depacketizer for FU-A fragmentation
/// Ref: RFC 6184
const FU_A_TYPE: u8 = 28;
// Resource policy: cap one pending NAL; this is not a robot/codec maximum.
const MAX_NAL_BYTES: usize = 8 * 1024 * 1024;

pub struct H264RtpReassembler {
    buffer: Vec<u8>,
    in_progress: bool,
    next_rtp: Option<(u16, u32)>,
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
            next_rtp: None,
        }
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.in_progress = false;
        self.next_rtp = None;
    }

    /// Reassemble RTP payloads, rejecting FU-A loss, duplicates, reordering and
    /// timestamp changes. A fresh start fragment recovers an abandoned NAL.
    pub fn process_rtp_packet(
        &mut self,
        payload: &[u8],
        sequence_number: u16,
        timestamp: u32,
    ) -> Option<Vec<u8>> {
        self.process_payload(payload, Some((sequence_number, timestamp)))
    }

    /// Legacy payload-only API; continuity cannot be checked without RTP headers.
    /// Do not mix this and `process_rtp_packet` within a fragmented NAL.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Retain the legacy payload-only entrypoint")
    )]
    pub fn process_packet(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        self.process_payload(payload, None)
    }

    fn process_payload(&mut self, payload: &[u8], rtp: Option<(u16, u32)>) -> Option<Vec<u8>> {
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
        if !start && self.next_rtp != rtp {
            self.reset();
            return None;
        }
        if start
            && rtp
                .zip(self.next_rtp)
                .is_some_and(|((sequence, _), (next_sequence, _))| {
                    sequence.wrapping_sub(next_sequence) >= 0x8000
                })
        {
            // Half-range RTP ordering also handles wraparound; stale starts are
            // duplicates/reordering, not fresh NAL boundaries.
            self.reset();
            return None;
        }
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
        self.next_rtp = rtp.map(|(sequence, timestamp)| (sequence.wrapping_add(1), timestamp));
        if !end {
            return None;
        }
        self.in_progress = false;
        self.next_rtp = None;
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

#[cfg(test)]
mod continuity_tests {
    use super::*;

    #[test]
    fn rtp_discontinuity_discards_the_entire_fragmented_nal() {
        // Each pair is independently invalid after seq=40, ts=90_000:
        // missing packet, duplicate, reordered packet, different access-unit time.
        for (sequence, timestamp) in [(42, 90_000), (40, 90_000), (39, 90_000), (41, 93_000)] {
            let mut nal = H264RtpReassembler::new();
            assert_eq!(
                nal.process_rtp_packet(&[0x7c, 0x85, 0xaa], 40, 90_000),
                None
            );
            assert_eq!(
                nal.process_rtp_packet(&[0x7c, 0x45, 0xbb], sequence, timestamp),
                None,
                "discontinuous FU-A must never reach the decoder"
            );
            assert_eq!(
                nal.process_rtp_packet(&[0x7c, 0x45, 0xcc], 43, 90_000),
                None
            );
            assert_eq!(
                nal.process_rtp_packet(&[0x7c, 0x85, 0x11], 50, 96_000),
                None
            );
            assert_eq!(
                nal.process_rtp_packet(&[0x7c, 0x45, 0x22], 51, 96_000),
                Some(vec![0x65, 0x11, 0x22])
            );
        }
    }

    #[test]
    fn rtp_wraparound_and_timestamps_between_complete_nals_are_valid() {
        let mut nal = H264RtpReassembler::new();
        assert_eq!(
            nal.process_rtp_packet(&[0x7c, 0x85, 0x10], 65535, u32::MAX),
            None
        );
        assert_eq!(
            nal.process_rtp_packet(&[0x7c, 0x45, 0x20], 0, u32::MAX),
            Some(vec![0x65, 0x10, 0x20])
        );
        assert_eq!(
            nal.process_rtp_packet(&[0x61, 0x33], 1, 2_999),
            Some(vec![0x61, 0x33])
        );
        assert_eq!(nal.process_rtp_packet(&[0x7c, 0x85, 0x40], 2, 5_999), None);
        assert_eq!(
            nal.process_rtp_packet(&[0x7c, 0x45, 0x50], 3, 5_999),
            Some(vec![0x65, 0x40, 0x50])
        );
    }

    #[test]
    fn new_start_recovers_without_reusing_abandoned_fragment_bytes() {
        let mut nal = H264RtpReassembler::new();
        nal.process_rtp_packet(&[0x7c, 0x85, 0xff], 10, 90_000);
        nal.process_rtp_packet(&[0x7c, 0x85, 0x11], 20, 93_000);
        assert_eq!(
            nal.process_rtp_packet(&[0x7c, 0x45, 0x22], 21, 93_000),
            Some(vec![0x65, 0x11, 0x22])
        );
    }

    #[test]
    fn rtp_metadata_cannot_bypass_assembly_size_limit() {
        let mut nal = H264RtpReassembler::new();
        nal.process_rtp_packet(&[0x7c, 0x85, 1], 100, 90_000);
        let mut fragment = vec![0; 8 * 1024 * 1024];
        fragment[..2].copy_from_slice(&[0x7c, 0x05]);
        assert_eq!(nal.process_rtp_packet(&fragment, 101, 90_000), None);
        assert_eq!(nal.process_rtp_packet(&[0x7c, 0x45, 2], 102, 90_000), None);
        nal.process_rtp_packet(&[0x7c, 0x85, 3], 103, 93_000);
        assert_eq!(
            nal.process_rtp_packet(&[0x7c, 0x45, 4], 104, 93_000),
            Some(vec![0x65, 3, 4])
        );
    }
}

#[cfg(test)]
mod rtp_api_tests {
    use super::*;

    #[test]
    fn duplicated_or_reordered_starts_cannot_revive_an_older_active_nal() {
        for (start_sequence, end_sequence) in [(40, 41), (39, 40)] {
            let mut nal = H264RtpReassembler::new();
            nal.process_rtp_packet(&[0x7c, 0x85, 0xaa], 40, 90_000);
            nal.process_rtp_packet(&[0x7c, 0x05, 0xbb], 41, 90_000);
            nal.process_rtp_packet(&[0x7c, 0x85, 0xcc], start_sequence, 90_000);
            assert_eq!(
                nal.process_rtp_packet(&[0x7c, 0x45, 0xdd], end_sequence, 90_000),
                None,
                "a stale start fragment must discard, not revive, the active NAL"
            );
        }
    }

    #[test]
    fn duplicated_start_and_mixed_payload_api_cannot_complete_a_nal() {
        let mut nal = H264RtpReassembler::new();
        nal.process_rtp_packet(&[0x7c, 0x85, 1], 10, 90_000);
        nal.process_rtp_packet(&[0x7c, 0x85, 1], 10, 90_000);
        assert_eq!(nal.process_rtp_packet(&[0x7c, 0x45, 2], 11, 90_000), None);
        nal.process_rtp_packet(&[0x7c, 0x85, 3], 12, 93_000);
        assert_eq!(nal.process_packet(&[0x7c, 0x45, 4]), None);
        nal.process_packet(&[0x7c, 0x85, 5]);
        assert_eq!(nal.process_rtp_packet(&[0x7c, 0x45, 6], 14, 96_000), None);
    }
}
