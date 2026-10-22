use lz4_flex::block::decompress;

// Allocation policies, not model maxima: 8x the existing 128 KiB bitmap fixture.
pub(crate) const MAX_BITMAP_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_COMPRESSED_BYTES: usize = MAX_BITMAP_BYTES + MAX_BITMAP_BYTES / 255 + 16;
// One million xyz points occupy 12 MB of f32 output before callback queuing.
const MAX_POINTS: usize = 1_000_000;
// Existing voxel wire layout: 128 x 128 bits per z slice.
const BYTES_PER_ROW: usize = 16;
const BYTES_PER_SLICE: usize = 2048;

pub fn decompress_lz4(compressed: &[u8], decompressed_size: usize) -> Result<Vec<u8>, String> {
    if decompressed_size > MAX_BITMAP_BYTES || compressed.len() > MAX_COMPRESSED_BYTES {
        return Err("LiDAR compressed/decompressed size exceeds resource budget".into());
    }
    decompress(compressed, decompressed_size).map_err(|e| format!("lz4 decompress failed: {e}"))
}

pub(crate) fn validate_geometry(
    size: usize,
    origin: &[f64; 3],
    resolution: f64,
) -> Result<(), String> {
    if size > MAX_BITMAP_BYTES || !resolution.is_finite() || resolution <= 0.0 {
        return Err("Invalid LiDAR size or resolution".into());
    }
    let resolution = resolution as f32;
    let max_indices = [
        BYTES_PER_ROW * 8 - 1,
        BYTES_PER_SLICE / BYTES_PER_ROW - 1,
        size.saturating_sub(1) / BYTES_PER_SLICE,
    ];
    if !resolution.is_finite()
        || resolution <= 0.0
        || origin.iter().zip(max_indices).any(|(&value, index)| {
            !value.is_finite()
                || !(value as f32).is_finite()
                || !((value as f32) + index as f32 * resolution).is_finite()
        })
    {
        return Err("LiDAR coordinates exceed finite f32 range".into());
    }
    Ok(())
}

/// Transfer-ready xyz values, bounded before allocating the point output.
pub fn bits_to_points_optimized(
    buffer: &[u8],
    origin: &[f64; 3],
    resolution: f64,
) -> Result<Vec<f32>, String> {
    validate_geometry(buffer.len(), origin, resolution)?;
    let point_count = buffer
        .iter()
        .try_fold(0usize, |count, byte| {
            count.checked_add(byte.count_ones() as usize)
        })
        .ok_or_else(|| "LiDAR point count overflow".to_string())?;
    if point_count > MAX_POINTS {
        return Err("LiDAR point count exceeds resource budget".into());
    }
    let capacity = point_count
        .checked_mul(3)
        .ok_or_else(|| "LiDAR output size overflow".to_string())?;
    let mut points = Vec::with_capacity(capacity);
    let resolution = resolution as f32;
    let origin_x = origin[0] as f32;
    let origin_y = origin[1] as f32;
    let origin_z = origin[2] as f32;

    for (byte_index, &byte_value) in buffer.iter().enumerate() {
        if byte_value == 0 {
            continue;
        }

        let z = (byte_index / BYTES_PER_SLICE) as f32;
        let n_slice = byte_index % BYTES_PER_SLICE;
        let y = (n_slice / BYTES_PER_ROW) as f32;
        let x_base = (n_slice % BYTES_PER_ROW) * 8;

        // Process bits with reduced branching
        let mut bits = byte_value;
        let mut bit_offset = 0;

        while bits != 0 {
            if (bits & 0x80) != 0 {
                let x = (x_base + bit_offset) as f32;
                points.push(x * resolution + origin_x);
                points.push(y * resolution + origin_y);
                points.push(z * resolution + origin_z);
            }
            bits <<= 1;
            bit_offset += 1;
        }
    }

    Ok(points)
}

/// Original scalar version (for comparison/testing)
#[cfg(test)]
pub fn bits_to_points(buffer: &[u8], origin: &[f64; 3], resolution: f64) -> Vec<f64> {
    let mut points = Vec::new();

    for (byte_index, &byte_value) in buffer.iter().enumerate() {
        if byte_value == 0 {
            continue;
        }

        let z = (byte_index / BYTES_PER_SLICE) as f64;
        let n_slice = byte_index % BYTES_PER_SLICE;
        let y = (n_slice / BYTES_PER_ROW) as f64;
        let x_base = (n_slice % BYTES_PER_ROW) * 8;

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
        // x = x_base + bit_offset, where x_base = (0 % BYTES_PER_ROW) * 8 = 0
        // x = 0, y = 0, z = 0
        // Byte 1 (index 1): 0b0000_0001 (bit 0 from left set)
        // bit_offset in loop: for bit 0, (1 << (7-7)) matches, so bit_offset=7
        // x_base = (1 % BYTES_PER_ROW) * 8 = 8, x = 8 + 7 = 15
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

#[cfg(test)]
mod bounds_tests {
    use super::*;

    #[test]
    fn decompression_rejects_resource_sizes_before_allocation() {
        assert!(decompress_lz4(&[], usize::MAX).is_err());
        assert!(decompress_lz4(&vec![0; MAX_COMPRESSED_BYTES + 1], 1).is_err());
    }

    #[test]
    fn point_output_is_bounded_and_valid_geometry_is_preserved() {
        assert!(bits_to_points_optimized(&vec![255; MAX_POINTS / 8 + 1], &[0.0; 3], 0.04).is_err());
        assert!(bits_to_points_optimized(&[1], &[f64::INFINITY, 0.0, 0.0], 0.04).is_err());
        assert!(bits_to_points_optimized(&[1], &[0.0; 3], f64::MIN_POSITIVE).is_err());
        let buffer = [0x80, 1];
        let actual = bits_to_points_optimized(&buffer, &[1.0; 3], 0.1).unwrap();
        let expected = bits_to_points(&buffer, &[1.0; 3], 0.1);
        for (actual, expected) in actual.iter().zip(expected) {
            assert!((*actual as f64 - expected).abs() < 1e-6);
        }
    }
}
