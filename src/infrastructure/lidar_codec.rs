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
}
