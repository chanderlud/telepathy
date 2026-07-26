use std::fmt;

pub const SEAC_MAGIC: u32 = u32::from_be_bytes(*b"seac"); // 0x73 0x65 0x61 0x63

#[inline(always)]
pub fn clamp_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum SeaResidualSize {
    One = 1,
    Two = 2,
    Three = 3,
    Four = 4,
    Five = 5,
    Six = 6,
    Seven = 7,
    Eight = 8,
}

impl SeaResidualSize {
    #[inline(always)]
    pub fn from(len: u8) -> Self {
        match len {
            1 => SeaResidualSize::One,
            2 => SeaResidualSize::Two,
            3 => SeaResidualSize::Three,
            4 => SeaResidualSize::Four,
            5 => SeaResidualSize::Five,
            6 => SeaResidualSize::Six,
            7 => SeaResidualSize::Seven,
            8 => SeaResidualSize::Eight,
            _ => panic!("Invalid residual length"),
        }
    }

    #[inline(always)]
    pub fn try_from_u8(len: u8) -> Option<Self> {
        match len {
            1 => Some(SeaResidualSize::One),
            2 => Some(SeaResidualSize::Two),
            3 => Some(SeaResidualSize::Three),
            4 => Some(SeaResidualSize::Four),
            5 => Some(SeaResidualSize::Five),
            6 => Some(SeaResidualSize::Six),
            7 => Some(SeaResidualSize::Seven),
            8 => Some(SeaResidualSize::Eight),
            _ => None,
        }
    }

    #[inline(always)]
    pub fn to_binary_combinations(self) -> usize {
        1usize << (self as u32)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SeaError {
    ReadError,
    InvalidParameters,
    InvalidFile,
    InvalidFrame,
    EncoderClosed,
    UnsupportedVersion,
    TooManyFrames,
    MetadataTooLarge,
}

impl fmt::Display for SeaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SeaError::ReadError => f.write_str("failed to read SEA data"),
            SeaError::InvalidParameters => f.write_str("invalid SEA parameters"),
            SeaError::InvalidFile => f.write_str("invalid SEA file"),
            SeaError::InvalidFrame => f.write_str("invalid SEA frame"),
            SeaError::EncoderClosed => f.write_str("SEA encoder is closed"),
            SeaError::UnsupportedVersion => f.write_str("unsupported SEA version"),
            SeaError::TooManyFrames => f.write_str("too many SEA frames"),
            SeaError::MetadataTooLarge => f.write_str("SEA metadata too large"),
        }
    }
}

impl std::error::Error for SeaError {}

pub trait SeaEncoderTrait {
    fn encode_into(
        &mut self,
        input_slice: &[i16],
        scale_factors: &mut Vec<u8>,
        residuals: &mut Vec<u8>,
        residual_bits: &mut Vec<u8>,
    );
}
