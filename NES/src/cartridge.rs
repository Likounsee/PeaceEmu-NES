//! Legacy iNES parsing and mapper implementations for NROM, UxROM, CNROM and AxROM.

use core::cell::Cell;
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
    /// All logical nametables map to the lower physical nametable.
    OneScreenLower,
    /// All logical nametables map to the upper physical nametable.
    OneScreenUpper,
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
    /// Mapper not implemented by this core was requested.
    UnsupportedMapper(u16),
    /// PRG size is invalid for the selected mapper.
    InvalidPrgSize(usize),
    /// CHR-ROM size is invalid for the selected mapper.
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
            Self::InvalidPrgSize(n) => write!(f, "invalid PRG-ROM size for selected mapper: {n} bytes"),
            Self::InvalidChrSize(n) => write!(f, "invalid CHR-ROM size for selected mapper: {n} bytes"),
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



/// MMC1 (mapper 1): serial register, switchable PRG/CHR banks and nametable mirroring.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mmc1 {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    four_screen: bool,
    shift: u8,
    control: u8,
    chr_bank_0: u8,
    chr_bank_1: u8,
    prg_bank: u8,
}

impl Mmc1 {
    /// Parses a legacy iNES MMC1 image with 2, 4, 8 or 16 16-KiB PRG banks.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 1 { return Err(CartridgeError::UnsupportedMapper(mapper)); }

        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if !matches!(image[4], 2 | 4 | 8 | 16) {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len > 16 * CHR_BANK_LEN {
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
        let chr = if chr_is_ram { vec![0; CHR_BANK_LEN] }
            else { image[chr_start..chr_start + chr_len].to_vec() };
        Ok(Self {
            prg_rom: image[prg_start..chr_start].to_vec(),
            prg_ram,
            chr,
            chr_is_ram,
            four_screen: image[6] & 0x08 != 0,
            shift: 0x10,
            control: 0x0C | if image[6] & 1 != 0 { 2 } else { 3 },
            chr_bank_0: 0,
            chr_bank_1: 0,
            prg_bank: 0,
        })
    }

    fn commit_register(&mut self, address: u16, value: u8) {
        match address {
            0x8000..=0x9FFF => self.control = value & 0x1F,
            0xA000..=0xBFFF => self.chr_bank_0 = value & 0x1F,
            0xC000..=0xDFFF => self.chr_bank_1 = value & 0x1F,
            0xE000..=0xFFFF => self.prg_bank = value & 0x1F,
            _ => {}
        }
    }

    fn prg_index(&self, address: u16) -> usize {
        let count = self.prg_rom.len() / PRG_BANK_LEN;
        let mode = (self.control >> 2) & 3;
        let selected = (self.prg_bank & 0x0F) as usize % count;
        let bank = match mode {
            0 | 1 => ((selected & !1) + usize::from(address >= 0xC000)) % count,
            2 => if address < 0xC000 { 0 } else { selected },
            _ => if address < 0xC000 { selected } else { count - 1 },
        };
        bank * PRG_BANK_LEN + (address as usize & 0x3FFF)
    }

    fn chr_index(&self, address: u16) -> usize {
        let bank_count = self.chr.len() / 0x1000;
        let bank = if self.control & 0x10 == 0 {
            (((self.chr_bank_0 & 0x1E) as usize) + usize::from(address >= 0x1000)) % bank_count
        } else if address < 0x1000 {
            self.chr_bank_0 as usize % bank_count
        } else {
            self.chr_bank_1 as usize % bank_count
        };
        bank * 0x1000 + (address as usize & 0x0FFF)
    }

    /// Reads PRG-RAM unless disabled, or the selected 16-KiB PRG-ROM window.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF if self.prg_bank & 0x10 == 0 =>
                Some(self.prg_ram[address as usize - 0x6000]),
            0x6000..=0x7FFF => None,
            0x8000..=0xFFFF => Some(self.prg_rom[self.prg_index(address)]),
            _ => None,
        }
    }

    /// Writes PRG-RAM or shifts one bit into the MMC1 serial register.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF if self.prg_bank & 0x10 == 0 => {
                self.prg_ram[address as usize - 0x6000] = value;
                true
            }
            0x6000..=0x7FFF => false,
            0x8000..=0xFFFF => {
                if value & 0x80 != 0 {
                    self.shift = 0x10;
                    self.control |= 0x0C;
                    return true;
                }
                let complete = self.shift & 1 != 0;
                self.shift = (self.shift >> 1) | ((value & 1) << 4);
                if complete {
                    self.commit_register(address, self.shift);
                    self.shift = 0x10;
                }
                true
            }
            _ => false,
        }
    }

    /// Returns MMC1-controlled nametable mirroring (unless the board hardwires four-screen).
    pub fn mirroring(&self) -> Mirroring {
        if self.four_screen { return Mirroring::FourScreen; }
        match self.control & 3 {
            0 => Mirroring::OneScreenLower,
            1 => Mirroring::OneScreenUpper,
            2 => Mirroring::Vertical,
            _ => Mirroring::Horizontal,
        }
    }
    /// Returns true when this cartridge uses writable CHR-RAM.
    pub fn chr_is_ram(&self) -> bool { self.chr_is_ram }
    /// Reads the selected 4-KiB CHR bank from the PPU pattern address space.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        Some(self.chr[self.chr_index(address)])
    }
    /// Writes pattern memory when the cartridge uses CHR-RAM.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 || !self.chr_is_ram { return false; }
        let index = self.chr_index(address);
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

/// AxROM (mapper 7): switchable 32 KiB PRG-ROM and bank-controlled one-screen mirroring.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Axrom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_ram: Vec<u8>,
    selected_prg_bank: usize,
    upper_nametable: bool,
}

impl Axrom {
    /// Parses a legacy iNES mapper-7 image with 2-32 16 KiB PRG units and CHR-RAM.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 7 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if image[4] < 2 || image[4] > 16 || !image[4].is_multiple_of(2) {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len != 0 { return Err(CartridgeError::InvalidChrSize(chr_len)); }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }
        let prg_start = INES_HEADER_LEN + trainer_len;
        let mut prg_ram = vec![0; PRG_RAM_LEN];
        if trainer_len != 0 {
            prg_ram[0x1000..0x1200].copy_from_slice(&image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN]);
        }
        Ok(Self {
            prg_rom: image[prg_start..prg_start + prg_len].to_vec(),
            prg_ram,
            chr_ram: vec![0; CHR_BANK_LEN],
            selected_prg_bank: 0,
            upper_nametable: false,
        })
    }

    /// Reads PRG-RAM or the selected 32 KiB PRG-ROM window.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0xFFFF => {
                let index = self.selected_prg_bank * 2 * PRG_BANK_LEN + address as usize - 0x8000;
                Some(self.prg_rom[index])
            }
            _ => None,
        }
    }

    /// Writes PRG-RAM or selects the PRG bank and one-screen nametable page.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF => { self.prg_ram[address as usize - 0x6000] = value; true }
            0x8000..=0xFFFF => {
                // Keep the selected window inside the ROM image. Smaller legacy
                // AxROM images can expose fewer than eight 32 KiB banks.
                let bank_count = self.prg_rom.len() / (2 * PRG_BANK_LEN);
                self.selected_prg_bank = (value & 0x07) as usize % bank_count;
                self.upper_nametable = value & 0x10 != 0;
                true
            }
            _ => false,
        }
    }

    /// Returns the currently selected one-screen nametable page.
    pub fn mirroring(&self) -> Mirroring {
        if self.upper_nametable { Mirroring::OneScreenUpper } else { Mirroring::OneScreenLower }
    }
    /// AxROM uses 8 KiB of CHR-RAM.
    pub fn chr_is_ram(&self) -> bool { true }
    /// Reads CHR-RAM from the PPU pattern address space.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        Some(self.chr_ram[address as usize])
    }
    /// Writes CHR-RAM from the PPU pattern address space.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 { return false; }
        self.chr_ram[address as usize] = value;
        true
    }
}


/// GxROM (mapper 66): independent 32 KiB PRG-ROM and 8 KiB CHR-ROM bank selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Gxrom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_rom: Vec<u8>,
    selected_prg_bank: usize,
    selected_chr_bank: usize,
    mirroring: Mirroring,
}

impl Gxrom {
    /// Parses a legacy iNES mapper-66 image with switchable PRG and CHR ROM.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 66 { return Err(CartridgeError::UnsupportedMapper(mapper)); }

        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if image[4] < 2 || !image[4].is_multiple_of(2) || prg_len > 4 * 2 * PRG_BANK_LEN {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len == 0 || chr_len > 8 * CHR_BANK_LEN {
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
            else if image[6] & 1 != 0 { Mirroring::Vertical }
            else { Mirroring::Horizontal };
        Ok(Self {
            prg_rom: image[prg_start..chr_start].to_vec(),
            prg_ram,
            chr_rom: image[chr_start..chr_start + chr_len].to_vec(),
            selected_prg_bank: 0,
            selected_chr_bank: 0,
            mirroring,
        })
    }

    /// Reads cartridge PRG-RAM or the selected 32 KiB PRG-ROM bank.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0xFFFF => {
                let index = self.selected_prg_bank * 2 * PRG_BANK_LEN + address as usize - 0x8000;
                Some(self.prg_rom[index])
            }
            _ => None,
        }
    }

    /// Writes PRG-RAM or selects PRG (bits 4-5) and CHR (bits 0-1) banks.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF => { self.prg_ram[address as usize - 0x6000] = value; true }
            0x8000..=0xFFFF => {
                let prg_count = self.prg_rom.len() / (2 * PRG_BANK_LEN);
                let chr_count = self.chr_rom.len() / CHR_BANK_LEN;
                self.selected_prg_bank = ((value >> 4) & 0x03) as usize % prg_count;
                self.selected_chr_bank = (value & 0x03) as usize % chr_count;
                true
            }
            _ => false,
        }
    }

    /// Returns the fixed mirroring mode encoded by the iNES header.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// GxROM uses CHR-ROM, not CHR-RAM.
    pub fn chr_is_ram(&self) -> bool { false }
    /// Reads from the selected 8 KiB CHR-ROM bank.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        Some(self.chr_rom[self.selected_chr_bank * CHR_BANK_LEN + address as usize])
    }
    /// GxROM CHR-ROM is read-only.
    pub fn ppu_write(&mut self, _address: u16, _value: u8) -> bool { false }
}


/// MMC3 / TxROM (mapper 4): 8 KiB PRG windows, 1/2 KiB CHR banks and scanline IRQs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mmc3 {
    prg_rom: Vec<u8>, prg_ram: Vec<u8>, chr: Vec<u8>, chr_is_ram: bool,
    banks: [u8; 8], bank_select: u8, mirroring: Mirroring,
    ram_enabled: bool, ram_write_protected: bool,
    irq_latch: u8, irq_counter: u8, irq_reload: bool, irq_enabled: bool, irq_pending: bool,
}
impl Mmc3 {
    /// Parses a legacy iNES mapper-4 image (MMC3/TxROM).
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16) | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 4 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if image[4] < 2 || !image[4].is_multiple_of(2) || prg_len > 32 * PRG_BANK_LEN {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len > 32 * CHR_BANK_LEN { return Err(CartridgeError::InvalidChrSize(chr_len)); }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len + chr_len;
        if image.len() < expected { return Err(CartridgeError::TruncatedImage { expected, actual: image.len() }); }
        let prg_start = INES_HEADER_LEN + trainer_len;
        let chr_start = prg_start + prg_len;
        let chr_is_ram = chr_len == 0;
        let chr = if chr_is_ram { vec![0; CHR_BANK_LEN] } else { image[chr_start..chr_start + chr_len].to_vec() };
        let mut prg_ram = vec![0; PRG_RAM_LEN];
        if trainer_len != 0 { prg_ram[0x1000..0x1200].copy_from_slice(&image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN]); }
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen } else if image[6] & 1 != 0 { Mirroring::Vertical } else { Mirroring::Horizontal };
        Ok(Self { prg_rom: image[prg_start..chr_start].to_vec(), prg_ram, chr, chr_is_ram,
            banks: [0, 2, 4, 5, 6, 7, 0, 1], bank_select: 0, mirroring, ram_enabled: true,
            ram_write_protected: false, irq_latch: 0, irq_counter: 0, irq_reload: false, irq_enabled: false, irq_pending: false })
    }
    fn prg_index(&self, address: u16) -> usize {
        let count = self.prg_rom.len() / 0x2000;
        let second_last = count - 2; let last = count - 1;
        let r6 = (self.banks[6] & 0x3F) as usize % count;
        let r7 = (self.banks[7] & 0x3F) as usize % count;
        let bank = match (address >> 13, self.bank_select & 0x40 != 0) {
            (4, false) | (6, true) => r6, (4, true) => second_last, (5, _) => r7,
            (6, false) => second_last, (7, _) => last, _ => 0,
        };
        bank * 0x2000 + (address as usize & 0x1FFF)
    }
    fn chr_index(&self, address: u16) -> usize {
        let count = (self.chr.len() / 0x400).max(1);
        let r = self.banks; let slot = (address as usize >> 10) & 7;
        let bank = if self.bank_select & 0x80 == 0 {
            match slot { 0 => (r[0] & 0xFE) as usize, 1 => (r[0] | 1) as usize,
                2 => (r[1] & 0xFE) as usize, 3 => (r[1] | 1) as usize, 4..=7 => r[slot - 2] as usize, _ => 0 }
        } else {
            match slot { 0..=3 => r[slot + 2] as usize, 4 => (r[0] & 0xFE) as usize,
                5 => (r[0] | 1) as usize, 6 => (r[1] & 0xFE) as usize, 7 => (r[1] | 1) as usize, _ => 0 }
        };
        (bank % count) * 0x400 + (address as usize & 0x3FF)
    }
    /// Reads enabled PRG-RAM or one of the four 8 KiB PRG-ROM windows.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address { 0x6000..=0x7FFF if self.ram_enabled => Some(self.prg_ram[address as usize - 0x6000]),
            0x6000..=0x7FFF => None, 0x8000..=0xFFFF => Some(self.prg_rom[self.prg_index(address)]), _ => None }
    }
    /// Writes PRG-RAM or one of the MMC3 register pairs.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF if self.ram_enabled && !self.ram_write_protected => { self.prg_ram[address as usize - 0x6000] = value; true }
            0x6000..=0x7FFF => true,
            0x8000..=0x9FFF if address & 1 == 0 => { self.bank_select = value; true }
            0x8000..=0x9FFF => { let r = (self.bank_select & 7) as usize;
                self.banks[r] = match r { 0 | 1 => value & 0xFE, 6 | 7 => value & 0x3F, _ => value }; true }
            0xA000..=0xBFFF if address & 1 == 0 => { if self.mirroring != Mirroring::FourScreen {
                self.mirroring = if value & 1 == 0 { Mirroring::Vertical } else { Mirroring::Horizontal }; } true }
            0xA000..=0xBFFF => { self.ram_enabled = value & 0x80 != 0; self.ram_write_protected = value & 0x40 != 0; true }
            0xC000..=0xDFFF if address & 1 == 0 => { self.irq_latch = value; true }
            0xC000..=0xDFFF => { self.irq_counter = 0; self.irq_reload = true; true }
            0xE000..=0xFFFF if address & 1 == 0 => { self.irq_enabled = false; self.irq_pending = false; true }
            0xE000..=0xFFFF => { self.irq_enabled = true; true }
            _ => false,
        }
    }
    /// Returns the cartridge mirroring mode.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// Returns whether the pattern memory is CHR-RAM.
    pub fn chr_is_ram(&self) -> bool { self.chr_is_ram }
    /// Reads the selected 1 KiB/2 KiB CHR bank.
    pub fn ppu_read(&self, address: u16) -> Option<u8> { if address >= 0x2000 { None } else { Some(self.chr[self.chr_index(address)]) } }
    /// Writes pattern memory only for CHR-RAM.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 || !self.chr_is_ram { return false; }
        let index = self.chr_index(address); self.chr[index] = value; true
    }
    /// Clocks the scanline IRQ counter; precise PPU A12 filtering is not yet modelled.
    pub fn clock_irq_counter(&mut self) {
        if self.irq_counter == 0 || self.irq_reload { self.irq_counter = self.irq_latch; }
        else { self.irq_counter = self.irq_counter.wrapping_sub(1); }
        self.irq_reload = false;
        if self.irq_counter == 0 && self.irq_enabled { self.irq_pending = true; }
    }
    /// Returns whether the mapper currently asserts its IRQ line.
    pub fn irq_pending(&self) -> bool { self.irq_pending }
}



/// BNROM (legacy mapper 34): switchable 32 KiB PRG-ROM and fixed CHR-RAM.
/// This deliberately implements the BNROM board variant only, not NINA-001.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bnrom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_ram: Vec<u8>,
    selected_prg_bank: usize,
    mirroring: Mirroring,
}

impl Bnrom {
    /// Parses a legacy iNES mapper-34 BNROM image using CHR-RAM.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 34 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if !(2..=8).contains(&image[4]) || !image[4].is_multiple_of(2) {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len != 0 { return Err(CartridgeError::InvalidChrSize(chr_len)); }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }
        let prg_start = INES_HEADER_LEN + trainer_len;
        let mut prg_ram = vec![0; PRG_RAM_LEN];
        if trainer_len != 0 {
            prg_ram[0x1000..0x1200].copy_from_slice(&image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN]);
        }
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen }
            else if image[6] & 1 != 0 { Mirroring::Vertical } else { Mirroring::Horizontal };
        Ok(Self {
            prg_rom: image[prg_start..prg_start + prg_len].to_vec(),
            prg_ram,
            chr_ram: vec![0; CHR_BANK_LEN],
            selected_prg_bank: 0,
            mirroring,
        })
    }

    /// Reads PRG-RAM or the selected 32 KiB PRG bank.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0xFFFF => {
                let index = self.selected_prg_bank * 2 * PRG_BANK_LEN + address as usize - 0x8000;
                Some(self.prg_rom[index])
            }
            _ => None,
        }
    }

    /// Writes PRG-RAM or selects the BNROM 32 KiB PRG bank.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF => {
                self.prg_ram[address as usize - 0x6000] = value;
                true
            }
            0x8000..=0xFFFF => {
                self.selected_prg_bank = (value as usize) % (self.prg_rom.len() / (2 * PRG_BANK_LEN));
                true
            }
            _ => false,
        }
    }

    /// Returns fixed mirroring declared by the legacy iNES header.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// BNROM uses CHR-RAM.
    pub fn chr_is_ram(&self) -> bool { true }
    /// Reads the 8 KiB CHR-RAM window.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        Some(self.chr_ram[address as usize])
    }
    /// Writes the 8 KiB CHR-RAM window.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 { return false; }
        self.chr_ram[address as usize] = value;
        true
    }
}

/// Camerica (mapper 71): switchable lower 16 KiB PRG bank and fixed final bank.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Camerica {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_ram: Vec<u8>,
    selected_prg_bank: usize,
    mirroring: Mirroring,
}

impl Camerica {
    /// Parses a legacy iNES mapper-71 image using CHR-RAM and 2-16 PRG banks.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 71 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if !(2..=16).contains(&image[4]) {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len != 0 { return Err(CartridgeError::InvalidChrSize(chr_len)); }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }
        let prg_start = INES_HEADER_LEN + trainer_len;
        let mut prg_ram = vec![0; PRG_RAM_LEN];
        if trainer_len != 0 {
            prg_ram[0x1000..0x1200].copy_from_slice(&image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN]);
        }
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen }
            else if image[6] & 1 != 0 { Mirroring::Vertical } else { Mirroring::Horizontal };
        Ok(Self {
            prg_rom: image[prg_start..prg_start + prg_len].to_vec(),
            prg_ram,
            chr_ram: vec![0; CHR_BANK_LEN],
            selected_prg_bank: 0,
            mirroring,
        })
    }

    /// Reads PRG-RAM or the selected lower bank / fixed final PRG bank.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0xBFFF => Some(self.prg_rom[self.selected_prg_bank * PRG_BANK_LEN + address as usize - 0x8000]),
            0xC000..=0xFFFF => {
                let last = self.prg_rom.len() - PRG_BANK_LEN;
                Some(self.prg_rom[last + address as usize - 0xC000])
            }
            _ => None,
        }
    }

    /// Writes PRG-RAM or selects the PRG bank; $9000 writes select one-screen mirroring.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF => {
                self.prg_ram[address as usize - 0x6000] = value;
                true
            }
            0x9000..=0x9FFF => {
                self.selected_prg_bank = (value & 0x0F) as usize % (self.prg_rom.len() / PRG_BANK_LEN - 1);
                self.mirroring = if value & 0x10 != 0 { Mirroring::OneScreenUpper } else { Mirroring::OneScreenLower };
                true
            }
            0x8000..=0xFFFF => {
                self.selected_prg_bank = (value & 0x0F) as usize % (self.prg_rom.len() / PRG_BANK_LEN - 1);
                true
            }
            _ => false,
        }
    }

    /// Returns the board's current mirroring mode.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// Camerica mapper 71 uses CHR-RAM in this implementation.
    pub fn chr_is_ram(&self) -> bool { true }
    /// Reads CHR-RAM.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        Some(self.chr_ram[address as usize])
    }
    /// Writes CHR-RAM.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 { return false; }
        self.chr_ram[address as usize] = value;
        true
    }
}


/// Color Dreams (mapper 11): one register selects 32 KiB PRG and 8 KiB CHR banks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColorDreams {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_rom: Vec<u8>,
    selected_prg_bank: usize,
    selected_chr_bank: usize,
    mirroring: Mirroring,
}

impl ColorDreams {
    /// Parses a legacy iNES mapper-11 image with switchable PRG-ROM and CHR-ROM.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 11 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if image[4] < 2 || !image[4].is_multiple_of(2) || image[4] > 32 {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len == 0 || chr_len > 16 * CHR_BANK_LEN {
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
            else if image[6] & 1 != 0 { Mirroring::Vertical } else { Mirroring::Horizontal };
        Ok(Self {
            prg_rom: image[prg_start..chr_start].to_vec(),
            prg_ram,
            chr_rom: image[chr_start..chr_start + chr_len].to_vec(),
            selected_prg_bank: 0,
            selected_chr_bank: 0,
            mirroring,
        })
    }

    /// Reads PRG-RAM or the selected 32 KiB PRG-ROM window.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0xFFFF => {
                let index = self.selected_prg_bank * 2 * PRG_BANK_LEN + address as usize - 0x8000;
                Some(self.prg_rom[index])
            }
            _ => None,
        }
    }

    /// Writes PRG-RAM or selects the 32 KiB PRG and 8 KiB CHR banks.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF => {
                self.prg_ram[address as usize - 0x6000] = value;
                true
            }
            0x8000..=0xFFFF => {
                let prg_count = self.prg_rom.len() / (2 * PRG_BANK_LEN);
                let chr_count = self.chr_rom.len() / CHR_BANK_LEN;
                self.selected_prg_bank = (value & 0x0F) as usize % prg_count;
                self.selected_chr_bank = (value >> 4) as usize % chr_count;
                true
            }
            _ => false,
        }
    }

    /// Returns the fixed mirroring mode from the iNES header.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// Color Dreams uses CHR-ROM in this implementation.
    pub fn chr_is_ram(&self) -> bool { false }
    /// Reads from the selected 8 KiB CHR-ROM bank.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        Some(self.chr_rom[self.selected_chr_bank * CHR_BANK_LEN + address as usize])
    }
    /// CHR-ROM is read-only.
    pub fn ppu_write(&mut self, _address: u16, _value: u8) -> bool { false }
}


/// CPROM (mapper 13): fixed 32 KiB PRG and switchable upper 4 KiB of CHR-RAM.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cprom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_ram: Vec<u8>,
    selected_chr_bank: usize,
    mirroring: Mirroring,
}

impl Cprom {
    /// Parses a legacy iNES CPROM image (exactly 32 KiB PRG-ROM, no CHR-ROM).
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 13 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if image[4] != 2 { return Err(CartridgeError::InvalidPrgSize(prg_len)); }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len != 0 { return Err(CartridgeError::InvalidChrSize(chr_len)); }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }
        let prg_start = INES_HEADER_LEN + trainer_len;
        let mut prg_ram = vec![0; PRG_RAM_LEN];
        if trainer_len != 0 {
            prg_ram[0x1000..0x1200].copy_from_slice(&image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN]);
        }
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen }
            else if image[6] & 1 != 0 { Mirroring::Vertical } else { Mirroring::Horizontal };
        Ok(Self {
            prg_rom: image[prg_start..prg_start + prg_len].to_vec(),
            prg_ram,
            chr_ram: vec![0; 16 * 1024],
            selected_chr_bank: 0,
            mirroring,
        })
    }

    /// Reads fixed PRG-ROM or PRG-RAM from the CPU address space.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0xFFFF => Some(self.prg_rom[address as usize - 0x8000]),
            _ => None,
        }
    }

    /// Writes PRG-RAM or selects one of the four 4 KiB CHR banks for $1000-$1FFF.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF => { self.prg_ram[address as usize - 0x6000] = value; true }
            0x8000..=0xFFFF => { self.selected_chr_bank = (value & 3) as usize; true }
            _ => false,
        }
    }

    /// Returns the mirroring mode declared by the iNES header.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// Returns true because CPROM provides CHR-RAM.
    pub fn chr_is_ram(&self) -> bool { true }

    /// Reads the fixed lower or selected upper CHR-RAM bank.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        let index = if address < 0x1000 { address as usize }
            else { self.selected_chr_bank * 0x1000 + address as usize - 0x1000 };
        Some(self.chr_ram[index])
    }

    /// Writes the fixed lower or selected upper CHR-RAM bank.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 { return false; }
        let index = if address < 0x1000 { address as usize }
            else { self.selected_chr_bank * 0x1000 + address as usize - 0x1000 };
        self.chr_ram[index] = value;
        true
    }
}


/// MMC2 (mapper 9): one switchable 8 KiB PRG window and two latch-controlled 4 KiB CHR windows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mmc2 {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_rom: Vec<u8>,
    prg_bank: usize,
    chr_fd0: usize,
    chr_fe0: usize,
    chr_fd1: usize,
    chr_fe1: usize,
    latch0_fe: Cell<bool>,
    latch1_fe: Cell<bool>,
    mirroring: Mirroring,
}

impl Mmc2 {
    /// Parses a legacy iNES MMC2 image (PRG-ROM and CHR-ROM required).
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 9 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if image[4] < 8 { return Err(CartridgeError::InvalidPrgSize(prg_len)); }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len == 0 { return Err(CartridgeError::InvalidChrSize(chr_len)); }
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
            else if image[6] & 1 != 0 { Mirroring::Vertical } else { Mirroring::Horizontal };
        Ok(Self {
            prg_rom: image[prg_start..prg_start + prg_len].to_vec(),
            prg_ram,
            chr_rom: image[chr_start..chr_start + chr_len].to_vec(),
            prg_bank: 0,
            chr_fd0: 0, chr_fe0: 0, chr_fd1: 0, chr_fe1: 0,
            latch0_fe: Cell::new(false), latch1_fe: Cell::new(false),
            mirroring,
        })
    }

    /// Reads PRG-RAM or the MMC2's four 8 KiB CPU windows.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0x9FFF => Some(self.prg_rom[(self.prg_bank % self.prg_rom.len().div_ceil(0x2000)) * 0x2000 + address as usize - 0x8000]),
            0xA000..=0xBFFF => Some(self.prg_rom[self.prg_rom.len() - 3 * 0x2000 + address as usize - 0xA000]),
            0xC000..=0xDFFF => Some(self.prg_rom[self.prg_rom.len() - 2 * 0x2000 + address as usize - 0xC000]),
            0xE000..=0xFFFF => Some(self.prg_rom[self.prg_rom.len() - 0x2000 + address as usize - 0xE000]),
            _ => None,
        }
    }

    /// Programs the PRG bank, four CHR latch banks, and nametable mirroring.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF => { self.prg_ram[address as usize - 0x6000] = value; true }
            0xA000..=0xAFFF => { self.prg_bank = value as usize; true }
            0xB000..=0xBFFF => { self.chr_fd0 = value as usize; true }
            0xC000..=0xCFFF => { self.chr_fe0 = value as usize; true }
            0xD000..=0xDFFF => { self.chr_fd1 = value as usize; true }
            0xE000..=0xEFFF => { self.chr_fe1 = value as usize; true }
            0xF000..=0xFFFF => {
                if self.mirroring != Mirroring::FourScreen {
                    self.mirroring = if value & 1 == 0 { Mirroring::Vertical } else { Mirroring::Horizontal };
                }
                true
            }
            _ => false,
        }
    }

    /// Returns the mapper's current nametable mirroring mode.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// Returns false because MMC2 requires CHR-ROM.
    pub fn chr_is_ram(&self) -> bool { false }

    /// Reads CHR-ROM using the active latch, then updates the latch on documented trigger addresses.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        let address = address as usize;
        let index = if address < 0x1000 {
            let bank = if self.latch0_fe.get() { self.chr_fe0 } else { self.chr_fd0 };
            (bank % (self.chr_rom.len() / 0x1000)) * 0x1000 + address
        } else {
            let bank = if self.latch1_fe.get() { self.chr_fe1 } else { self.chr_fd1 };
            (bank % (self.chr_rom.len() / 0x1000)) * 0x1000 + address - 0x1000
        };
        let value = self.chr_rom[index];
        match address {
            0x0FD8 => self.latch0_fe.set(false),
            0x0FE8 => self.latch0_fe.set(true),
            0x1FD8..=0x1FDF => self.latch1_fe.set(false),
            0x1FE8..=0x1FEF => self.latch1_fe.set(true),
            _ => {}
        }
        Some(value)
    }

    /// Rejects writes because MMC2 uses CHR-ROM rather than CHR-RAM.
    pub fn ppu_write(&mut self, _address: u16, _value: u8) -> bool { false }
}

/// Supported cartridge mapper implementations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cartridge {
    /// Mapper 0, no PRG banking.
    Nrom(Nrom),
    /// Mapper 1, serial PRG/CHR banking and mirroring.
    Mmc1(Mmc1),
    /// Mapper 2, switchable lower PRG bank.
    Uxrom(Uxrom),
    /// Mapper 3, switchable CHR-ROM bank.
    Cnrom(Cnrom),
    /// Mapper 7, switchable 32 KiB PRG bank and one-screen mirroring.
    Axrom(Axrom),
    /// Mapper 4, MMC3 PRG/CHR banking and scanline IRQ counter.
    Mmc3(Mmc3),
    /// Mapper 66, independently switchable PRG and CHR banks.
    Gxrom(Gxrom),
    /// Mapper 71, Camerica switchable PRG bank and one-screen mirroring.
    Camerica(Camerica),
    /// Mapper 11, Color Dreams independently switchable PRG/CHR banks.
    ColorDreams(ColorDreams),
    /// Mapper 34 BNROM variant, switchable 32 KiB PRG and CHR-RAM.
    Bnrom(Bnrom),
    /// Mapper 13 CPROM with a fixed lower CHR-RAM region and banked upper region.
    Cprom(Cprom),
    /// Mapper 9 MMC2 latch-controlled CHR and switchable PRG.
    Mmc2(Mmc2),
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
            1 => Mmc1::parse(image).map(Self::Mmc1),
            2 => Uxrom::parse(image).map(Self::Uxrom),
            3 => Cnrom::parse(image).map(Self::Cnrom),
            4 => Mmc3::parse(image).map(Self::Mmc3),
            7 => Axrom::parse(image).map(Self::Axrom),
            66 => Gxrom::parse(image).map(Self::Gxrom),
            71 => Camerica::parse(image).map(Self::Camerica),
            11 => ColorDreams::parse(image).map(Self::ColorDreams),
            34 => Bnrom::parse(image).map(Self::Bnrom),
            13 => Cprom::parse(image).map(Self::Cprom),
            9 => Mmc2::parse(image).map(Self::Mmc2),
            other => Err(CartridgeError::UnsupportedMapper(other)),
        }
    }

    /// Reads a byte from the CPU address space.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match self { Self::Nrom(c) => c.cpu_read(address), Self::Mmc1(c) => c.cpu_read(address), Self::Uxrom(c) => c.cpu_read(address), Self::Cnrom(c) => c.cpu_read(address), Self::Mmc3(c) => c.cpu_read(address), Self::Axrom(c) => c.cpu_read(address), Self::Gxrom(c) => c.cpu_read(address), Self::Camerica(c) => c.cpu_read(address), Self::ColorDreams(c) => c.cpu_read(address), Self::Bnrom(c) => c.cpu_read(address), Self::Cprom(c) => c.cpu_read(address), Self::Mmc2(c) => c.cpu_read(address) }
    }
    /// Writes cartridge RAM or mapper registers.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match self { Self::Nrom(c) => c.cpu_write(address, value), Self::Mmc1(c) => c.cpu_write(address, value), Self::Uxrom(c) => c.cpu_write(address, value), Self::Cnrom(c) => c.cpu_write(address, value), Self::Mmc3(c) => c.cpu_write(address, value), Self::Axrom(c) => c.cpu_write(address, value), Self::Gxrom(c) => c.cpu_write(address, value), Self::Camerica(c) => c.cpu_write(address, value), Self::ColorDreams(c) => c.cpu_write(address, value), Self::Bnrom(c) => c.cpu_write(address, value), Self::Cprom(c) => c.cpu_write(address, value), Self::Mmc2(c) => c.cpu_write(address, value) }
    }
    /// Returns cartridge nametable mirroring.
    pub fn mirroring(&self) -> Mirroring {
        match self { Self::Nrom(c) => c.mirroring(), Self::Mmc1(c) => c.mirroring(), Self::Uxrom(c) => c.mirroring(), Self::Cnrom(c) => c.mirroring(), Self::Mmc3(c) => c.mirroring(), Self::Axrom(c) => c.mirroring(), Self::Gxrom(c) => c.mirroring(), Self::Camerica(c) => c.mirroring(), Self::ColorDreams(c) => c.mirroring(), Self::Bnrom(c) => c.mirroring(), Self::Cprom(c) => c.mirroring(), Self::Mmc2(c) => c.mirroring() }
    }
    /// Returns true when character memory is RAM.
    pub fn chr_is_ram(&self) -> bool {
        match self { Self::Nrom(c) => c.chr_is_ram(), Self::Mmc1(c) => c.chr_is_ram(), Self::Uxrom(c) => c.chr_is_ram(), Self::Cnrom(c) => c.chr_is_ram(), Self::Mmc3(c) => c.chr_is_ram(), Self::Axrom(c) => c.chr_is_ram(), Self::Gxrom(c) => c.chr_is_ram(), Self::Camerica(c) => c.chr_is_ram(), Self::ColorDreams(c) => c.chr_is_ram(), Self::Bnrom(c) => c.chr_is_ram(), Self::Cprom(c) => c.chr_is_ram(), Self::Mmc2(c) => c.chr_is_ram() }
    }
    /// Clocks a mapper IRQ counter once per approximated scanline boundary.
    pub fn clock_irq_counter(&mut self) { if let Self::Mmc3(c) = self { c.clock_irq_counter(); } }
    /// Returns whether the cartridge mapper asserts an IRQ line.
    pub fn irq_pending(&self) -> bool { matches!(self, Self::Mmc3(c) if c.irq_pending()) }
    /// Reads a byte from PPU pattern memory.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        match self { Self::Nrom(c) => c.ppu_read(address), Self::Mmc1(c) => c.ppu_read(address), Self::Uxrom(c) => c.ppu_read(address), Self::Cnrom(c) => c.ppu_read(address), Self::Mmc3(c) => c.ppu_read(address), Self::Axrom(c) => c.ppu_read(address), Self::Gxrom(c) => c.ppu_read(address), Self::Camerica(c) => c.ppu_read(address), Self::ColorDreams(c) => c.ppu_read(address), Self::Bnrom(c) => c.ppu_read(address), Self::Cprom(c) => c.ppu_read(address), Self::Mmc2(c) => c.ppu_read(address) }
    }
    /// Writes pattern memory only when it is CHR-RAM.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        match self { Self::Nrom(c) => c.ppu_write(address, value), Self::Mmc1(c) => c.ppu_write(address, value), Self::Uxrom(c) => c.ppu_write(address, value), Self::Cnrom(c) => c.ppu_write(address, value), Self::Mmc3(c) => c.ppu_write(address, value), Self::Axrom(c) => c.ppu_write(address, value), Self::Gxrom(c) => c.ppu_write(address, value), Self::Camerica(c) => c.ppu_write(address, value), Self::ColorDreams(c) => c.ppu_write(address, value), Self::Bnrom(c) => c.ppu_write(address, value), Self::Cprom(c) => c.ppu_write(address, value), Self::Mmc2(c) => c.ppu_write(address, value) }
    }
}


#[cfg(test)]
mod axrom_tests {
    use super::*;

    fn axrom_image(prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN + chr_banks as usize * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[5] = chr_banks;
        image[6] = 0x70;
        for bank in 0..(prg_banks as usize / 2) {
            let start = INES_HEADER_LEN + bank * 2 * PRG_BANK_LEN;
            image[start..start + 2 * PRG_BANK_LEN].fill((0x40 + bank) as u8);
        }
        image
    }

    #[test]
    fn mapper_seven_switches_32k_prg_and_one_screen_page() {
        let mut cartridge = Cartridge::parse(&axrom_image(8, 0)).unwrap();
        assert_eq!(cartridge.cpu_read(0x8000), Some(0x40));
        assert_eq!(cartridge.cpu_read(0xFFFF), Some(0x40));
        assert_eq!(cartridge.mirroring(), Mirroring::OneScreenLower);
        assert!(cartridge.cpu_write(0x8000, 0x11));
        assert_eq!(cartridge.cpu_read(0x8000), Some(0x41));
        assert_eq!(cartridge.cpu_read(0xFFFF), Some(0x41));
        assert_eq!(cartridge.mirroring(), Mirroring::OneScreenUpper);
        assert!(cartridge.ppu_write(0x1FFF, 0xA5));
        assert_eq!(cartridge.ppu_read(0x1FFF), Some(0xA5));
    }

    #[test]
    fn mapper_seven_masks_bank_register_to_available_rom_banks() {
        let mut one_bank = Cartridge::parse(&axrom_image(2, 0)).unwrap();
        assert!(one_bank.cpu_write(0x8000, 0x07));
        assert_eq!(one_bank.cpu_read(0x8000), Some(0x40));
        assert_eq!(one_bank.cpu_read(0xFFFF), Some(0x40));

        let mut two_banks = Cartridge::parse(&axrom_image(4, 0)).unwrap();
        assert!(two_banks.cpu_write(0x8000, 0x07));
        assert_eq!(two_banks.cpu_read(0x8000), Some(0x41));
        assert_eq!(two_banks.cpu_read(0xFFFF), Some(0x41));
    }

    #[test]
    fn mapper_seven_rejects_odd_or_oversized_prg_and_chr_rom() {
        assert!(matches!(Cartridge::parse(&axrom_image(3, 0)), Err(CartridgeError::InvalidPrgSize(_))));
        assert!(matches!(Cartridge::parse(&axrom_image(18, 0)), Err(CartridgeError::InvalidPrgSize(_))));
        assert!(matches!(Cartridge::parse(&axrom_image(2, 1)), Err(CartridgeError::InvalidChrSize(_))));
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


#[cfg(test)]
mod mmc1_tests {
    use super::*;

    fn mmc1_image(prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN + chr_banks as usize * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[5] = chr_banks;
        image[6] = 0x10;
        for bank in 0..prg_banks as usize {
            let start = INES_HEADER_LEN + bank * PRG_BANK_LEN;
            image[start..start + PRG_BANK_LEN].fill(bank as u8);
        }
        let chr_start = INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN;
        // Mark each 4-KiB MMC1 CHR bank separately, not each 8-KiB iNES unit.
        for bank in 0..chr_banks as usize * 2 {
            let start = chr_start + bank * 0x1000;
            image[start..start + 0x1000].fill((0x20 + bank) as u8);
        }
        image
    }

    fn serial_write(cart: &mut Cartridge, address: u16, value: u8) {
        for bit in 0..5 { assert!(cart.cpu_write(address, (value >> bit) & 1)); }
    }

    #[test]
    fn mapper_one_serial_register_selects_prg_bank_and_fixed_bank_mode() {
        let mut cart = Cartridge::parse(&mmc1_image(4, 0)).unwrap();
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
        serial_write(&mut cart, 0xE000, 2);
        assert_eq!(cart.cpu_read(0x8000), Some(2));
        assert_eq!(cart.cpu_read(0xC000), Some(3));

        serial_write(&mut cart, 0x8000, 0x08); // Mode 2: first bank fixed at $8000.
        serial_write(&mut cart, 0xE000, 2);
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xC000), Some(2));

        serial_write(&mut cart, 0x8000, 0x00); // Mode 0: paired 32-KiB bank.
        serial_write(&mut cart, 0xE000, 2);
        assert_eq!(cart.cpu_read(0x8000), Some(2));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
    }

    #[test]
    fn mapper_one_switches_chr_4k_banks_and_chr_ram_is_writable() {
        let mut rom = Cartridge::parse(&mmc1_image(2, 4)).unwrap();
        serial_write(&mut rom, 0x8000, 0x1C); // Enable 4-KiB CHR banking.
        serial_write(&mut rom, 0xA000, 2);
        serial_write(&mut rom, 0xC000, 3);
        assert_eq!(rom.ppu_read(0x0000), Some(0x22));
        assert_eq!(rom.ppu_read(0x1000), Some(0x23));
        assert!(!rom.ppu_write(0, 0xAA));

        let mut ram = Cartridge::parse(&mmc1_image(2, 0)).unwrap();
        serial_write(&mut ram, 0x8000, 0x1C);
        serial_write(&mut ram, 0xA000, 1);
        assert!(ram.ppu_write(0x0000, 0xA5));
        assert_eq!(ram.ppu_read(0x0000), Some(0xA5));
    }

    #[test]
    fn mapper_one_controls_mirroring_and_can_disable_prg_ram() {
        let mut cart = Cartridge::parse(&mmc1_image(2, 0)).unwrap();
        serial_write(&mut cart, 0x8000, 0x0C); // One-screen lower.
        assert_eq!(cart.mirroring(), Mirroring::OneScreenLower);
        serial_write(&mut cart, 0x8000, 0x0D); // One-screen upper.
        assert_eq!(cart.mirroring(), Mirroring::OneScreenUpper);
        serial_write(&mut cart, 0x8000, 0x0E); // Vertical.
        assert_eq!(cart.mirroring(), Mirroring::Vertical);
        serial_write(&mut cart, 0x8000, 0x0F); // Horizontal.
        assert_eq!(cart.mirroring(), Mirroring::Horizontal);

        assert!(cart.cpu_write(0x6000, 0x5A));
        assert_eq!(cart.cpu_read(0x6000), Some(0x5A));
        serial_write(&mut cart, 0xE000, 0x10);
        assert_eq!(cart.cpu_read(0x6000), None);
        assert!(!cart.cpu_write(0x6000, 0xA5));
    }

    #[test]
    fn mapper_one_reset_bit_resets_serial_input_and_forces_prg_mode() {
        let mut cart = Cartridge::parse(&mmc1_image(4, 0)).unwrap();
        cart.cpu_write(0xE000, 1);
        cart.cpu_write(0xE000, 1);
        assert!(cart.cpu_write(0x8000, 0x80));
        assert_eq!(cart.mirroring(), Mirroring::Horizontal);
        assert_eq!(cart.cpu_read(0xC000), Some(3));
        serial_write(&mut cart, 0xE000, 2);
        assert_eq!(cart.cpu_read(0x8000), Some(2));
    }

    #[test]
    fn mapper_one_rejects_unsupported_prg_sizes_and_truncated_images() {
        assert!(matches!(Cartridge::parse(&mmc1_image(3, 0)), Err(CartridgeError::InvalidPrgSize(_))));
        let mut image = mmc1_image(2, 1);
        image.pop();
        assert!(matches!(Cartridge::parse(&image), Err(CartridgeError::TruncatedImage { .. })));
    }
}


#[cfg(test)]
mod gxrom_tests {
    use super::*;

    fn gxrom_image(prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN + chr_banks as usize * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[5] = chr_banks;
        image[6] = 0x20; // Mapper 66 low mapper nibble, horizontal mirroring.
        image[7] = 0x40; // Mapper 66 high mapper nibble.
        for bank in 0..(prg_banks as usize / 2) {
            let start = INES_HEADER_LEN + bank * 2 * PRG_BANK_LEN;
            image[start..start + 2 * PRG_BANK_LEN].fill((0x20 + bank) as u8);
        }
        let chr_start = INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN;
        for bank in 0..chr_banks as usize {
            let start = chr_start + bank * CHR_BANK_LEN;
            image[start..start + CHR_BANK_LEN].fill((0x70 + bank) as u8);
        }
        image
    }

    #[test]
    fn mapper_66_switches_prg_and_chr_banks_independently() {
        let mut cartridge = Cartridge::parse(&gxrom_image(8, 4)).unwrap();
        assert_eq!(cartridge.cpu_read(0x8000), Some(0x20));
        assert_eq!(cartridge.cpu_read(0xFFFF), Some(0x20));
        assert_eq!(cartridge.ppu_read(0), Some(0x70));
        assert!(cartridge.cpu_write(0x8000, 0x12));
        assert_eq!(cartridge.cpu_read(0x8000), Some(0x21));
        assert_eq!(cartridge.cpu_read(0xFFFF), Some(0x21));
        assert_eq!(cartridge.ppu_read(0), Some(0x72));
        assert_eq!(cartridge.ppu_read(0x1FFF), Some(0x72));
        assert!(!cartridge.chr_is_ram());
        assert!(!cartridge.ppu_write(0, 0xAA));
        assert_eq!(cartridge.ppu_read(0), Some(0x72));
    }

    #[test]
    fn mapper_66_masks_unavailable_banks_and_preserves_header_mirroring() {
        let mut cartridge = Cartridge::parse(&gxrom_image(2, 1)).unwrap();
        assert_eq!(cartridge.mirroring(), Mirroring::Horizontal);
        assert!(cartridge.cpu_write(0xFFFF, 0x33));
        assert_eq!(cartridge.cpu_read(0x8000), Some(0x20));
        assert_eq!(cartridge.ppu_read(0), Some(0x70));

        let mut vertical_image = gxrom_image(2, 1);
        vertical_image[6] |= 1;
        let vertical = Cartridge::parse(&vertical_image).unwrap();
        assert_eq!(vertical.mirroring(), Mirroring::Vertical);
    }

    #[test]
    fn mapper_66_rejects_invalid_rom_layouts_and_truncation() {
        assert!(matches!(Cartridge::parse(&gxrom_image(1, 1)), Err(CartridgeError::InvalidPrgSize(_))));
        assert!(matches!(Cartridge::parse(&gxrom_image(2, 0)), Err(CartridgeError::InvalidChrSize(0))));
        assert!(matches!(Cartridge::parse(&gxrom_image(10, 1)), Err(CartridgeError::InvalidPrgSize(_))));
        let mut image = gxrom_image(2, 1);
        image.pop();
        assert!(matches!(Cartridge::parse(&image), Err(CartridgeError::TruncatedImage { .. })));
    }
}


#[cfg(test)]
mod mmc3_tests {
    use super::*;

    fn mmc3_image(prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN + chr_banks as usize * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[5] = chr_banks;
        image[6] = 0x40; // Mapper 4.
        let prg_start = INES_HEADER_LEN;
        for bank in 0..(prg_banks as usize * 2) {
            let start = prg_start + bank * 0x2000;
            image[start..start + 0x2000].fill(bank as u8);
        }
        let chr_start = prg_start + prg_banks as usize * PRG_BANK_LEN;
        for bank in 0..(chr_banks as usize * 8) {
            let start = chr_start + bank * 0x400;
            image[start..start + 0x400].fill((0x40 + bank) as u8);
        }
        image
    }

    #[test]
    fn mapper_4_maps_switchable_and_fixed_prg_windows() {
        let mut cart = Cartridge::parse(&mmc3_image(4, 2)).unwrap();
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xA000), Some(1));
        assert_eq!(cart.cpu_read(0xC000), Some(6));
        assert_eq!(cart.cpu_read(0xE000), Some(7));
        cart.cpu_write(0x8000, 6);
        cart.cpu_write(0x8001, 3);
        assert_eq!(cart.cpu_read(0x8000), Some(3));
        cart.cpu_write(0x8000, 0x46);
        assert_eq!(cart.cpu_read(0x8000), Some(6));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
        assert_eq!(cart.cpu_read(0xE000), Some(7));
    }

    #[test]
    fn mapper_4_maps_chr_banks_and_supports_chr_ram() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 2)).unwrap();
        assert_eq!(cart.ppu_read(0), Some(0x40));
        cart.cpu_write(0x8000, 2);
        cart.cpu_write(0x8001, 5);
        assert_eq!(cart.ppu_read(0x1000), Some(0x45));
        cart.cpu_write(0x8000, 0x82);
        cart.cpu_write(0x8001, 6);
        assert_eq!(cart.ppu_read(0), Some(0x46));

        let mut ram = Cartridge::parse(&mmc3_image(2, 0)).unwrap();
        assert!(ram.chr_is_ram());
        assert!(ram.ppu_write(0x0123, 0xA5));
        assert_eq!(ram.ppu_read(0x0123), Some(0xA5));
    }

    #[test]
    fn mapper_4_ram_protection_mirroring_and_irq_registers_work() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        assert_eq!(cart.mirroring(), Mirroring::Horizontal);
        cart.cpu_write(0xA000, 0);
        assert_eq!(cart.mirroring(), Mirroring::Vertical);
        cart.cpu_write(0xA001, 0xC0);
        cart.cpu_write(0x6000, 0x55);
        assert_eq!(cart.cpu_read(0x6000), Some(0));
        cart.cpu_write(0xA001, 0x80);
        cart.cpu_write(0x6000, 0x5A);
        assert_eq!(cart.cpu_read(0x6000), Some(0x5A));
        cart.cpu_write(0xC000, 1);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);
        cart.clock_irq_counter();
        assert!(!cart.irq_pending());
        cart.clock_irq_counter();
        assert!(cart.irq_pending());
        cart.cpu_write(0xE000, 0);
        assert!(!cart.irq_pending());
    }

    #[test]
    fn mapper_4_preserves_hardwired_four_screen_mirroring() {
        let mut image = mmc3_image(2, 1);
        image[6] |= 0x08;
        let mut cart = Cartridge::parse(&image).unwrap();
        cart.cpu_write(0xA000, 0);
        assert_eq!(cart.mirroring(), Mirroring::FourScreen);
    }
}


#[cfg(test)]
mod camerica_tests {
    use super::*;

    fn camerica_image(banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + banks as usize * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = banks;
        image[6] = 0x70 | 0x01; // Mapper 71 low bits, vertical mirroring by default.
        image[7] = 0x40; // Mapper number bits 4-7.
        for bank in 0..banks as usize {
            let start = INES_HEADER_LEN + bank * PRG_BANK_LEN;
            image[start..start + PRG_BANK_LEN].fill(bank as u8);
        }
        image
    }

    #[test]
    fn mapper_71_switches_lower_bank_and_keeps_last_bank_fixed() {
        let mut cart = Cartridge::parse(&camerica_image(4)).unwrap();
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xBFFF), Some(0));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
        assert_eq!(cart.cpu_read(0xFFFF), Some(3));
        assert!(cart.cpu_write(0x8000, 2));
        assert_eq!(cart.cpu_read(0x8000), Some(2));
        assert_eq!(cart.cpu_read(0xBFFF), Some(2));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
    }

    #[test]
    fn mapper_71_supports_camerica_one_screen_mirroring_and_chr_ram() {
        let mut cart = Cartridge::parse(&camerica_image(4)).unwrap();
        assert_eq!(cart.mirroring(), Mirroring::Vertical);
        assert!(cart.cpu_write(0x9000, 0x11));
        assert_eq!(cart.mirroring(), Mirroring::OneScreenUpper);
        assert_eq!(cart.cpu_read(0x8000), Some(1));
        assert!(cart.ppu_write(0x1FFF, 0xA5));
        assert_eq!(cart.ppu_read(0x1FFF), Some(0xA5));
        assert!(cart.chr_is_ram());
    }

    #[test]
    fn mapper_71_rejects_chr_rom_and_invalid_prg_sizes() {
        assert!(matches!(Cartridge::parse(&camerica_image(1)), Err(CartridgeError::InvalidPrgSize(_))));
        let mut image = camerica_image(4);
        image[5] = 1;
        image.extend(vec![0; CHR_BANK_LEN]);
        assert!(matches!(Cartridge::parse(&image), Err(CartridgeError::InvalidChrSize(_))));
    }
}


#[cfg(test)]
mod color_dreams_tests {
    use super::*;

    fn color_dreams_image(prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN + chr_banks as usize * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[5] = chr_banks;
        image[6] = 0xB0;
        for bank in 0..(prg_banks as usize / 2) {
            let start = INES_HEADER_LEN + bank * 2 * PRG_BANK_LEN;
            image[start..start + 2 * PRG_BANK_LEN].fill((0x20 + bank) as u8);
        }
        let chr_start = INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN;
        for bank in 0..chr_banks as usize {
            let start = chr_start + bank * CHR_BANK_LEN;
            image[start..start + CHR_BANK_LEN].fill((0x60 + bank) as u8);
        }
        image
    }

    #[test]
    fn mapper_11_selects_prg_and_chr_banks_independently() {
        let mut cart = Cartridge::parse(&color_dreams_image(8, 4)).unwrap();
        assert_eq!(cart.cpu_read(0x8000), Some(0x20));
        assert_eq!(cart.cpu_read(0xFFFF), Some(0x20));
        assert_eq!(cart.ppu_read(0x0000), Some(0x60));
        assert_eq!(cart.ppu_read(0x1FFF), Some(0x60));
        assert!(cart.cpu_write(0x8000, 0x32));
        assert_eq!(cart.cpu_read(0x8000), Some(0x22));
        assert_eq!(cart.cpu_read(0xFFFF), Some(0x22));
        assert_eq!(cart.ppu_read(0x0000), Some(0x63));
        assert_eq!(cart.ppu_read(0x1FFF), Some(0x63));
        assert!(!cart.ppu_write(0x0000, 0xFF));
    }

    #[test]
    fn mapper_11_rejects_missing_chr_rom_and_invalid_prg_size() {
        assert!(matches!(Cartridge::parse(&color_dreams_image(1, 1)), Err(CartridgeError::InvalidPrgSize(_))));
        assert!(matches!(Cartridge::parse(&color_dreams_image(2, 0)), Err(CartridgeError::InvalidChrSize(_))));
        assert!(matches!(Cartridge::parse(&color_dreams_image(34, 1)), Err(CartridgeError::InvalidPrgSize(_))));
    }
}


#[cfg(test)]
mod bnrom_tests {
    use super::*;

    fn bnrom_image(prg_banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[6] = 0x20 | 0x01;
        image[7] = 0x20; // Mapper 34 high nibble.
        for bank in 0..prg_banks as usize / 2 {
            let start = INES_HEADER_LEN + bank * 2 * PRG_BANK_LEN;
            image[start..start + 2 * PRG_BANK_LEN].fill((0x40 + bank) as u8);
        }
        image
    }

    #[test]
    fn mapper_34_bnrom_switches_entire_32k_prg_window() {
        let mut cart = Cartridge::parse(&bnrom_image(8)).unwrap();
        assert_eq!(cart.cpu_read(0x8000), Some(0x40));
        assert_eq!(cart.cpu_read(0xFFFF), Some(0x40));
        assert!(cart.cpu_write(0x8000, 2));
        assert_eq!(cart.cpu_read(0x8000), Some(0x42));
        assert_eq!(cart.cpu_read(0xFFFF), Some(0x42));
        assert_eq!(cart.mirroring(), Mirroring::Vertical);
    }

    #[test]
    fn mapper_34_bnrom_supports_chr_ram_and_rejects_chr_rom() {
        let mut cart = Cartridge::parse(&bnrom_image(2)).unwrap();
        assert!(cart.chr_is_ram());
        assert!(cart.ppu_write(0x0000, 0x5A));
        assert!(cart.ppu_write(0x1FFF, 0xA5));
        assert_eq!(cart.ppu_read(0x0000), Some(0x5A));
        assert_eq!(cart.ppu_read(0x1FFF), Some(0xA5));
        let mut invalid = bnrom_image(2);
        invalid[5] = 1;
        invalid.extend(vec![0; CHR_BANK_LEN]);
        assert!(matches!(Cartridge::parse(&invalid), Err(CartridgeError::InvalidChrSize(_))));
    }

    #[test]
    fn mapper_34_bnrom_rejects_unsupported_prg_sizes() {
        assert!(matches!(Cartridge::parse(&bnrom_image(1)), Err(CartridgeError::InvalidPrgSize(_))));
        assert!(matches!(Cartridge::parse(&bnrom_image(3)), Err(CartridgeError::InvalidPrgSize(_))));
        assert!(matches!(Cartridge::parse(&bnrom_image(10)), Err(CartridgeError::InvalidPrgSize(_))));
    }
}


#[cfg(test)]
mod cprom_tests {
    use super::*;

    fn cprom_image() -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 2;
        image[6] = 0xD0;
        image[7] = 0;
        image[16..16 + PRG_BANK_LEN].fill(0x31);
        image[16 + PRG_BANK_LEN..].fill(0x32);
        image
    }

    #[test]
    fn mapper_13_keeps_prg_fixed_and_switches_only_upper_chr_4k() {
        let mut cart = Cartridge::parse(&cprom_image()).unwrap();
        assert_eq!(cart.cpu_read(0x8000), Some(0x31));
        assert_eq!(cart.cpu_read(0xC000), Some(0x32));
        assert!(cart.chr_is_ram());
        cart.ppu_write(0x0000, 0xA1);
        cart.ppu_write(0x1000, 0xB1);
        assert_eq!(cart.ppu_read(0x0000), Some(0xB1), "upper bank zero aliases the fixed physical bank");
        assert_eq!(cart.ppu_read(0x1000), Some(0xB1));
        cart.cpu_write(0x8000, 2);
        cart.ppu_write(0x1000, 0xB2);
        assert_eq!(cart.ppu_read(0x1000), Some(0xB2));
        assert_eq!(cart.ppu_read(0x0000), Some(0xB1), "lower address remains mapped to physical bank zero");
        cart.cpu_write(0x8000, 0);
        assert_eq!(cart.ppu_read(0x1000), Some(0xB1), "bank zero aliases the fixed physical CHR bank");
    }

    #[test]
    fn mapper_13_rejects_chr_rom_and_non_32k_prg() {
        let mut invalid = cprom_image();
        invalid[4] = 1;
        invalid.truncate(16 + PRG_BANK_LEN);
        assert!(matches!(Cartridge::parse(&invalid), Err(CartridgeError::InvalidPrgSize(_))));
        let mut invalid = cprom_image();
        invalid[5] = 1;
        invalid.extend(vec![0; CHR_BANK_LEN]);
        assert!(matches!(Cartridge::parse(&invalid), Err(CartridgeError::InvalidChrSize(_))));
    }
}


#[cfg(test)]
mod mmc2_tests {
    use super::*;

    fn mmc2_image(prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN + chr_banks as usize * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[5] = chr_banks;
        image[6] = 0x90;
        let prg_start = INES_HEADER_LEN;
        for bank in 0..prg_banks as usize * 2 {
            image[prg_start + bank * 0x2000..prg_start + (bank + 1) * 0x2000].fill(bank as u8);
        }
        let chr_start = prg_start + prg_banks as usize * PRG_BANK_LEN;
        for bank in 0..chr_banks as usize * 2 {
            image[chr_start + bank * 0x1000..chr_start + (bank + 1) * 0x1000].fill((0x20 + bank) as u8);
        }
        image
    }

    #[test]
    fn mmc2_switches_one_prg_window_and_keeps_three_fixed() {
        let mut c = Mmc2::parse(&mmc2_image(8, 2)).unwrap();
        assert_eq!(c.cpu_read(0x8000), Some(0));
        c.cpu_write(0xA000, 5);
        assert_eq!(c.cpu_read(0x8000), Some(5));
        assert_eq!(c.cpu_read(0xA000), Some(13));
        assert_eq!(c.cpu_read(0xC000), Some(14));
        assert_eq!(c.cpu_read(0xE000), Some(15));
    }

    #[test]
    fn mmc2_chr_latches_select_fd_fe_banks_after_trigger_reads() {
        let mut c = Mmc2::parse(&mmc2_image(8, 2)).unwrap();
        c.cpu_write(0xB000, 2);
        c.cpu_write(0xC000, 3);
        c.cpu_write(0xD000, 2);
        c.cpu_write(0xE000, 3);
        assert_eq!(c.ppu_read(0x0000), Some(0x22));
        assert_eq!(c.ppu_read(0x0FD8), Some(0x22));
        assert_eq!(c.ppu_read(0x0000), Some(0x22));
        assert_eq!(c.ppu_read(0x0FE8), Some(0x22));
        assert_eq!(c.ppu_read(0x0000), Some(0x23));
        assert_eq!(c.ppu_read(0x1FD8), Some(0x22));
        assert_eq!(c.ppu_read(0x1000), Some(0x22));
        assert_eq!(c.ppu_read(0x1FE8), Some(0x22));
        assert_eq!(c.ppu_read(0x1000), Some(0x23));
    }

    #[test]
    fn mmc2_mirroring_register_respects_four_screen_header() {
        let mut c = Mmc2::parse(&mmc2_image(8, 1)).unwrap();
        assert_eq!(c.mirroring(), Mirroring::Horizontal);
        c.cpu_write(0xF000, 0);
        assert_eq!(c.mirroring(), Mirroring::Vertical);
        c.cpu_write(0xF000, 1);
        assert_eq!(c.mirroring(), Mirroring::Horizontal);
        let mut image = mmc2_image(8, 1);
        image[6] |= 0x08;
        let mut four = Mmc2::parse(&image).unwrap();
        four.cpu_write(0xF000, 0);
        assert_eq!(four.mirroring(), Mirroring::FourScreen);
    }

    #[test]
    fn mmc2_rejects_missing_chr_rom_and_small_prg() {
        assert_eq!(Mmc2::parse(&mmc2_image(4, 1)), Err(CartridgeError::InvalidPrgSize(4 * PRG_BANK_LEN)));
        assert_eq!(Mmc2::parse(&mmc2_image(8, 0)), Err(CartridgeError::InvalidChrSize(0)));
    }

    #[test]
    fn mmc2_dispatches_through_cartridge_enum() {
        let mut c = Cartridge::parse(&mmc2_image(8, 1)).unwrap();
        assert!(matches!(c, Cartridge::Mmc2(_)));
        c.cpu_write(0xA000, 6);
        assert_eq!(c.cpu_read(0x8000), Some(6));
        assert_eq!(c.ppu_read(0), Some(0x20));
    }
}

