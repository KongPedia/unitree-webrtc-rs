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
    let end = start
        .checked_add(length)
        .ok_or("Binary frame length overflow")?;
    let json = buffer
        .get(start..end)
        .ok_or("Binary JSON header exceeds frame length")?;
    let header: Value = serde_json::from_slice(json)
        .map_err(|error| format!("Invalid {frame:?} binary JSON header: {error}"))?;
    if !header.is_object() {
        return Err(format!("{frame:?} binary JSON header must be an object"));
    }
    Ok((frame, header, &buffer[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

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
