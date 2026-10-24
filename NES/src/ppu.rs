//! NES picture processing unit registers, video memory, timing, and snapshot renderer.

use crate::cartridge::{Cartridge, Mirroring};

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
    frame_scroll_v: u16,
    frame_fine_x: u8,
    frame_scroll_valid: bool,
    line_scroll_v: u16,
    nmi_pending: bool,
    nmi_line: bool,
    suppress_vblank: bool,
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
            scanline: 261, dot: 0, odd_frame: false, frame_scroll_v: 0, frame_fine_x: 0,
            frame_scroll_valid: false, line_scroll_v: 0,
            nmi_pending: false, nmi_line: false, suppress_vblank: false,
            framebuffer: vec![0; 256 * 240], background_opaque: vec![false; 256 * 240],
        }
    }
}

impl Ppu {
    /// Creates a PPU in its pre-render scanline.
    pub fn new() -> Self { Self::default() }

    /// Reads one CPU-visible PPU register (register address is mirrored to 0..7).
    pub fn cpu_read_register(&mut self, register: u16, cartridge: Option<&Cartridge>) -> u8 {
        let value = match register & 7 {
            2 => {
                let value = (self.status & 0xE0) | (self.open_bus & 0x1F);
                // A status read immediately before the vblank-set dot suppresses
                // this frame's vblank flag and therefore its NMI edge.
                if self.scanline == 241 && self.dot == 0 {
                    self.suppress_vblank = true;
                }
                self.status &= !0x80;
                self.update_nmi_line();
                self.write_latch = false;
                value
            }
            4 => self.oam[self.oam_addr as usize],
            7 => {
                let address = self.v & 0x3FFF;
                let fetched = self.mem_read(address, cartridge);
                let value = if address < 0x3F00 {
                    let old = self.read_buffer;
                    self.read_buffer = fetched;
                    old
                } else {
                    self.read_buffer = self.mem_read(address.wrapping_sub(0x1000), cartridge);
                    // Palette RAM drives only PPU data bits 0-5; bits 6-7 retain
                    // the PPU I/O latch just like other partially-driven reads.
                    (fetched & 0x3F) | (self.open_bus & 0xC0)
                };
                self.increment_vram_address();
                value
            }
            _ => self.open_bus,
        };
        self.open_bus = value;
        value
    }

    /// Writes one CPU-visible PPU register (register address is mirrored to 0..7).
    pub fn cpu_write_register(&mut self, register: u16, value: u8, cartridge: Option<&mut Cartridge>) {
        self.open_bus = value;
        match register & 7 {
            0 => {
                self.ctrl = value;
                self.t = (self.t & !0x0C00) | (((value as u16) & 3) << 10);
                self.update_nmi_line();
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
    pub fn tick(&mut self, dots: u32) -> u32 {
        self.tick_with_cartridge(dots, None)
    }

    /// Advances the PPU while allowing visible scanlines to fetch cartridge pattern data.
    pub(crate) fn tick_with_cartridge(&mut self, dots: u32, cartridge: Option<&Cartridge>) -> u32 {
        let mut crossed_scanlines = 0;
        for _ in 0..dots {
            // NTSC skips pre-render dot 340 on odd frames while rendering is active.
            if self.scanline == 261 && self.dot == 339 && self.odd_frame && self.mask & 0x18 != 0 {
                self.dot = 0;
                self.scanline = 0;
                self.odd_frame = false;
                crossed_scanlines += 1;
                continue;
            }
            self.dot += 1;

            // Latch the scroll address at the start of each visible scanline. The
            // live v register is changed by coarse-X fetches and the dot-256 Y increment,
            // so using it at the end of the line would shift the row being rendered.
            if self.dot == 1 && self.scanline < 240 {
                self.line_scroll_v = self.v;
            }

            // Emit the background pixel before testing sprite zero, so hit detection
            // uses the exact opacity/alignment of the pixel visible at this beam dot.
            // Re-decoding the background independently here can disagree with the renderer.
            if (1..=256).contains(&self.dot) && self.scanline < 240 {
                let x = (self.dot - 1) as usize;
                let y = self.scanline as usize;
                self.render_background_pixel(x, y, cartridge);
                if self.status & 0x40 == 0 && self.sprite_zero_hit_at(x, y, cartridge) {
                    self.status |= 0x40;
                }
                // Composite sprites at the beam position rather than painting the
                // entire row at dot 256. Mid-scanline OAM, CHR-RAM and palette writes
                // can then affect only pixels the PPU has not emitted yet.
                self.render_sprite_pixel(x, y, cartridge);
            }
            if self.dot == 256 && self.scanline < 240 && self.mask & 0x18 != 0 {
                // Sprite evaluation is disabled when both rendering layers are off.
                self.update_sprite_overflow(self.scanline as usize);
            }

            // During visible and pre-render scanlines, the rendering pipeline advances
            // coarse X every tile, increments Y at dot 256, and copies scroll fields at
            // the hardware reload points. Pixel fetches are still approximated by a line renderer.
            if self.mask & 0x18 != 0 && (self.scanline < 240 || self.scanline == 261) {
                if ((1..=256).contains(&self.dot) || (328..=336).contains(&self.dot))
                    && self.dot % 8 == 0
                {
                    self.increment_coarse_x();
                }
                if self.dot == 256 {
                    self.increment_y();
                }
                if self.dot == 257 {
                    self.v = (self.v & !0x041F) | (self.t & 0x041F);
                }
                if self.scanline == 261 && (280..=304).contains(&self.dot) {
                    self.v = (self.v & !0x7BE0) | (self.t & 0x7BE0);
                }
            }

            // Preserve the upcoming frame's starting scroll even when rendering is
            // disabled during pre-render. In that case hardware does not copy t into v,
            // so latch the current v rather than retaining a stale prior-frame snapshot.
            if self.scanline == 261 && self.dot == 304 {
                self.frame_scroll_v = self.v;
                self.frame_fine_x = self.fine_x;
                self.frame_scroll_valid = true;
            }

            if self.scanline == 241 && self.dot == 1 {
                if !self.suppress_vblank {
                    self.status |= 0x80;
                    self.update_nmi_line();
                }
                self.suppress_vblank = false;
            }
            if self.scanline == 261 && self.dot == 1 {
                self.status &= !0xE0;
                self.update_nmi_line();
            }
            if self.dot >= 341 {
                self.dot = 0;
                self.scanline += 1;
                crossed_scanlines += 1;
                if self.scanline >= 262 { self.scanline = 0; self.odd_frame = !self.odd_frame; }
            }
        }
        crossed_scanlines
    }

    /// Checks sprite-zero/background opacity for one beam pixel.
    ///
    /// This is deliberately separate from scanline composition: games such as SMB poll
    /// the hit flag mid-scanline and immediately change scroll registers. The flag must
    /// become visible at the overlapping pixel, not after dot 256.
    fn sprite_zero_hit_at(&self, x: usize, y: usize, cartridge: Option<&Cartridge>) -> bool {
        if x >= 255 || self.mask & 0x18 != 0x18
            || (x < 8 && self.mask & 0x06 != 0x06)
        {
            return false;
        }

        let sprite_top = (self.oam[0] as usize + 1) & 0xFF;
        let height = if self.ctrl & 0x20 != 0 { 16usize } else { 8usize };
        if y < sprite_top || y >= sprite_top + height {
            return false;
        }
        let sprite_left = self.oam[3] as usize;
        if x < sprite_left || x >= sprite_left + 8 {
            return false;
        }

        // Background opacity is taken from the pixel already emitted at this dot.
        // This keeps sprite-zero hit in lockstep with fine-X, masking and scroll rendering.
        if !self.background_opaque[y * 256 + x] {
            return false;
        }

        let attributes = self.oam[2];
        let row_in_sprite = y - sprite_top;
        let source_y = if attributes & 0x80 != 0 { height - 1 - row_in_sprite } else { row_in_sprite };
        let tile_index = self.oam[1] as usize;
        let (sprite_table, sprite_tile, sprite_row) = if height == 16 {
            (if tile_index & 1 != 0 { 0x1000u16 } else { 0 },
             (tile_index & 0xFE) + source_y / 8, source_y % 8)
        } else {
            (if self.ctrl & 0x08 != 0 { 0x1000u16 } else { 0 }, tile_index, source_y)
        };
        let sprite_addr = sprite_table + (sprite_tile * 16 + sprite_row) as u16;
        let sprite_x = x - sprite_left;
        let sprite_bit = if attributes & 0x40 != 0 { sprite_x } else { 7 - sprite_x };
        let sprite_pixel = ((self.mem_read(sprite_addr, cartridge) >> sprite_bit) & 1)
            | (((self.mem_read(sprite_addr + 8, cartridge) >> sprite_bit) & 1) << 1);
        sprite_pixel != 0
    }

    /// Renders one background pixel when the emulated beam reaches it.
    ///
    /// Pixel-at-a-time output prevents writes to palette/mask/CHR-visible state later in
    /// a scanline from retroactively changing pixels already emitted by the PPU.
    fn render_background_pixel(&mut self, x: usize, y: usize, cartridge: Option<&Cartridge>) {
        if x >= 256 || y >= 240 { return; }
        let index = y * 256 + x;
        let bg_enabled = self.mask & 0x08 != 0;
        let show_bg_left = self.mask & 0x02 != 0;
        if !bg_enabled || (x < 8 && !show_bg_left) {
            self.framebuffer[index] = self.output_color(self.palette[0]);
            self.background_opaque[index] = false;
            return;
        }

        let scroll_v = self.line_scroll_v;
        let fine_x = self.fine_x as usize;
        let base_nt_x = ((scroll_v >> 10) & 1) as usize;
        let nt_y = ((scroll_v >> 11) & 1) as usize;
        let base_coarse_x = (scroll_v & 0x1F) as usize;
        let coarse_y = ((scroll_v >> 5) & 0x1F) as usize;
        let fine_y = ((scroll_v >> 12) & 7) as usize;
        let pattern_base = if self.ctrl & 0x10 != 0 { 0x1000u16 } else { 0 };
        let scroll_x = base_coarse_x * 8 + fine_x + x;
        let tile_x = scroll_x / 8;
        let nt_x = base_nt_x ^ ((tile_x / 32) & 1);
        let nametable = 0x2000u16 + ((nt_y * 2 + nt_x) as u16) * 0x400;
        let coarse_x = tile_x % 32;
        let tile_addr = nametable + (coarse_y * 32 + coarse_x) as u16;
        let tile = self.mem_read(tile_addr, cartridge) as u16;
        let col = (scroll_x % 8) as u16;
        let pattern_addr = pattern_base + tile * 16 + fine_y as u16;
        let low = self.mem_read(pattern_addr, cartridge);
        let high = self.mem_read(pattern_addr + 8, cartridge);
        let pixel = ((low >> (7 - col)) & 1) | (((high >> (7 - col)) & 1) << 1);
        self.background_opaque[index] = pixel != 0;
        if pixel == 0 {
            self.framebuffer[index] = self.output_color(self.palette[0]);
        } else {
            let attr_addr = nametable + 0x3C0 + ((coarse_y / 4) * 8 + coarse_x / 4) as u16;
            let attr = self.mem_read(attr_addr, cartridge);
            let shift = (((coarse_y % 4) / 2) * 4 + ((coarse_x % 4) / 2) * 2) as u8;
            let palette_id = (attr >> shift) & 3;
            self.framebuffer[index] = self.output_color(self.palette[(palette_id * 4 + pixel) as usize]);
        }
    }

    /// Draws the highest-priority opaque sprite pixel at the current beam position.
    ///
    /// The first eight sprites intersecting a scanline participate, in OAM order.
    /// Pixel composition uses current sprite/palette memory so later writes do not
    /// retroactively change already-emitted pixels.
    fn render_sprite_pixel(&mut self, x: usize, y: usize, cartridge: Option<&Cartridge>) {
        if x >= 256 || y >= 240 || self.mask & 0x10 == 0
            || (x < 8 && self.mask & 0x04 == 0)
        {
            return;
        }

        let height = if self.ctrl & 0x20 != 0 { 16usize } else { 8usize };
        let mut in_range = 0usize;
        for sprite in 0..64usize {
            let base = sprite * 4;
            let top = (self.oam[base] as usize + 1) & 0xFF;
            if y < top || y >= top + height { continue; }
            in_range += 1;
            if in_range > 8 { break; }

            let left = self.oam[base + 3] as usize;
            if x < left || x >= left + 8 { continue; }

            let tile = self.oam[base + 1] as usize;
            let attributes = self.oam[base + 2];
            let row_in_sprite = y - top;
            let source_y = if attributes & 0x80 != 0 { height - 1 - row_in_sprite } else { row_in_sprite };
            let (pattern_base, tile_index, row) = if height == 16 {
                (if tile & 1 != 0 { 0x1000u16 } else { 0 },
                 (tile & 0xFE) + source_y / 8, source_y % 8)
            } else {
                (if self.ctrl & 0x08 != 0 { 0x1000u16 } else { 0 }, tile, source_y)
            };
            let address = pattern_base + (tile_index * 16 + row) as u16;
            let low = self.mem_read(address, cartridge);
            let high = self.mem_read(address + 8, cartridge);
            let offset_x = x - left;
            let bit = if attributes & 0x40 != 0 { offset_x } else { 7 - offset_x };
            let pixel = ((low >> bit) & 1) | (((high >> bit) & 1) << 1);
            if pixel == 0 { continue; }

            // Even a sprite hidden behind opaque background claims this pixel, so
            // lower-priority sprites cannot incorrectly appear through it.
            if attributes & 0x20 == 0 || !self.background_opaque[y * 256 + x] {
                let palette_index = 0x10 + ((attributes as usize & 3) * 4) + pixel as usize;
                let color = self.palette[Self::palette_index(0x3F00 + palette_index as u16)];
                self.framebuffer[y * 256 + x] = self.output_color(color);
            }
            return;
        }
    }

    /// Sets the sticky overflow flag when more than eight sprites cover this scanline.
    fn update_sprite_overflow(&mut self, y: usize) {
        let height = if self.ctrl & 0x20 != 0 { 16usize } else { 8usize };
        let mut count = 0usize;
        for sprite in 0..64usize {
            let top = (self.oam[sprite * 4] as usize + 1) & 0xFF;
            if y >= top && y < top + height {
                count += 1;
                if count > 8 {
                    self.status |= 0x20;
                    return;
                }
            }
        }
    }

    /// Renders a complete background snapshot using the current VRAM scroll address.
    ///
    /// The framebuffer contains 6-bit NES palette indices (not host RGB pixels), so the
    /// frontend can apply its chosen NTSC/PAL palette without changing emulation state.
    /// Background rendering is intentionally frame-based; per-dot fetch timing and
    /// mid-frame scroll changes are not implemented yet.
    pub fn render_background(&mut self, cartridge: Option<&Cartridge>) {
        const WIDTH: usize = 256;
        const HEIGHT: usize = 240;
        let rendering_enabled = self.mask & 0x08 != 0;
        let show_left = self.mask & 0x02 != 0;
        let pattern_base = if self.ctrl & 0x10 != 0 { 0x1000 } else { 0x0000 };
        // Direct/manual snapshots before the first pre-render latch use live registers;
        // normal frame rendering uses the captured start scroll rather than end-of-frame v.
        let scroll_v = if self.frame_scroll_valid { self.frame_scroll_v } else { self.v };
        let scroll_fine_x = if self.frame_scroll_valid { self.frame_fine_x } else { self.fine_x };
        let base_nt_x = ((scroll_v >> 10) & 1) as usize;
        let base_nt_y = ((scroll_v >> 11) & 1) as usize;
        let base_coarse_x = (scroll_v & 0x1F) as usize;
        let base_coarse_y = ((scroll_v >> 5) & 0x1F) as usize;
        let fine_y = ((scroll_v >> 12) & 7) as usize;

        for y in 0..HEIGHT {
            // Vertical scroll is constant across a scanline; compute its wrapped
            // coarse-Y/nametable address once instead of repeating it for every pixel.
            let scroll_y = base_coarse_y * 8 + fine_y + y;
            let tile_y = scroll_y / 8;
            let (coarse_y, nt_y) = Self::scroll_coarse_y(base_coarse_y, base_nt_y, tile_y - base_coarse_y);
            for x in 0..WIDTH {
                let output = y * WIDTH + x;
                if !rendering_enabled || (x < 8 && !show_left) {
                    self.framebuffer[output] = self.output_color(self.palette[0]);
                    self.background_opaque[output] = false;
                    continue;
                }
                let scroll_x = base_coarse_x * 8 + (scroll_fine_x as usize) + x;
                let tile_x = scroll_x / 8;
                let nt_x = base_nt_x ^ ((tile_x / 32) & 1);
                let nametable = 0x2000u16 + ((nt_y * 2 + nt_x) as u16) * 0x400;
                let coarse_x = tile_x % 32;
                let tile_addr = nametable + (coarse_y * 32 + coarse_x) as u16;
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
                        + ((coarse_y / 4) * 8 + coarse_x / 4) as u16;
                    let attr = self.mem_read(attr_addr, cartridge);
                    let shift = (((coarse_y % 4) / 2) * 4 + ((coarse_x % 4) / 2) * 2) as u8;
                    let palette_id = (attr >> shift) & 3;
                    self.framebuffer[output] = self.output_color(self.palette[(palette_id * 4 + pixel) as usize]);
                }
            }
        }
    }

    /// Renders a background snapshot and composites sprites in OAM priority order.
    /// Sprite 0 hit and first-eight-per-scanline overflow are approximated; this is not
    /// a dot-accurate sprite evaluator and does not model fetch contention.
    pub fn render_frame(&mut self, cartridge: Option<&Cartridge>) {
        self.render_background(cartridge);
        // Sprite 0 hit and overflow are derived from this snapshot, not persistent events.
        // Recompute both so changing OAM or masks cannot leave stale status behind.
        self.status &= !0x60;
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
                    // Background and sprite left-edge clipping are independent PPUMASK bits.
                    // The background clip must not hide a sprite whose own left-edge bit is set.
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

    /// Advances the coarse X scroll field, wrapping into the horizontal nametable bit.
    fn increment_coarse_x(&mut self) {
        if self.v & 0x001F == 31 {
            self.v &= !0x001F;
            self.v ^= 0x0400;
        } else {
            self.v = self.v.wrapping_add(1);
        }
    }

    /// Advances fine Y and then coarse Y, toggling the vertical nametable when needed.
    fn increment_y(&mut self) {
        if self.v & 0x7000 != 0x7000 {
            self.v = self.v.wrapping_add(0x1000);
            return;
        }
        self.v &= !0x7000;
        let coarse_y = (self.v & 0x03E0) >> 5;
        match coarse_y {
            29 => {
                self.v &= !0x03E0;
                self.v ^= 0x0800;
            }
            31 => self.v &= !0x03E0,
            _ => self.v = (self.v & !0x03E0) | ((coarse_y + 1) << 5),
        }
    }

    /// Applies the PPU grayscale output mask while retaining a 6-bit palette index.
    fn output_color(&self, color: u8) -> u8 {
        if self.mask & 0x01 != 0 { color & 0x30 } else { color & 0x3F }
    }

    /// Returns the most recently rendered 256x240 image as NES palette indices.
    pub fn framebuffer(&self) -> &[u8] { &self.framebuffer }

    /// Returns whether either background or sprite rendering is enabled in PPUMASK.
    /// MMC3 scanline IRQ approximation must not advance while the PPU is idle.
    pub fn rendering_enabled(&self) -> bool { self.mask & 0x18 != 0 }

    /// Returns the current scanline and dot for diagnostics and tests.
    pub fn position(&self) -> (u16, u16) { (self.scanline, self.dot) }

    /// Returns a read-only view of the primary OAM bytes.
    pub fn oam(&self) -> &[u8; 256] { &self.oam }

    /// Updates the NMI output line and latches only low-to-high transitions.
    fn update_nmi_line(&mut self) {
        let line = self.ctrl & 0x80 != 0 && self.status & 0x80 != 0;
        if line && !self.nmi_line { self.nmi_pending = true; }
        self.nmi_line = line;
    }

    /// Takes a pending rising-edge NMI request generated by vblank.
    pub fn take_nmi_request(&mut self) -> bool { core::mem::replace(&mut self.nmi_pending, false) }

    fn increment_vram_address(&mut self) {
        self.v = self.v.wrapping_add(if self.ctrl & 4 != 0 { 32 } else { 1 }) & 0x7FFF;
    }

    fn mem_read(&self, address: u16, cartridge: Option<&Cartridge>) -> u8 {
        let address = address & 0x3FFF;
        match address {
            0x0000..=0x1FFF => cartridge.and_then(|c| c.ppu_read(address)).unwrap_or(0),
            0x2000..=0x3EFF => self.nametables[Self::nametable_index(address, cartridge.map(Cartridge::mirroring).unwrap_or(Mirroring::Horizontal))],
            _ => self.palette[Self::palette_index(address)],
        }
    }

    fn mem_write(&mut self, address: u16, value: u8, cartridge: Option<&mut Cartridge>) {
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

    /// Advances coarse Y using the two special PPU rollover rows.
    fn scroll_coarse_y(mut coarse_y: usize, mut nt_y: usize, rows: usize) -> (usize, usize) {
        for _ in 0..rows {
            match coarse_y {
                29 => { coarse_y = 0; nt_y ^= 1; }
                31 => coarse_y = 0,
                _ => coarse_y += 1,
            }
        }
        (coarse_y, nt_y)
    }

    fn nametable_index(address: u16, mirroring: Mirroring) -> usize {
        let relative = (address as usize - 0x2000) % 0x1000;
        let table = relative / 0x400;
        let offset = relative % 0x400;
        let physical = match mirroring {
            Mirroring::Vertical => table & 1,
            Mirroring::Horizontal => table >> 1,
            Mirroring::FourScreen => table,
            Mirroring::OneScreenLower => 0,
            Mirroring::OneScreenUpper => 1,
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
    fn one_screen_mirroring_maps_all_nametables_to_selected_physical_page() {
        for (mirroring, physical) in [
            (Mirroring::OneScreenLower, 0usize),
            (Mirroring::OneScreenUpper, 1usize),
        ] {
            for table in 0..4u16 {
                assert_eq!(
                    Ppu::nametable_index(0x2000 + table * 0x400 + 0x123, mirroring),
                    physical * 0x400 + 0x123
                );
            }
        }
    }

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
    fn status_read_returns_open_bus_low_bits_and_resets_scroll_latch() {
        let mut ppu = Ppu::new();
        ppu.cpu_write_register(0, 0x1B, None);
        ppu.cpu_write_register(5, 0x03, None); // First $2005 write.
        let _ = ppu.cpu_read_register(2, None); // Reading status resets the shared write toggle.
        ppu.cpu_write_register(5, 0x28, None); // Must be interpreted as the first write again.
        assert_eq!(ppu.fine_x, 0);
        assert_eq!(ppu.t & 0x001F, 5);
        assert_eq!(ppu.cpu_read_register(2, None) & 0x1F, 0x08);
    }

    #[test]
    fn vblank_flag_clears_at_pre_render_start() {
        let mut ppu = Ppu::new();
        ppu.status |= 0xE0;
        ppu.scanline = 261;
        ppu.dot = 0;
        ppu.tick(1);
        assert_eq!(ppu.status & 0xE0, 0);
    }

    #[test]
    fn palette_ppudata_reads_preserve_open_bus_high_bits() {
        let mut ppu = Ppu::new();
        ppu.open_bus = 0xC0;
        ppu.v = 0x3F00;
        ppu.palette[0] = 0x2A;

        assert_eq!(ppu.cpu_read_register(7, None), 0xEA);
        assert_eq!(ppu.open_bus, 0xEA);
        assert_eq!(ppu.v, 0x3F01, "PPUDATA increments after a palette read");
        assert_eq!(ppu.read_buffer, 0, "palette reads refill the buffer from the nametable mirror");
    }

    #[test]
    fn ppudata_non_palette_reads_are_buffered_and_increment_by_one() {
        let mut ppu = Ppu::new();
        ppu.cpu_write_register(6, 0x20, None);
        ppu.cpu_write_register(6, 0x00, None);
        ppu.cpu_write_register(7, 0x5A, None);
        ppu.cpu_write_register(6, 0x20, None);
        ppu.cpu_write_register(6, 0x00, None);
        assert_eq!(ppu.cpu_read_register(7, None), 0);
        assert_eq!(ppu.cpu_read_register(7, None), 0x5A);
        assert_eq!(ppu.v & 0x3FFF, 0x2002);
    }

    #[test]
    fn ppudata_increment_by_32_follows_control_register() {
        let mut ppu = Ppu::new();
        ppu.cpu_write_register(0, 0x04, None);
        ppu.cpu_write_register(6, 0x20, None);
        ppu.cpu_write_register(6, 0x00, None);
        ppu.cpu_write_register(7, 0xA7, None);
        assert_eq!(ppu.v & 0x3FFF, 0x2020);
        ppu.cpu_write_register(6, 0x20, None);
        ppu.cpu_write_register(6, 0x00, None);
        assert_eq!(ppu.cpu_read_register(7, None), 0);
        assert_eq!(ppu.cpu_read_register(7, None), 0xA7);
        assert_eq!(ppu.v & 0x3FFF, 0x2040);
    }

    #[test]
    fn frame_renderer_latches_start_scroll_before_live_address_advances() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x08;
        ppu.t = 0x7BE0 | 0x041F;
        ppu.fine_x = 5;
        // Reach pre-render dot 304, where horizontal and vertical scroll have transferred.
        ppu.tick(304);
        assert!(ppu.frame_scroll_valid);
        assert_eq!(ppu.frame_scroll_v, 0x7FFF);
        assert_eq!(ppu.frame_fine_x, 5);

        // Later scroll changes must not move a frame snapshot back to end-of-frame v.
        ppu.v = 0x0123;
        ppu.fine_x = 1;
        ppu.render_background(None);
        assert_eq!(ppu.frame_scroll_v, 0x7FFF);
        assert_eq!(ppu.frame_fine_x, 5);
    }

    #[test]
    fn frame_snapshot_reads_pixels_from_latched_nametable_scroll() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[6] = 0x08; // Four-screen nametables keep nametable 0 and 1 independent.
        let mut cartridge = Cartridge::parse(&image).unwrap(); // CHR-RAM
        for row in 0..8u16 {
            cartridge.ppu_write(16 + row, 0xFF);
            cartridge.ppu_write(16 + row + 8, 0x00);
        }

        let mut ppu = Ppu::new();
        ppu.mask = 0x0A; // Background enabled, including the leftmost eight pixels.
        ppu.nametables[0x400] = 1; // Nametable 1 points at opaque tile 1.
        ppu.palette[1] = 0x21;
        ppu.frame_scroll_v = 0x0400; // The frame began in horizontal nametable 1.
        ppu.frame_fine_x = 0;
        ppu.frame_scroll_valid = true;
        ppu.v = 0; // Live v later advanced into nametable 0; it must not override the latch.

        ppu.render_background(Some(&cartridge));
        assert_eq!(ppu.framebuffer[0], 0x21);
    }

    #[test]
    fn rendering_pipeline_increments_coarse_x_and_wraps_nametable() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x08;
        ppu.v = 0x001F;
        ppu.tick(8);
        assert_eq!(ppu.v & 0x041F, 0x0400, "coarse X wraps and toggles horizontal nametable bit");
    }

    #[test]
    fn rendering_pipeline_fetches_next_tiles_at_dots_328_and_336() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x08;
        ppu.v = 0x0000;
        ppu.scanline = 0;
        ppu.dot = 327;
        ppu.tick(1);
        assert_eq!(ppu.v & 0x001F, 1);
        ppu.dot = 335;
        ppu.v = 0x0001;
        ppu.tick(1);
        assert_eq!(ppu.v & 0x001F, 2);
    }

    #[test]
    fn sprite_overflow_is_recomputed_instead_of_sticking_between_snapshots() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x10;
        for sprite in 0..9usize {
            ppu.oam[sprite * 4] = 255;
        }
        ppu.render_frame(None);
        assert_ne!(ppu.status & 0x20, 0);
        for sprite in 0..64usize {
            ppu.oam[sprite * 4] = 239; // starts below visible scanlines
        }
        ppu.render_frame(None);
        assert_eq!(ppu.status & 0x20, 0);
    }

    #[test]
    fn rendering_pipeline_increments_y_at_dot_256() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x08;
        ppu.v = 0x7000 | (4 << 5);
        ppu.scanline = 0;
        ppu.dot = 255;
        ppu.tick(1);
        assert_eq!(ppu.v & 0x7000, 0);
        assert_eq!((ppu.v >> 5) & 0x1F, 5);
    }

    #[test]
    fn rendering_pipeline_copies_horizontal_and_prerender_vertical_scroll() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x08;
        ppu.t = 0x7BE0 | 0x041F;
        ppu.v = 0;
        ppu.scanline = 0;
        ppu.dot = 256;
        ppu.tick(1);
        assert_eq!(ppu.v & 0x041F, ppu.t & 0x041F);

        ppu.v = 0;
        ppu.scanline = 261;
        ppu.dot = 279;
        ppu.tick(1);
        assert_eq!(ppu.v & 0x7BE0, ppu.t & 0x7BE0);
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
    fn palette_mirroring_covers_all_universal_background_aliases() {
        let mut ppu = Ppu::new();
        for (alias, base) in [(0x3F10, 0x3F00), (0x3F14, 0x3F04), (0x3F18, 0x3F08), (0x3F1C, 0x3F0C)] {
            ppu.mem_write(alias, 0x2A, None);
            assert_eq!(ppu.mem_read(base, None), 0x2A, "alias {alias:#06X} must target {base:#06X}");
            ppu.mem_write(base, 0x15, None);
            assert_eq!(ppu.mem_read(alias, None), 0x15, "base {base:#06X} must be visible at {alias:#06X}");
        }
    }

    #[test]
    fn nametable_mirroring_maps_each_board_layout() {
        // The four logical nametables map to physical CIRAM/extra RAM tables.
        let horizontal = [0x000, 0x000, 0x400, 0x400];
        let vertical = [0x000, 0x400, 0x000, 0x400];
        let four_screen = [0x000, 0x400, 0x800, 0xC00];

        for table in 0..4u16 {
            let address = 0x2000 + table * 0x400 + 0x23;
            assert_eq!(
                Ppu::nametable_index(address, Mirroring::Horizontal),
                horizontal[table as usize] + 0x23
            );
            assert_eq!(
                Ppu::nametable_index(address, Mirroring::Vertical),
                vertical[table as usize] + 0x23
            );
            assert_eq!(
                Ppu::nametable_index(address, Mirroring::FourScreen),
                four_screen[table as usize] + 0x23
            );
        }
        // $3000-$3EFF mirrors $2000-$2EFF regardless of board mirroring.
        assert_eq!(
            Ppu::nametable_index(0x3000 + 0x123, Mirroring::Vertical),
            Ppu::nametable_index(0x2000 + 0x123, Mirroring::Vertical)
        );
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
    fn nmi_is_edge_triggered_and_control_toggles_create_only_new_edges() {
        let mut ppu = Ppu::new();
        ppu.tick(243 * 341 + 1);
        ppu.cpu_write_register(0, 0x80, None);
        assert!(ppu.take_nmi_request());
        ppu.cpu_write_register(0, 0x80, None);
        assert!(!ppu.take_nmi_request(), "writing an already-high NMI line must not retrigger it");
        ppu.cpu_write_register(0, 0x00, None);
        ppu.cpu_write_register(0, 0x80, None);
        assert!(ppu.take_nmi_request(), "re-enabling NMI during vblank creates a new rising edge");
        let _ = ppu.cpu_read_register(2, None);
        ppu.cpu_write_register(0, 0x80, None);
        assert!(!ppu.take_nmi_request(), "cleared vblank keeps the NMI line low");
    }

    #[test]
    fn background_renderer_decodes_pattern_bits_and_palette() {
        let mut ppu = Ppu::new();
        // Use CHR-RAM so the test can construct a tiny tile.
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
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
    fn background_left_edge_mask_hides_only_the_first_eight_pixels() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        ppu.mask = 0x08; // Background on, background's leftmost eight pixels hidden.
        ppu.mem_write(0x0000, 0xFF, Some(&mut cartridge));
        ppu.mem_write(0x3F00, 0x0F, Some(&mut cartridge));
        ppu.mem_write(0x3F01, 0x21, Some(&mut cartridge));
        ppu.render_background(Some(&cartridge));
        assert_eq!(ppu.framebuffer()[0], 0x0F);
        assert_eq!(ppu.framebuffer()[7], 0x0F);
        assert_eq!(ppu.framebuffer()[8], 0x21);
        assert!(ppu.background_opaque[8]);
        assert!(!ppu.background_opaque[0]);
    }

    #[test]
    fn grayscale_mask_restricts_background_and_sprite_palette_indices() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
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
    fn scanline_renderer_uses_scroll_latched_at_line_start() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.mask = 0x0A; // Background on, leftmost eight pixels visible.
        ppu.v = 0x1000; // Fine Y = 1 at the start of this line.
        ppu.mem_write(0x0001, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x3F01, 0x21, Some(&mut cartridge));

        ppu.tick_with_cartridge(256, Some(&cartridge));

        assert_eq!(ppu.framebuffer()[0], 0x21);
        assert!(ppu.background_opaque[0]);
        assert_eq!(ppu.line_scroll_v & 0x7000, 0x1000);
    }

    #[test]
    fn background_pixels_keep_the_palette_seen_at_their_beam_dot() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.mask = 0x0A;
        ppu.mem_write(0x0000, 0xFF, Some(&mut cartridge));
        ppu.palette[1] = 0x21;

        ppu.tick_with_cartridge(4, Some(&cartridge));
        ppu.palette[1] = 0x22;
        ppu.tick_with_cartridge(252, Some(&cartridge));

        assert!(ppu.framebuffer()[0..4].iter().all(|&pixel| pixel == 0x21));
        assert_eq!(ppu.framebuffer()[4], 0x22);
    }

    #[test]
    fn sprite_pixels_keep_the_palette_seen_at_each_beam_dot() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        cartridge.ppu_write(16, 0xFF); // Sprite tile 1: opaque first row.
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.mask = 0x14; // Sprites enabled, including the leftmost eight pixels.
        ppu.oam[0..4].copy_from_slice(&[255, 1, 0, 0]);
        ppu.palette[0x11] = 0x21;

        ppu.tick_with_cartridge(4, Some(&cartridge));
        ppu.palette[0x11] = 0x22;
        ppu.tick_with_cartridge(4, Some(&cartridge));

        assert_eq!(&ppu.framebuffer()[0..4], &[0x21; 4]);
        assert_eq!(&ppu.framebuffer()[4..8], &[0x22; 4]);
    }

    #[test]
    fn first_opaque_sprite_keeps_priority_even_when_behind_background() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        cartridge.ppu_write(0, 0xFF); // Opaque background tile 0.
        cartridge.ppu_write(16, 0xFF); // Sprite tile 1.
        cartridge.ppu_write(32, 0xFF); // Sprite tile 2.
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.mask = 0x1E; // Background/sprites and both left-edge enables.
        ppu.oam[0..8].copy_from_slice(&[255, 1, 0x20, 0, 255, 2, 0, 0]);
        ppu.palette[1] = 0x09; // Visible background color.
        ppu.palette[0x11] = 0x21;
        ppu.palette[0x12] = 0x22;

        ppu.tick_with_cartridge(1, Some(&cartridge));

        assert_eq!(ppu.framebuffer()[0], 0x09, "sprite 1 must not show through the higher-priority sprite 0");
    }

    #[test]
    fn sprite_zero_hit_tracks_the_emitted_background_pixel_dot_by_dot() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        // Background tile 0 and sprite tile 1 are opaque on their first row.
        cartridge.ppu_write(0, 0xFF);
        cartridge.ppu_write(8, 0x00);
        cartridge.ppu_write(16, 0xFF);
        cartridge.ppu_write(24, 0x00);

        let mut ppu = Ppu::new();
        ppu.scanline = 1;
        ppu.dot = 0;
        ppu.mask = 0x1E; // Background, sprites, and both left-edge enables.
        ppu.oam[0..4].copy_from_slice(&[0, 1, 0, 8]); // Sprite zero begins at (8, 1).

        ppu.tick_with_cartridge(8, Some(&cartridge));
        assert_eq!(ppu.status & 0x40, 0, "hit must not be raised before sprite/background overlap");
        ppu.tick_with_cartridge(1, Some(&cartridge));
        assert_ne!(ppu.status & 0x40, 0, "hit must be visible on the overlap dot");
    }

    #[test]
    fn fine_x_scroll_write_affects_only_pixels_not_yet_emitted() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.mask = 0x0A;
        ppu.mem_write(0x0000, 0xC0, Some(&mut cartridge));
        ppu.palette[1] = 0x21;

        ppu.tick_with_cartridge(1, Some(&cartridge));
        ppu.fine_x = 1;
        ppu.tick_with_cartridge(255, Some(&cartridge));

        assert_eq!(ppu.framebuffer()[0], 0x21, "the first pixel keeps the old fine-X alignment");
        assert!(!ppu.background_opaque[1], "later pixels use the updated fine-X alignment");
    }

    #[test]
    fn scanline_renderer_tracks_vertical_scroll_instead_of_reusing_one_tile_row() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.mask = 0x0A;
        ppu.mem_write(0x0000, 0x80, Some(&mut cartridge)); // Row 0: pixel at x=0.
        ppu.mem_write(0x0001, 0x40, Some(&mut cartridge)); // Row 1: pixel at x=1.
        ppu.mem_write(0x3F01, 0x21, Some(&mut cartridge));

        ppu.tick_with_cartridge(256, Some(&cartridge));
        assert_eq!(ppu.framebuffer()[0], 0x21);
        ppu.tick_with_cartridge(85, Some(&cartridge)); // Enter visible scanline 1.
        ppu.tick_with_cartridge(256, Some(&cartridge));

        assert_eq!(ppu.framebuffer()[256], 0x00, "row 1 must not reuse row 0's pattern");
        assert_eq!(ppu.framebuffer()[257], 0x21, "row 1 must use the incremented fine-Y address");
    }

    #[test]
    fn sprite_zero_hit_becomes_visible_during_scanline_not_only_at_frame_end() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        ppu.mask = 0x1E;
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.frame_scroll_valid = true;
        ppu.frame_scroll_v = 0;
        ppu.frame_fine_x = 0;
        ppu.mem_write(0x0000, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[255, 1, 0, 0]);
        assert_eq!(ppu.status & 0x40, 0);
        ppu.tick_with_cartridge(1, Some(&cartridge));
        assert_ne!(ppu.status & 0x40, 0, "hit must become visible at the overlapping pixel");
        ppu.tick_with_cartridge(255, Some(&cartridge));
        assert_ne!(ppu.status & 0x40, 0, "hit remains latched until software clears it");
    }

    #[test]
    fn sprite_renderer_composites_sprite_pixel_and_sets_zero_hit() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
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
    fn sprite_zero_hit_is_raised_at_the_overlapping_beam_pixel() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();

        ppu.mask = 0x1E; // Background/sprites and both left-edge pixels enabled.
        ppu.mem_write(0x0000, 0x80, Some(&mut cartridge)); // BG tile 0, pixel x=0.
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge)); // Sprite tile 1, pixel x=0.
        ppu.oam[0..4].copy_from_slice(&[255, 1, 0, 0]); // Sprite 0 begins at screen (0, 0).

        // Leave the pre-render line. The hit must not be delayed until dot 256.
        ppu.tick_with_cartridge(341, Some(&cartridge));
        assert_eq!(ppu.position(), (0, 0));
        assert_eq!(ppu.status & 0x40, 0);
        ppu.tick_with_cartridge(1, Some(&cartridge));
        assert_ne!(ppu.status & 0x40, 0, "sprite-zero hit should occur at dot 1 for x=0");
    }

    #[test]
    fn sprite_zero_hit_requires_visible_background_and_never_occurs_at_x_255() {
        // Sprite 0 hit is a background/sprite opaque-pixel overlap, not merely
        // a sprite pixel being drawn. Keep the test fixture local to our PPU.
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();

        let mut background_disabled = Ppu::new();
        background_disabled.mask = 0x14; // Sprites enabled, background disabled.
        background_disabled.mem_write(0x0000, 0x80, Some(&mut cartridge));
        background_disabled.mem_write(0x0010, 0x80, Some(&mut cartridge));
        background_disabled.oam[0..4].copy_from_slice(&[255, 1, 0, 0]);
        background_disabled.render_frame(Some(&cartridge));
        assert_eq!(background_disabled.status & 0x40, 0);

        let mut rightmost_pixel = Ppu::new();
        rightmost_pixel.mask = 0x1E;
        rightmost_pixel.mem_write(0x0000, 0x01, Some(&mut cartridge));
        rightmost_pixel.mem_write(0x0010, 0x01, Some(&mut cartridge));
        rightmost_pixel.mem_write(0x3F01, 0x22, Some(&mut cartridge));
        rightmost_pixel.mem_write(0x3F11, 0x30, Some(&mut cartridge));
        // OAM X=248 puts the pattern's rightmost set bit at screen x=255.
        rightmost_pixel.oam[0..4].copy_from_slice(&[255, 1, 0, 248]);
        rightmost_pixel.render_frame(Some(&cartridge));
        assert_eq!(rightmost_pixel.status & 0x40, 0);
    }

    #[test]
    fn sprite_8x16_mode_uses_tile_bit_for_pattern_table_and_next_tile_for_lower_half() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();

        ppu.ctrl = 0x20; // 8x16 sprites: tile bit 0 selects the pattern table.
        ppu.mask = 0x14;
        // Tile 3 selects table $1000 and its even partner tile 2 for the upper half.
        ppu.mem_write(0x1020, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x1030, 0x80, Some(&mut cartridge)); // Lower half uses the next tile.
        ppu.mem_write(0x3F11, 0x2A, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[255, 3, 0, 16]);

        ppu.render_frame(Some(&cartridge));

        assert_eq!(ppu.framebuffer()[16], 0x2A);
        assert_eq!(ppu.framebuffer()[16 + 8 * 256], 0x2A);
    }

    #[test]
    fn sprite_horizontal_flip_reverses_pattern_bits() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();

        ppu.mask = 0x14;
        ppu.mem_write(0x0010, 0x01, Some(&mut cartridge)); // Only the source rightmost pixel is opaque.
        ppu.mem_write(0x3F00, 0x0F, Some(&mut cartridge));
        ppu.mem_write(0x3F11, 0x2C, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[255, 1, 0x40, 16]);

        ppu.render_frame(Some(&cartridge));

        assert_eq!(ppu.framebuffer()[16], 0x2C, "horizontal flip moves the source rightmost pixel to the left");
        assert_eq!(ppu.framebuffer()[23], 0x0F, "the opposite output edge remains transparent");
    }

    #[test]
    fn sprite_vertical_flip_reads_the_opposite_pattern_row() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();

        ppu.mask = 0x14;
        ppu.mem_write(0x3F00, 0x0F, Some(&mut cartridge));
        // Only row 7 has an opaque leftmost pixel; vertical flip moves it to output row 0.
        ppu.mem_write(0x0017, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x3F11, 0x2B, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[255, 1, 0x80, 16]);

        ppu.render_frame(Some(&cartridge));

        assert_eq!(ppu.framebuffer()[16], 0x2B);
        assert_eq!(ppu.framebuffer()[16 + 256], 0x0F);
    }

    #[test]
    fn background_left_clip_does_not_hide_sprites_when_sprite_left_clip_is_enabled() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        // Background is enabled but clipped on the left; sprites are enabled and visible there.
        ppu.mask = 0x1C;
        ppu.mem_write(0x0000, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x3F00, 0x0F, Some(&mut cartridge));
        ppu.mem_write(0x3F01, 0x21, Some(&mut cartridge));
        ppu.mem_write(0x3F11, 0x30, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[255, 1, 0, 0]);

        ppu.render_frame(Some(&cartridge));

        assert_eq!(ppu.framebuffer()[0], 0x30, "sprite pixel must remain visible in the leftmost eight pixels");
        assert_eq!(ppu.framebuffer()[1], 0x0F, "transparent sprite pixels show the clipped background color");
        assert_eq!(ppu.status & 0x40, 0, "sprite-zero hit is suppressed where the background is clipped");
    }

    #[test]
    fn sprite_priority_preserves_lower_oam_index() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();

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
    fn sprite_overflow_is_not_evaluated_when_rendering_is_disabled() {
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.mask = 0;
        for sprite in 0..9usize {
            ppu.oam[sprite * 4] = 255; // All nine sprites cover scanline zero.
        }
        ppu.tick(256);
        assert_eq!(ppu.status & 0x20, 0);
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
        let mut cartridge = Cartridge::parse(&image).unwrap();
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
    fn tick_reports_each_scanline_boundary_even_across_frame_wrap() {
        let mut ppu = Ppu::new();
        ppu.scanline = 260;
        ppu.dot = 330;
        assert_eq!(ppu.tick(341 * 3), 3);
        assert_eq!(ppu.position(), (1, 330));
    }

    #[test]
    fn odd_frame_skip_counts_as_a_scanline_boundary() {
        let mut ppu = Ppu::new();
        ppu.scanline = 261;
        ppu.dot = 339;
        ppu.odd_frame = true;
        ppu.mask = 0x08;
        assert_eq!(ppu.tick(1), 1);
        assert_eq!(ppu.position(), (0, 0));
    }


    
    #[test]
    fn vblank_status_read_on_pre_vblank_dot_suppresses_flag_and_nmi() {
        let mut ppu = Ppu::new();
        ppu.scanline = 241;
        ppu.dot = 0;
        ppu.cpu_write_register(0, 0x80, None);
        let _ = ppu.cpu_read_register(2, None);
        ppu.tick(1);
        assert_eq!(ppu.status & 0x80, 0, "reading status at scanline 241 dot 0 suppresses vblank");
        assert!(!ppu.take_nmi_request(), "suppressed vblank must not raise NMI");
    }

    #[test]
    fn vblank_status_read_after_flag_set_clears_vblank_and_nmi_line() {
        let mut ppu = Ppu::new();
        ppu.scanline = 241;
        ppu.dot = 0;
        ppu.cpu_write_register(0, 0x80, None);
        ppu.tick(1);
        assert_ne!(ppu.status & 0x80, 0, "vblank is set at scanline 241 dot 1 in this timing model");
        let _ = ppu.cpu_read_register(2, None);
        assert_eq!(ppu.status & 0x80, 0);
        assert!(!ppu.nmi_line);
    }

    #[test]
    fn rendering_disabled_odd_frame_has_no_shortened_prerender_line() {
        let mut ppu = Ppu::new();
        ppu.scanline = 261;
        ppu.dot = 339;
        ppu.odd_frame = true;
        ppu.mask = 0;
        ppu.tick(1);
        assert_eq!(ppu.position(), (261, 340));
        assert!(ppu.odd_frame, "frame parity changes only when the pre-render line wraps");
    }

    #[test]
    fn ppudata_increment_wraps_only_the_fifteen_bit_internal_address() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x04;
        ppu.v = 0x7FF0;
        ppu.increment_vram_address();
        assert_eq!(ppu.v, 0x0010, "increment-by-32 wraps the internal 15-bit VRAM address");
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

    #[test]
    fn ppudata_increment_crosses_palette_boundary_without_wrapping_internal_v_early() {
        let mut ppu = Ppu::new();
        ppu.cpu_write_register(6, 0x3F, None);
        ppu.cpu_write_register(6, 0xFF, None);
        ppu.cpu_write_register(7, 0x2A, None);
        assert_eq!(ppu.v, 0x4000, "v is a 15-bit internal address; CPU memory access masks to 14 bits");
        ppu.cpu_write_register(6, 0x20, None);
        ppu.cpu_write_register(6, 0x00, None);
        ppu.cpu_write_register(7, 0x55, None);
        assert_eq!(ppu.mem_read(0x2000, None), 0x55);
    }

    #[test]
    fn status_read_on_vblank_boundary_suppresses_flag_and_nmi() {
        let mut ppu = Ppu::new();
        ppu.scanline = 241;
        ppu.dot = 0;
        ppu.ctrl = 0x80;

        let status = ppu.cpu_read_register(2, None);
        assert_eq!(status & 0x80, 0);
        ppu.tick(1);

        assert_eq!(ppu.position(), (241, 1));
        assert_eq!(ppu.status & 0x80, 0);
        assert!(!ppu.take_nmi_request());
        assert!(!ppu.suppress_vblank);
    }

    #[test]
    fn vblank_still_sets_flag_and_nmi_when_status_is_not_read_early() {
        let mut ppu = Ppu::new();
        ppu.scanline = 241;
        ppu.dot = 0;
        ppu.ctrl = 0x80;

        ppu.tick(1);

        assert_ne!(ppu.status & 0x80, 0);
        assert!(ppu.take_nmi_request());
    }

    #[test]
    fn vblank_flag_clears_at_prerender_start_and_does_not_stick_between_frames() {
        let mut ppu = Ppu::new();
        ppu.tick(242 * 341 + 1);
        assert_eq!(ppu.position(), (241, 1));
        assert_ne!(ppu.status & 0x80, 0, "vblank starts at scanline 241 dot 1");
        // Advance from scanline 241 dot 1 to pre-render scanline 261 dot 1.
        ppu.tick(20 * 341);
        assert_eq!(ppu.position(), (261, 1));
        assert_eq!(ppu.status & 0x80, 0, "pre-render dot 1 clears vblank");
        ppu.tick(340);
        assert_eq!(ppu.position(), (0, 0));
        assert_eq!(ppu.status & 0x80, 0);
    }

    #[test]
    fn frame_scroll_latches_even_when_rendering_is_disabled_during_prerender() {
        let mut ppu = Ppu::new();
        ppu.scanline = 261;
        ppu.dot = 303;
        ppu.mask = 0;
        ppu.frame_scroll_valid = true;
        ppu.frame_scroll_v = 0x0412;
        ppu.frame_fine_x = 1;
        ppu.v = 0x2C35;
        ppu.fine_x = 6;

        ppu.tick(1);

        assert_eq!(ppu.frame_scroll_v, 0x2C35);
        assert_eq!(ppu.frame_fine_x, 6);
        assert!(ppu.frame_scroll_valid);
    }

    #[test]
    fn palette_addresses_mirror_universal_background_entries() {
        for (address, expected) in [
            (0x3F00, 0x00),
            (0x3F10, 0x00),
            (0x3F14, 0x04),
            (0x3F18, 0x08),
            (0x3F1C, 0x0C),
            (0x3F20, 0x00),
        ] {
            assert_eq!(Ppu::palette_index(address), expected);
        }
    }

    #[test]
    fn nametable_address_range_above_3000_mirrors_the_first_three_kib() {
        for mirroring in [Mirroring::Horizontal, Mirroring::Vertical, Mirroring::OneScreenLower, Mirroring::OneScreenUpper] {
            for offset in [0x000u16, 0x123, 0x3FF] {
                assert_eq!(
                    Ppu::nametable_index(0x3000 + offset, mirroring),
                    Ppu::nametable_index(0x2000 + offset, mirroring)
                );
            }
        }
    }

    #[test]
    fn grayscale_mask_limits_palette_output_to_two_high_bits() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x01;
        assert_eq!(ppu.output_color(0x2F), 0x20);
        assert_eq!(ppu.output_color(0x3C), 0x30);
        ppu.mask = 0;
        assert_eq!(ppu.output_color(0x2F), 0x2F);
    }

    #[test]
    fn snapshot_scroll_uses_hardware_coarse_y_rollover_rules() {
        assert_eq!(Ppu::scroll_coarse_y(29, 0, 1), (0, 1));
        assert_eq!(Ppu::scroll_coarse_y(31, 0, 1), (0, 0));
        assert_eq!(Ppu::scroll_coarse_y(30, 0, 1), (31, 0));
        assert_eq!(Ppu::scroll_coarse_y(30, 0, 2), (0, 0));
        assert_eq!(Ppu::scroll_coarse_y(28, 1, 2), (0, 0));
    }

}
