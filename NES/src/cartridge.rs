//! iNES cartridge parsing and initial NROM mapper 0 support.

use core::fmt;

/// iNES header length.
pub const INES_HEADER_LEN: usize = 16;
const TRAINER_LEN: usize = 512;
const PRG_BANK_LEN: usize = 16 * 1024;
const CHR_BANK_LEN: usize = 8 * 1024;
const PRG_RAM_LEN: usize = 8 * 1024;

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
    prg_ram: Vec<u8>,
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
    /// CHR-ROM size is not valid for NROM (only zero or one 8 KiB bank is supported).
    InvalidChrSize(usize),
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
            Self::InvalidChrSize(n) => write!(f, "invalid NROM CHR-ROM size: {n} bytes"),
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
        if chr_len != 0 && chr_len != CHR_BANK_LEN {
            return Err(CartridgeError::InvalidChrSize(chr_len));
        }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len + chr_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }

        let prg_start = INES_HEADER_LEN + trainer_len;
        let chr_start = prg_start + prg_len;
        let prg_rom = image[prg_start..chr_start].to_vec();
        // NROM exposes 8 KiB of PRG-RAM at $6000-$7FFF. Legacy iNES trainers
        // are copied to $7000-$71FF before the game starts.
        let mut prg_ram = vec![0; PRG_RAM_LEN];
        if trainer_len != 0 {
            prg_ram[0x1000..0x1200].copy_from_slice(&image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN]);
        }
        let chr_is_ram = chr_len == 0;
        let chr = if chr_is_ram { vec![0; CHR_BANK_LEN] } else { image[chr_start..chr_start + chr_len].to_vec() };
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen }
            else if image[6] & 0x01 != 0 { Mirroring::Vertical }
            else { Mirroring::Horizontal };
        Ok(Self { prg_rom, prg_ram, chr, chr_is_ram, mirroring })
    }

    /// Reads cartridge RAM or PRG-ROM from the CPU address space.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0xFFFF => Some(self.prg_rom[(address as usize - 0x8000) % self.prg_rom.len()]),
            _ => None,
        }
    }

    /// Writes the cartridge's 8 KiB PRG-RAM; PRG-ROM remains read-only.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        if !(0x6000..=0x7FFF).contains(&address) { return false; }
        self.prg_ram[address as usize - 0x6000] = value;
        true
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


/// UxROM (mapper 2): switchable lower PRG bank and fixed final PRG bank.
///
/// Bus-conflict variants are not modelled yet; writes select the bank directly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Uxrom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    mirroring: Mirroring,
    selected_bank: usize,
}

impl Uxrom {
    /// Parses a legacy iNES mapper-2 image with at least two 16 KiB PRG banks.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 2 { return Err(CartridgeError::UnsupportedMapper(mapper)); }

        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if image[4] < 2 { return Err(CartridgeError::InvalidPrgSize(prg_len)); }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len != 0 && chr_len != CHR_BANK_LEN {
            return Err(CartridgeError::InvalidChrSize(chr_len));
        }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len + chr_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }
        let prg_start = INES_HEADER_LEN + trainer_len;
        let chr_start = prg_start + prg_len;
        let mut prg_ram = vec![0; PRG_RAM_LEN];
        if trainer_len != 0 {
            prg_ram[0x1000..0x1200].copy_from_slice(&image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN]);
        }
        let chr_is_ram = chr_len == 0;
        let chr = if chr_is_ram { vec![0; CHR_BANK_LEN] } else { image[chr_start..chr_start + chr_len].to_vec() };
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen }
            else if image[6] & 0x01 != 0 { Mirroring::Vertical }
            else { Mirroring::Horizontal };
        Ok(Self { prg_rom: image[prg_start..chr_start].to_vec(), prg_ram, chr, chr_is_ram, mirroring, selected_bank: 0 })
    }

    /// Reads PRG-RAM or the currently selected/fixed PRG-ROM bank.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0xBFFF => {
                let index = self.selected_bank * PRG_BANK_LEN + address as usize - 0x8000;
                Some(self.prg_rom[index])
            }
            0xC000..=0xFFFF => {
                let fixed_bank = self.prg_rom.len() / PRG_BANK_LEN - 1;
                let index = fixed_bank * PRG_BANK_LEN + address as usize - 0xC000;
                Some(self.prg_rom[index])
            }
            _ => None,
        }
    }

    /// Writes PRG-RAM or selects a lower PRG bank; ROM bytes themselves remain read-only.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF => {
                self.prg_ram[address as usize - 0x6000] = value;
                true
            }
            0x8000..=0xFFFF => {
                let switchable_banks = self.prg_rom.len() / PRG_BANK_LEN - 1;
                self.selected_bank = value as usize % switchable_banks;
                true
            }
            _ => false,
        }
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


/// CNROM (mapper 3): fixed NROM-style PRG mapping and switchable 8 KiB CHR-ROM bank.
///
/// Bus-conflict variants are not modelled yet; writes select the CHR bank directly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cnrom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_rom: Vec<u8>,
    mirroring: Mirroring,
    selected_chr_bank: usize,
}

impl Cnrom {
    /// Parses a legacy iNES mapper-3 image with one/two PRG banks and 1-4 CHR banks.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 3 { return Err(CartridgeError::UnsupportedMapper(mapper)); }

        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if prg_len != PRG_BANK_LEN && prg_len != 2 * PRG_BANK_LEN {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len == 0 || chr_len > 4 * CHR_BANK_LEN {
            return Err(CartridgeError::InvalidChrSize(chr_len));
        }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len + chr_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }
        let prg_start = INES_HEADER_LEN + trainer_len;
        let chr_start = prg_start + prg_len;
        let mut prg_ram = vec![0; PRG_RAM_LEN];
        if trainer_len != 0 {
            prg_ram[0x1000..0x1200].copy_from_slice(&image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN]);
        }
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen }
            else if image[6] & 0x01 != 0 { Mirroring::Vertical }
            else { Mirroring::Horizontal };
        Ok(Self {
            prg_rom: image[prg_start..chr_start].to_vec(),
            prg_ram,
            chr_rom: image[chr_start..chr_start + chr_len].to_vec(),
            mirroring,
            selected_chr_bank: 0,
        })
    }

    /// Reads PRG-RAM or NROM-mapped PRG-ROM.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0xFFFF => Some(self.prg_rom[(address as usize - 0x8000) % self.prg_rom.len()]),
            _ => None,
        }
    }

    /// Writes PRG-RAM or selects the CHR-ROM bank.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF => {
                self.prg_ram[address as usize - 0x6000] = value;
                true
            }
            0x8000..=0xFFFF => {
                self.selected_chr_bank = value as usize % (self.chr_rom.len() / CHR_BANK_LEN);
                true
            }
            _ => false,
        }
    }

    /// Returns cartridge nametable mirroring.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// CNROM stores character data in ROM.
    pub fn chr_is_ram(&self) -> bool { false }
    /// Reads from the currently selected 8 KiB CHR-ROM bank.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        Some(self.chr_rom[self.selected_chr_bank * CHR_BANK_LEN + address as usize])
    }
    /// CHR-ROM is read-only.
    pub fn ppu_write(&mut self, _address: u16, _value: u8) -> bool { false }
}

/// Supported cartridge mapper implementations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cartridge {
    /// Mapper 0, no PRG banking.
    Nrom(Nrom),
    /// Mapper 2, switchable lower PRG bank.
    Uxrom(Uxrom),
    /// Mapper 3, switchable CHR-ROM bank.
    Cnrom(Cnrom),
}

impl Cartridge {
    /// Parses a supported legacy iNES cartridge and selects its mapper implementation.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        match mapper {
            0 => Nrom::parse(image).map(Self::Nrom),
            2 => Uxrom::parse(image).map(Self::Uxrom),
            3 => Cnrom::parse(image).map(Self::Cnrom),
            other => Err(CartridgeError::UnsupportedMapper(other)),
        }
    }

    /// Reads a byte from the CPU address space.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match self { Self::Nrom(c) => c.cpu_read(address), Self::Uxrom(c) => c.cpu_read(address), Self::Cnrom(c) => c.cpu_read(address) }
    }
    /// Writes cartridge RAM or mapper registers.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match self { Self::Nrom(c) => c.cpu_write(address, value), Self::Uxrom(c) => c.cpu_write(address, value), Self::Cnrom(c) => c.cpu_write(address, value) }
    }
    /// Returns cartridge nametable mirroring.
    pub fn mirroring(&self) -> Mirroring {
        match self { Self::Nrom(c) => c.mirroring(), Self::Uxrom(c) => c.mirroring(), Self::Cnrom(c) => c.mirroring() }
    }
    /// Returns true when character memory is RAM.
    pub fn chr_is_ram(&self) -> bool {
        match self { Self::Nrom(c) => c.chr_is_ram(), Self::Uxrom(c) => c.chr_is_ram(), Self::Cnrom(c) => c.chr_is_ram() }
    }
    /// Reads a byte from PPU pattern memory.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        match self { Self::Nrom(c) => c.ppu_read(address), Self::Uxrom(c) => c.ppu_read(address), Self::Cnrom(c) => c.ppu_read(address) }
    }
    /// Writes pattern memory only when it is CHR-RAM.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        match self { Self::Nrom(c) => c.ppu_write(address, value), Self::Uxrom(c) => c.ppu_write(address, value), Self::Cnrom(c) => c.ppu_write(address, value) }
    }
}


#[cfg(test)]
mod cnrom_tests {
    use super::*;

    fn cnrom_image(prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN + chr_banks as usize * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[5] = chr_banks;
        image[6] = 0x30;
        for bank in 0..prg_banks as usize {
            let start = INES_HEADER_LEN + bank * PRG_BANK_LEN;
            image[start..start + PRG_BANK_LEN].fill((0xA0 + bank) as u8);
        }
        let chr_start = INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN;
        for bank in 0..chr_banks as usize {
            let start = chr_start + bank * CHR_BANK_LEN;
            image[start..start + CHR_BANK_LEN].fill((0x10 + bank) as u8);
        }
        image
    }

    #[test]
    fn mapper_three_switches_chr_bank_without_changing_prg_mapping() {
        let mut cartridge = Cartridge::parse(&cnrom_image(2, 4)).unwrap();
        assert_eq!(cartridge.cpu_read(0x8000), Some(0xA0));
        assert_eq!(cartridge.cpu_read(0xC000), Some(0xA1));
        assert_eq!(cartridge.ppu_read(0x0000), Some(0x10));
        assert_eq!(cartridge.ppu_read(0x1FFF), Some(0x10));
        assert!(cartridge.cpu_write(0x8000, 2));
        assert_eq!(cartridge.ppu_read(0x0000), Some(0x12));
        assert_eq!(cartridge.ppu_read(0x1FFF), Some(0x12));
        assert_eq!(cartridge.cpu_read(0xC000), Some(0xA1));
        assert!(!cartridge.ppu_write(0x0000, 0xFF));
        assert_eq!(cartridge.ppu_read(0x0000), Some(0x12));
    }

    #[test]
    fn mapper_three_rejects_chr_ram_and_more_than_four_chr_banks() {
        assert!(matches!(Cartridge::parse(&cnrom_image(1, 0)), Err(CartridgeError::InvalidChrSize(0))));
        assert!(matches!(Cartridge::parse(&cnrom_image(1, 5)), Err(CartridgeError::InvalidChrSize(_))));
    }
}

#[cfg(test)]
mod uxrom_tests {
    use super::*;

    fn uxrom_image(banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + banks as usize * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = banks;
        image[6] = 0x20;
        for bank in 0..banks as usize {
            image[INES_HEADER_LEN + bank * PRG_BANK_LEN..INES_HEADER_LEN + (bank + 1) * PRG_BANK_LEN].fill(bank as u8);
        }
        image
    }

    #[test]
    fn mapper_two_switches_lower_bank_and_keeps_last_bank_fixed() {
        let image = uxrom_image(4);
        let mut cartridge = Cartridge::parse(&image).unwrap();
        assert_eq!(cartridge.cpu_read(0x8000), Some(0));
        assert_eq!(cartridge.cpu_read(0xBFFF), Some(0));
        assert_eq!(cartridge.cpu_read(0xC000), Some(3));
        assert_eq!(cartridge.cpu_read(0xFFFF), Some(3));
        assert!(cartridge.cpu_write(0x8000, 2));
        assert_eq!(cartridge.cpu_read(0x8000), Some(2));
        assert_eq!(cartridge.cpu_read(0xC000), Some(3));
        assert!(cartridge.cpu_write(0xFFFF, 1));
        assert_eq!(cartridge.cpu_read(0x8000), Some(1));
        assert_eq!(cartridge.cpu_read(0xC000), Some(3));
    }

    #[test]
    fn mapper_two_supports_prg_ram_and_chr_ram() {
        let mut cartridge = Cartridge::parse(&uxrom_image(2)).unwrap();
        assert!(cartridge.cpu_write(0x6000, 0xA7));
        assert_eq!(cartridge.cpu_read(0x6000), Some(0xA7));
        assert!(cartridge.ppu_write(0x1FFF, 0x5A));
        assert_eq!(cartridge.ppu_read(0x1FFF), Some(0x5A));
    }

    #[test]
    fn mapper_two_rejects_single_prg_bank() {
        assert!(matches!(Cartridge::parse(&uxrom_image(1)), Err(CartridgeError::InvalidPrgSize(_))));
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
        assert_eq!(c.cpu_read(0xFFFF), Some(0x99)); assert_eq!(c.cpu_read(0x5FFF), None); assert_eq!(c.cpu_read(0x7FFF), Some(0));
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
    #[test] fn rejects_chr_rom_sizes_outside_nrom_hardware() {
        assert_eq!(Nrom::parse(&image(1, 2, 0, 0)), Err(CartridgeError::InvalidChrSize(2 * CHR_BANK_LEN)));
    }
    #[test]
    fn prg_ram_is_writable_and_trainer_is_loaded_at_7000() {
        // Trainer payload is the 512 bytes immediately following the header.
        let mut with_trainer = vec![0; 16 + TRAINER_LEN + PRG_BANK_LEN];
        with_trainer[0..4].copy_from_slice(b"NES\x1A");
        with_trainer[4] = 1;
        with_trainer[6] = 0x04;
        with_trainer[16] = 0x5A;
        let mut c = Nrom::parse(&with_trainer).unwrap();
        assert_eq!(c.cpu_read(0x7000), Some(0x5A));
        assert!(c.cpu_write(0x6000, 0xA5));
        assert_eq!(c.cpu_read(0x6000), Some(0xA5));
        assert!(!c.cpu_write(0x8000, 0x11));
        assert_eq!(c.cpu_read(0x8000), Some(0));
    }


    #[test]
    fn supports_horizontal_and_four_screen_mirroring() {
        let horizontal = Nrom::parse(&image(1, 1, 0, 0)).unwrap();
        assert_eq!(horizontal.mirroring(), Mirroring::Horizontal);

        let four_screen = Nrom::parse(&image(1, 1, 0x08, 0)).unwrap();
        assert_eq!(four_screen.mirroring(), Mirroring::FourScreen);
    }

    #[test]
    fn legacy_mapper_number_uses_upper_nibble_of_flags7() {
        let mut image = image(1, 1, 0x20, 0);
        assert_eq!(
            Nrom::parse(&image),
            Err(CartridgeError::UnsupportedMapper(2))
        );

        // NES 2.0's format marker takes precedence over legacy mapper decoding.
        image[7] = 0x08;
        assert_eq!(
            Nrom::parse(&image),
            Err(CartridgeError::Nes2NotSupported)
        );
    }

    #[test]
    fn trainer_does_not_shift_prg_rom_or_chr_rom_offsets() {
        let mut image = vec![0; 16 + TRAINER_LEN + PRG_BANK_LEN + CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1;
        image[6] = 0x04;
        let prg_start = 16 + TRAINER_LEN;
        image[prg_start] = 0xA9;
        image[prg_start + PRG_BANK_LEN] = 0x5C;

        let cartridge = Nrom::parse(&image).unwrap();
        assert_eq!(cartridge.cpu_read(0x8000), Some(0xA9));
        assert_eq!(cartridge.ppu_read(0), Some(0x5C));
    }


    #[test]
    fn prg_rom_and_chr_rom_are_read_only_while_chr_ram_is_writable() {
        let mut chr_rom_image = image(2, 1, 0, 0);
        let prg_start = INES_HEADER_LEN;
        let chr_start = prg_start + 2 * PRG_BANK_LEN;
        chr_rom_image[prg_start] = 0x31;
        chr_rom_image[chr_start] = 0x72;
        let mut chr_rom = Nrom::parse(&chr_rom_image).unwrap();

        assert_eq!(chr_rom.cpu_read(0x8000), Some(0x31));
        assert!(!chr_rom.cpu_write(0x8000, 0x99));
        assert_eq!(chr_rom.cpu_read(0x8000), Some(0x31));
        assert_eq!(chr_rom.ppu_read(0), Some(0x72));
        assert!(!chr_rom.ppu_write(0, 0x99));
        assert_eq!(chr_rom.ppu_read(0), Some(0x72));

        let mut chr_ram = Nrom::parse(&image(1, 0, 0, 0)).unwrap();
        assert!(chr_ram.ppu_write(0x1FFF, 0xA6));
        assert_eq!(chr_ram.ppu_read(0x1FFF), Some(0xA6));
    }

    #[test]
    fn two_bank_nrom_maps_distinct_prg_rom_halves() {
        let mut image = image(2, 0, 0, 0);
        let prg_start = INES_HEADER_LEN;
        image[prg_start] = 0x12;
        image[prg_start + PRG_BANK_LEN] = 0x34;
        image[prg_start + PRG_BANK_LEN - 1] = 0x56;
        image[prg_start + 2 * PRG_BANK_LEN - 1] = 0x78;

        let cartridge = Nrom::parse(&image).unwrap();
        assert_eq!(cartridge.cpu_read(0x8000), Some(0x12));
        assert_eq!(cartridge.cpu_read(0xC000), Some(0x34));
        assert_eq!(cartridge.cpu_read(0xBFFF), Some(0x56));
        assert_eq!(cartridge.cpu_read(0xFFFF), Some(0x78));
    }

}
