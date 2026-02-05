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
