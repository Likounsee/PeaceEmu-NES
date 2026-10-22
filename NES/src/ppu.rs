//! NES picture processing unit register and memory model (rendering is not yet implemented).

use crate::cartridge::{Mirroring, Nrom};

/// Minimal PPU register and memory state.
///
/// Implements CPU-visible register semantics, pattern/nametable/palette access,
/// OAM storage, vblank status, and the NTSC scanline/dot counter. Pixel rendering
/// and sprite evaluation remain future work.
#[derive(Debug, Clone)]
pub struct Ppu {
    ctrl: u8,
    mask: u8,
    status: u8,
    oam_addr: u8,
    oam: [u8; 256],
    nametables: [u8; 4096],
    palette: [u8; 32],
    v: u16,
    t: u16,
    fine_x: u8,
    write_latch: bool,
    read_buffer: u8,
    open_bus: u8,
    scanline: u16,
    dot: u16,
    odd_frame: bool,
}

impl Default for Ppu {
    fn default() -> Self {
        Self {
            ctrl: 0, mask: 0, status: 0, oam_addr: 0, oam: [0; 256],
            nametables: [0; 4096], palette: [0; 32], v: 0, t: 0,
            fine_x: 0, write_latch: false, read_buffer: 0, open_bus: 0,
            scanline: 261, dot: 0, odd_frame: false,
        }
    }
}

impl Ppu {
    /// Creates a PPU in its pre-render scanline.
    pub fn new() -> Self { Self::default() }

    /// Reads one CPU-visible PPU register (register address is mirrored to 0..7).
    pub fn cpu_read_register(&mut self, register: u16, cartridge: Option<&Nrom>) -> u8 {
        let value = match register & 7 {
            2 => {
                let value = (self.status & 0xE0) | (self.open_bus & 0x1F);
                self.status &= !0x80;
                self.write_latch = false;
                value
            }
            4 => self.oam[self.oam_addr as usize],
            7 => {
                let address = self.v & 0x3FFF;
                let fetched = self.mem_read(address, cartridge);
                let value = if address < 0x3F00 { let old = self.read_buffer; self.read_buffer = fetched; old }
                    else { self.read_buffer = self.mem_read(address.wrapping_sub(0x1000), cartridge); fetched };
                self.increment_vram_address();
                value
            }
            _ => self.open_bus,
        };
        self.open_bus = value;
        value
    }

    /// Writes one CPU-visible PPU register (register address is mirrored to 0..7).
    pub fn cpu_write_register(&mut self, register: u16, value: u8, cartridge: Option<&mut Nrom>) {
        self.open_bus = value;
        match register & 7 {
            0 => { self.ctrl = value; self.t = (self.t & !0x0C00) | (((value as u16) & 3) << 10); }
            1 => self.mask = value,
            3 => self.oam_addr = value,
            4 => { self.oam[self.oam_addr as usize] = value; self.oam_addr = self.oam_addr.wrapping_add(1); }
            5 => {
                if !self.write_latch {
                    self.fine_x = value & 7;
                    self.t = (self.t & !0x001F) | ((value as u16) >> 3);
                } else {
                    self.t = (self.t & !0x73E0) | (((value as u16) & 0xF8) << 2) | (((value as u16) & 7) << 12);
                }
                self.write_latch = !self.write_latch;
            }
            6 => {
                if !self.write_latch { self.t = (self.t & 0x00FF) | (((value as u16) & 0x3F) << 8); }
                else { self.t = (self.t & 0x7F00) | value as u16; self.v = self.t; }
                self.write_latch = !self.write_latch;
            }
            7 => { self.mem_write(self.v & 0x3FFF, value, cartridge); self.increment_vram_address(); }
            _ => {}
        }
    }

    /// Advances the PPU by a number of dots and updates vblank timing.
    pub fn tick(&mut self, dots: u32) {
        for _ in 0..dots {
            self.dot += 1;
            if self.scanline == 241 && self.dot == 1 { self.status |= 0x80; }
            if self.scanline == 261 && self.dot == 1 { self.status &= !0xE0; }
            if self.dot >= 341 {
                self.dot = 0;
                self.scanline += 1;
                if self.scanline >= 262 { self.scanline = 0; self.odd_frame = !self.odd_frame; }
            }
        }
    }

    /// Returns the current scanline and dot for diagnostics and tests.
    pub fn position(&self) -> (u16, u16) { (self.scanline, self.dot) }

    /// Returns a read-only view of the primary OAM bytes.
    pub fn oam(&self) -> &[u8; 256] { &self.oam }

    fn increment_vram_address(&mut self) {
        self.v = self.v.wrapping_add(if self.ctrl & 4 != 0 { 32 } else { 1 }) & 0x7FFF;
    }

    fn mem_read(&self, address: u16, cartridge: Option<&Nrom>) -> u8 {
        let address = address & 0x3FFF;
        match address {
            0x0000..=0x1FFF => cartridge.and_then(|c| c.ppu_read(address)).unwrap_or(0),
            0x2000..=0x3EFF => self.nametables[Self::nametable_index(address, cartridge.map(Nrom::mirroring).unwrap_or(Mirroring::Horizontal))],
            _ => self.palette[Self::palette_index(address)],
        }
    }

    fn mem_write(&mut self, address: u16, value: u8, cartridge: Option<&mut Nrom>) {
        let address = address & 0x3FFF;
        match address {
            0x0000..=0x1FFF => { if let Some(c) = cartridge { c.ppu_write(address, value); } }
            0x2000..=0x3EFF => {
                let mirroring = cartridge.as_ref().map(|c| c.mirroring()).unwrap_or(Mirroring::Horizontal);
                let index = Self::nametable_index(address, mirroring);
                self.nametables[index] = value;
            }
            _ => { self.palette[Self::palette_index(address)] = value & 0x3F; }
        }
    }

    fn nametable_index(address: u16, mirroring: Mirroring) -> usize {
        let relative = (address as usize - 0x2000) % 0x1000;
        let table = relative / 0x400;
        let offset = relative % 0x400;
        let physical = match mirroring {
            Mirroring::Vertical => table & 1,
            Mirroring::Horizontal => table >> 1,
            Mirroring::FourScreen => table,
        };
        physical * 0x400 + offset
    }

    fn palette_index(address: u16) -> usize {
        let mut index = (address as usize - 0x3F00) & 0x1F;
        if index == 0x10 || index == 0x14 || index == 0x18 || index == 0x1C { index -= 0x10; }
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_read_clears_vblank_and_write_toggle() {
        let mut ppu = Ppu::new();
        ppu.tick(241 * 341 + 1);
        assert_ne!(ppu.cpu_read_register(2, None) & 0x80, 0);
        assert_eq!(ppu.cpu_read_register(2, None) & 0x80, 0);
        ppu.cpu_write_register(5, 0x12, None);
        ppu.cpu_write_register(6, 0x20, None);
        ppu.cpu_write_register(6, 0x00, None);
        assert_eq!(ppu.v, 0x2000);
    }

    #[test]
    fn palette_mirrors_universal_background_entries() {
        let mut ppu = Ppu::new();
        ppu.cpu_write_register(6, 0x3F, None);
        ppu.cpu_write_register(6, 0x10, None);
        ppu.cpu_write_register(7, 0x2A, None);
        ppu.cpu_write_register(6, 0x3F, None);
        ppu.cpu_write_register(6, 0x00, None);
        ppu.cpu_read_register(7, None);
        assert_eq!(ppu.cpu_read_register(7, None), 0x2A);
    }

    #[test]
    fn oam_data_auto_increments_address() {
        let mut ppu = Ppu::new();
        ppu.cpu_write_register(3, 0xFE, None);
        ppu.cpu_write_register(4, 0xAB, None);
        ppu.cpu_write_register(4, 0xCD, None);
        assert_eq!(ppu.oam()[0xFE], 0xAB);
        assert_eq!(ppu.oam()[0xFF], 0xCD);
    }
}
