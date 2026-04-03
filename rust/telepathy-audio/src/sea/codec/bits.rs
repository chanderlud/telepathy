use std::mem;

pub struct BitUnpacker {
    bits_stored: u32,
    carry: u32,
    bitlengths: Vec<u8>,
    bitlengths_index: usize,
    output: Vec<u8>,
}

impl BitUnpacker {
    pub fn new_const_bits(bitlength: u8) -> Self {
        Self {
            bits_stored: 0,
            carry: 0,
            bitlengths: vec![bitlength; 1],
            bitlengths_index: 0,
            output: Vec::new(),
        }
    }

    pub fn new_var_bits(bitlengths: &[u8]) -> Self {
        Self {
            bits_stored: 0,
            carry: 0,
            bitlengths: bitlengths.to_vec(),
            bitlengths_index: 0,
            output: Vec::new(),
        }
    }

    const MASKS: [u32; 9] = [0, 1, 3, 7, 15, 31, 63, 127, 255];

    fn process_bytes_const(&mut self, input: &[u8]) {
        let bits = self.bitlengths[0] as u32;
        let mask = BitUnpacker::MASKS[bits as usize];

        for input_byte in input {
            let value: u32 = (self.carry << 8) | (*input_byte as u32);
            self.bits_stored += 8;

            while self.bits_stored >= bits {
                let item = (value >> (self.bits_stored - bits)) & mask;
                self.output.push(item as u8);
                self.bits_stored -= bits;
            }

            self.carry = value & ((1 << self.bits_stored) - 1);
        }
    }

    fn process_bytes_variable(&mut self, input: &[u8]) {
        for input_byte in input {
            let value: u32 = (self.carry << 8) | (*input_byte as u32);
            self.bits_stored += 8;

            while self.bitlengths_index < self.bitlengths.len()
                && self.bits_stored >= self.bitlengths[self.bitlengths_index] as u32
            {
                let bits = self.bitlengths[self.bitlengths_index] as u32;
                let mask = BitUnpacker::MASKS[bits as usize];
                let item = (value >> (self.bits_stored - bits)) & mask;
                self.output.push(item as u8);
                self.bits_stored -= bits;
                self.bitlengths_index += 1;
            }

            self.carry = value & ((1 << self.bits_stored) - 1);
        }
    }

    pub fn process_bytes(&mut self, input: &[u8]) {
        if self.bitlengths.len() == 1 {
            self.process_bytes_const(input);
            return;
        }
        self.process_bytes_variable(input);
    }

    pub fn finish(&mut self) -> Vec<u8> {
        self.bitlengths.clear();
        self.bitlengths_index = 0;
        self.carry = 0;
        self.bits_stored = 0;
        mem::take(&mut self.output)
    }
}

pub struct BitPacker {
    accum: u32,
    bits_stored: u32,
    output: Vec<u8>,
}

impl BitPacker {
    pub fn new() -> Self {
        Self {
            accum: 0,
            bits_stored: 0,
            output: Vec::new(),
        }
    }

    pub fn push(&mut self, input: u32, bits: u8) {
        debug_assert!(bits <= 8);
        let mask: u32 = (1 << bits as u32) - 1;
        let value = (input) & mask;
        debug_assert!(
            input == value,
            "cannot pack value={} into {} bits",
            input,
            bits
        );
        self.accum = (self.accum << bits) | value;
        self.bits_stored += bits as u32;

        if self.bits_stored >= 8 {
            let value = self.accum >> (self.bits_stored - 8);
            self.output.push(value as u8);
            self.bits_stored -= 8;
            self.accum &= (1 << self.bits_stored) - 1;
        }
    }

    pub fn finish(&mut self) -> Vec<u8> {
        if self.bits_stored > 0 {
            let byte = (self.accum << (8 - self.bits_stored)) as u8;
            self.output.push(byte);
        }
        self.accum = 0;
        self.bits_stored = 0;

        mem::take(&mut self.output)
    }
}

#[cfg(test)]
mod tests {
    use super::{BitPacker, BitUnpacker};

    #[test]
    fn bit_packing_round_trip_for_all_widths() {
        for bits in 1u8..=8 {
            let mask = (1u32 << bits) - 1;
            let values: Vec<u8> = (0..257)
                .map(|i| (((i * 37) as u32 + bits as u32) & mask) as u8)
                .collect();

            let mut packer = BitPacker::new();
            for value in &values {
                packer.push(*value as u32, bits);
            }
            let encoded = packer.finish();

            let split = encoded.len() / 2;
            let mut unpacker = BitUnpacker::new_const_bits(bits);
            unpacker.process_bytes(&encoded[..split]);
            unpacker.process_bytes(&encoded[split..]);

            let mut decoded = unpacker.finish();
            decoded.truncate(values.len());

            assert_eq!(decoded, values, "mismatch for bit width {bits}");
        }
    }
}
