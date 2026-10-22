//! iNES cartridge parsing and initial NROM mapper 0 support.

use core::fmt;

/// iNES header length.
pub const INES_HEADER_LEN: usize = 16;
const TRAINER_LEN: usize = 512;
const PRG_BANK_LEN: usize = 16 * 1024;
const CHR_BANK_LEN: usize = 8 * 1024;

/// Cartridge nametable mirroring mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mirroring {
    /// Horizontal mirroring.
    Horizontal,
    /// Vertical mirroring.
    Vertical,
    /// Four-screen memory supplied by the cartridge.
    FourScreen,
}

/// Parsed NROM cartridge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Nrom {
    prg_rom: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    mirroring: Mirroring,
}

/// Invalid or unsupported cartridge image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CartridgeError {
    /// Image is shorter than the header.
    TruncatedHeader,
    /// iNES magic signature is invalid.
    InvalidMagic,
    /// NES 2.0 is recognized but not implemented.
    Nes2NotSupported,
    /// Mapper other than zero was requested.
    UnsupportedMapper(u16),
    /// PRG size is not valid for NROM.
    InvalidPrgSize(usize),
    /// Declared cartridge data is truncated.
    TruncatedImage {
        /// Minimum image length required by the header.
        expected: usize,
        /// Actual image length.
        actual: usize,
    },
}

impl fmt::Display for CartridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader => f.write_str("cartridge image is shorter than the 16-byte iNES header"),
            Self::InvalidMagic => f.write_str("cartridge image has an invalid iNES signature"),
            Self::Nes2NotSupported => f.write_str("NES 2.0 cartridge headers are not supported yet"),
            Self::UnsupportedMapper(m) => write!(f, "unsupported cartridge mapper: {m}"),
            Self::InvalidPrgSize(n) => write!(f, "invalid NROM PRG-ROM size: {n} bytes"),
            Self::TruncatedImage { expected, actual } => write!(f, "truncated cartridge image: expected at least {expected} bytes, got {actual}"),
        }
    }
}
impl std::error::Error for CartridgeError {}

impl Nrom {
    /// Parses a legacy iNES image and accepts mapper 0 with one or two PRG banks.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }

        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 0 { return Err(CartridgeError::UnsupportedMapper(mapper)); }

        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if prg_len != PRG_BANK_LEN && prg_len != 2 * PRG_BANK_LEN {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len + chr_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }

        let prg_start = INES_HEADER_LEN + trainer_len;
        let chr_start = prg_start + prg_len;
        let prg_rom = image[prg_start..chr_start].to_vec();
        let chr_is_ram = chr_len == 0;
        let chr = if chr_is_ram { vec![0; CHR_BANK_LEN] } else { image[chr_start..chr_start + chr_len].to_vec() };
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen }
            else if image[6] & 0x01 != 0 { Mirroring::Vertical }
            else { Mirroring::Horizontal };
        Ok(Self { prg_rom, chr, chr_is_ram, mirroring })
    }

    /// Reads PRG-ROM at a CPU address from 0x8000 to 0xFFFF.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        if address < 0x8000 { return None; }
        Some(self.prg_rom[(address as usize - 0x8000) % self.prg_rom.len()])
    }

    /// Returns cartridge nametable mirroring.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// Returns true when character memory is RAM.
    pub fn chr_is_ram(&self) -> bool { self.chr_is_ram }

    /// Reads a byte from PPU pattern memory.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        Some(self.chr[address as usize % self.chr.len()])
    }

    /// Writes pattern memory only when it is CHR-RAM.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 || !self.chr_is_ram { return false; }
        let index = address as usize % self.chr.len();
        self.chr[index] = value;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(prg: u8, chr: u8, flags6: u8, flags7: u8) -> Vec<u8> {
        let mut v = vec![0; 16 + prg as usize * PRG_BANK_LEN + chr as usize * CHR_BANK_LEN];
        v[0..4].copy_from_slice(b"NES\x1A"); v[4] = prg; v[5] = chr; v[6] = flags6; v[7] = flags7; v
    }
    #[test] fn rejects_short_or_invalid_header() {
        assert_eq!(Nrom::parse(&[0; 8]), Err(CartridgeError::TruncatedHeader));
        assert_eq!(Nrom::parse(&[0; 16]), Err(CartridgeError::InvalidMagic));
    }
    #[test] fn one_prg_bank_is_mirrored() {
        let mut v = image(1, 1, 0, 0); v[16] = 0x42; v[16 + PRG_BANK_LEN - 1] = 0x99;
        let c = Nrom::parse(&v).unwrap();
        assert_eq!(c.cpu_read(0x8000), Some(0x42)); assert_eq!(c.cpu_read(0xC000), Some(0x42));
        assert_eq!(c.cpu_read(0xFFFF), Some(0x99)); assert_eq!(c.cpu_read(0x7FFF), None);
    }
    #[test] fn supports_chr_ram_and_vertical_mirroring() {
        let mut c = Nrom::parse(&image(2, 0, 1, 0)).unwrap();
        assert_eq!(c.mirroring(), Mirroring::Vertical); assert!(c.chr_is_ram());
        assert!(c.ppu_write(0x1FFF, 0xA5)); assert_eq!(c.ppu_read(0x1FFF), Some(0xA5));
    }
    #[test] fn rejects_mapper_invalid_prg_and_truncation() {
        assert_eq!(Nrom::parse(&image(1, 1, 0x10, 0)), Err(CartridgeError::UnsupportedMapper(1)));
        assert_eq!(Nrom::parse(&image(0, 1, 0, 0)), Err(CartridgeError::InvalidPrgSize(0)));
        let mut v = image(1, 1, 0, 0); v.pop();
        assert!(matches!(Nrom::parse(&v), Err(CartridgeError::TruncatedImage { .. })));
    }
    #[test] fn rejects_nes2_until_implemented() {
        assert_eq!(Nrom::parse(&image(1, 1, 0, 8)), Err(CartridgeError::Nes2NotSupported));
    }
}
