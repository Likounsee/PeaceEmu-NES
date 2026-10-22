//! NES picture processing unit registers, video memory, timing, and snapshot renderer.

use crate::cartridge::{Mirroring, Nrom};

/// Minimal PPU register and memory state.
///
/// Implements CPU-visible registers, pattern/nametable/palette access, OAM storage,
/// vblank/NMI state, NTSC scanline/dot counting, and frame-based background/sprite rendering.
/// Rendering is an approximation, not a dot-accurate PPU pipeline.
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
    nmi_pending: bool,
    /// Completed 256x240 background image, stored as 6-bit NES palette indices.
    framebuffer: Vec<u8>,
    background_opaque: Vec<bool>,
}

impl Default for Ppu {
    fn default() -> Self {
        Self {
            ctrl: 0, mask: 0, status: 0, oam_addr: 0, oam: [0; 256],
            nametables: [0; 4096], palette: [0; 32], v: 0, t: 0,
            fine_x: 0, write_latch: false, read_buffer: 0, open_bus: 0,
            scanline: 261, dot: 0, odd_frame: false, nmi_pending: false,
            framebuffer: vec![0; 256 * 240], background_opaque: vec![false; 256 * 240],
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
            0 => {
                self.ctrl = value;
                self.t = (self.t & !0x0C00) | (((value as u16) & 3) << 10);
                if value & 0x80 != 0 && self.status & 0x80 != 0 { self.nmi_pending = true; }
            }
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
            // NTSC skips pre-render dot 340 on odd frames while rendering is active.
            if self.scanline == 261 && self.dot == 339 && self.odd_frame && self.mask & 0x18 != 0 {
                self.dot = 0;
                self.scanline = 0;
                self.odd_frame = false;
                continue;
            }
            self.dot += 1;
            if self.scanline == 241 && self.dot == 1 {
                self.status |= 0x80;
                if self.ctrl & 0x80 != 0 { self.nmi_pending = true; }
            }
            if self.scanline == 261 && self.dot == 1 { self.status &= !0xE0; }
            if self.dot >= 341 {
                self.dot = 0;
                self.scanline += 1;
                if self.scanline >= 262 { self.scanline = 0; self.odd_frame = !self.odd_frame; }
            }
        }
    }

    /// Renders a complete background snapshot using the current VRAM scroll address.
    ///
    /// The framebuffer contains 6-bit NES palette indices (not host RGB pixels), so the
    /// frontend can apply its chosen NTSC/PAL palette without changing emulation state.
    /// Background rendering is intentionally frame-based; per-dot fetch timing and
    /// mid-frame scroll changes are not implemented yet.
    pub fn render_background(&mut self, cartridge: Option<&Nrom>) {
        const WIDTH: usize = 256;
        const HEIGHT: usize = 240;
        let rendering_enabled = self.mask & 0x08 != 0;
        let show_left = self.mask & 0x02 != 0;
        let pattern_base = if self.ctrl & 0x10 != 0 { 0x1000 } else { 0x0000 };
        let base_nt_x = ((self.v >> 10) & 1) as usize;
        let base_nt_y = ((self.v >> 11) & 1) as usize;
        let base_coarse_x = (self.v & 0x1F) as usize;
        let base_coarse_y = ((self.v >> 5) & 0x1F) as usize;
        let fine_y = ((self.v >> 12) & 7) as usize;

        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let output = y * WIDTH + x;
                if !rendering_enabled || (x < 8 && !show_left) {
                    self.framebuffer[output] = self.output_color(self.palette[0]);
                    self.background_opaque[output] = false;
                    continue;
                }
                let scroll_x = base_coarse_x * 8 + (self.fine_x as usize) + x;
                let scroll_y = base_coarse_y * 8 + fine_y + y;
                let tile_x = scroll_x / 8;
                let tile_y = scroll_y / 8;
                let nt_x = base_nt_x ^ ((tile_x / 32) & 1);
                let nt_y = base_nt_y ^ ((tile_y / 30) & 1);
                let nametable = 0x2000u16 + ((nt_y * 2 + nt_x) as u16) * 0x400;
                let tile_addr = nametable + ((tile_y % 30) * 32 + tile_x % 32) as u16;
                let tile = self.mem_read(tile_addr, cartridge) as u16;
                let row = (scroll_y % 8) as u16;
                let col = (scroll_x % 8) as u16;
                let pattern_addr = pattern_base + tile * 16 + row;
                let low = self.mem_read(pattern_addr, cartridge);
                let high = self.mem_read(pattern_addr + 8, cartridge);
                let bit = 7 - col;
                let pixel = ((low >> bit) & 1) | (((high >> bit) & 1) << 1);
                self.background_opaque[output] = pixel != 0;
                if pixel == 0 {
                    self.framebuffer[output] = self.output_color(self.palette[0]);
                } else {
                    let attr_addr = nametable + 0x3C0
                        + (((tile_y % 30) / 4) * 8 + (tile_x % 32) / 4) as u16;
                    let attr = self.mem_read(attr_addr, cartridge);
                    let shift = (((tile_y % 4) / 2) * 4 + ((tile_x % 4) / 2) * 2) as u8;
                    let palette_id = (attr >> shift) & 3;
                    self.framebuffer[output] = self.output_color(self.palette[(palette_id * 4 + pixel) as usize]);
                }
            }
        }
    }

    /// Renders a background snapshot and composites sprites in OAM priority order.
    /// Sprite 0 hit and first-eight-per-scanline overflow are approximated; this is not
    /// a dot-accurate sprite evaluator and does not model fetch contention.
    pub fn render_frame(&mut self, cartridge: Option<&Nrom>) {
        self.render_background(cartridge);
        self.status &= !0x40;
        if self.mask & 0x10 == 0 { return; }
        let sprite_height: usize = if self.ctrl & 0x20 != 0 { 16 } else { 8 };
        let mut rows_per_scanline = [0u8; 240];
        // The first opaque sprite in OAM owns each pixel, even if hidden behind BG.
        let mut sprite_claimed = vec![false; 256 * 240];
        for sprite in 0..64usize {
            let base = sprite * 4;
            let top = (self.oam[base] as usize + 1) & 0xFF;
            let tile = self.oam[base + 1] as usize;
            let attributes = self.oam[base + 2];
            let left = self.oam[base + 3] as usize;
            let flip_h = attributes & 0x40 != 0;
            let flip_v = attributes & 0x80 != 0;
            let behind_background = attributes & 0x20 != 0;
            for sy in 0..sprite_height {
                let y = top + sy;
                if y >= 240 { continue; }
                rows_per_scanline[y] = rows_per_scanline[y].saturating_add(1);
                if rows_per_scanline[y] > 8 { self.status |= 0x20; continue; }
                let source_y = if flip_v { sprite_height - 1 - sy } else { sy };
                let (pattern_base, tile_index, row) = if sprite_height == 16 {
                    let base_addr = if tile & 1 != 0 { 0x1000u16 } else { 0 };
                    (base_addr, (tile & 0xFE) + source_y / 8, source_y % 8)
                } else {
                    (if self.ctrl & 0x08 != 0 { 0x1000u16 } else { 0 }, tile, source_y)
                };
                let address = pattern_base + (tile_index * 16 + row) as u16;
                let low = self.mem_read(address, cartridge);
                let high = self.mem_read(address + 8, cartridge);
                for sx in 0..8usize {
                    let x = left + sx;
                    if x >= 256 || (x < 8 && self.mask & 0x04 == 0) { continue; }
                    let source_x = if flip_h { sx } else { 7 - sx };
                    let pixel = ((low >> source_x) & 1) | (((high >> source_x) & 1) << 1);
                    if pixel == 0 { continue; }
                    let index = y * 256 + x;
                    if x < 8 && self.mask & 0x02 == 0 { continue; }
                    if sprite == 0 && self.background_opaque[index] && x < 255 {
                        self.status |= 0x40;
                    }
                    // Lower OAM indices have priority. A sprite hidden by the background
                    // still claims the pixel and must not let a later sprite show through.
                    if sprite_claimed[index] { continue; }
                    sprite_claimed[index] = true;
                    if !behind_background || !self.background_opaque[index] {
                        let palette_index = 0x10 + ((attributes as usize & 3) * 4) + pixel as usize;
                        self.framebuffer[index] = self.output_color(self.palette[Self::palette_index(0x3F00 + palette_index as u16)]);
                    }
                }
            }
        }
    }

    /// Applies the PPU grayscale output mask while retaining a 6-bit palette index.
    fn output_color(&self, color: u8) -> u8 {
        if self.mask & 0x01 != 0 { color & 0x30 } else { color & 0x3F }
    }

    /// Returns the most recently rendered 256x240 image as NES palette indices.
    pub fn framebuffer(&self) -> &[u8] { &self.framebuffer }

    /// Returns the current scanline and dot for diagnostics and tests.
    pub fn position(&self) -> (u16, u16) { (self.scanline, self.dot) }

    /// Returns a read-only view of the primary OAM bytes.
    pub fn oam(&self) -> &[u8; 256] { &self.oam }

    /// Takes a pending rising-edge NMI request generated by vblank.
    pub fn take_nmi_request(&mut self) -> bool { core::mem::replace(&mut self.nmi_pending, false) }

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
        ppu.tick(243 * 341 + 1);
        assert_ne!(ppu.cpu_read_register(2, None) & 0x80, 0);
        assert_eq!(ppu.cpu_read_register(2, None) & 0x80, 0);
        ppu.cpu_write_register(5, 0x12, None);
        ppu.cpu_write_register(5, 0x34, None);
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
        assert_eq!(ppu.cpu_read_register(7, None), 0x2A);
    }

    #[test]
    fn vblank_requests_nmi_when_enabled() {
        let mut ppu = Ppu::new();
        ppu.cpu_write_register(0, 0x80, None);
        ppu.tick(243 * 341 + 1);
        assert!(ppu.take_nmi_request());
        assert!(!ppu.take_nmi_request());
    }

    #[test]
    fn enabling_nmi_during_vblank_requests_interrupt() {
        let mut ppu = Ppu::new();
        ppu.tick(243 * 341 + 1);
        ppu.cpu_write_register(0, 0x80, None);
        assert!(ppu.take_nmi_request());
    }

    #[test]
    fn background_renderer_decodes_pattern_bits_and_palette() {
        let mut ppu = Ppu::new();
        // Use CHR-RAM so the test can construct a tiny tile.
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Nrom::parse(&image).unwrap();
        // Build known video memory directly; register-latch behavior is tested separately.
        ppu.mask = 0x0A;
        ppu.mem_write(0x0000, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x3F00, 0x0F, Some(&mut cartridge));
        ppu.mem_write(0x3F01, 0x21, Some(&mut cartridge));
        ppu.mem_write(0x2000, 0x00, Some(&mut cartridge));
        ppu.render_background(Some(&cartridge));
        assert_eq!(ppu.framebuffer()[0], 0x21);
        assert_eq!(ppu.framebuffer()[1], 0x0F);
        assert_eq!(ppu.framebuffer().len(), 256 * 240);
    }

    #[test]
    fn grayscale_mask_restricts_background_and_sprite_palette_indices() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Nrom::parse(&image).unwrap();
        ppu.mask = 0x1F; // Background, sprites, both left columns and grayscale.
        ppu.mem_write(0x0000, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x3F01, 0x2B, Some(&mut cartridge));
        ppu.mem_write(0x3F11, 0x17, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[255, 1, 0, 16]);
        ppu.render_frame(Some(&cartridge));
        assert_eq!(ppu.framebuffer()[16], 0x10);
        assert_eq!(ppu.framebuffer()[0], 0x20);
    }

    #[test]
    fn sprite_renderer_composites_sprite_pixel_and_sets_zero_hit() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Nrom::parse(&image).unwrap();
        // Build the pattern/palette fixture directly; OAM Y=255 wraps to row zero.
        ppu.mask = 0x1E;
        ppu.mem_write(0x0000, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x3F01, 0x22, Some(&mut cartridge));
        ppu.mem_write(0x3F11, 0x30, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[255, 1, 0, 0]);
        ppu.mem_write(0x2000, 0x00, Some(&mut cartridge));
        ppu.render_frame(Some(&cartridge));
        assert_ne!(ppu.status & 0x40, 0);
        assert_eq!(ppu.framebuffer()[0], 0x30);
    }

    #[test]
    fn sprite_priority_preserves_lower_oam_index() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Nrom::parse(&image).unwrap();

        // Two opaque sprites overlap at (0, 0), with distinct sprite palettes.
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0020, 0x80, Some(&mut cartridge));
        ppu.palette[0x11] = 0x30;
        ppu.palette[0x15] = 0x27;
        ppu.mask = 0x14; // Show sprites and the leftmost eight sprite pixels.
        ppu.oam[0..8].copy_from_slice(&[255, 1, 0, 16, 255, 2, 1, 16]);
        assert_eq!(cartridge.ppu_read(0x10), Some(0x80));
        assert_eq!(ppu.palette[0x11], 0x30);

        ppu.render_frame(Some(&cartridge));

        assert_eq!(ppu.framebuffer()[16], 0x30);
    }

    #[test]
    fn ninth_sprite_on_scanline_sets_overflow_flag() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x10;
        for sprite in 0..9usize {
            ppu.oam[sprite * 4] = 255; // Every sprite starts at scanline zero.
        }
        ppu.render_frame(None);
        assert_ne!(ppu.status & 0x20, 0);
    }

    #[test]
    fn sprite_behind_background_does_not_replace_opaque_background() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Nrom::parse(&image).unwrap();
        ppu.mask = 0x1E;
        ppu.mem_write(0x0000, 0x80, Some(&mut cartridge)); // Background tile zero.
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge)); // Sprite tile one.
        ppu.mem_write(0x3F01, 0x22, Some(&mut cartridge));
        ppu.mem_write(0x3F11, 0x30, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[255, 1, 0x20, 16]);
        ppu.render_frame(Some(&cartridge));
        assert_eq!(ppu.framebuffer()[16], 0x22);
    }

    #[test]
    fn odd_ntsc_frame_skips_pre_render_dot_when_rendering_is_enabled() {
        let mut ppu = Ppu::new();
        ppu.scanline = 261;
        ppu.dot = 339;
        ppu.odd_frame = true;
        ppu.mask = 0x08;
        ppu.tick(1);
        assert_eq!(ppu.scanline, 0);
        assert_eq!(ppu.dot, 0);
        assert!(!ppu.odd_frame);
    }

    #[test]
    fn even_frame_does_not_skip_pre_render_dot() {
        let mut ppu = Ppu::new();
        ppu.scanline = 261;
        ppu.dot = 339;
        ppu.odd_frame = false;
        ppu.mask = 0x08;
        ppu.tick(1);
        assert_eq!(ppu.scanline, 261);
        assert_eq!(ppu.dot, 340);
    }

    #[test]
    fn nametable_indices_follow_horizontal_vertical_and_four_screen_mirroring() {
        assert_eq!(Ppu::nametable_index(0x2000, Mirroring::Vertical), 0x000);
        assert_eq!(Ppu::nametable_index(0x2400, Mirroring::Vertical), 0x400);
        assert_eq!(Ppu::nametable_index(0x2800, Mirroring::Vertical), 0x000);
        assert_eq!(Ppu::nametable_index(0x2C00, Mirroring::Vertical), 0x400);
        assert_eq!(Ppu::nametable_index(0x2000, Mirroring::Horizontal), 0x000);
        assert_eq!(Ppu::nametable_index(0x2400, Mirroring::Horizontal), 0x000);
        assert_eq!(Ppu::nametable_index(0x2800, Mirroring::Horizontal), 0x400);
        assert_eq!(Ppu::nametable_index(0x2C00, Mirroring::Horizontal), 0x400);
        assert_eq!(Ppu::nametable_index(0x2C00, Mirroring::FourScreen), 0xC00);
        assert_eq!(Ppu::nametable_index(0x3EFF, Mirroring::FourScreen), 0xEFF);
    }

    #[test]
    fn palette_mirrors_repeat_every_32_bytes_and_alias_sprite_universal_colors() {
        assert_eq!(Ppu::palette_index(0x3F00), 0);
        assert_eq!(Ppu::palette_index(0x3F20), 0);
        assert_eq!(Ppu::palette_index(0x3F30), 0);
        assert_eq!(Ppu::palette_index(0x3F10), Ppu::palette_index(0x3F00));
        assert_eq!(Ppu::palette_index(0x3F14), Ppu::palette_index(0x3F04));
        assert_eq!(Ppu::palette_index(0x3F18), Ppu::palette_index(0x3F08));
        assert_eq!(Ppu::palette_index(0x3F1C), Ppu::palette_index(0x3F0C));
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
