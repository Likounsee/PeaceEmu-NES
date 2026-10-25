//! iNES parsing and mapper implementations for NROM, MMC1, UxROM, CNROM, MMC3, AxROM, GxROM, Color Dreams, Camerica, BNROM, CPROM, MMC2/MMC4, UNROM 512, and Mapper 15.

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

/// Decodes a NES 2.0 ROM size field. When the high nibble is 0xF, the low byte uses exponent/multiplier encoding; otherwise it is a linear bank count.
fn nes2_rom_size(lsb: u8, msb_nibble: u8, unit: usize) -> usize {
    if msb_nibble == 0x0F {
        let exponent = (lsb >> 2) as u32;
        let multiplier = ((lsb & 0x03) as usize) * 2 + 1;
        // Keep malformed exponent encodings from overflowing the header-plus-ROM
        // length arithmetic used by mapper parsers. Such enormous sizes are invalid
        // for the currently supported boards and will be rejected before slicing.
        1usize.checked_shl(exponent).unwrap_or(usize::MAX)
            .saturating_mul(multiplier)
            .min(usize::MAX / 4)
    } else {
        (((msb_nibble as usize) << 8) | lsb as usize).saturating_mul(unit)
    }
}

/// Decodes a NES 2.0 RAM-size shift nibble (zero means no RAM).
fn nes2_ram_size(shift: u8) -> usize {
    if shift == 0 { 0 } else { 64usize.checked_shl(u32::from(shift)).unwrap_or(usize::MAX) }
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
    /// Parses an iNES or NES 2.0 mapper-0 image with one or two PRG banks.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if image[0] != b'N' || image[1] != b'E' || image[2] != b'S' || image[3] != 0x1A { return Err(CartridgeError::InvalidMagic); }
        let nes2 = image[7] & 0x0C == 0x08;
        let mapper = if nes2 {
            ((image[6] >> 4) as u16)
                | (image[7] & 0xF0) as u16
                | (((image[8] & 0x0F) as u16) << 8)
        } else {
            ((image[6] >> 4) as u16)
                | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 }
        };
        if mapper != 0 { return Err(if nes2 { CartridgeError::Nes2NotSupported } else { CartridgeError::UnsupportedMapper(mapper) }); }

        // NROM has only 16/32 KiB of PRG-ROM and 0/8 KiB of CHR-ROM. Decode
        // both the legacy iNES count fields and NES 2.0's extended linear or
        // exponent/multiplier sizes before applying those board limits.
        let prg_len = if nes2 {
            nes2_rom_size(image[4], image[9] & 0x0F, PRG_BANK_LEN)
        } else {
            image[4] as usize * PRG_BANK_LEN
        };
        if prg_len != PRG_BANK_LEN && prg_len != 2 * PRG_BANK_LEN {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = if nes2 {
            nes2_rom_size(image[5], image[9] >> 4, CHR_BANK_LEN)
        } else {
            image[5] as usize * CHR_BANK_LEN
        };
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
        // Legacy iNES conventionally exposes 8 KiB of PRG-RAM. NES 2.0
        // declares volatile and non-volatile PRG-RAM independently in byte 10;
        // this NROM implementation maps at most the 8 KiB CPU window and mirrors
        // smaller chips across it. A trainer requires the conventional window so
        // its bytes can be installed at $7000-$71FF.
        let declared_prg_ram_len = if nes2 {
            nes2_ram_size(image[10] & 0x0F).saturating_add(nes2_ram_size(image[10] >> 4))
        } else {
            PRG_RAM_LEN
        };
        let prg_ram_len = declared_prg_ram_len.min(PRG_RAM_LEN)
            .max(if trainer_len != 0 { PRG_RAM_LEN } else { 0 });
        let mut prg_ram = vec![0; prg_ram_len];
        if trainer_len != 0 {
            prg_ram[0x1000..0x1200].copy_from_slice(&image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN]);
        }
        // Legacy iNES conventionally supplies 8 KiB of CHR-RAM when CHR-ROM
        // is absent. NES 2.0 instead declares CHR-RAM and CHR-NVRAM in byte 11;
        // a zero shift means that no such memory is present.
        let declared_chr_ram_len = if nes2 {
            nes2_ram_size(image[11] & 0x0F).saturating_add(nes2_ram_size(image[11] >> 4))
        } else {
            CHR_BANK_LEN
        };
        let chr_ram_len = declared_chr_ram_len.min(CHR_BANK_LEN);
        let chr_is_ram = chr_len == 0 && chr_ram_len > 0;
        let chr = if chr_len != 0 {
            image[chr_start..chr_start + chr_len].to_vec()
        } else {
            vec![0; chr_ram_len]
        };
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen }
            else if image[6] & 0x01 != 0 { Mirroring::Vertical }
            else { Mirroring::Horizontal };
        Ok(Self { prg_rom, prg_ram, chr, chr_is_ram, mirroring })
    }

    /// Reads cartridge RAM or PRG-ROM from the CPU address space.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF if !self.prg_ram.is_empty() => {
                Some(self.prg_ram[(address as usize - 0x6000) % self.prg_ram.len()])
            }
            0x6000..=0x7FFF => None,
            0x8000..=0xFFFF => Some(self.prg_rom[(address as usize - 0x8000) % self.prg_rom.len()]),
            _ => None,
        }
    }

    /// Writes the cartridge's 8 KiB PRG-RAM; PRG-ROM remains read-only.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        if !(0x6000..=0x7FFF).contains(&address) || self.prg_ram.is_empty() { return false; }
        let index = (address as usize - 0x6000) % self.prg_ram.len();
        self.prg_ram[index] = value;
        true
    }

    /// Returns cartridge nametable mirroring.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// Returns true when character memory is RAM.
    pub fn chr_is_ram(&self) -> bool { self.chr_is_ram }

    /// Reads a byte from PPU pattern memory.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 || self.chr.is_empty() { return None; }
        Some(self.chr[address as usize % self.chr.len()])
    }

    /// Writes pattern memory only when it is CHR-RAM.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 || !self.chr_is_ram || self.chr.is_empty() { return false; }
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
    /// Parses a legacy iNES MMC1 image with 2, 4, 8, 16 or 32 16-KiB PRG banks.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 1 { return Err(CartridgeError::UnsupportedMapper(mapper)); }

        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if !matches!(image[4], 2 | 4 | 8 | 16 | 32) {
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
        // SUROM boards use CHR bank 0's high bit as the outer 256-KiB PRG
        // selector when the cartridge supplies CHR-RAM. The low four PRG
        // register bits select a bank within that half.
        let outer = if self.chr_is_ram && count > 16 {
            (self.chr_bank_0 & 0x10) as usize
        } else {
            0
        };
        let selected = (outer | (self.prg_bank & 0x0F) as usize) % count;
        let bank = match mode {
            0 | 1 => {
                let paired = (selected & !1) + usize::from(address >= 0xC000);
                paired.min(outer + 15).min(count - 1)
            }
            2 => if address < 0xC000 { outer } else { selected },
            _ => if address < 0xC000 { selected } else { (outer + 15).min(count - 1) },
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
/// NES 2.0 submapper 1 models boards whose ROM drives the data bus during writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Uxrom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    mirroring: Mirroring,
    selected_bank: usize,
    bus_conflicts: bool,
}

impl Uxrom {
    /// Parses an iNES or NES 2.0 mapper-2 image with at least two 16 KiB PRG banks.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        let nes2 = image[7] & 0x0C == 0x08;
        let mapper = if nes2 {
            ((image[6] >> 4) as u16) | (image[7] & 0xF0) as u16
                | (((image[8] & 0x0F) as u16) << 8)
        } else {
            ((image[6] >> 4) as u16)
                | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 }
        };
        if mapper != 2 { return Err(CartridgeError::UnsupportedMapper(mapper)); }

        let prg_len = if nes2 { nes2_rom_size(image[4], image[9] & 0x0F, PRG_BANK_LEN) }
            else { image[4] as usize * PRG_BANK_LEN };
        if prg_len < 2 * PRG_BANK_LEN || prg_len % PRG_BANK_LEN != 0 {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = if nes2 { nes2_rom_size(image[5], image[9] >> 4, CHR_BANK_LEN) }
            else { image[5] as usize * CHR_BANK_LEN };
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
        let declared_prg_ram_len = if nes2 {
            nes2_ram_size(image[10] & 0x0F).saturating_add(nes2_ram_size(image[10] >> 4))
        } else { PRG_RAM_LEN };
        let prg_ram_len = declared_prg_ram_len.min(PRG_RAM_LEN)
            .max(if trainer_len != 0 { PRG_RAM_LEN } else { 0 });
        let mut prg_ram = vec![0; prg_ram_len];
        if trainer_len != 0 {
            prg_ram[0x1000..0x1200].copy_from_slice(&image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN]);
        }
        let declared_chr_ram_len = if nes2 {
            nes2_ram_size(image[11] & 0x0F).saturating_add(nes2_ram_size(image[11] >> 4))
        } else { CHR_BANK_LEN };
        let chr_ram_len = declared_chr_ram_len.min(CHR_BANK_LEN);
        let chr_is_ram = chr_len == 0 && chr_ram_len > 0;
        let chr = if chr_len != 0 { image[chr_start..chr_start + chr_len].to_vec() }
            else { vec![0; chr_ram_len] };
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen }
            else if image[6] & 0x01 != 0 { Mirroring::Vertical }
            else { Mirroring::Horizontal };
        let bus_conflicts = nes2 && image[8] >> 4 == 1;
        Ok(Self { prg_rom: image[prg_start..chr_start].to_vec(), prg_ram, chr, chr_is_ram, mirroring, selected_bank: 0, bus_conflicts })
    }

    /// Reads PRG-RAM or the currently selected/fixed PRG-ROM bank.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF if !self.prg_ram.is_empty() =>
                Some(self.prg_ram[(address as usize - 0x6000) % self.prg_ram.len()]),
            0x6000..=0x7FFF => None,
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
            0x6000..=0x7FFF if !self.prg_ram.is_empty() => {
                let index = (address as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[index] = value;
                true
            }
            0x6000..=0x7FFF => false,
            0x8000..=0xFFFF => {
                // The bank register selects from the complete PRG-ROM bank set.
                // The upper window remains hard-wired to the final bank, but the
                // lower window may also select that same bank on UxROM boards.
                let bank_count = self.prg_rom.len() / PRG_BANK_LEN;
                let effective_value = if self.bus_conflicts {
                    value & self.cpu_read(address).unwrap_or(0xFF)
                } else { value };
                self.selected_bank = effective_value as usize % bank_count;
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
        if address >= 0x2000 || self.chr.is_empty() { return None; }
        Some(self.chr[address as usize % self.chr.len()])
    }
    /// Writes pattern memory only when it is CHR-RAM.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 || !self.chr_is_ram || self.chr.is_empty() { return false; }
        let index = address as usize % self.chr.len();
        self.chr[index] = value;
        true
    }
}


/// CNROM (mapper 3): fixed NROM-style PRG mapping and switchable 8 KiB CHR-ROM bank.
/// NES 2.0 submapper 1 models boards whose ROM drives the data bus during writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cnrom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_rom: Vec<u8>,
    mirroring: Mirroring,
    selected_chr_bank: usize,
    bus_conflicts: bool,
}

impl Cnrom {
    /// Parses an iNES or NES 2.0 mapper-3 image with NROM-sized PRG and 1-4 CHR banks.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        let nes2 = image[7] & 0x0C == 0x08;
        let mapper = if nes2 {
            ((image[6] >> 4) as u16) | (image[7] & 0xF0) as u16
                | (((image[8] & 0x0F) as u16) << 8)
        } else {
            ((image[6] >> 4) as u16)
                | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 }
        };
        if mapper != 3 { return Err(CartridgeError::UnsupportedMapper(mapper)); }

        let prg_len = if nes2 { nes2_rom_size(image[4], image[9] & 0x0F, PRG_BANK_LEN) }
            else { image[4] as usize * PRG_BANK_LEN };
        if prg_len != PRG_BANK_LEN && prg_len != 2 * PRG_BANK_LEN {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = if nes2 { nes2_rom_size(image[5], image[9] >> 4, CHR_BANK_LEN) }
            else { image[5] as usize * CHR_BANK_LEN };
        if chr_len == 0 || chr_len > 4 * CHR_BANK_LEN || chr_len % CHR_BANK_LEN != 0 {
            return Err(CartridgeError::InvalidChrSize(chr_len));
        }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len + chr_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }
        let prg_start = INES_HEADER_LEN + trainer_len;
        let chr_start = prg_start + prg_len;
        let declared_prg_ram_len = if nes2 {
            nes2_ram_size(image[10] & 0x0F).saturating_add(nes2_ram_size(image[10] >> 4))
        } else { PRG_RAM_LEN };
        let prg_ram_len = declared_prg_ram_len.min(PRG_RAM_LEN)
            .max(if trainer_len != 0 { PRG_RAM_LEN } else { 0 });
        let mut prg_ram = vec![0; prg_ram_len];
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
            bus_conflicts: nes2 && image[8] >> 4 == 1,
        })
    }

    /// Reads PRG-RAM or NROM-mapped PRG-ROM.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF if !self.prg_ram.is_empty() =>
                Some(self.prg_ram[(address as usize - 0x6000) % self.prg_ram.len()]),
            0x6000..=0x7FFF => None,
            0x8000..=0xFFFF => Some(self.prg_rom[(address as usize - 0x8000) % self.prg_rom.len()]),
            _ => None,
        }
    }

    /// Writes PRG-RAM or selects the CHR-ROM bank.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF if !self.prg_ram.is_empty() => {
                let index = (address as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[index] = value;
                true
            }
            0x6000..=0x7FFF => false,
            0x8000..=0xFFFF => {
                let effective_value = if self.bus_conflicts {
                    value & self.cpu_read(address).unwrap_or(0xFF)
                } else { value };
                self.selected_chr_bank = effective_value as usize % (self.chr_rom.len() / CHR_BANK_LEN);
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



/// Front Fareast F3xxx (mapper 8): switchable 32-KiB PRG and 8-KiB CHR banks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mapper8 {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    chr_bank_count: usize,
    prg_bank_count: usize,
    selected: u8,
    mirroring: Mirroring,
}

impl Mapper8 {
    /// Parses an iNES or NES 2.0 mapper-8 image with 32-KiB PRG and 8-KiB CHR banks.
    /// NES 2.0 support is limited to submapper 0 and the board's four-bank CHR selector.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        let nes2 = image[7] & 0x0C == 0x08;
        let mapper = if nes2 {
            ((image[6] >> 4) as u16) | (image[7] & 0xF0) as u16
                | (((image[8] & 0x0F) as u16) << 8)
        } else {
            ((image[6] >> 4) as u16)
                | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 }
        };
        if mapper != 8 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        if nes2 && image[8] >> 4 != 0 { return Err(CartridgeError::Nes2NotSupported); }
        let prg_len = if nes2 { nes2_rom_size(image[4], image[9] & 0x0F, PRG_BANK_LEN) }
            else { image[4] as usize * PRG_BANK_LEN };
        if prg_len < 2 * PRG_BANK_LEN || prg_len % (2 * PRG_BANK_LEN) != 0 {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = if nes2 { nes2_rom_size(image[5], image[9] >> 4, CHR_BANK_LEN) }
            else { image[5] as usize * CHR_BANK_LEN };
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len + chr_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }
        let prg_start = INES_HEADER_LEN + trainer_len;
        let chr_start = prg_start + prg_len;
        let declared_prg_ram_len = if nes2 {
            nes2_ram_size(image[10] & 0x0F).saturating_add(nes2_ram_size(image[10] >> 4))
        } else { PRG_RAM_LEN };
        let prg_ram_len = declared_prg_ram_len.min(PRG_RAM_LEN)
            .max(if trainer_len != 0 { PRG_RAM_LEN } else { 0 });
        let mut prg_ram = vec![0; prg_ram_len];
        if trainer_len != 0 {
            prg_ram[0x1000..0x1200].copy_from_slice(&image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN]);
        }
        let declared_chr_ram_len = if nes2 {
            nes2_ram_size(image[11] & 0x0F).saturating_add(nes2_ram_size(image[11] >> 4))
        } else { CHR_BANK_LEN };
        let chr_ram_len = declared_chr_ram_len.min(4 * CHR_BANK_LEN);
        let chr_is_ram = chr_len == 0;
        // The mapper's CHR register selects whole 8 KiB banks. Pad smaller
        // NES 2.0 RAM declarations to one bank to avoid zero-bank indexing.
        let chr = if chr_len != 0 { image[chr_start..chr_start + chr_len].to_vec() }
            else { vec![0; chr_ram_len.max(CHR_BANK_LEN)] };
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen }
            else if image[6] & 0x01 != 0 { Mirroring::Vertical }
            else { Mirroring::Horizontal };
        Ok(Self {
            prg_rom: image[prg_start..chr_start].to_vec(),
            prg_ram,
            chr_bank_count: chr.len() / CHR_BANK_LEN,
            chr,
            chr_is_ram,
            prg_bank_count: prg_len / (2 * PRG_BANK_LEN),
            selected: 0,
            mirroring,
        })
    }

    /// Reads PRG-RAM or the currently selected 32-KiB PRG-ROM bank.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0xFFFF => {
                let bank = ((self.selected >> 4) as usize & 0x03) % self.prg_bank_count;
                let index = bank * 2 * PRG_BANK_LEN
                    + (address as usize - 0x8000);
                Some(self.prg_rom[index])
            }
            _ => None,
        }
    }

    /// Writes PRG-RAM or selects the PRG and CHR banks.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF => {
                self.prg_ram[address as usize - 0x6000] = value;
                true
            }
            0x8000..=0xFFFF => {
                self.selected = value;
                true
            }
            _ => false,
        }
    }

    /// Returns the cartridge's fixed nametable mirroring.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// Returns true when pattern memory is writable CHR-RAM.
    pub fn chr_is_ram(&self) -> bool { self.chr_is_ram }
    /// Reads the selected CHR bank.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 || self.chr.is_empty() { return None; }
        let bank = (self.selected as usize & 0x03) % self.chr_bank_count;
        Some(self.chr[bank * CHR_BANK_LEN + address as usize])
    }
    /// Writes pattern memory only when the cartridge uses CHR-RAM.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 || !self.chr_is_ram || self.chr.is_empty() { return false; }
        let bank = (self.selected as usize & 0x03) % self.chr_bank_count;
        self.chr[bank * CHR_BANK_LEN + address as usize] = value;
        true
    }
}

/// Jaleco JF-16 (mapper 87): fixed 32 KiB PRG-ROM and a two-bank CHR-ROM selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mapper87 {
    prg_rom: Vec<u8>,
    chr_rom: Vec<u8>,
    selected_chr_bank: usize,
    mirroring: Mirroring,
}

impl Mapper87 {
    /// Parses a legacy iNES mapper-87 image with 32 KiB PRG and 32 KiB CHR-ROM.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 87 { return Err(CartridgeError::UnsupportedMapper(mapper)); }

        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if prg_len != 2 * PRG_BANK_LEN {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len != 4 * CHR_BANK_LEN {
            return Err(CartridgeError::InvalidChrSize(chr_len));
        }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len + chr_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }
        let prg_start = INES_HEADER_LEN + trainer_len;
        let chr_start = prg_start + prg_len;
        let mirroring = if image[6] & 0x08 != 0 { Mirroring::FourScreen }
            else if image[6] & 1 != 0 { Mirroring::Vertical }
            else { Mirroring::Horizontal };
        Ok(Self {
            prg_rom: image[prg_start..chr_start].to_vec(),
            chr_rom: image[chr_start..chr_start + chr_len].to_vec(),
            selected_chr_bank: 0,
            mirroring,
        })
    }

    /// Reads the fixed 32 KiB PRG-ROM window; this board has no PRG-RAM.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => None,
            0x8000..=0xFFFF => Some(self.prg_rom[address as usize - 0x8000]),
            _ => None,
        }
    }

    /// Selects the CHR bank; hardware swaps the two low register bits.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        if !(0x6000..=0x7FFF).contains(&address) { return false; }
        self.selected_chr_bank = (((value & 1) << 1) | ((value & 2) >> 1)) as usize;
        true
    }

    /// Returns the cartridge's fixed nametable mirroring mode.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// Mapper 87 stores character data in read-only ROM.
    pub fn chr_is_ram(&self) -> bool { false }
    /// Reads from the currently selected 8 KiB CHR-ROM bank.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        Some(self.chr_rom[self.selected_chr_bank * CHR_BANK_LEN + address as usize])
    }
    /// CHR-ROM is read-only, so writes are ignored.
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
        if image[4] < 2 || image[4] > 16 || image[4] % 2 != 0 {
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
        let nes2 = image[7] & 0x0C == 0x08;
        let mapper = if nes2 {
            ((image[6] >> 4) as u16) | (image[7] & 0xF0) as u16
                | (((image[8] & 0x0F) as u16) << 8)
        } else {
            ((image[6] >> 4) as u16)
                | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 }
        };
        if mapper != 66 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        if nes2 && image[8] >> 4 != 0 { return Err(CartridgeError::Nes2NotSupported); }

        let prg_len = if nes2 { nes2_rom_size(image[4], image[9] & 0x0F, PRG_BANK_LEN) }
            else { image[4] as usize * PRG_BANK_LEN };
        if prg_len < 2 * PRG_BANK_LEN || prg_len % (2 * PRG_BANK_LEN) != 0
            || prg_len > 4 * 2 * PRG_BANK_LEN {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = if nes2 { nes2_rom_size(image[5], image[9] >> 4, CHR_BANK_LEN) }
            else { image[5] as usize * CHR_BANK_LEN };
        // GxROM exposes at most four 8 KiB CHR-ROM banks.
        if chr_len == 0 || chr_len > 4 * CHR_BANK_LEN || chr_len % CHR_BANK_LEN != 0 {
            return Err(CartridgeError::InvalidChrSize(chr_len));
        }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len + chr_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }

        let prg_start = INES_HEADER_LEN + trainer_len;
        let chr_start = prg_start + prg_len;
        let prg_ram_len = if nes2 { nes2_ram_size(image[10] & 0x0F).saturating_add(nes2_ram_size(image[10] >> 4)).min(PRG_RAM_LEN) } else { PRG_RAM_LEN };
        let mut prg_ram = vec![0; prg_ram_len.max(if trainer_len != 0 { PRG_RAM_LEN } else { 0 })];
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

    /// Reads cartridge PRG-RAM (when present) or the selected 32 KiB PRG-ROM bank.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF if !self.prg_ram.is_empty() =>
                Some(self.prg_ram[(address as usize - 0x6000) % self.prg_ram.len()]),
            0x6000..=0x7FFF => None,
            0x8000..=0xFFFF => {
                let index = self.selected_prg_bank * 2 * PRG_BANK_LEN + address as usize - 0x8000;
                Some(self.prg_rom[index])
            }
            _ => None,
        }
    }

    /// Writes PRG-RAM when present or selects PRG (bits 4-5) and CHR (bits 0-1) banks.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF if !self.prg_ram.is_empty() => {
                let index = (address as usize - 0x6000) % self.prg_ram.len();
                self.prg_ram[index] = value;
                true
            }
            0x6000..=0x7FFF => false,
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
    a12_high: bool, a12_low_since: Option<u64>, last_ppu_cycle: Option<u64>,
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
        if image[4] < 2 || image[4] % 2 != 0 || prg_len > 32 * PRG_BANK_LEN {
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
            ram_write_protected: false, irq_latch: 0, irq_counter: 0, irq_reload: false, irq_enabled: false, irq_pending: false, a12_high: false, a12_low_since: None, last_ppu_cycle: None })
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
    /// Observes the PPU address bus for MMC3's filtered A12 rising-edge IRQ clock.
    ///
    /// `ppu_cycle` is a monotonically increasing PPU-dot counter. A rising A12 edge
    /// clocks the IRQ counter only after A12 has remained low for at least eight dots.
    /// Call this once for each actual PPU bus address, not once per CPU instruction.
    pub fn observe_ppu_address(&mut self, address: u16, ppu_cycle: u64) {
        // A PPU reset can restart the cycle counter while the cartridge remains
        // inserted. Rebase the A12 filter instead of treating a backwards timestamp
        // as an enormous low interval or waiting for the old counter to catch up.
        if self.last_ppu_cycle.is_some_and(|last| ppu_cycle < last) {
            // Rebase to the bus level actually observed after reset. Treating a
            // high A12 address as low starts the filter too early and can admit
            // a rising edge after only seven real low dots.
            let a12 = address & 0x1000 != 0;
            self.a12_high = a12;
            self.a12_low_since = if a12 { None } else { Some(ppu_cycle) };
            self.last_ppu_cycle = Some(ppu_cycle);
            return;
        }
        self.last_ppu_cycle = Some(ppu_cycle);

        let a12 = address & 0x1000 != 0;
        if !a12 {
            if self.a12_high || self.a12_low_since.is_none() {
                self.a12_low_since = Some(ppu_cycle);
            }
            self.a12_high = false;
            return;
        }

        if !self.a12_high {
            let low_duration = self.a12_low_since
                .map(|start| ppu_cycle.saturating_sub(start))
                .unwrap_or(0);
            if low_duration >= 8 {
                self.clock_irq_counter();
            }
        }
        self.a12_high = true;
        self.a12_low_since = None;
    }

    /// Manually clocks the IRQ counter for diagnostics; normal PPU operation clocks it through filtered A12 rising edges.
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
        if !(2..=8).contains(&image[4]) || image[4] % 2 != 0 {
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
    /// Parses an iNES or NES 2.0 mapper-71 image using CHR-RAM and 2-16 PRG banks.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        let nes2 = image[7] & 0x0C == 0x08;
        let mapper = if nes2 {
            ((image[6] >> 4) as u16) | (image[7] & 0xF0) as u16
                | (((image[8] & 0x0F) as u16) << 8)
        } else {
            ((image[6] >> 4) as u16)
                | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 }
        };
        if mapper != 71 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        if nes2 && image[8] >> 4 != 0 { return Err(CartridgeError::Nes2NotSupported); }
        let prg_len = if nes2 { nes2_rom_size(image[4], image[9] & 0x0F, PRG_BANK_LEN) }
            else { image[4] as usize * PRG_BANK_LEN };
        if !(2 * PRG_BANK_LEN..=16 * PRG_BANK_LEN).contains(&prg_len)
            || prg_len % PRG_BANK_LEN != 0 {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        let chr_len = if nes2 { nes2_rom_size(image[5], image[9] >> 4, CHR_BANK_LEN) }
            else { image[5] as usize * CHR_BANK_LEN };
        if chr_len != 0 { return Err(CartridgeError::InvalidChrSize(chr_len)); }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }
        let prg_start = INES_HEADER_LEN + trainer_len;
        let ram_len = if nes2 {
            nes2_ram_size(image[10] & 0x0F).saturating_add(nes2_ram_size(image[10] >> 4))
        } else { PRG_RAM_LEN };
        if ram_len > PRG_RAM_LEN { return Err(CartridgeError::Nes2NotSupported); }
        let mut prg_ram = vec![0; if trainer_len != 0 { ram_len.max(PRG_RAM_LEN) } else { ram_len }];
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
            0x6000..=0x7FFF => self.prg_ram.get(address as usize - 0x6000).copied(),
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
                if let Some(byte) = self.prg_ram.get_mut(address as usize - 0x6000) {
                    *byte = value;
                    true
                } else { false }
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
        if image[4] < 2 || image[4] % 2 != 0 || image[4] > 32 {
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
    mmc4: bool,
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
        if mapper != 9 && mapper != 10 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        let mmc4 = mapper == 10;
        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if (mmc4 && image[4] < 2) || (!mmc4 && image[4] < 8) {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
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
            mmc4,
            prg_bank: 0,
            chr_fd0: 0, chr_fe0: 0, chr_fd1: 0, chr_fe1: 0,
            latch0_fe: Cell::new(false), latch1_fe: Cell::new(false),
            mirroring,
        })
    }

    /// Reads PRG-RAM and the mapper's banked/fixed PRG-ROM windows.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        if self.mmc4 {
            let bank_len = 0x4000;
            let bank_count = self.prg_rom.len() / bank_len;
            return match address {
                0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
                0x8000..=0xBFFF => {
                    let bank = self.prg_bank % bank_count;
                    Some(self.prg_rom[bank * bank_len + address as usize - 0x8000])
                }
                0xC000..=0xFFFF => {
                    let bank = bank_count - 1;
                    Some(self.prg_rom[bank * bank_len + address as usize - 0xC000])
                }
                _ => None,
            };
        }
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
            // MMC2 latches respond to the whole eight-byte trigger ranges, not
            // just the first address. PPU fetches may hit any byte in each range.
            0x0FD8..=0x0FDF => self.latch0_fe.set(false),
            0x0FE8..=0x0FEF => self.latch0_fe.set(true),
            0x1FD8..=0x1FDF => self.latch1_fe.set(false),
            0x1FE8..=0x1FEF => self.latch1_fe.set(true),
            _ => {}
        }
        Some(value)
    }

    /// Rejects writes because MMC2 uses CHR-ROM rather than CHR-RAM.
    pub fn ppu_write(&mut self, _address: u16, _value: u8) -> bool { false }
}


/// UNROM-512 (mapper 30): switchable 16 KiB PRG, 32 KiB banked CHR-RAM and one-screen mirroring.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unrom512 {
    prg_rom: Vec<u8>,
    chr_ram: Vec<u8>,
    prg_bank: usize,
    chr_bank: usize,
    mirroring: Mirroring,
    four_screen: bool,
    switchable_mirroring: bool,
}

impl Unrom512 {
    /// Parses the legacy iNES mapper-30 subset with PRG-ROM and CHR-RAM.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 30 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if image[4] != 16 && image[4] != 32 { return Err(CartridgeError::InvalidPrgSize(prg_len)); }
        let chr_len = image[5] as usize * CHR_BANK_LEN;
        if chr_len != 0 { return Err(CartridgeError::InvalidChrSize(chr_len)); }
        let trainer_len = if image[6] & 0x04 != 0 { TRAINER_LEN } else { 0 };
        let expected = INES_HEADER_LEN + trainer_len + prg_len;
        if image.len() < expected {
            return Err(CartridgeError::TruncatedImage { expected, actual: image.len() });
        }
        let prg_start = INES_HEADER_LEN + trainer_len;
        Ok(Self {
            prg_rom: image[prg_start..prg_start + prg_len].to_vec(),
            chr_ram: vec![0; 4 * CHR_BANK_LEN],
            prg_bank: 0,
            chr_bank: 0,
            mirroring: if image[6] & 0x08 != 0 && image[6] & 1 != 0 { Mirroring::FourScreen }
                else if image[6] & 0x08 != 0 { Mirroring::OneScreenLower }
                else if image[6] & 1 != 0 { Mirroring::Vertical } else { Mirroring::Horizontal },
            four_screen: image[6] & 0x08 != 0 && image[6] & 1 != 0,
            switchable_mirroring: image[6] & 0x08 != 0 && image[6] & 1 == 0,
        })
    }

    /// Reads a selectable 16 KiB bank or the fixed final 16 KiB bank.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x8000..=0xBFFF => {
                let bank_count = self.prg_rom.len() / PRG_BANK_LEN;
                Some(self.prg_rom[(self.prg_bank % bank_count) * PRG_BANK_LEN + address as usize - 0x8000])
            }
            0xC000..=0xFFFF => Some(self.prg_rom[self.prg_rom.len() - PRG_BANK_LEN + address as usize - 0xC000]),
            _ => None,
        }
    }

    /// Selects PRG bank (bits 0-4), CHR-RAM bank (bits 5-6), and one-screen page (bit 7).
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x8000..=0xFFFF => {
                self.prg_bank = (value & 0x1F) as usize;
                if self.switchable_mirroring && !self.four_screen {
                    self.mirroring = if value & 0x80 == 0 { Mirroring::OneScreenLower } else { Mirroring::OneScreenUpper };
                }
                self.chr_bank = ((value >> 5) & 3) as usize;
                true
            }
            _ => false,
        }
    }

    /// Returns the current one-screen nametable page.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// UNROM-512 stores pattern data in CHR-RAM.
    pub fn chr_is_ram(&self) -> bool { true }

    /// Reads from the selected 8 KiB CHR-RAM bank.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { return None; }
        Some(self.chr_ram[self.chr_bank * CHR_BANK_LEN + address as usize])
    }

    /// Writes to the selected 8 KiB CHR-RAM bank.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 { return false; }
        self.chr_ram[self.chr_bank * CHR_BANK_LEN + address as usize] = value;
        true
    }
}

/// Mapper 15 (100-in-1 Contra Function 16): address-selected PRG banking with CHR-RAM.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mapper15 {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_ram: Vec<u8>,
    bank_register: u8,
    mode: u8,
    mirroring: Mirroring,
}

impl Mapper15 {
    /// Parses a legacy iNES mapper-15 image with PRG-ROM and 8 KiB CHR-RAM.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        if image[7] & 0x0C == 0x08 { return Err(CartridgeError::Nes2NotSupported); }
        let mapper = ((image[6] >> 4) as u16)
            | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 };
        if mapper != 15 { return Err(CartridgeError::UnsupportedMapper(mapper)); }
        let prg_len = image[4] as usize * PRG_BANK_LEN;
        if image[4] < 2 || image[4] % 2 != 0 || prg_len > 64 * PRG_BANK_LEN {
            return Err(CartridgeError::InvalidPrgSize(prg_len));
        }
        if image[5] != 0 { return Err(CartridgeError::InvalidChrSize(image[5] as usize * CHR_BANK_LEN)); }
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
            bank_register: 0,
            mode: 0,
            mirroring: Mirroring::Vertical,
        })
    }

    fn bank8k(&self, address: u16) -> usize {
        let count = self.prg_rom.len() / 0x2000;
        let base = (usize::from(self.bank_register & 0x3F) * 2) % count;
        let slot = ((address - 0x8000) as usize) / 0x2000;
        let bank = match self.mode & 3 {
            // NROM-256: two adjacent 16 KiB banks.
            0 => base + slot,
            // UNROM: selected 16 KiB lower window and fixed final 16 KiB.
            1 => if slot < 2 { base + slot } else { count - 2 + (slot & 1) },
            // NROM-64: a single selected 8 KiB bank mirrored across the CPU window.
            2 => (base + usize::from(self.bank_register >> 7)) % count,
            // NROM-128: selected 16 KiB bank mirrored in both halves.
            _ => base + (slot & 1),
        };
        bank % count
    }

    /// Reads PRG-RAM or the mapper-selected 8 KiB PRG-ROM bank.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7FFF => Some(self.prg_ram[address as usize - 0x6000]),
            0x8000..=0xFFFF => Some(self.prg_rom[self.bank8k(address) * 0x2000 + (address as usize & 0x1FFF)]),
            _ => None,
        }
    }

    /// Selects mapper mode from address A1-A0 and PRG bank/mirroring from the data byte.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7FFF => { self.prg_ram[address as usize - 0x6000] = value; true }
            0x8000..=0xFFFF => {
                self.mode = (address & 3) as u8;
                self.bank_register = value;
                self.mirroring = if value & 0x40 == 0 { Mirroring::Vertical } else { Mirroring::Horizontal };
                true
            }
            _ => false,
        }
    }

    /// Returns the currently selected nametable mirroring mode.
    pub fn mirroring(&self) -> Mirroring { self.mirroring }
    /// Mapper 15 uses 8 KiB of CHR-RAM.
    pub fn chr_is_ram(&self) -> bool { true }
    /// Reads from the 8 KiB CHR-RAM region.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        if address >= 0x2000 { None } else { Some(self.chr_ram[address as usize]) }
    }
    /// Writes to CHR-RAM (including mapper-hacked ROMs that expect writable CHR memory).
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address >= 0x2000 { return false; }
        self.chr_ram[address as usize] = value;
        true
    }
}

fn copy_ram(target: &mut [u8], source: &[u8]) -> bool {
    if target.len() != source.len() { return false; }
    target.copy_from_slice(source);
    true
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
    /// Mapper 9 MMC2 / mapper 10 MMC4 latch-controlled CHR and switchable PRG.
    Mmc2(Mmc2),
    /// Mapper 30 UNROM-512 with banked CHR-RAM.
    Unrom512(Unrom512),
    /// Mapper 15 100-in-1 multicart with four PRG banking modes.
    Mapper15(Mapper15),
    /// Mapper 8 Front Fareast F3xxx with switchable PRG/CHR banks.
    Mapper8(Mapper8),
    /// Mapper 87 Jaleco JF-16 with a two-bank CHR selector.
    Mapper87(Mapper87),
}

impl Cartridge {
    /// Parses a supported legacy iNES cartridge and selects its mapper implementation.
    pub fn parse(image: &[u8]) -> Result<Self, CartridgeError> {
        if image.len() < INES_HEADER_LEN { return Err(CartridgeError::TruncatedHeader); }
        if &image[0..4] != b"NES\x1A" { return Err(CartridgeError::InvalidMagic); }
        let nes2 = image[7] & 0x0C == 0x08;
        if nes2 {
            let mapper = ((image[6] >> 4) as u16)
                | (image[7] & 0xF0) as u16
                | (((image[8] & 0x0F) as u16) << 8);
            if !matches!(mapper, 0 | 2 | 3 | 8 | 66 | 71) { return Err(CartridgeError::Nes2NotSupported); }
        }
        let mapper = if nes2 {
            ((image[6] >> 4) as u16)
                | (image[7] & 0xF0) as u16
                | (((image[8] & 0x0F) as u16) << 8)
        } else {
            ((image[6] >> 4) as u16)
                | if image[7] & 0x0C == 0 { (image[7] & 0xF0) as u16 } else { 0 }
        };
        match mapper {
            0 => Nrom::parse(image).map(Self::Nrom),
            1 => Mmc1::parse(image).map(Self::Mmc1),
            2 => Uxrom::parse(image).map(Self::Uxrom),
            3 => Cnrom::parse(image).map(Self::Cnrom),
            4 => Mmc3::parse(image).map(Self::Mmc3),
            7 => Axrom::parse(image).map(Self::Axrom),
            8 => Mapper8::parse(image).map(Self::Mapper8),
            66 => Gxrom::parse(image).map(Self::Gxrom),
            71 => Camerica::parse(image).map(Self::Camerica),
            11 => ColorDreams::parse(image).map(Self::ColorDreams),
            34 => Bnrom::parse(image).map(Self::Bnrom),
            13 => Cprom::parse(image).map(Self::Cprom),
            15 => Mapper15::parse(image).map(Self::Mapper15),
            9 | 10 => Mmc2::parse(image).map(Self::Mmc2),
            30 => Unrom512::parse(image).map(Self::Unrom512),
            87 => Mapper87::parse(image).map(Self::Mapper87),
            other => Err(CartridgeError::UnsupportedMapper(other)),
        }
    }

    /// Reads a byte from the CPU address space.
    pub fn cpu_read(&self, address: u16) -> Option<u8> {
        match self { Self::Nrom(c) => c.cpu_read(address), Self::Mmc1(c) => c.cpu_read(address), Self::Uxrom(c) => c.cpu_read(address), Self::Cnrom(c) => c.cpu_read(address), Self::Mmc3(c) => c.cpu_read(address), Self::Axrom(c) => c.cpu_read(address), Self::Gxrom(c) => c.cpu_read(address), Self::Camerica(c) => c.cpu_read(address), Self::ColorDreams(c) => c.cpu_read(address), Self::Bnrom(c) => c.cpu_read(address), Self::Cprom(c) => c.cpu_read(address), Self::Mmc2(c) => c.cpu_read(address), Self::Unrom512(c) => c.cpu_read(address), Self::Mapper15(c) => c.cpu_read(address), Self::Mapper8(c) => c.cpu_read(address), Self::Mapper87(c) => c.cpu_read(address) }
    }

    /// Returns a copy of the cartridge's battery-backed PRG-RAM region, if present.
    ///
    /// This deliberately bypasses mapper runtime read-enable bits so a save can be
    /// captured consistently even while game code has temporarily disabled RAM.
    pub fn prg_ram_snapshot(&self) -> Option<Vec<u8>> {
        match self {
            Self::Nrom(c) => Some(c.prg_ram.clone()),
            Self::Mmc1(c) => Some(c.prg_ram.clone()),
            Self::Uxrom(c) => Some(c.prg_ram.clone()),
            Self::Cnrom(c) => Some(c.prg_ram.clone()),
            Self::Mmc3(c) => Some(c.prg_ram.clone()),
            Self::Axrom(c) => Some(c.prg_ram.clone()),
            Self::Gxrom(c) => Some(c.prg_ram.clone()),
            Self::Camerica(c) => Some(c.prg_ram.clone()),
            Self::ColorDreams(c) => Some(c.prg_ram.clone()),
            Self::Bnrom(c) => Some(c.prg_ram.clone()),
            Self::Cprom(c) => Some(c.prg_ram.clone()),
            Self::Mmc2(c) => Some(c.prg_ram.clone()),
            Self::Unrom512(_) => None,
            Self::Mapper15(c) => Some(c.prg_ram.clone()), Self::Mapper8(c) => Some(c.prg_ram.clone()), Self::Mapper87(_) => None,
        }
    }

    /// Restores cartridge PRG-RAM after the caller validates the complete payload.
    pub fn restore_prg_ram(&mut self, data: &[u8]) -> bool {
        match self {
            Self::Nrom(c) => copy_ram(&mut c.prg_ram, data),
            Self::Mmc1(c) => copy_ram(&mut c.prg_ram, data),
            Self::Uxrom(c) => copy_ram(&mut c.prg_ram, data),
            Self::Cnrom(c) => copy_ram(&mut c.prg_ram, data),
            Self::Mmc3(c) => copy_ram(&mut c.prg_ram, data),
            Self::Axrom(c) => copy_ram(&mut c.prg_ram, data),
            Self::Gxrom(c) => copy_ram(&mut c.prg_ram, data),
            Self::Camerica(c) => copy_ram(&mut c.prg_ram, data),
            Self::ColorDreams(c) => copy_ram(&mut c.prg_ram, data),
            Self::Bnrom(c) => copy_ram(&mut c.prg_ram, data),
            Self::Cprom(c) => copy_ram(&mut c.prg_ram, data),
            Self::Mmc2(c) => copy_ram(&mut c.prg_ram, data),
            Self::Unrom512(_) => false,
            Self::Mapper15(c) => copy_ram(&mut c.prg_ram, data), Self::Mapper8(c) => copy_ram(&mut c.prg_ram, data), Self::Mapper87(_) => false,
        }
    }
    /// Writes cartridge RAM or mapper registers.
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match self { Self::Nrom(c) => c.cpu_write(address, value), Self::Mmc1(c) => c.cpu_write(address, value), Self::Uxrom(c) => c.cpu_write(address, value), Self::Cnrom(c) => c.cpu_write(address, value), Self::Mmc3(c) => c.cpu_write(address, value), Self::Axrom(c) => c.cpu_write(address, value), Self::Gxrom(c) => c.cpu_write(address, value), Self::Camerica(c) => c.cpu_write(address, value), Self::ColorDreams(c) => c.cpu_write(address, value), Self::Bnrom(c) => c.cpu_write(address, value), Self::Cprom(c) => c.cpu_write(address, value), Self::Mmc2(c) => c.cpu_write(address, value), Self::Unrom512(c) => c.cpu_write(address, value), Self::Mapper15(c) => c.cpu_write(address, value), Self::Mapper8(c) => c.cpu_write(address, value), Self::Mapper87(c) => c.cpu_write(address, value) }
    }
    /// Returns cartridge nametable mirroring.
    pub fn mirroring(&self) -> Mirroring {
        match self { Self::Nrom(c) => c.mirroring(), Self::Mmc1(c) => c.mirroring(), Self::Uxrom(c) => c.mirroring(), Self::Cnrom(c) => c.mirroring(), Self::Mmc3(c) => c.mirroring(), Self::Axrom(c) => c.mirroring(), Self::Gxrom(c) => c.mirroring(), Self::Camerica(c) => c.mirroring(), Self::ColorDreams(c) => c.mirroring(), Self::Bnrom(c) => c.mirroring(), Self::Cprom(c) => c.mirroring(), Self::Mmc2(c) => c.mirroring(), Self::Unrom512(c) => c.mirroring(), Self::Mapper15(c) => c.mirroring(), Self::Mapper8(c) => c.mirroring(), Self::Mapper87(c) => c.mirroring() }
    }
    /// Returns true when character memory is RAM.
    pub fn chr_is_ram(&self) -> bool {
        match self { Self::Nrom(c) => c.chr_is_ram(), Self::Mmc1(c) => c.chr_is_ram(), Self::Uxrom(c) => c.chr_is_ram(), Self::Cnrom(c) => c.chr_is_ram(), Self::Mmc3(c) => c.chr_is_ram(), Self::Axrom(c) => c.chr_is_ram(), Self::Gxrom(c) => c.chr_is_ram(), Self::Camerica(c) => c.chr_is_ram(), Self::ColorDreams(c) => c.chr_is_ram(), Self::Bnrom(c) => c.chr_is_ram(), Self::Cprom(c) => c.chr_is_ram(), Self::Mmc2(c) => c.chr_is_ram(), Self::Unrom512(c) => c.chr_is_ram(), Self::Mapper15(c) => c.chr_is_ram(), Self::Mapper8(c) => c.chr_is_ram(), Self::Mapper87(c) => c.chr_is_ram() }
    }
    /// Reports a PPU address-bus observation to mappers with edge-clocked IRQs.
    pub fn observe_ppu_address(&mut self, address: u16, ppu_cycle: u64) {
        if let Self::Mmc3(c) = self { c.observe_ppu_address(address, ppu_cycle); }
    }
    /// Clocks a mapper IRQ counter once per approximated scanline boundary.
    pub fn clock_irq_counter(&mut self) { if let Self::Mmc3(c) = self { c.clock_irq_counter(); } }
    /// Returns whether the cartridge mapper asserts an IRQ line.
    pub fn irq_pending(&self) -> bool { matches!(self, Self::Mmc3(c) if c.irq_pending()) }
    /// Reads a byte from PPU pattern memory.
    pub fn ppu_read(&self, address: u16) -> Option<u8> {
        match self { Self::Nrom(c) => c.ppu_read(address), Self::Mmc1(c) => c.ppu_read(address), Self::Uxrom(c) => c.ppu_read(address), Self::Cnrom(c) => c.ppu_read(address), Self::Mmc3(c) => c.ppu_read(address), Self::Axrom(c) => c.ppu_read(address), Self::Gxrom(c) => c.ppu_read(address), Self::Camerica(c) => c.ppu_read(address), Self::ColorDreams(c) => c.ppu_read(address), Self::Bnrom(c) => c.ppu_read(address), Self::Cprom(c) => c.ppu_read(address), Self::Mmc2(c) => c.ppu_read(address), Self::Unrom512(c) => c.ppu_read(address), Self::Mapper15(c) => c.ppu_read(address), Self::Mapper8(c) => c.ppu_read(address), Self::Mapper87(c) => c.ppu_read(address) }
    }
    /// Writes pattern memory only when it is CHR-RAM.
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        match self { Self::Nrom(c) => c.ppu_write(address, value), Self::Mmc1(c) => c.ppu_write(address, value), Self::Uxrom(c) => c.ppu_write(address, value), Self::Cnrom(c) => c.ppu_write(address, value), Self::Mmc3(c) => c.ppu_write(address, value), Self::Axrom(c) => c.ppu_write(address, value), Self::Gxrom(c) => c.ppu_write(address, value), Self::Camerica(c) => c.ppu_write(address, value), Self::ColorDreams(c) => c.ppu_write(address, value), Self::Bnrom(c) => c.ppu_write(address, value), Self::Cprom(c) => c.ppu_write(address, value), Self::Mmc2(c) => c.ppu_write(address, value), Self::Unrom512(c) => c.ppu_write(address, value), Self::Mapper15(c) => c.ppu_write(address, value), Self::Mapper8(c) => c.ppu_write(address, value), Self::Mapper87(c) => c.ppu_write(address, value) }
    }
}


    #[test]
    fn mmc1_prg_ram_disable_bit_blocks_reads_and_writes_until_reenabled() {
        let mut image = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN];
        image[0..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        image[4] = 2;
        image[6] = 0x10; // Mapper 1, CHR-RAM.
        let mut cart = Cartridge::parse(&image).unwrap();

        assert!(cart.cpu_write(0x6000, 0x5A));
        assert_eq!(cart.cpu_read(0x6000), Some(0x5A));

        // MMC1 PRG register is loaded least-significant bit first; bit 4 disables RAM.
        for bit in [0, 0, 0, 0, 1] {
            assert!(cart.cpu_write(0xE000, bit));
        }
        assert_eq!(cart.cpu_read(0x6000), None);
        assert!(!cart.cpu_write(0x6000, 0xA5));

        // Clear the disable bit and verify the original byte was not mutated.
        for bit in [0, 0, 0, 0, 0] {
            assert!(cart.cpu_write(0xE000, bit));
        }
        assert_eq!(cart.cpu_read(0x6000), Some(0x5A));
    }

    #[test]
    fn mmc1_surom_chr_ram_high_bit_selects_outer_prg_half() {
        let mut image = vec![0; INES_HEADER_LEN + 32 * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 32;
        image[6] = 0x10; // MMC1 with CHR-RAM.
        for bank in 0..32usize {
            image[INES_HEADER_LEN + bank * PRG_BANK_LEN..INES_HEADER_LEN + (bank + 1) * PRG_BANK_LEN]
                .fill(bank as u8);
        }
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xC000), Some(15));

        // Set CHR0 bit 4 using five serial writes; SUROM routes it to PRG A18.
        for bit in [0, 0, 0, 0, 1] {
            cart.cpu_write(0xA000, bit);
        }
        assert_eq!(cart.cpu_read(0x8000), Some(16));
        assert_eq!(cart.cpu_read(0xC000), Some(31));

        // In 32-KiB mode, the selected pair remains inside the upper half.
        for bit in [0, 0, 0, 0, 0] {
            cart.cpu_write(0x8000, bit);
        }
        assert_eq!(cart.cpu_read(0x8000), Some(16));
        assert_eq!(cart.cpu_read(0xC000), Some(17));
    }

#[cfg(test)]
mod mapper15_tests {
    use super::*;

    fn image(prg_banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[6] = 0xF0;
        for bank in 0..prg_banks as usize {
            image[INES_HEADER_LEN + bank * PRG_BANK_LEN..INES_HEADER_LEN + (bank + 1) * PRG_BANK_LEN]
                .fill(bank as u8);
        }
        image
    }

    #[test]
    fn mapper15_loads_and_maps_adjacent_banks_in_mode_zero() {
        let mut cart = Cartridge::parse(&image(8)).unwrap();
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xC000), Some(1));
        assert!(cart.cpu_write(0x8000, 2));
        // The fixture fills each 16 KiB PRG bank with its bank index,
        // while the mapper exposes 8 KiB windows.
        assert_eq!(cart.cpu_read(0x8000), Some(2));
        assert_eq!(cart.cpu_read(0xA000), Some(2));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
        assert_eq!(cart.mirroring(), Mirroring::Vertical);
    }

    #[test]
    fn mapper15_selects_address_mode_mirroring_and_chr_ram() {
        let mut cart = Cartridge::parse(&image(8)).unwrap();
        assert!(cart.cpu_write(0x8003, 0x41));
        assert_eq!(cart.mirroring(), Mirroring::Horizontal);
        assert_eq!(cart.cpu_read(0x8000), Some(1));
        assert_eq!(cart.cpu_read(0xC000), Some(1));
        assert!(cart.ppu_write(0x1FFF, 0xA5));
        assert_eq!(cart.ppu_read(0x1FFF), Some(0xA5));
    }

    #[test]
    fn mapper15_rejects_chr_rom_and_odd_prg_sizes() {
        assert!(matches!(Cartridge::parse(&image(3)), Err(CartridgeError::InvalidPrgSize(_))));
        let mut chr_rom = image(2);
        chr_rom[5] = 1;
        chr_rom.extend(vec![0; CHR_BANK_LEN]);
        assert!(matches!(Cartridge::parse(&chr_rom), Err(CartridgeError::InvalidChrSize(_))));
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
    fn mapper_three_nes2_decodes_extended_header_and_chr_banks() {
        let mut image = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN + 4 * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 2;
        image[5] = 4;
        image[6] = 0x30;
        image[7] = 0x08; // NES 2.0.
        let chr_start = INES_HEADER_LEN + 2 * PRG_BANK_LEN;
        for bank in 0..4usize {
            image[chr_start + bank * CHR_BANK_LEN..chr_start + (bank + 1) * CHR_BANK_LEN]
                .fill(0x40 + bank as u8);
        }

        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.ppu_read(0x0000), Some(0x40));
        assert!(cart.cpu_write(0x8000, 3));
        assert_eq!(cart.ppu_read(0x0000), Some(0x43));
        assert_eq!(cart.ppu_read(0x1FFF), Some(0x43));
    }

    #[test]
    fn mapper_three_nes2_respects_absent_prg_ram_and_rejects_oversized_chr() {
        let mut image = vec![0; INES_HEADER_LEN + PRG_BANK_LEN + CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1;
        image[6] = 0x30;
        image[7] = 0x08;
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.cpu_read(0x6000), None);
        assert!(!cart.cpu_write(0x6000, 0xA5));

        image[5] = 5;
        image.extend(vec![0; 4 * CHR_BANK_LEN]);
        assert!(matches!(Cartridge::parse(&image), Err(CartridgeError::InvalidChrSize(_))));
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
    fn mapper_two_can_select_the_fixed_last_bank_in_the_lower_window() {
        let mut cartridge = Cartridge::parse(&uxrom_image(4)).unwrap();

        assert!(cartridge.cpu_write(0x8000, 3));
        assert_eq!(cartridge.cpu_read(0x8000), Some(3));
        assert_eq!(cartridge.cpu_read(0xBFFF), Some(3));
        assert_eq!(cartridge.cpu_read(0xC000), Some(3));

        // Register values beyond the installed bank count wrap safely.
        assert!(cartridge.cpu_write(0x8000, 4));
        assert_eq!(cartridge.cpu_read(0x8000), Some(0));
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
    #[test] fn accepts_nes2_mapper_zero() {
        assert!(Nrom::parse(&image(1, 1, 0, 8)).is_ok());
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

        // NES 2.0's format marker selects its mapper fields; mapper 0 is supported.
        image[6] = 0;
        image[7] = 0x08;
        assert!(Nrom::parse(&image).is_ok());
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
    fn surom_outer_bank_select_works_in_all_mmc1_prg_modes() {
        let mut cart = Cartridge::parse(&mmc1_image(32, 0)).unwrap();

        // CHR0 bit 4 selects the upper 256 KiB SUROM half; the PRG register
        // still selects the bank within that half.
        serial_write(&mut cart, 0xA000, 0x10);
        serial_write(&mut cart, 0xE000, 3);
        serial_write(&mut cart, 0x8000, 0x00); // 32 KiB mode: banks 18/19.
        assert_eq!(cart.cpu_read(0x8000), Some(18));
        assert_eq!(cart.cpu_read(0xC000), Some(19));

        serial_write(&mut cart, 0x8000, 0x04); // Mode 1 is also paired 32 KiB.
        assert_eq!(cart.cpu_read(0x8000), Some(18));
        assert_eq!(cart.cpu_read(0xC000), Some(19));

        serial_write(&mut cart, 0x8000, 0x08); // Mode 2: outer-half first bank is fixed.
        assert_eq!(cart.cpu_read(0x8000), Some(16));
        assert_eq!(cart.cpu_read(0xC000), Some(19));

        serial_write(&mut cart, 0x8000, 0x0C); // Mode 3: outer-half last bank is fixed.
        assert_eq!(cart.cpu_read(0x8000), Some(19));
        assert_eq!(cart.cpu_read(0xC000), Some(31));

        // Selecting the lower half must restore the original 16 KiB bank range.
        serial_write(&mut cart, 0xA000, 0x00);
        assert_eq!(cart.cpu_read(0x8000), Some(3));
        assert_eq!(cart.cpu_read(0xC000), Some(15));
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
        assert!(cartridge.cpu_write(0x8000, 0x03));
        assert_eq!(cartridge.ppu_read(0), Some(0x73));
        assert!(!cartridge.chr_is_ram());
        assert!(!cartridge.ppu_write(0, 0xAA));
        assert_eq!(cartridge.ppu_read(0), Some(0x73));
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
        assert!(matches!(Cartridge::parse(&gxrom_image(2, 5)), Err(CartridgeError::InvalidChrSize(_))));
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
    fn mapper_4_prg_ram_disable_and_write_protect_are_independent() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();

        cart.cpu_write(0x6000, 0x31);
        assert_eq!(cart.cpu_read(0x6000), Some(0x31));

        // Bit 7 disables reads and writes; a disabled read is unmapped.
        cart.cpu_write(0xA001, 0x00);
        assert_eq!(cart.cpu_read(0x6000), None);
        cart.cpu_write(0x6000, 0x42);
        cart.cpu_write(0xA001, 0x80);
        assert_eq!(cart.cpu_read(0x6000), Some(0x31));

        // Bit 6 protects writes while leaving reads enabled.
        cart.cpu_write(0xA001, 0xC0);
        assert_eq!(cart.cpu_read(0x6000), Some(0x31));
        cart.cpu_write(0x6000, 0x53);
        assert_eq!(cart.cpu_read(0x6000), Some(0x31));

        cart.cpu_write(0xA001, 0x80);
        cart.cpu_write(0x6000, 0x64);
        assert_eq!(cart.cpu_read(0x6000), Some(0x64));
    }


    #[test]
    fn mapper_4_irq_latch_zero_reasserts_until_disabled_and_acknowledged() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        cart.cpu_write(0xC000, 0); // Latch zero.
        cart.cpu_write(0xC001, 0); // Request reload on the next A12/scanline clock.
        cart.cpu_write(0xE001, 0); // Enable IRQ.
        assert!(!cart.irq_pending());
        cart.clock_irq_counter();
        assert!(cart.irq_pending(), "a zero latch asserts IRQ on the first counter clock");

        cart.cpu_write(0xE001, 0);
        assert!(cart.irq_pending(), "re-enabling does not acknowledge a pending IRQ");
        cart.cpu_write(0xE000, 0);
        assert!(!cart.irq_pending(), "the disable/acknowledge register clears the pending IRQ");
        cart.clock_irq_counter();
        assert!(!cart.irq_pending(), "a disabled counter cannot assert IRQ");
        cart.cpu_write(0xE001, 0);
        cart.clock_irq_counter();
        assert!(cart.irq_pending(), "zero latch can assert again after re-enable and a new clock");
    }

    

    #[test]
    fn mapper_4_a12_irq_requires_eight_low_dots_and_only_clocks_on_rising_edges() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        cart.cpu_write(0xC000, 0);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);

        cart.observe_ppu_address(0x0000, 0);
        cart.observe_ppu_address(0x1000, 7);
        assert!(!cart.irq_pending(), "seven low dots must be filtered");

        cart.observe_ppu_address(0x0000, 8);
        cart.observe_ppu_address(0x1000, 16);
        assert!(cart.irq_pending(), "eight low dots qualify an A12 rising edge");

        cart.cpu_write(0xE000, 0);
        cart.cpu_write(0xC000, 1);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);
        cart.observe_ppu_address(0x1000, 17);
        assert!(!cart.irq_pending(), "a continuously high A12 line has no second rising edge");
        cart.observe_ppu_address(0x0000, 18);
        cart.observe_ppu_address(0x1000, 25);
        assert!(!cart.irq_pending(), "a fresh seven-dot low interval remains filtered");
        cart.observe_ppu_address(0x0000, 26);
        cart.observe_ppu_address(0x1000, 34);
        assert!(!cart.irq_pending(), "first qualified edge reloads the nonzero latch");
        cart.observe_ppu_address(0x0000, 35);
        cart.observe_ppu_address(0x1000, 43);
        assert!(cart.irq_pending(), "second qualified edge clocks the counter to zero");
    }


    #[test]
    fn mapper_4_prg_mode_swaps_r6_and_fixed_second_last_bank() {
        let mut cart = Cartridge::parse(&mmc3_image(4, 1)).unwrap();
        cart.cpu_write(0x8000, 0x06);
        cart.cpu_write(0x8001, 1);
        assert_eq!(cart.cpu_read(0x8000), Some(1));
        assert_eq!(cart.cpu_read(0xC000), Some(6));

        cart.cpu_write(0x8000, 0x46); // PRG mode 1.
        assert_eq!(cart.cpu_read(0x8000), Some(6), "mode 1 fixes the first slot to second-last bank");
        assert_eq!(cart.cpu_read(0xC000), Some(1), "mode 1 moves R6 to the third slot");
        assert_eq!(cart.cpu_read(0xE000), Some(7), "last 8 KiB bank remains fixed in both modes");
    }


    #[test]
    fn mapper_4_a12_low_duration_is_measured_from_first_low_dot() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        cart.cpu_write(0xC000, 0);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);

        cart.observe_ppu_address(0x0000, 100);
        cart.observe_ppu_address(0x0000, 104);
        cart.observe_ppu_address(0x0000, 107);
        cart.observe_ppu_address(0x1000, 107);
        assert!(!cart.irq_pending(), "repeated low observations must not manufacture eight low dots");

        cart.observe_ppu_address(0x0000, 108);
        cart.observe_ppu_address(0x0000, 111);
        cart.observe_ppu_address(0x1000, 113);
        assert!(!cart.irq_pending(), "five elapsed low dots are below the MMC3 filter threshold");

        cart.observe_ppu_address(0x0000, 117);
        cart.observe_ppu_address(0x1000, 125);
        assert!(cart.irq_pending(), "exactly eight elapsed low dots qualify the next rising edge");
    }

    #[test]
    fn mapper_4_a12_timestamp_regression_cannot_underflow_low_duration() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        cart.cpu_write(0xC000, 0);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);

        cart.observe_ppu_address(0x0000, 50);
        cart.observe_ppu_address(0x1000, 40);
        assert!(!cart.irq_pending(), "a non-monotonic timestamp must not pass the low-time filter");
        cart.observe_ppu_address(0x0000, 60);
        cart.observe_ppu_address(0x1000, 68);
        assert!(cart.irq_pending(), "a subsequent valid eight-dot interval still clocks IRQ");
    }

    #[test]
    fn mapper_4_a12_edges_do_not_clock_when_irq_is_disabled() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        cart.cpu_write(0xC000, 0);
        cart.cpu_write(0xC001, 0);
        cart.observe_ppu_address(0x0000, 0);
        cart.observe_ppu_address(0x1000, 8);
        assert!(!cart.irq_pending());

        cart.cpu_write(0xE001, 0);
        cart.observe_ppu_address(0x0000, 9);
        cart.observe_ppu_address(0x1000, 17);
        assert!(cart.irq_pending(), "a subsequent qualified edge clocks the enabled counter");
    }

    #[test]
    fn mapper_4_prg_ram_enable_and_write_protect_bits_are_independent() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        cart.cpu_write(0x6000, 0x31);
        assert_eq!(cart.cpu_read(0x6000), Some(0x31));

        cart.cpu_write(0xA001, 0x80); // Enabled, writable.
        cart.cpu_write(0x6000, 0x52);
        assert_eq!(cart.cpu_read(0x6000), Some(0x52));

        cart.cpu_write(0xA001, 0xC0); // Enabled, write protected.
        cart.cpu_write(0x6000, 0x99);
        assert_eq!(cart.cpu_read(0x6000), Some(0x52), "write protection must not disable reads");

        cart.cpu_write(0xA001, 0x00); // Disabled.
        assert_eq!(cart.cpu_read(0x6000), None, "disabled PRG-RAM must not drive the bus");
    }

    #[test]
    fn mapper_4_irq_counter_reloads_nonzero_latch_then_counts_down() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        cart.cpu_write(0xC000, 2);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);

        cart.clock_irq_counter();
        assert!(!cart.irq_pending(), "reload to two must not assert IRQ");
        cart.clock_irq_counter();
        assert!(!cart.irq_pending(), "counter decrements from two to one");
        cart.clock_irq_counter();
        assert!(cart.irq_pending(), "counter reaching zero asserts IRQ when enabled");
    }

    #[test]
    fn mapper_4_chr_pair_register_ignores_low_bit_and_keeps_pair_contiguous() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 4)).unwrap();
        cart.cpu_write(0x8000, 0);
        cart.cpu_write(0x8001, 5);
        assert_eq!(cart.ppu_read(0x0000), Some(0x44), "R0 bit zero is ignored");
        assert_eq!(cart.ppu_read(0x0400), Some(0x45), "R0 maps an aligned 2 KiB pair");
    }

#[test]
    fn mapper_4_four_screen_mirroring_cannot_be_overridden_by_register_write() {
        let mut image = mmc3_image(2, 1);
        image[6] |= 0x08;
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.mirroring(), Mirroring::FourScreen);
        cart.cpu_write(0xA000, 0);
        assert_eq!(cart.mirroring(), Mirroring::FourScreen);
        cart.cpu_write(0xA000, 1);
        assert_eq!(cart.mirroring(), Mirroring::FourScreen);
    }

    #[test]
    fn mapper_4_irq_latch_reload_zero_and_acknowledge_behave_independently() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();

        // A zero latch requests an IRQ on the next counter clock after enabling.
        cart.cpu_write(0xC000, 0);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);
        cart.clock_irq_counter();
        assert!(cart.irq_pending());

        // Disabling IRQ clears the pending line; re-enabling does not recreate it
        // until a subsequent counter clock.
        cart.cpu_write(0xE000, 0);
        assert!(!cart.irq_pending());
        cart.cpu_write(0xE001, 0);
        assert!(!cart.irq_pending());
        cart.clock_irq_counter();
        assert!(cart.irq_pending());
    }

    #[test]
    fn mapper_4_prg_bank_select_keeps_fixed_windows_in_both_modes() {
        let mut cart = Cartridge::parse(&mmc3_image(4, 1)).unwrap();
        cart.cpu_write(0x8000, 6);
        cart.cpu_write(0x8001, 2);
        assert_eq!(cart.cpu_read(0x8000), Some(2));
        assert_eq!(cart.cpu_read(0xC000), Some(6));
        assert_eq!(cart.cpu_read(0xE000), Some(7));

        cart.cpu_write(0x8000, 0x46);
        assert_eq!(cart.cpu_read(0x8000), Some(6));
        assert_eq!(cart.cpu_read(0xC000), Some(2));
        assert_eq!(cart.cpu_read(0xE000), Some(7));

        // R6 is six bits wide and wraps safely when a board exposes fewer banks.
        cart.cpu_write(0x8000, 6);
        cart.cpu_write(0x8001, 0x3F);
        assert_eq!(cart.cpu_read(0x8000), Some(7));
    }

    #[test]
    fn mapper_4_chr_mode_switch_preserves_all_eight_kib_windows() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 4)).unwrap();
        cart.cpu_write(0x8000, 0);
        cart.cpu_write(0x8001, 4); // R0 maps an aligned pair (banks 4 and 5).
        assert_eq!(cart.ppu_read(0x0000), Some(0x44));
        assert_eq!(cart.ppu_read(0x0400), Some(0x45));

        cart.cpu_write(0x8000, 0x80); // CHR inversion puts R2-R5 in the first 4 KiB.
        cart.cpu_write(0x8000, 0x82); // Select R2 without clearing CHR inversion.
        cart.cpu_write(0x8001, 7);
        assert_eq!(cart.ppu_read(0x0000), Some(0x47));
        assert_eq!(cart.ppu_read(0x0400), Some(0x45));
        assert_eq!(cart.ppu_read(0x1000), Some(0x44));
    }

    #[test]
    fn mapper_4_irq_a12_filter_requires_eight_low_dots() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        cart.cpu_write(0xC000, 1);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);

        cart.observe_ppu_address(0x0000, 0);
        cart.observe_ppu_address(0x1000, 7);
        assert!(!cart.irq_pending(), "short A12-low pulse must be filtered");

        cart.observe_ppu_address(0x0000, 8);
        cart.observe_ppu_address(0x1000, 16);
        assert!(!cart.irq_pending(), "first qualified edge reloads latch value 1");
        cart.observe_ppu_address(0x0000, 17);
        cart.observe_ppu_address(0x1000, 25);
        assert!(cart.irq_pending(), "second qualified edge decrements counter to zero");
    }

    #[test]
    fn mapper_4_irq_disable_acknowledges_pending_and_reenable_waits_for_next_edge() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        cart.cpu_write(0xC000, 0);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);

        cart.observe_ppu_address(0x0000, 0);
        cart.observe_ppu_address(0x1000, 8);
        assert!(cart.irq_pending());

        cart.cpu_write(0xE000, 0);
        assert!(!cart.irq_pending(), "writing $E000 must acknowledge a pending IRQ");
        cart.cpu_write(0xE001, 0);
        assert!(!cart.irq_pending(), "enabling IRQ must not immediately restore the old pending state");

        cart.observe_ppu_address(0x0000, 9);
        cart.observe_ppu_address(0x1000, 17);
        assert!(cart.irq_pending(), "a new qualified A12 edge may assert IRQ again");
    }

    #[test]
    fn mapper_4_irq_filter_does_not_count_reset_high_as_a_low_interval() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        cart.cpu_write(0xC000, 0);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);

        cart.observe_ppu_address(0x0000, 100);
        cart.observe_ppu_address(0x1000, 0); // Counter reset while A12 is high.
        cart.observe_ppu_address(0x0000, 1);
        cart.observe_ppu_address(0x1000, 8); // Seven low dots, not eight.
        assert!(!cart.irq_pending(), "A12-high reset sample must not start a low interval");

        cart.observe_ppu_address(0x0000, 9);
        cart.observe_ppu_address(0x1000, 17); // Now eight full low dots.
        assert!(cart.irq_pending());
    }

    #[test]
    fn mapper_4_irq_a12_filter_rebases_after_ppu_cycle_counter_reset() {
        let mut cart = Cartridge::parse(&mmc3_image(2, 1)).unwrap();
        cart.cpu_write(0xC000, 0);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);
        cart.observe_ppu_address(0x0000, 100);
        cart.observe_ppu_address(0x1000, 109);
        assert!(cart.irq_pending());

        cart.cpu_write(0xE000, 0);
        cart.observe_ppu_address(0x0000, 0);
        cart.observe_ppu_address(0x1000, 1);
        assert!(!cart.irq_pending(), "backwards timestamp must not fabricate an A12 edge");
        cart.observe_ppu_address(0x0000, 2);
        cart.observe_ppu_address(0x1000, 10);
        assert!(!cart.irq_pending(), "IRQ remains disabled after reset");
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
    fn mapper_71_nes2_parses_extended_header_and_maps_banks() {
        let mut image = camerica_image(4);
        image[7] = 0x48; // NES 2.0 marker and mapper 71 high nibble.
        image[8] = 0;
        image[9] = 0;
        image[10] = 7; // 8 KiB volatile PRG-RAM.
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
        assert!(cart.cpu_write(0x6000, 0xA6));
        assert_eq!(cart.cpu_read(0x6000), Some(0xA6));
        assert!(cart.cpu_write(0x8000, 2));
        assert_eq!(cart.cpu_read(0x8000), Some(2));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
    }

    #[test]
    fn mapper_71_nes2_without_ram_does_not_expose_ram_window() {
        let mut image = camerica_image(4);
        image[7] = 0x48;
        image[8] = 0;
        image[9] = 0;
        image[10] = 0;
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.cpu_read(0x6000), None);
        assert!(!cart.cpu_write(0x6000, 0xA6));
    }

    #[test]
    fn mapper_71_nes2_trainer_is_loaded_when_ram_window_is_needed() {
        let mut image = camerica_image(4);
        image[6] |= 0x04; // Trainer present.
        image[7] = 0x48;
        image[8] = 0;
        image[9] = 0;
        image[10] = 0; // No RAM explicitly declared, trainer still needs backing storage.
        image.splice(16..16, vec![0x5A; TRAINER_LEN]);
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.cpu_read(0x7000), Some(0x5A));
        assert!(cart.cpu_write(0x7000, 0xA5));
        assert_eq!(cart.cpu_read(0x7000), Some(0xA5));
    }

    #[test]
    fn mapper_71_nes2_rejects_unsupported_submappers() {
        let mut image = camerica_image(4);
        image[7] = 0x48;
        image[8] = 0x10; // Submapper 1 is not implemented.
        image[9] = 0;
        assert!(matches!(Cartridge::parse(&image), Err(CartridgeError::Nes2NotSupported)));
    }

    #[test]
    fn mapper_71_nes2_rejects_ram_larger_than_the_cpu_window() {
        let mut image = camerica_image(4);
        image[7] = 0x48;
        image[8] = 0;
        image[9] = 0;
        image[10] = 8; // 16 KiB volatile RAM, beyond this board implementation.
        assert!(matches!(Cartridge::parse(&image), Err(CartridgeError::Nes2NotSupported)));
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
    fn mmc2_latches_trigger_on_every_address_in_the_eight_byte_ranges() {
        let mut c = Mmc2::parse(&mmc2_image(8, 4)).unwrap();
        c.cpu_write(0xB000, 2);
        c.cpu_write(0xC000, 3);
        c.cpu_write(0xD000, 4);
        c.cpu_write(0xE000, 5);

        // Reading the final byte in FE0's trigger range selects its FE bank.
        let _ = c.ppu_read(0x0FEF);
        assert_eq!(c.ppu_read(0x0000), Some(0x23));
        // The upper window's FD trigger range also includes its last byte.
        let _ = c.ppu_read(0x1FDF);
        assert_eq!(c.ppu_read(0x1000), Some(0x24));
        // And the lower FD trigger range includes non-leading addresses too.
        let _ = c.ppu_read(0x0FD9);
        assert_eq!(c.ppu_read(0x0000), Some(0x22));
        let _ = c.ppu_read(0x1FE9);
        assert_eq!(c.ppu_read(0x1000), Some(0x25));
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



#[cfg(test)]
mod unrom512_tests {
    use super::*;

    fn image(prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut data = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN + chr_banks as usize * CHR_BANK_LEN];
        data[0..4].copy_from_slice(b"NES\x1A");
        data[4] = prg_banks;
        data[5] = chr_banks;
        data[6] = 0xE8;
        data[7] = 0x10;
        for bank in 0..prg_banks as usize {
            let start = INES_HEADER_LEN + bank * PRG_BANK_LEN;
            data[start..start + PRG_BANK_LEN].fill(bank as u8);
        }
        data
    }

    #[test]
    fn mapper_30_switches_lower_prg_and_keeps_last_bank_fixed() {
        let mut c = Cartridge::parse(&image(16, 0)).unwrap();
        assert_eq!(c.cpu_read(0x8000), Some(0));
        assert_eq!(c.cpu_read(0xC000), Some(15));
        c.cpu_write(0x8000, 7);
        assert_eq!(c.cpu_read(0x8000), Some(7));
        assert_eq!(c.cpu_read(0xBFFF), Some(7));
        assert_eq!(c.cpu_read(0xC000), Some(15));
        assert_eq!(c.cpu_read(0xFFFF), Some(15));

        let mut full = Cartridge::parse(&image(32, 0)).unwrap();
        full.cpu_write(0x8000, 0x1F);
        assert_eq!(full.cpu_read(0x8000), Some(31), "bit 4 selects the upper PRG bank range");
        assert_eq!(full.cpu_read(0xC000), Some(31), "the final bank remains hardwired");
    }

    #[test]
    fn mapper_30_switches_chr_ram_banks_and_one_screen_page() {
        let mut c = Cartridge::parse(&image(16, 0)).unwrap();
        assert_eq!(c.mirroring(), Mirroring::OneScreenLower);
        c.ppu_write(0x0123, 0x11);
        c.cpu_write(0x8000, 0x20);
        assert_eq!(c.mirroring(), Mirroring::OneScreenLower);
        assert_eq!(c.ppu_read(0x0123), Some(0));
        c.ppu_write(0x0123, 0x22);
        c.cpu_write(0x8000, 0x80);
        assert_eq!(c.mirroring(), Mirroring::OneScreenUpper);
        assert_eq!(c.ppu_read(0x0123), Some(0x11));
        c.cpu_write(0x8000, 0xC0);
        assert_eq!(c.ppu_read(0x0123), Some(0));
        c.ppu_write(0x0123, 0x33);
        c.cpu_write(0x8000, 0x20);
        assert_eq!(c.ppu_read(0x0123), Some(0x22));
    }

    #[test]
    fn mapper_30_skips_trainer_when_mapping_prg_and_has_no_prg_ram() {
        let mut data = image(16, 0);
        data[6] |= 0x04;
        data.splice(16..16, vec![0xA5; TRAINER_LEN]);
        let mut c = Cartridge::parse(&data).unwrap();
        assert_eq!(c.cpu_read(0x8000), Some(0));
        assert_eq!(c.cpu_read(0x7000), None);
        assert!(!c.cpu_write(0x6000, 0x5A));
    }

    #[test]
    fn mapper_30_rejects_chr_rom_and_unsupported_prg_sizes() {
        assert!(matches!(Cartridge::parse(&image(7, 0)), Err(CartridgeError::InvalidPrgSize(_))));
        assert!(matches!(Cartridge::parse(&image(8, 0)), Err(CartridgeError::InvalidPrgSize(_))));
        assert!(matches!(Cartridge::parse(&image(33, 0)), Err(CartridgeError::InvalidPrgSize(_))));
        assert!(matches!(Cartridge::parse(&image(16, 1)), Err(CartridgeError::InvalidChrSize(_))));
    }
}



#[cfg(test)]
mod nes2_nrom_tests {
    use super::*;

    fn nes2_nrom(prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN + chr_banks as usize * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[5] = chr_banks;
        image[7] = 0x08; // NES 2.0, mapper 0.
        image
    }

    #[test]
    fn nes2_nrom_with_chr_ram_loads_and_mirrors_16k_prg() {
        let mut image = nes2_nrom(1, 0);
        image[11] = 7; // 8 KiB of volatile CHR-RAM (64 << 7).
        image[INES_HEADER_LEN..INES_HEADER_LEN + PRG_BANK_LEN].fill(0x5A);
        let cart = Cartridge::parse(&image).unwrap();
        assert!(matches!(cart, Cartridge::Nrom(_)));
        assert_eq!(cart.cpu_read(0x8000), Some(0x5A));
        assert_eq!(cart.cpu_read(0xC000), Some(0x5A));
        assert!(cart.chr_is_ram());
    }

    #[test]
    fn nes2_nrom_with_32k_prg_and_chr_rom_loads() {
        let mut image = nes2_nrom(2, 1);
        let chr_start = INES_HEADER_LEN + 2 * PRG_BANK_LEN;
        image[chr_start..chr_start + CHR_BANK_LEN].fill(0xA6);
        let cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xC000), Some(0));
        assert_eq!(cart.ppu_read(0x123), Some(0xA6));
        assert!(!cart.chr_is_ram());
    }

    #[test]
    fn nes2_nrom_respects_declared_absence_of_chr_ram() {
        let image = nes2_nrom(1, 0);
        let mut cart = Cartridge::parse(&image).unwrap();
        assert!(!cart.chr_is_ram());
        assert_eq!(cart.ppu_read(0x0000), None);
        assert!(!cart.ppu_write(0x0000, 0xA5));
    }

    #[test]
    fn nes2_nrom_maps_declared_small_chr_ram_by_mirroring() {
        let mut image = nes2_nrom(1, 0);
        image[11] = 1; // 128 bytes of volatile CHR-RAM.
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.ppu_read(0x0000), Some(0));
        assert!(cart.ppu_write(0x0080, 0xA5));
        assert_eq!(cart.ppu_read(0x0000), Some(0xA5));
        assert_eq!(cart.ppu_read(0x0080), Some(0xA5));
    }

    #[test]
    fn nes2_uxrom_supports_banked_prg_and_declared_ram() {
        let mut image = nes2_nrom(3, 0);
        image[6] = 0x20; // Mapper 2.
        image[7] = 0x08; // NES 2.0 marker; mapper 2 is encoded in byte 6.
        image[10] = 7; // 8 KiB PRG-RAM.
        image[11] = 7; // 8 KiB CHR-RAM.
        for bank in 0..3usize {
            let start = INES_HEADER_LEN + bank * PRG_BANK_LEN;
            image[start..start + PRG_BANK_LEN].fill((bank + 1) as u8);
        }
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.cpu_read(0x8000), Some(1));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
        assert!(cart.cpu_write(0x8000, 1));
        assert_eq!(cart.cpu_read(0x8000), Some(2));
        assert!(cart.cpu_write(0x6000, 0xA5));
        assert_eq!(cart.cpu_read(0x6000), Some(0xA5));
        assert!(cart.ppu_write(0x0000, 0x5A));
        assert_eq!(cart.ppu_read(0x0000), Some(0x5A));
    }

    #[test]
    fn nes2_uxrom_with_no_declared_ram_exposes_no_memory_window() {
        let mut image = nes2_nrom(2, 0);
        image[6] = 0x20; // Mapper 2.
        image[7] = 0x08;
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.cpu_read(0x6000), None);
        assert!(!cart.cpu_write(0x6000, 0xA5));
        assert_eq!(cart.ppu_read(0x0000), None);
        assert!(!cart.ppu_write(0x0000, 0x5A));
    }

    #[test]
    fn nes2_nrom_respects_declared_absence_of_prg_ram() {
        let image = nes2_nrom(1, 0);
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.cpu_read(0x6000), None);
        assert!(!cart.cpu_write(0x6000, 0xA5));
    }

    #[test]
    fn nes2_nrom_maps_declared_small_prg_ram_by_mirroring() {
        let mut image = nes2_nrom(1, 0);
        image[10] = 1; // 128 bytes of volatile PRG-RAM (64 << 1).
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.cpu_read(0x6000), Some(0));
        assert!(cart.cpu_write(0x6080, 0xA5));
        assert_eq!(cart.cpu_read(0x6000), Some(0xA5));
        assert_eq!(cart.cpu_read(0x6080), Some(0xA5));
    }

    #[test]
    fn nes2_nrom_rejects_extended_size_that_exceeds_nrom_board_limits() {
        let mut image = nes2_nrom(1, 0);
        image[9] = 0x01; // 257 PRG banks in linear NES 2.0 encoding.
        assert!(matches!(Cartridge::parse(&image), Err(CartridgeError::InvalidPrgSize(_))));
    }

    #[test]
    fn nes2_non_nrom_mappers_remain_explicitly_unsupported() {
        let mut image = nes2_nrom(1, 0);
        image[6] = 0x10;
        assert_eq!(Cartridge::parse(&image), Err(CartridgeError::Nes2NotSupported));
    }
}


#[cfg(test)]
mod mmc3_chr_layout_regression_tests {
    use super::*;

    fn image() -> Vec<u8> {
        let mut data = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN + 2 * CHR_BANK_LEN];
        data[0..4].copy_from_slice(b"NES\x1A");
        data[4] = 2;
        data[5] = 2;
        data[6] = 0x40;
        let chr = INES_HEADER_LEN + 2 * PRG_BANK_LEN;
        for bank in 0..16usize {
            data[chr + bank * 0x400..chr + (bank + 1) * 0x400].fill(bank as u8);
        }
        data
    }

    #[test]
    fn mmc3_chr_mode_bit_reorders_all_eight_one_kib_windows() {
        let mut cart = Cartridge::parse(&image()).unwrap();
        for (register, bank) in [(0, 2), (1, 4), (2, 6), (3, 7), (4, 1), (5, 3), (6, 5), (7, 7)] {
            cart.cpu_write(0x8000, register);
            cart.cpu_write(0x8001, bank);
        }
        let mode_zero = [2, 3, 4, 5, 6, 7, 1, 3];
        for (slot, expected) in mode_zero.into_iter().enumerate() {
            assert_eq!(cart.ppu_read((slot * 0x400) as u16), Some(expected));
        }

        cart.cpu_write(0x8000, 0x80);
        let mode_one = [6, 7, 1, 3, 2, 3, 4, 5];
        for (slot, expected) in mode_one.into_iter().enumerate() {
            assert_eq!(cart.ppu_read((slot * 0x400) as u16), Some(expected));
        }
    }

    #[test]
    fn mmc3_irq_latch_reload_and_disable_acknowledge_are_independent() {
        let mut cart = Cartridge::parse(&image()).unwrap();
        cart.cpu_write(0xC000, 1);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);
        cart.observe_ppu_address(0x0000, 10);
        cart.observe_ppu_address(0x1000, 18);
        assert!(!cart.irq_pending());
        cart.observe_ppu_address(0x0000, 19);
        cart.observe_ppu_address(0x1000, 27);
        assert!(cart.irq_pending());
        cart.cpu_write(0xE000, 0);
        assert!(!cart.irq_pending());
        cart.cpu_write(0xE001, 0);
        cart.observe_ppu_address(0x0000, 28);
        cart.observe_ppu_address(0x1000, 36); // Reloads latch to 1.
        assert!(!cart.irq_pending());
        cart.observe_ppu_address(0x0000, 37);
        cart.observe_ppu_address(0x1000, 45); // Decrements to zero again.
        assert!(cart.irq_pending());
    }

    #[test]
    fn mmc3_chr_ram_writes_follow_the_current_bank_mapping() {
        let mut data = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN];
        data[0..4].copy_from_slice(b"NES\x1A");
        data[4] = 2;
        data[6] = 0x40;
        let mut cart = Cartridge::parse(&data).unwrap();
        cart.cpu_write(0x8000, 0); // R0 selects a paired 2 KiB window.
        cart.cpu_write(0x8001, 2);
        assert!(cart.ppu_write(0x0000, 0xA1));
        assert_eq!(cart.ppu_read(0x0000), Some(0xA1));
        cart.cpu_write(0x8001, 4);
        assert_eq!(cart.ppu_read(0x0000), Some(0));
        assert!(cart.ppu_write(0x0000, 0xB2));
        cart.cpu_write(0x8001, 2);
        assert_eq!(cart.ppu_read(0x0000), Some(0xA1));
        cart.cpu_write(0x8001, 4);
        assert_eq!(cart.ppu_read(0x0000), Some(0xB2));
    }
}


#[cfg(test)]
mod mmc1_serial_reset_regression_tests {
    use super::*;

    #[test]
    fn serial_reset_discards_partial_write_and_restores_prg_mode_bits() {
        let mut image = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 2;
        image[6] = 0x10; // MMC1.
        let mut cart = Cartridge::parse(&image).unwrap();

        for bit in [0, 1, 0, 1] {
            cart.cpu_write(0x8000, bit);
        }
        cart.cpu_write(0x8000, 0x80); // Abort the incomplete serial value.
        for bit in [0, 1, 0, 0, 0] {
            cart.cpu_write(0x8000, bit);
        } // Control = $02.

        assert_eq!(cart.mirroring(), Mirroring::Vertical);
    }
}


#[cfg(test)]
mod mapper_bounds_regression_tests {
    use super::*;

    #[test]
    fn cnrom_bank_selection_wraps_to_available_chr_rom_banks() {
        let mut image = vec![0; INES_HEADER_LEN + PRG_BANK_LEN + 2 * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 2;
        image[6] = 0x30; // Mapper 3 (CNROM).
        let chr_start = INES_HEADER_LEN + PRG_BANK_LEN;
        image[chr_start..chr_start + CHR_BANK_LEN].fill(0x11);
        image[chr_start + CHR_BANK_LEN..].fill(0x22);
        let mut cart = Cnrom::parse(&image).unwrap();

        assert_eq!(cart.ppu_read(0), Some(0x11));
        assert!(cart.cpu_write(0x8000, 1));
        assert_eq!(cart.ppu_read(0), Some(0x22));
        assert!(cart.cpu_write(0x8000, 2));
        assert_eq!(cart.ppu_read(0), Some(0x11), "bank values wrap to the installed CHR count");
        assert!(!cart.ppu_write(0, 0xFF), "CNROM CHR-ROM must remain read-only");
    }
}


#[cfg(test)]
mod mmc1_prg_ram_and_mode_regression_tests {
    use super::*;

    fn image() -> Vec<u8> {
        let mut data = vec![0; INES_HEADER_LEN + 4 * PRG_BANK_LEN];
        data[0..4].copy_from_slice(b"NES\x1A");
        data[4] = 4;
        data[6] = 0x10; // MMC1, horizontal initial mirroring.
        for bank in 0..4usize {
            let start = INES_HEADER_LEN + bank * PRG_BANK_LEN;
            data[start..start + PRG_BANK_LEN].fill(bank as u8);
        }
        data
    }

    fn serial_write(cart: &mut Mmc1, address: u16, value: u8) {
        for bit in 0..5 {
            assert!(cart.cpu_write(address, (value >> bit) & 1));
        }
    }

    #[test]
    fn prg_modes_two_and_three_switch_the_expected_16k_windows() {
        let mut cart = Mmc1::parse(&image()).unwrap();

        serial_write(&mut cart, 0x8000, 0x08); // Mode 2: first bank fixed at $8000.
        serial_write(&mut cart, 0xE000, 0x02);
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xC000), Some(2));

        serial_write(&mut cart, 0x8000, 0x0C); // Mode 3: last bank fixed at $C000.
        serial_write(&mut cart, 0xE000, 0x01);
        assert_eq!(cart.cpu_read(0x8000), Some(1));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
    }

    #[test]
    fn prg_register_bit_four_disables_ram_without_destroying_saved_contents() {
        let mut cart = Mmc1::parse(&image()).unwrap();
        assert!(cart.cpu_write(0x6000, 0xA7));
        assert_eq!(cart.cpu_read(0x6000), Some(0xA7));

        serial_write(&mut cart, 0xE000, 0x10);
        assert_eq!(cart.cpu_read(0x6000), None, "disabled PRG-RAM should leave the bus undriven");
        assert!(!cart.cpu_write(0x6000, 0x11));

        serial_write(&mut cart, 0xE000, 0x00);
        assert_eq!(cart.cpu_read(0x6000), Some(0xA7), "re-enabling RAM must preserve its contents");
    }
}


#[cfg(test)]
mod mmc1_prg_ram_protection_regression_tests {
    use super::*;

    #[test]
    fn prg_bank_bit_four_disables_prg_ram_without_destroying_contents() {
        let mut image = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 2;
        image[6] = 0x10; // Mapper 1.
        let mut cartridge = Mmc1::parse(&image).unwrap();
        assert!(cartridge.cpu_write(0x6000, 0xA5));
        assert_eq!(cartridge.cpu_read(0x6000), Some(0xA5));

        // MMC1 PRG register bit 4 disables PRG-RAM.
        for bit in [1, 0, 0, 0, 1] {
            assert!(cartridge.cpu_write(0xE000, bit));
        }
        assert_eq!(cartridge.cpu_read(0x6000), None);
        assert!(!cartridge.cpu_write(0x6000, 0x11));

        // Write a zero PRG-bank value to re-enable RAM; the previous byte survives.
        for bit in [0, 0, 0, 0, 0] {
            assert!(cartridge.cpu_write(0xE000, bit));
        }
        assert_eq!(cartridge.cpu_read(0x6000), Some(0xA5));
    }
}


#[cfg(test)]
mod image_boundary_regressions {
    use super::*;

    #[test]
    fn parser_reports_truncated_trainer_before_prg_data() {
        let mut image = vec![0; INES_HEADER_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[6] = 0x04; // Trainer is declared, but no trainer bytes follow.
        assert_eq!(Cartridge::parse(&image), Err(CartridgeError::TruncatedImage {
            expected: INES_HEADER_LEN + TRAINER_LEN + PRG_BANK_LEN,
            actual: INES_HEADER_LEN,
        }));
    }

    #[test]
    fn parser_rejects_an_invalid_magic_without_panicking_on_short_inputs() {
        for length in 0..INES_HEADER_LEN {
            assert_eq!(Cartridge::parse(&vec![0; length]), Err(CartridgeError::TruncatedHeader));
        }
        let mut image = vec![0; INES_HEADER_LEN];
        image[0..4].copy_from_slice(b"NOPE");
        assert_eq!(Cartridge::parse(&image), Err(CartridgeError::InvalidMagic));
    }
}


#[cfg(test)]
mod mmc4_tests {
    use super::*;

    pub(crate) fn mmc4_image(prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut image = vec![
            0;
            INES_HEADER_LEN
                + prg_banks as usize * PRG_BANK_LEN
                + chr_banks as usize * CHR_BANK_LEN
        ];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[5] = chr_banks;
        image[6] = 0xA0; // Mapper 10.
        for bank in 0..prg_banks as usize {
            let start = INES_HEADER_LEN + bank * PRG_BANK_LEN;
            image[start..start + PRG_BANK_LEN].fill(bank as u8);
        }
        let chr_start = INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN;
        for bank in 0..chr_banks as usize * 2 {
            image[chr_start + bank * 0x1000..chr_start + (bank + 1) * 0x1000]
                .fill((0x20 + bank) as u8);
        }
        image
    }

    #[test]
    fn mapper10_switches_16k_prg_and_keeps_last_bank_fixed() {
        let mut cart = Cartridge::parse(&mmc4_image(8, 2)).unwrap();
        assert!(matches!(&cart, Cartridge::Mmc2(_)));
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xBFFF), Some(0));
        assert_eq!(cart.cpu_read(0xC000), Some(7));
        assert_eq!(cart.cpu_read(0xFFFF), Some(7));
        cart.cpu_write(0xA000, 3);
        assert_eq!(cart.cpu_read(0x8000), Some(3));
        assert_eq!(cart.cpu_read(0xBFFF), Some(3));
        assert_eq!(cart.cpu_read(0xC000), Some(7));
    }

    #[test]
    fn mapper10_uses_chr_latches_and_mirroring_register() {
        let mut cart = Cartridge::parse(&mmc4_image(2, 4)).unwrap();
        cart.cpu_write(0xB000, 2);
        cart.cpu_write(0xC000, 3);
        cart.cpu_write(0xD000, 4);
        cart.cpu_write(0xE000, 5);
        assert_eq!(cart.ppu_read(0x0000), Some(0x22));
        let _ = cart.ppu_read(0x0FE8);
        assert_eq!(cart.ppu_read(0x0000), Some(0x23));
        cart.cpu_write(0xF000, 0);
        assert_eq!(cart.mirroring(), Mirroring::Vertical);
        cart.cpu_write(0xF000, 1);
        assert_eq!(cart.mirroring(), Mirroring::Horizontal);
    }

    #[test]
    fn mapper10_rejects_invalid_prg_and_missing_chr_rom() {
        assert!(matches!(
            Cartridge::parse(&mmc4_image(1, 1)),
            Err(CartridgeError::InvalidPrgSize(_))
        ));
        assert!(matches!(
            Cartridge::parse(&mmc4_image(2, 0)),
            Err(CartridgeError::InvalidChrSize(_))
        ));
    }
}

#[cfg(test)]
mod mmc4_extended_tests {
    use super::*;
    use super::mmc4_tests::mmc4_image;

    #[test]
    fn mapper10_switches_the_upper_chr_latch_on_the_full_trigger_range() {
        let mut cart = Cartridge::parse(&mmc4_image(2, 4)).unwrap();
        cart.cpu_write(0xD000, 4);
        cart.cpu_write(0xE000, 5);
        assert_eq!(cart.ppu_read(0x1000), Some(0x24));

        // The latch changes after the triggering read, including the last byte.
        let _ = cart.ppu_read(0x1FEF);
        assert_eq!(cart.ppu_read(0x1000), Some(0x25));
        let _ = cart.ppu_read(0x1FD8);
        assert_eq!(cart.ppu_read(0x1000), Some(0x24));
    }

    #[test]
    fn mapper10_chr_rom_is_read_only_and_four_screen_mirroring_is_locked() {
        let mut image = mmc4_image(2, 2);
        image[6] |= 0x08; // Four-screen layout.
        let mut cart = Cartridge::parse(&image).unwrap();

        assert_eq!(cart.mirroring(), Mirroring::FourScreen);
        cart.cpu_write(0xF000, 0);
        assert_eq!(cart.mirroring(), Mirroring::FourScreen);
        assert!(!cart.ppu_write(0x0000, 0xFF));
        assert_eq!(cart.ppu_read(0x0000), Some(0x20));
    }

    #[test]
    fn mapper10_prg_bank_register_wraps_to_available_16k_banks() {
        let mut cart = Cartridge::parse(&mmc4_image(4, 2)).unwrap();
        cart.cpu_write(0xA000, 0xFF);
        assert_eq!(cart.cpu_read(0x8000), Some(3));
        assert_eq!(cart.cpu_read(0xBFFF), Some(3));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
    }
}


#[cfg(test)]
mod mapper_register_boundary_regressions {
    use super::*;

    fn mmc3_ram_image() -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 2;
        image[6] = 0x40; // Mapper 4, CHR-RAM.
        image
    }

    #[test]
    fn mmc3_prg_ram_enable_and_write_protect_bits_are_independent() {
        let mut cart = Cartridge::parse(&mmc3_ram_image()).unwrap();

        assert!(cart.cpu_write(0x6000, 0x5A));
        assert_eq!(cart.cpu_read(0x6000), Some(0x5A));

        // $A001 bit 7 disables PRG-RAM; bit 6 controls write protection.
        assert!(cart.cpu_write(0xA001, 0x00));
        assert_eq!(cart.cpu_read(0x6000), None);
        assert!(cart.cpu_write(0x6000, 0xA5)); // Disabled writes are consumed but ignored.
        assert_eq!(cart.cpu_read(0x6000), None);

        assert!(cart.cpu_write(0xA001, 0xC0)); // Enabled and protected.
        assert_eq!(cart.cpu_read(0x6000), Some(0x5A));
        assert!(cart.cpu_write(0x6000, 0xA5));
        assert_eq!(cart.cpu_read(0x6000), Some(0x5A), "write protection must preserve RAM");

        assert!(cart.cpu_write(0xA001, 0x80)); // Enabled and writable again.
        assert!(cart.cpu_write(0x6000, 0xA5));
        assert_eq!(cart.cpu_read(0x6000), Some(0xA5));
    }

    #[test]
    fn gxrom_prg_and_chr_register_fields_select_their_full_ranges_independently() {
        let prg_banks = 8usize;
        let chr_banks = 4usize;
        let mut image = vec![0; INES_HEADER_LEN + prg_banks * PRG_BANK_LEN + chr_banks * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks as u8;
        image[5] = chr_banks as u8;
        // Mapper 66: mapper low nibble 2 in flags 6, high nibble 4 in flags 7.
        image[6] = 0x20;
        image[7] = 0x40;
        for bank in 0..(prg_banks / 2) {
            let start = INES_HEADER_LEN + bank * 2 * PRG_BANK_LEN;
            image[start..start + 2 * PRG_BANK_LEN].fill(0x40 + bank as u8);
        }
        let chr_start = INES_HEADER_LEN + prg_banks * PRG_BANK_LEN;
        for bank in 0..chr_banks {
            let start = chr_start + bank * CHR_BANK_LEN;
            image[start..start + CHR_BANK_LEN].fill(0x70 + bank as u8);
        }
        let mut cart = Cartridge::parse(&image).unwrap();

        assert_eq!(cart.cpu_read(0x8000), Some(0x40));
        assert_eq!(cart.ppu_read(0x0000), Some(0x70));

        assert!(cart.cpu_write(0x8000, 0x30)); // PRG bank 3, CHR bank 0.
        assert_eq!(cart.cpu_read(0x8000), Some(0x43));
        assert_eq!(cart.ppu_read(0x0000), Some(0x70));

        assert!(cart.cpu_write(0x8000, 0x23)); // PRG bank 2, CHR bank 3.
        assert_eq!(cart.cpu_read(0x8000), Some(0x42));
        assert_eq!(cart.ppu_read(0x0000), Some(0x73));
        assert_eq!(cart.ppu_read(0x1FFF), Some(0x73));
    }
}


#[cfg(test)]
mod mapper15_bank_mode_regressions {
    use super::*;

    fn tagged_image() -> Vec<u8> {
        let prg_banks = 8usize;
        let mut image = vec![0; INES_HEADER_LEN + prg_banks * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks as u8;
        image[6] = 0xF0; // Mapper 15.
        let start = INES_HEADER_LEN;
        for bank in 0..prg_banks * 2 {
            image[start + bank * 0x2000..start + (bank + 1) * 0x2000]
                .fill(bank as u8);
        }
        image
    }

    fn assert_slots(cart: &Cartridge, expected: [u8; 4]) {
        for (slot, bank) in expected.into_iter().enumerate() {
            let address = 0x8000 + (slot as u16) * 0x2000;
            assert_eq!(cart.cpu_read(address), Some(bank), "unexpected PRG bank in slot {slot}");
        }
    }

    #[test]
    fn mapper15_all_four_modes_map_the_expected_8k_banks() {
        let mut cart = Cartridge::parse(&tagged_image()).unwrap();

        // X=2 selects 16-KiB bank 2 (8-KiB banks 4/5); bit 7 selects
        // the odd 8-KiB half in NROM-64 mode.
        cart.cpu_write(0x8000, 0x42);
        assert_slots(&cart, [4, 5, 6, 7]); // NROM-256, consecutive 32 KiB.

        cart.cpu_write(0x8001, 0x42);
        assert_slots(&cart, [4, 5, 14, 15]); // UNROM, fixed final 16 KiB.

        cart.cpu_write(0x8002, 0xC2);
        assert_slots(&cart, [5, 5, 5, 5]); // NROM-64, selected 8-KiB bank mirrored.

        cart.cpu_write(0x8003, 0x42);
        assert_slots(&cart, [4, 5, 4, 5]); // NROM-128, selected 16 KiB mirrored.
    }

    #[test]
    fn mapper15_bank_register_wraps_and_mirroring_tracks_bit_six() {
        let mut cart = Cartridge::parse(&tagged_image()).unwrap();
        cart.cpu_write(0x8000, 0xFF);
        assert_eq!(cart.cpu_read(0x8000), Some(14), "bank indices wrap to installed ROM");
        assert_eq!(cart.mirroring(), Mirroring::Horizontal);
        cart.cpu_write(0x8000, 0x00);
        assert_eq!(cart.mirroring(), Mirroring::Vertical);
    }

    #[test]
    fn mapper15_chr_ram_is_writable_and_rejects_out_of_range_addresses() {
        let mut cart = Cartridge::parse(&tagged_image()).unwrap();
        assert!(cart.chr_is_ram());
        assert!(cart.ppu_write(0x1FFF, 0xA5));
        assert_eq!(cart.ppu_read(0x1FFF), Some(0xA5));
        assert!(!cart.ppu_write(0x2000, 0xFF));
        assert_eq!(cart.ppu_read(0x2000), None);
    }
}


#[cfg(test)]
mod mmc1_board_boundary_regressions {
    use super::*;

    fn image(prg_banks: u8, four_screen: bool) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[6] = 0x10 | if four_screen { 0x08 } else { 0 };
        for bank in 0..prg_banks as usize {
            let start = INES_HEADER_LEN + bank * PRG_BANK_LEN;
            image[start..start + PRG_BANK_LEN].fill(bank as u8);
        }
        image
    }

    fn write_serial(cart: &mut Cartridge, address: u16, value: u8) {
        for bit in 0..5 {
            assert!(cart.cpu_write(address, (value >> bit) & 1));
        }
    }

    #[test]
    fn mmc1_four_screen_wiring_overrides_serial_mirroring_changes() {
        let mut cart = Cartridge::parse(&image(4, true)).unwrap();
        assert_eq!(cart.mirroring(), Mirroring::FourScreen);
        write_serial(&mut cart, 0x8000, 0x0C);
        assert_eq!(cart.mirroring(), Mirroring::FourScreen);
        write_serial(&mut cart, 0x8000, 0x0E);
        assert_eq!(cart.mirroring(), Mirroring::FourScreen);
    }

    #[test]
    fn mmc1_prg_ram_enable_bit_does_not_corrupt_prg_bank_selection() {
        let mut cart = Cartridge::parse(&image(4, false)).unwrap();
        assert!(cart.cpu_write(0x6000, 0x5A));
        write_serial(&mut cart, 0xE000, 0x10);
        assert_eq!(cart.cpu_read(0x6000), None);
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xC000), Some(3));
        write_serial(&mut cart, 0xE000, 0x02);
        assert_eq!(cart.cpu_read(0x6000), Some(0x5A));
        assert_eq!(cart.cpu_read(0x8000), Some(2));
    }

    #[test]
    fn mmc1_chr_bank_selection_wraps_to_installed_four_kib_banks() {
        let mut image = image(2, false);
        image[5] = 2; // Two 8-KiB CHR banks = four 4-KiB banks.
        image.resize(INES_HEADER_LEN + 2 * PRG_BANK_LEN + 2 * CHR_BANK_LEN, 0);
        let chr_start = INES_HEADER_LEN + 2 * PRG_BANK_LEN;
        for bank in 0..4 {
            image[chr_start + bank * 0x1000..chr_start + (bank + 1) * 0x1000]
                .fill(0x40 + bank as u8);
        }
        let mut cart = Cartridge::parse(&image).unwrap();
        write_serial(&mut cart, 0x8000, 0x1C); // 4-KiB CHR mode.
        write_serial(&mut cart, 0xA000, 0x1F);
        write_serial(&mut cart, 0xC000, 0xFE);
        assert_eq!(cart.ppu_read(0x0000), Some(0x43));
        assert_eq!(cart.ppu_read(0x1000), Some(0x42));
    }
}


#[cfg(test)]
mod unrom512_regressions {
    use super::*;

    fn image(prg_banks: u8, board_flags: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks;
        image[6] = 0xE0 | board_flags; // Mapper 30 low nibble and board wiring flags.
        image[7] = 0x10; // Mapper 30 high nibble in legacy iNES.
        for bank in 0..prg_banks as usize {
            let start = INES_HEADER_LEN + bank * PRG_BANK_LEN;
            image[start..start + PRG_BANK_LEN].fill(bank as u8);
        }
        image
    }

    #[test]
    fn prg_bank_selection_wraps_to_installed_banks_and_keeps_last_bank_fixed() {
        let mut cart = Cartridge::parse(&image(16, 0)).unwrap();
        assert!(cart.cpu_write(0x8000, 0x1F));
        assert_eq!(cart.cpu_read(0x8000), Some(15));
        assert_eq!(cart.cpu_read(0xBFFF), Some(15));
        assert_eq!(cart.cpu_read(0xC000), Some(15));
        assert_eq!(cart.cpu_read(0xFFFF), Some(15));

        assert!(cart.cpu_write(0xFFFF, 0x02));
        assert_eq!(cart.cpu_read(0x8000), Some(2));
        assert_eq!(cart.cpu_read(0xC000), Some(15));
        assert!(!cart.cpu_write(0x7FFF, 0x01));
    }

    #[test]
    fn chr_ram_banks_are_independent_and_address_bounds_are_enforced() {
        let mut cart = Cartridge::parse(&image(16, 0)).unwrap();
        for (bank, value) in [(0u8, 0x11), (1, 0x22), (2, 0x33), (3, 0x44)] {
            assert!(cart.cpu_write(0x8000, bank << 5));
            assert!(cart.ppu_write(0x0000, value));
            assert!(cart.ppu_write(0x1FFF, value.wrapping_add(1)));
        }
        for (bank, value) in [(0u8, 0x11), (1, 0x22), (2, 0x33), (3, 0x44)] {
            assert!(cart.cpu_write(0x8000, bank << 5));
            assert_eq!(cart.ppu_read(0x0000), Some(value));
            assert_eq!(cart.ppu_read(0x1FFF), Some(value + 1));
        }
        assert!(!cart.ppu_write(0x2000, 0xFF));
        assert_eq!(cart.ppu_read(0x2000), None);
    }

    #[test]
    fn one_screen_mirroring_switches_only_on_switchable_boards() {
        let mut cart = Cartridge::parse(&image(16, 0x08)).unwrap();
        assert_eq!(cart.mirroring(), Mirroring::OneScreenLower);
        cart.cpu_write(0x8000, 0x00);
        assert_eq!(cart.mirroring(), Mirroring::OneScreenLower);
        cart.cpu_write(0x8000, 0x80);
        assert_eq!(cart.mirroring(), Mirroring::OneScreenUpper);
        cart.cpu_write(0x8000, 0x00);
        assert_eq!(cart.mirroring(), Mirroring::OneScreenLower);

        let mut four_screen = Cartridge::parse(&image(16, 0x09)).unwrap();
        assert_eq!(four_screen.mirroring(), Mirroring::FourScreen);
        four_screen.cpu_write(0x8000, 0x80);
        assert_eq!(four_screen.mirroring(), Mirroring::FourScreen);
    }

    #[test]
    fn parser_rejects_chr_rom_and_unsupported_prg_sizes() {
        let mut chr_rom = image(16, 0);
        chr_rom[5] = 1;
        chr_rom.resize(INES_HEADER_LEN + 16 * PRG_BANK_LEN + CHR_BANK_LEN, 0);
        assert!(matches!(Cartridge::parse(&chr_rom), Err(CartridgeError::InvalidChrSize(_))));

        let invalid_prg = image(8, 0);
        assert!(matches!(Cartridge::parse(&invalid_prg), Err(CartridgeError::InvalidPrgSize(_))));
    }
}


#[cfg(test)]
mod mapper87_tests {
    use super::*;

    fn image() -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN + 4 * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 2;
        image[5] = 4;
        image[6] = 0x70; // Mapper 87 low nibble.
        image[7] = 0x50; // Mapper 87 high nibble.
        for bank in 0..2usize {
            let start = INES_HEADER_LEN + bank * PRG_BANK_LEN;
            image[start..start + PRG_BANK_LEN].fill(0x30 + bank as u8);
        }
        let chr_start = INES_HEADER_LEN + 2 * PRG_BANK_LEN;
        for bank in 0..4usize {
            let start = chr_start + bank * CHR_BANK_LEN;
            image[start..start + CHR_BANK_LEN].fill(0x60 + bank as u8);
        }
        image
    }

    #[test]
    fn mapper87_keeps_prg_fixed_and_swaps_chr_register_bits() {
        let mut cart = Cartridge::parse(&image()).unwrap();
        assert!(matches!(cart, Cartridge::Mapper87(_)));
        assert_eq!(cart.cpu_read(0x8000), Some(0x30));
        assert_eq!(cart.cpu_read(0xC000), Some(0x31));
        assert_eq!(cart.ppu_read(0x0000), Some(0x60));

        // The two low register bits are swapped by the board wiring.
        assert!(cart.cpu_write(0x6000, 1));
        assert_eq!(cart.ppu_read(0x0000), Some(0x62));
        assert!(cart.cpu_write(0x7FFF, 2));
        assert_eq!(cart.ppu_read(0x0000), Some(0x61));
        assert!(cart.cpu_write(0x6000, 3));
        assert_eq!(cart.ppu_read(0x0000), Some(0x63));
        assert!(!cart.ppu_write(0x0000, 0xFF));
        assert_eq!(cart.cpu_read(0x6000), None);
    }

    #[test]
    fn mapper87_rejects_wrong_rom_sizes_and_truncated_images() {
        let mut invalid_prg = image();
        invalid_prg[4] = 1;
        assert!(matches!(Cartridge::parse(&invalid_prg), Err(CartridgeError::InvalidPrgSize(_))));

        let mut invalid_chr = image();
        invalid_chr[5] = 2;
        assert!(matches!(Cartridge::parse(&invalid_chr), Err(CartridgeError::InvalidChrSize(_))));

        let mut truncated = image();
        truncated.pop();
        assert!(matches!(Cartridge::parse(&truncated), Err(CartridgeError::TruncatedImage { .. })));
    }
}

#[cfg(test)]
mod added_nes2_size_regression_tests {
    use super::*;

    #[test]
    fn nes2_exponent_sizes_decode_exponent_and_multiplier_independently() {
        assert_eq!(nes2_rom_size(0x00, 0x0F, PRG_BANK_LEN), 1);
        assert_eq!(nes2_rom_size(0x04, 0x0F, PRG_BANK_LEN), 2);
        assert_eq!(nes2_rom_size(0x03, 0x0F, CHR_BANK_LEN), 7);
        assert_eq!(nes2_rom_size(0x07, 0x0F, CHR_BANK_LEN), 14);
    }

    #[test]
    fn extreme_nes2_exponent_size_cannot_overflow_image_length_checks() {
        assert_eq!(nes2_rom_size(0xFC, 0x0F, PRG_BANK_LEN), (usize::MAX / 4));
        assert_eq!(nes2_rom_size(0xFC, 0x0F, CHR_BANK_LEN), (usize::MAX / 4));
    }

    #[test]
    fn nes2_ram_shift_zero_means_absent_and_positive_shifts_are_exact() {
        assert_eq!(nes2_ram_size(0), 0);
        assert_eq!(nes2_ram_size(1), 128);
        assert_eq!(nes2_ram_size(7), 8_192);
    }

    #[test]
    fn nes2_nrom_rejects_truncated_exponent_encoded_prg_rom() {
        let mut image = vec![0; INES_HEADER_LEN + 1];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 0x04; // Exponent encoding: 2 bytes, not a linear bank count.
        image[5] = 0;
        image[7] = 0x08; // NES 2.0.
        image[9] = 0x0F; // Exponent/multiplier encoding for PRG size.
        assert!(matches!(
            Nrom::parse(&image),
            Err(CartridgeError::InvalidPrgSize(_)) | Err(CartridgeError::TruncatedImage { .. })
        ));
    }
}


#[cfg(test)]
mod mapper_header_error_regressions {
    use super::*;

    #[test]
    fn unsupported_ines_mapper_is_reported_before_rom_size_validation() {
        let mut rom = vec![0; INES_HEADER_LEN];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[6] = 0x50; // Mapper 5 (MMC5), not implemented by this core.
        assert!(matches!(Cartridge::parse(&rom), Err(CartridgeError::UnsupportedMapper(5))));
    }

    #[test]
    fn nes2_nrom_header_decodes_rom_size_before_truncation_check() {
        let mut rom = vec![0; INES_HEADER_LEN];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1; // 16 KiB PRG-ROM in NES 2.0 linear size encoding.
        rom[7] = 0x08; // NES 2.0 marker.
        assert!(matches!(Cartridge::parse(&rom), Err(CartridgeError::TruncatedImage { .. })));
    }
}


#[cfg(test)]
mod mapper8_regressions {
    use super::*;

    fn image(prg_banks: u8, chr_banks: u8, flags: u8) -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + prg_banks as usize * 2 * PRG_BANK_LEN
            + chr_banks as usize * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks * 2;
        image[5] = chr_banks;
        image[6] = 0x80 | flags;
        for bank in 0..prg_banks as usize {
            let start = INES_HEADER_LEN + bank * 2 * PRG_BANK_LEN;
            image[start..start + 2 * PRG_BANK_LEN].fill(bank as u8);
        }
        let chr_start = INES_HEADER_LEN + prg_banks as usize * 2 * PRG_BANK_LEN;
        for bank in 0..chr_banks as usize {
            image[chr_start + bank * CHR_BANK_LEN..chr_start + (bank + 1) * CHR_BANK_LEN]
                .fill(0x40 + bank as u8);
        }
        image
    }

    #[test]
    fn mapper8_switches_32k_prg_and_8k_chr_banks() {
        let mut cart = Cartridge::parse(&image(4, 4, 0)).unwrap();
        assert!(matches!(cart, Cartridge::Mapper8(_)));
        assert_eq!(cart.cpu_read(0x8000), Some(0));
        assert_eq!(cart.cpu_read(0xFFFF), Some(0));
        assert_eq!(cart.ppu_read(0), Some(0x40));
        cart.cpu_write(0x8000, 0x33);
        assert_eq!(cart.cpu_read(0x8000), Some(3));
        assert_eq!(cart.cpu_read(0xFFFF), Some(3));
        assert_eq!(cart.ppu_read(0), Some(0x43));
    }

    #[test]
    fn mapper8_chr_ram_and_fixed_four_screen_mirroring_work() {
        let mut cart = Cartridge::parse(&image(2, 0, 0x08)).unwrap();
        assert!(cart.chr_is_ram());
        assert_eq!(cart.mirroring(), Mirroring::FourScreen);
        cart.cpu_write(0x8000, 2);
        assert!(cart.ppu_write(0x123, 0xA5));
        assert_eq!(cart.ppu_read(0x123), Some(0xA5));
        assert_eq!(cart.mirroring(), Mirroring::FourScreen);
    }

    #[test]
    fn mapper8_rejects_invalid_prg_sizes_and_truncated_roms() {
        let mut bad = image(2, 0, 0);
        bad[4] = 3;
        assert!(matches!(Cartridge::parse(&bad), Err(CartridgeError::InvalidPrgSize(_))));
        let mut short = image(2, 1, 0);
        short.pop();
        assert!(matches!(Cartridge::parse(&short), Err(CartridgeError::TruncatedImage { .. })));
    }
}


#[cfg(test)]
mod bus_conflict_regression_tests {
    use super::*;

    fn nes2_uxrom(submapper: u8) -> Vec<u8> {
        let prg_banks = 4usize;
        let chr_banks = 1usize;
        let mut image = vec![0; INES_HEADER_LEN + prg_banks * PRG_BANK_LEN + chr_banks * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = prg_banks as u8;
        image[5] = chr_banks as u8;
        image[6] = 0x20;
        image[7] = 0x08;
        image[8] = submapper << 4;
        let start = INES_HEADER_LEN;
        for bank in 0..prg_banks {
            image[start + bank * PRG_BANK_LEN..start + (bank + 1) * PRG_BANK_LEN].fill((bank as u8) * 0x10);
        }
        image[start..start + PRG_BANK_LEN].fill(0x02);
        image
    }

    fn nes2_cnrom(submapper: u8) -> Vec<u8> {
        let prg_len = 2 * PRG_BANK_LEN;
        let chr_banks = 4usize;
        let mut image = vec![0; INES_HEADER_LEN + prg_len + chr_banks * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 2;
        image[5] = chr_banks as u8;
        image[6] = 0x30;
        image[7] = 0x08;
        image[8] = submapper << 4;
        let prg_start = INES_HEADER_LEN;
        image[prg_start..prg_start + prg_len].fill(0x02);
        let chr_start = prg_start + prg_len;
        for bank in 0..chr_banks {
            image[chr_start + bank * CHR_BANK_LEN..chr_start + (bank + 1) * CHR_BANK_LEN].fill(0x40 + bank as u8);
        }
        image
    }

    #[test]
    fn nes2_uxrom_submapper_selects_bus_conflict_behavior() {
        let mut conflict = Cartridge::parse(&nes2_uxrom(1)).unwrap();
        conflict.cpu_write(0x8000, 0x03);
        assert_eq!(conflict.cpu_read(0x8000), Some(0x20), "ROM value 0x02 masks write value 0x03");

        let mut no_conflict = Cartridge::parse(&nes2_uxrom(2)).unwrap();
        no_conflict.cpu_write(0x8000, 0x03);
        assert_eq!(no_conflict.cpu_read(0x8000), Some(0x30));
    }

    #[test]
    fn nes2_cnrom_submapper_selects_bus_conflict_behavior() {
        let mut conflict = Cartridge::parse(&nes2_cnrom(1)).unwrap();
        conflict.cpu_write(0x8000, 0x03);
        assert_eq!(conflict.ppu_read(0), Some(0x42), "ROM value 0x02 masks CHR bank write");

        let mut no_conflict = Cartridge::parse(&nes2_cnrom(2)).unwrap();
        no_conflict.cpu_write(0x8000, 0x03);
        assert_eq!(no_conflict.ppu_read(0), Some(0x43));
    }
}


#[cfg(test)]
mod mapper8_nes2_tests {
    use super::*;

    fn nes2_mapper8() -> Vec<u8> {
        let prg_len = 4 * PRG_BANK_LEN;
        let mut image = vec![0; INES_HEADER_LEN + prg_len];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 4;
        image[5] = 0;
        image[6] = 0x80;
        image[7] = 0x08;
        image[8] = 0;
        image[9] = 0;
        image[10] = 7;
        image[11] = 9;
        let start = INES_HEADER_LEN;
        for bank in 0..2 {
            image[start + bank * 2 * PRG_BANK_LEN..start + (bank + 1) * 2 * PRG_BANK_LEN]
                .fill(0x20 + bank as u8);
        }
        image
    }

    #[test]
    fn mapper8_nes2_header_and_extended_ram_sizes_are_parsed() {
        let mut cart = Cartridge::parse(&nes2_mapper8()).unwrap();
        assert!(matches!(cart, Cartridge::Mapper8(_)));
        assert_eq!(cart.cpu_read(0x8000), Some(0x20));
        cart.cpu_write(0x8000, 0x10);
        assert_eq!(cart.cpu_read(0x8000), Some(0x21));
        cart.cpu_write(0x6000, 0x5A);
        assert_eq!(cart.cpu_read(0x6000), Some(0x5A));
        assert!(cart.chr_is_ram());
        cart.cpu_write(0x8000, 0x03);
        assert!(cart.ppu_write(0x123, 0xA5));
        assert_eq!(cart.ppu_read(0x123), Some(0xA5));
    }

    #[test]
    fn mapper8_nes2_rejects_unknown_submappers() {
        let mut image = nes2_mapper8();
        image[8] = 0x10;
        assert!(matches!(Cartridge::parse(&image), Err(CartridgeError::Nes2NotSupported)));
    }
}


#[cfg(test)]
mod gxrom_nes2_tests {
    use super::*;

    fn nes2_gxrom() -> Vec<u8> {
        let prg_len = 4 * PRG_BANK_LEN;
        let chr_len = 2 * CHR_BANK_LEN;
        let mut image = vec![0; INES_HEADER_LEN + prg_len + chr_len];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 4;
        image[5] = 2;
        image[6] = 0x20;
        image[7] = 0x48;
        image[8] = 0;
        image[9] = 0;
        image[10] = 7;
        let prg_start = INES_HEADER_LEN;
        for bank in 0..2 {
            image[prg_start + bank * 2 * PRG_BANK_LEN..prg_start + (bank + 1) * 2 * PRG_BANK_LEN]
                .fill(0x40 + bank as u8);
        }
        let chr_start = prg_start + prg_len;
        for bank in 0..2 {
            image[chr_start + bank * CHR_BANK_LEN..chr_start + (bank + 1) * CHR_BANK_LEN]
                .fill(0x60 + bank as u8);
        }
        image
    }

    #[test]
    fn gxrom_nes2_decodes_prg_chr_banks_and_prg_ram() {
        let mut cart = Cartridge::parse(&nes2_gxrom()).unwrap();
        assert!(matches!(cart, Cartridge::Gxrom(_)));
        assert_eq!(cart.cpu_read(0x8000), Some(0x40));
        assert_eq!(cart.ppu_read(0), Some(0x60));
        cart.cpu_write(0x8000, 0x11);
        assert_eq!(cart.cpu_read(0x8000), Some(0x41));
        assert_eq!(cart.ppu_read(0), Some(0x61));
        cart.cpu_write(0x6000, 0xA7);
        assert_eq!(cart.cpu_read(0x6000), Some(0xA7));
    }

    #[test]
    fn gxrom_nes2_without_prg_ram_returns_unmapped_reads() {
        let mut image = nes2_gxrom();
        image[10] = 0;
        let mut cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.cpu_read(0x6000), None);
        assert!(!cart.cpu_write(0x6000, 0xA7));
        assert_eq!(cart.cpu_read(0x8000), Some(0x40));
    }

    #[test]
    fn gxrom_nes2_rejects_unknown_submappers() {
        let mut image = nes2_gxrom();
        image[8] = 0x10;
        assert!(matches!(Cartridge::parse(&image), Err(CartridgeError::Nes2NotSupported)));
    }
}

#[cfg(test)]
mod mmc3_a12_exact_boundary_regressions {
    use super::*;

    fn boundary_test_image() -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN];
        image[0..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        image[4] = 2;
        image[6] = 0x40; // Mapper 4, CHR-RAM.
        image
    }

    #[test]
    fn a12_filter_rejects_seven_low_dots_and_accepts_eight() {
        let mut cart = Cartridge::parse(&boundary_test_image()).unwrap();
        cart.cpu_write(0xC000, 0);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);

        cart.observe_ppu_address(0x0000, 10);
        cart.observe_ppu_address(0x1000, 17);
        assert!(!cart.irq_pending(), "seven low dots must not clock MMC3 IRQ");

        cart.observe_ppu_address(0x0000, 18);
        cart.observe_ppu_address(0x1000, 26);
        assert!(cart.irq_pending(), "eight low dots must clock MMC3 IRQ");
    }
}


#[cfg(test)]
mod mmc3_irq_counter_sequence_regressions {
    use super::*;

    fn image() -> Vec<u8> {
        let mut image = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN];
        image[0..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        image[4] = 2;
        image[6] = 0x40;
        image
    }

    #[test]
    fn nonzero_irq_latch_reaches_zero_on_the_following_qualified_edge() {
        let mut cart = Cartridge::parse(&image()).unwrap();
        cart.cpu_write(0xC000, 1);
        cart.cpu_write(0xC001, 0);
        cart.cpu_write(0xE001, 0);

        cart.observe_ppu_address(0x0000, 10);
        cart.observe_ppu_address(0x1000, 18); // Reload counter with latch 1.
        assert!(!cart.irq_pending());

        cart.observe_ppu_address(0x0000, 19);
        cart.observe_ppu_address(0x1000, 27); // Decrement 1 -> 0.
        assert!(cart.irq_pending(), "the IRQ asserts when the counter decrements to zero");
    }
}


#[cfg(test)]
mod mmc3_irq_reload_command_regressions {
    use super::*;

    fn image() -> Vec<u8> {
        let mut data = vec![0; INES_HEADER_LEN + 2 * PRG_BANK_LEN];
        data[0..4].copy_from_slice(b"NES\\x1A");
        data[4] = 2;
        data[6] = 0x40;
        data
    }

    #[test]
    fn c001_reloads_the_counter_without_asserting_irq_until_a_later_zero() {
        let mut cart = Cartridge::parse(&image()).unwrap();
        cart.cpu_write(0xC000, 2);
        cart.cpu_write(0xE001, 0);

        cart.observe_ppu_address(0x0000, 0);
        cart.observe_ppu_address(0x1000, 8);
        assert!(!cart.irq_pending(), "initial qualified edge loads latch 2");

        cart.cpu_write(0xC001, 0); // Request counter reload on the next qualified edge.
        cart.observe_ppu_address(0x0000, 9);
        cart.observe_ppu_address(0x1000, 17);
        assert!(!cart.irq_pending(), "reload command must not assert IRQ prematurely");

        cart.observe_ppu_address(0x0000, 18);
        cart.observe_ppu_address(0x1000, 26);
        assert!(!cart.irq_pending(), "counter decrements from 2 to 1");

        cart.observe_ppu_address(0x0000, 27);
        cart.observe_ppu_address(0x1000, 35);
        assert!(cart.irq_pending(), "IRQ asserts only when the counter reaches zero");
    }
}
