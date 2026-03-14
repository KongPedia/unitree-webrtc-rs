use lz4_flex::block::decompress;

pub fn decompress_lz4(compressed: &[u8], decompressed_size: usize) -> Result<Vec<u8>, String> {
    decompress(compressed, decompressed_size).map_err(|e| format!("lz4 decompress failed: {e}"))
}

pub fn bits_to_points(buffer: &[u8], origin: &[f64; 3], resolution: f64) -> Vec<f64> {
    let mut points = Vec::new();

    for (byte_index, &byte_value) in buffer.iter().enumerate() {
        if byte_value == 0 {
            continue;
        }

        let z = (byte_index / 0x800) as f64;
        let n_slice = byte_index % 0x800;
        let y = (n_slice / 0x10) as f64;
        let x_base = (n_slice % 0x10) * 8;

        for bit_offset in 0..8 {
            if (byte_value & (1 << (7 - bit_offset))) != 0 {
                let x = (x_base + bit_offset) as f64;
                // Direct f64 calculation (no f32 intermediate)
                let px = x * resolution + origin[0];
                let py = y * resolution + origin[1];
                let pz = z * resolution + origin[2];
                points.push(px);
                points.push(py);
                points.push(pz);
            }
        }
    }

    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_to_points_produces_xyz_triplets() {
        let buffer = vec![0b0000_0001, 0, 0];
        let origin = [0.0, 0.0, 0.0];
        let resolution = 0.05;
        let points = bits_to_points(&buffer, &origin, resolution);
        assert_eq!(points.len() % 3, 0);
    }

    /// Empty buffer produces empty points.
    #[test]
    fn empty_buffer_produces_no_points() {
        let buffer = vec![];
        let origin = [0.0, 0.0, 0.0];
        let resolution = 0.05;
        let points = bits_to_points(&buffer, &origin, resolution);
        assert_eq!(points.len(), 0);
    }

    /// All-zero buffer produces no points (optimization check).
    #[test]
    fn all_zero_buffer_produces_no_points() {
        let buffer = vec![0u8; 100];
        let origin = [0.0, 0.0, 0.0];
        let resolution = 0.05;
        let points = bits_to_points(&buffer, &origin, resolution);
        assert_eq!(points.len(), 0, "all-zero bytes should skip processing");
    }

    /// Multi-byte pattern with origin offset.
    #[test]
    fn multi_byte_pattern_with_origin_offset() {
        // Byte 0 (index 0): 0b1000_0000 (bit 7 from left set)
        // bit_offset in loop: for bit 7, (1 << (7-0)) matches, so bit_offset=0
        // x = x_base + bit_offset, where x_base = (0 % 0x10) * 8 = 0
        // x = 0, y = 0, z = 0
        // Byte 1 (index 1): 0b0000_0001 (bit 0 from left set)
        // bit_offset in loop: for bit 0, (1 << (7-7)) matches, so bit_offset=7
        // x_base = (1 % 0x10) * 8 = 8, x = 8 + 7 = 15
        let buffer = vec![0b1000_0000, 0b0000_0001];
        let origin = [10.0, 20.0, 30.0];
        let resolution = 0.1;
        let points = bits_to_points(&buffer, &origin, resolution);

        assert_eq!(points.len(), 6, "2 points = 6 values");
        // Point 1: x=0*0.1+10.0=10.0, y=0*0.1+20.0=20.0, z=0*0.1+30.0=30.0
        assert!((points[0] - 10.0).abs() < 1e-9);
        assert!((points[1] - 20.0).abs() < 1e-9);
        assert!((points[2] - 30.0).abs() < 1e-9);
        // Point 2: x=15*0.1+10.0=11.5, y=0*0.1+20.0=20.0, z=0*0.1+30.0=30.0
        assert!((points[3] - 11.5).abs() < 1e-9);
        assert!((points[4] - 20.0).abs() < 1e-9);
        assert!((points[5] - 30.0).abs() < 1e-9);
    }

    /// decompress_lz4 with valid input.
    #[test]
    fn decompress_lz4_valid_input() {
        let data = b"hello world";
        let compressed = lz4_flex::block::compress(data);
        let decompressed = decompress_lz4(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    /// decompress_lz4 with invalid compressed data returns error.
    #[test]
    fn decompress_lz4_invalid_data_returns_error() {
        let garbage = vec![0xFF, 0xAA, 0xBB, 0xCC];
        let result = decompress_lz4(&garbage, 100);
        assert!(result.is_err(), "invalid compressed data should fail");
    }
}
