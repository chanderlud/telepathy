use nnnoiseless::FRAME_SIZE;

pub const MAX_PACKED_BYTES: usize = FRAME_SIZE;

pub struct BitUnpacker {
    bits_stored: u32,
    carry: u64,
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
            output: Vec::with_capacity(MAX_PACKED_BYTES),
        }
    }

    pub fn reset_const(&mut self, bitlength: u8) {
        self.bits_stored = 0;
        self.carry = 0;
        self.bitlengths.clear();
        self.bitlengths.push(bitlength);
        self.bitlengths_index = 0;
        self.output.clear();
        self.output
            .reserve(MAX_PACKED_BYTES.saturating_sub(self.output.capacity()));
    }

    pub fn reset_var(&mut self, bitlengths: &[u8]) {
        self.bits_stored = 0;
        self.carry = 0;
        self.bitlengths.clear();
        self.bitlengths.extend_from_slice(bitlengths);
        self.bitlengths_index = 0;
        self.output.clear();
        let expected_items = bitlengths.len().max(MAX_PACKED_BYTES);
        self.output
            .reserve(expected_items.saturating_sub(self.output.capacity()));
    }

    const MASKS: [u64; 9] = [0, 1, 3, 7, 15, 31, 63, 127, 255];

    fn process_bytes_const(&mut self, input: &[u8]) {
        let bits = self.bitlengths[0] as u32;
        let mask = BitUnpacker::MASKS[bits as usize];

        for input_byte in input {
            let value = (self.carry << 8) | u64::from(*input_byte);
            self.bits_stored += 8;

            while self.bits_stored >= bits {
                let item = (value >> (self.bits_stored - bits)) & mask;
                self.output.push(item as u8);
                self.bits_stored -= bits;
            }

            self.carry = if self.bits_stored == 0 {
                0
            } else {
                value & ((1u64 << self.bits_stored) - 1)
            };
        }
    }

    fn process_bytes_variable(&mut self, input: &[u8]) {
        for input_byte in input {
            let value = (self.carry << 8) | u64::from(*input_byte);
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

            self.carry = if self.bits_stored == 0 {
                0
            } else {
                value & ((1u64 << self.bits_stored) - 1)
            };
        }
    }

    pub fn process_bytes(&mut self, input: &[u8]) {
        if self.bitlengths.len() == 1 {
            self.process_bytes_const(input);
            return;
        }
        self.process_bytes_variable(input);
    }

    pub fn finish(&mut self) -> &[u8] {
        self.bitlengths_index = 0;
        self.carry = 0;
        self.bits_stored = 0;
        &self.output
    }

    pub fn take_output(&mut self) -> &mut Vec<u8> {
        self.bitlengths_index = 0;
        self.carry = 0;
        self.bits_stored = 0;
        &mut self.output
    }
}

pub struct BitPacker {
    accum: u64,
    bits_stored: u32,
    output: Vec<u8>,
}

impl Default for BitPacker {
    fn default() -> Self {
        Self {
            accum: 0,
            bits_stored: 0,
            output: Vec::with_capacity(MAX_PACKED_BYTES),
        }
    }
}

impl BitPacker {
    pub fn reset(&mut self) {
        self.accum = 0;
        self.bits_stored = 0;
        self.output.clear();
        self.output
            .reserve(MAX_PACKED_BYTES.saturating_sub(self.output.capacity()));
    }

    pub fn reset_writer(&mut self) {
        self.accum = 0;
        self.bits_stored = 0;
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
        self.accum = (self.accum << bits) | u64::from(value);
        self.bits_stored += bits as u32;

        while self.bits_stored >= 8 {
            let value = self.accum >> (self.bits_stored - 8);
            self.output.push(value as u8);
            self.bits_stored -= 8;
            self.accum = if self.bits_stored == 0 {
                0
            } else {
                self.accum & ((1u64 << self.bits_stored) - 1)
            };
        }
    }

    pub fn push_into(&mut self, input: u32, bits: u8, output: &mut Vec<u8>) {
        debug_assert!(bits <= 8);
        let mask: u32 = (1 << bits as u32) - 1;
        let value = (input) & mask;
        debug_assert!(
            input == value,
            "cannot pack value={} into {} bits",
            input,
            bits
        );
        self.accum = (self.accum << bits) | u64::from(value);
        self.bits_stored += bits as u32;

        while self.bits_stored >= 8 {
            let packed_byte = self.accum >> (self.bits_stored - 8);
            output.push(packed_byte as u8);
            self.bits_stored -= 8;
            self.accum = if self.bits_stored == 0 {
                0
            } else {
                self.accum & ((1u64 << self.bits_stored) - 1)
            };
        }
    }

    pub fn finish(&mut self) -> &[u8] {
        if self.bits_stored > 0 {
            let byte = (self.accum << (8 - self.bits_stored)) as u8;
            self.output.push(byte);
        }
        self.accum = 0;
        self.bits_stored = 0;

        &self.output
    }

    pub fn finish_into(&mut self, output: &mut Vec<u8>) {
        if self.bits_stored > 0 {
            let byte = (self.accum << (8 - self.bits_stored)) as u8;
            output.push(byte);
        }
        self.accum = 0;
        self.bits_stored = 0;
    }

    pub fn drain_into(&mut self, output: &mut Vec<u8>) {
        if self.bits_stored > 0 {
            let byte = (self.accum << (8 - self.bits_stored)) as u8;
            self.output.push(byte);
        }
        self.accum = 0;
        self.bits_stored = 0;
        output.append(&mut self.output);
        self.output.reserve(MAX_PACKED_BYTES);
    }
}
