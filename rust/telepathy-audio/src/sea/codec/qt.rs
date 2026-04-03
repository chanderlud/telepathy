#[derive(Debug, PartialEq)]
pub struct SeaQuantTab {
    pub offsets: [usize; 9],
    pub quant_tab: [u8; 5 + 9 + 17 + 33 + 65 + 129 + 257 + 513],
}

impl SeaQuantTab {
    // use zig-zag pattern to decrease quantization error
    fn fill_dqt_table(slice: &mut [u8], items: usize) {
        let midpoint = items / 2;
        let mut x = (items / 2 - 1) as i32;
        slice[0] = x as u8;
        for i in (1..midpoint).step_by(2) {
            slice[i] = x as u8;
            slice[i + 1] = x as u8;
            x -= 2;
        }
        x = 0;
        for i in (midpoint..(items - 1)).step_by(2) {
            slice[i] = x as u8;
            slice[i + 1] = x as u8;
            x += 2;
        }
        slice[items - 1] = (x - 2) as u8;

        // special case when residual_size = 2
        if items == 9 {
            slice[2] = 1;
            slice[6] = 0;
        }
    }

    pub fn init() -> Self {
        let mut offsets = [0; 9];
        let mut quant_tab = [0; 5 + 9 + 17 + 33 + 65 + 129 + 257 + 513];

        let mut current_offset = 0;
        for shift in 2..=9 {
            offsets[shift - 1] = current_offset;

            let items = (1 << shift) + 1;

            Self::fill_dqt_table(
                &mut quant_tab[current_offset..current_offset + items],
                items,
            );

            current_offset += items;
        }

        Self { offsets, quant_tab }
    }
}

#[cfg(test)]
mod tests {
    use super::SeaQuantTab;
    use crate::sea::codec::{
        common::SeaResidualSize,
        dqt::SeaDequantTab,
        encoder_base::sea_div,
    };

    fn all_residual_sizes() -> [SeaResidualSize; 8] {
        [
            SeaResidualSize::One,
            SeaResidualSize::Two,
            SeaResidualSize::Three,
            SeaResidualSize::Four,
            SeaResidualSize::Five,
            SeaResidualSize::Six,
            SeaResidualSize::Seven,
            SeaResidualSize::Eight,
        ]
    }

    #[test]
    fn quantize_then_dequantize_round_trip_for_all_residual_sizes() {
        let quant = SeaQuantTab::init();
        let dequant = SeaDequantTab::init(4);

        for residual_size in all_residual_sizes() {
            let bits = residual_size as usize;
            let clamp_limit = residual_size.to_binary_combinations() as i32;
            let quant_offset = quant.offsets[bits] as i32 + clamp_limit;

            let reciprocal = dequant.get_scalefactor_reciprocals(bits)[0] as i64;
            let dqt_row = &dequant.get_dqt(bits)[0];

            for expected in dqt_row {
                let scaled = sea_div(*expected, reciprocal);
                let clamped = scaled.clamp(-clamp_limit, clamp_limit);
                let quantized = quant.quant_tab[(quant_offset + clamped) as usize] as usize;
                let reconstructed = dqt_row[quantized];

                assert_eq!(
                    reconstructed, *expected,
                    "round-trip mismatch for residual_size={bits}, expected={expected}, quantized={quantized}"
                );
            }
        }
    }
}
