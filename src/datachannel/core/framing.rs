use crate::protocol::ingress::{parse_json, MAX_BINARY_HEADER_BYTES};
use serde_json::Value;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum BinaryFrame {
    Normal,
    Lidar,
}

pub(super) fn parse(buffer: &[u8]) -> Result<(BinaryFrame, Value, &[u8]), String> {
    let prefix = buffer.get(..4).ok_or("Binary frame is too short")?;
    let frame = match prefix {
        [2, 0, 0, 0] => BinaryFrame::Lidar,
        _ => BinaryFrame::Normal,
    };
    let (start, length) = match frame {
        BinaryFrame::Normal => (
            4usize,
            usize::from(u16::from_le_bytes([prefix[0], prefix[1]])),
        ),
        BinaryFrame::Lidar => {
            let bytes: [u8; 4] = buffer
                .get(4..8)
                .ok_or("Missing LiDAR frame length")?
                .try_into()
                .unwrap();
            (
                12,
                usize::try_from(u32::from_le_bytes(bytes))
                    .map_err(|_| "LiDAR frame length overflow")?,
            )
        }
    };
    if length > MAX_BINARY_HEADER_BYTES {
        return Err("Binary JSON header exceeds 64 KiB".into());
    }
    let end = start
        .checked_add(length)
        .ok_or("Binary frame length overflow")?;
    let json = buffer
        .get(start..end)
        .ok_or("Binary JSON header exceeds frame length")?;
    let header = parse_json(json).map_err(str::to_owned)?;
    if !header.is_object() {
        return Err(format!("{frame:?} binary JSON header must be an object"));
    }
    Ok((frame, header, &buffer[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_binary_header_byte_limit_preserves_lidar_payload() {
        let mut header = br#"{"type":"msg","topic":"lidar"}"#.to_vec();
        header.resize(64 * 1024, b' ');
        let mut frame = vec![2, 0, 0, 0];
        frame.extend_from_slice(&(header.len() as u32).to_le_bytes());
        frame.extend_from_slice(&[0; 4]);
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&[7, 8, 9]);
        let (kind, parsed, payload) = parse(&frame).unwrap();
        assert_eq!(kind, BinaryFrame::Lidar);
        assert_eq!(parsed["topic"], "lidar");
        assert_eq!(payload, [7, 8, 9]);
        frame[4..8].copy_from_slice(&(64u32 * 1024 + 1).to_le_bytes());
        assert_eq!(
            parse(&frame).unwrap_err(),
            "Binary JSON header exceeds 64 KiB"
        );
    }

    #[test]
    fn boundary_binary_headers_share_preallocation_json_depth_policy() {
        let header = format!("{{\"data\":{}null{}}}", "[".repeat(64), "]".repeat(64));
        let mut frame = (header.len() as u16).to_le_bytes().to_vec();
        frame.extend_from_slice(&[1, 0]);
        frame.extend_from_slice(header.as_bytes());
        assert!(parse(&frame).is_err());
    }

    #[test]
    fn framing_rejects_truncation_and_invalid_json() {
        for buffer in [
            &[][..],
            &[2, 0, 0, 0],
            &[255, 255, 0, 0],
            &[2, 0, 0, 0, 255, 255, 255, 255],
            &[1, 0, 0, 0, 255],
        ] {
            assert!(parse(buffer).is_err());
        }
    }

    #[test]
    fn both_schemas_preserve_header_and_body() {
        for mut buffer in [vec![2, 0, 1, 0], vec![2, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]] {
            buffer.extend_from_slice(b"{}");
            buffer.extend_from_slice(&[1, 2, 3]);
            let (_frame, header, body) = parse(&buffer).unwrap();
            assert_eq!(header, serde_json::json!({}));
            assert_eq!(body, &[1, 2, 3]);
        }
    }

    #[test]
    fn both_schemas_reject_non_object_headers() {
        for header in ["null", "0", "true", "\"text\"", "[]"] {
            let length = header.len();
            let mut normal = Vec::from((length as u16).to_le_bytes());
            normal.extend_from_slice(&[1, 0]);
            let mut lidar = vec![2, 0, 0, 0];
            lidar.extend_from_slice(&(length as u32).to_le_bytes());
            lidar.extend_from_slice(&[0; 4]);
            for mut buffer in [normal, lidar] {
                buffer.extend_from_slice(header.as_bytes());
                buffer.extend_from_slice(&[1, 2, 3]);
                let error = parse(&buffer).expect_err("binary headers must be objects");
                assert!(error.contains("must be an object"), "{error}");
            }
        }
    }
}
