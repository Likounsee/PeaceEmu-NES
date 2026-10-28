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
    /// Secondary-OAM snapshot used by the live scanline renderer.
    sprite_line_oam: [[u8; 4]; 8],
    sprite_line_indices: [u8; 8],
    sprite_line_count: u8,
    sprite_line: u16,
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
    /// Screen X at which the current line's latched scroll address became active.
    line_scroll_start_x: usize,
    nmi_pending: bool,
    nmi_line: bool,
    suppress_vblank: bool,
    /// Completed 256x240 background image, stored as 6-bit NES palette indices.
    framebuffer: Vec<u8>,
    background_opaque: Vec<bool>,
    ppu_cycle: u64,
    mapper_a12_high: bool,
    mapper_bus_initialized: bool,
    mapper_bus_events: Vec<(u16, u64)>,
}

impl Default for Ppu {
    fn default() -> Self {
        Self {
            ctrl: 0, mask: 0, status: 0, oam_addr: 0, oam: [0; 256],
            sprite_line_oam: [[0; 4]; 8], sprite_line_indices: [0; 8],
            sprite_line_count: 0, sprite_line: u16::MAX,
            nametables: [0; 4096], palette: [0; 32], v: 0, t: 0,
            fine_x: 0, write_latch: false, read_buffer: 0, open_bus: 0,
            scanline: 261, dot: 0, odd_frame: false, frame_scroll_v: 0, frame_fine_x: 0,
            frame_scroll_valid: false, line_scroll_v: 0, line_scroll_start_x: 0,
            nmi_pending: false, nmi_line: false, suppress_vblank: false,
            framebuffer: vec![0; 256 * 240], background_opaque: vec![false; 256 * 240],
            ppu_cycle: 0, mapper_a12_high: false, mapper_bus_initialized: false, mapper_bus_events: Vec::new(),
        }
    }
}

/// Converts the OAM Y byte to the first visible scanline. The NES treats
/// $FF as scanline 0 because sprite Y is offset by one with 8-bit wraparound.
#[inline]
fn sprite_top(oam_y: u8) -> usize {
    oam_y.wrapping_add(1) as usize
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
                self.sync_visible_scroll_after_vram_access();
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
                    // This renderer samples fine-X directly for each emitted pixel.
                    // Anchor a visible mid-scanline change at the beam so it cannot
                    // shift pixels that have already been produced on this line.
                    if self.scanline < 240
                        && (1..=256).contains(&self.dot)
                        && self.mask & 0x08 != 0
                    {
                        self.line_scroll_start_x = self.dot as usize;
                    }
                } else {
                    self.t = (self.t & !0x73E0) | (((value as u16) & 0xF8) << 2) | (((value as u16) & 7) << 12);
                }
                self.write_latch = !self.write_latch;
            }
            6 => {
                if !self.write_latch { self.t = (self.t & 0x00FF) | (((value as u16) & 0x3F) << 8); }
                else {
                    self.t = (self.t & 0x7F00) | value as u16;
                    self.v = self.t;
                    // A mid-scanline $2006 write changes the address used by
                    // subsequent background pixels. Keep the current scanline's
                    // sampling origin in step with the beam instead of applying
                    // the new address retroactively to pixels already emitted.
                    if self.scanline < 240 && (1..=256).contains(&self.dot) && self.mask & 0x18 != 0 {
                        self.line_scroll_v = self.v;
                        self.line_scroll_start_x = self.dot as usize;
                    }
                }
                self.write_latch = !self.write_latch;
            }
            7 => {
                self.mem_write(self.v & 0x3FFF, value, cartridge);
                self.increment_vram_address();
                self.sync_visible_scroll_after_vram_access();
            }
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
            self.ppu_cycle = self.ppu_cycle.wrapping_add(1);
            // NTSC skips pre-render dot 340 on odd frames while rendering is active.
            if self.scanline == 261 && self.dot == 339 && self.odd_frame && self.mask & 0x18 != 0 {
                self.dot = 0;
                self.scanline = 0;
                self.odd_frame = false;
                crossed_scanlines += 1;
                continue;
            }
            self.dot += 1;
            self.update_mapper_a12_bus(cartridge);

            // Latch the scroll address at the start of each visible scanline. The
            // live v register is changed by coarse-X fetches and the dot-256 Y increment,
            // so using it at the end of the line would shift the row being rendered.
            if self.dot == 1 && self.scanline < 240 {
                self.sprite_line = u16::MAX;
                self.line_scroll_v = self.v;
                self.line_scroll_start_x = 0;
            }

            // Emit the background pixel before testing sprite zero, so hit detection
            // uses the exact opacity/alignment of the pixel visible at this beam dot.
            // Re-decoding the background independently here can disagree with the renderer.
            if (1..=256).contains(&self.dot) && self.scanline < 240 {
                let x = (self.dot - 1) as usize;
                let y = self.scanline as usize;
                self.evaluate_sprite_line_if_needed(y);
                self.render_background_pixel(x, y, cartridge);
                if self.status & 0x40 == 0 && self.sprite_zero_hit_at(x, y, cartridge) {
                    self.status |= 0x40;
                }
                // Composite sprites at the beam position rather than painting the
                // entire row at dot 256. Mid-scanline OAM, CHR-RAM and palette writes
                // can then affect only pixels the PPU has not emitted yet.
                self.render_sprite_pixel(x, y, cartridge);
            }
            if self.dot == 256 && (self.scanline < 240 || self.scanline == 261) && self.mask & 0x18 != 0 {
                // The pre-render scanline evaluates sprites for visible scanline zero.
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

    /// Tracks mapper-visible A12 transitions at scheduled pattern-fetch slots.
    fn update_mapper_a12_bus(&mut self, cartridge: Option<&Cartridge>) {
        // Publish the initial bus level once. Without this baseline event, an MMC3
        // mapper first seeing A12 high cannot measure how long the bus was low.
        if !self.mapper_bus_initialized {
            self.mapper_bus_events.push((0, self.ppu_cycle.saturating_sub(1)));
            self.mapper_bus_initialized = true;
        }
        if self.mask & 0x18 == 0 || !(self.scanline < 240 || self.scanline == 261) {
            if self.mapper_a12_high {
                self.mapper_a12_high = false;
                self.mapper_bus_events.push((0, self.ppu_cycle));
            }
            return;
        }
        let dot = self.dot;
        let mut address = 0u16;
        if (1..=256).contains(&dot) || (321..=336).contains(&dot) {
            let phase = (dot - 1) & 7;
            if (4..=7).contains(&phase) {
                // The fetched tile number also contributes to CHR A12.
                let tile_address = 0x2000 | (self.v & 0x0FFF);
                let tile = self.mem_read(tile_address, cartridge) as u16;
                let fine_y = (self.v >> 12) & 7;
                let pattern_base = if self.ctrl & 0x10 != 0 { 0x1000 } else { 0 };
                address = pattern_base + tile * 16 + fine_y;
            }
        } else if (257..=320).contains(&dot) {
            let phase = (dot - 257) & 7;
            if (4..=7).contains(&phase) {
                let sprite = ((dot - 257) / 8) as usize;
                let base = sprite * 4;
                let top = sprite_top(self.oam[base]);
                let target_y = if self.scanline == 261 { 0 } else { self.scanline as usize + 1 };
                let row = target_y.checked_sub(top).unwrap_or(usize::MAX);
                let height = if self.ctrl & 0x20 != 0 { 16 } else { 8 };
                if row < height {
                    address = self.sprite_pattern_address(self.oam[base + 1] as usize, self.oam[base + 2], row);
                } else if self.ctrl & 0x20 == 0 {
                    address = if self.ctrl & 0x08 != 0 { 0x1000 } else { 0 };
                }
            }
        }
        let high = address & 0x1000 != 0;
        if high != self.mapper_a12_high {
            self.mapper_a12_high = high;
            self.mapper_bus_events.push((if high { 0x1000 } else { 0 }, self.ppu_cycle));
        }
    }

    pub(crate) fn take_mapper_bus_events(&mut self) -> Vec<(u16, u64)> {
        core::mem::take(&mut self.mapper_bus_events)
    }

    /// Resolves a sprite row to its low-plane CHR address.
    ///
    /// In 8x16 mode tile bit 0 selects the pattern table and the even tile
    /// starts the pair; vertical flipping reverses the whole 16-pixel sprite.
    /// Keeping this calculation shared prevents sprite-zero detection and
    /// sprite composition from disagreeing about CHR addresses.
    fn sprite_pattern_address(&self, tile_index: usize, attributes: u8, row_in_sprite: usize) -> u16 {
        let height = if self.ctrl & 0x20 != 0 { 16usize } else { 8usize };
        let source_y = if attributes & 0x80 != 0 {
            height - 1 - row_in_sprite
        } else {
            row_in_sprite
        };
        let (pattern_base, tile, row) = if height == 16 {
            (
                if tile_index & 1 != 0 { 0x1000u16 } else { 0 },
                (tile_index & 0xFE) + source_y / 8,
                source_y % 8,
            )
        } else {
            (
                if self.ctrl & 0x08 != 0 { 0x1000u16 } else { 0 },
                tile_index,
                source_y,
            )
        };
        pattern_base + (tile * 16 + row) as u16
    }

    /// Samples one sprite pixel using the same flip/address rules for composition and hit tests.
    ///
    /// `screen_x` is the pixel offset from the sprite's left edge (0..8); horizontal
    /// and vertical flips are applied here so sprite-zero detection cannot drift from
    /// the sprite image rendered at the same beam coordinate.
    fn sprite_pixel(
        &self,
        tile_index: usize,
        attributes: u8,
        row_in_sprite: usize,
        screen_x: usize,
        cartridge: Option<&Cartridge>,
    ) -> u8 {
        if screen_x >= 8 { return 0; }
        let address = self.sprite_pattern_address(tile_index, attributes, row_in_sprite);
        let bit = if attributes & 0x40 != 0 { screen_x } else { 7 - screen_x };
        ((self.mem_read(address, cartridge) >> bit) & 1)
            | (((self.mem_read(address + 8, cartridge) >> bit) & 1) << 1)
    }

    /// Checks sprite-zero/background opacity for one beam pixel.
    ///
    /// This is deliberately separate from scanline composition: games such as SMB poll
    /// the hit flag mid-scanline and immediately change scroll registers. The flag must
    /// become visible at the overlapping pixel, not after dot 256.
    fn sprite_zero_hit_at(&mut self, x: usize, y: usize, cartridge: Option<&Cartridge>) -> bool {
        self.evaluate_sprite_line_if_needed(y);
        if x >= 255 || self.mask & 0x18 != 0x18
            || (x < 8 && self.mask & 0x06 != 0x06)
        {
            return false;
        }

        if !self.sprite_line_indices[..self.sprite_line_count as usize].contains(&0) {
            return false;
        }
        let sprite = self.sprite_line_oam[0];
        let sprite_top = sprite_top(sprite[0]);
        let height = if self.ctrl & 0x20 != 0 { 16usize } else { 8usize };
        if y < sprite_top || y >= sprite_top + height {
            return false;
        }
        let sprite_left = sprite[3] as usize;
        if x < sprite_left || x >= sprite_left + 8 {
            return false;
        }

        // Background opacity is taken from the pixel already emitted at this dot.
        // This keeps sprite-zero hit in lockstep with fine-X, masking and scroll rendering.
        if !self.background_opaque[y * 256 + x] {
            return false;
        }

        let attributes = sprite[2];
        let row_in_sprite = y - sprite_top;
        self.sprite_pixel(
            sprite[1] as usize,
            attributes,
            row_in_sprite,
            x - sprite_left,
            cartridge,
        ) != 0
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
        let scroll_x = base_coarse_x * 8 + fine_x + x.saturating_sub(self.line_scroll_start_x);
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
    /// OAM bytes are latched for the line; pattern and palette memory remain live so
    /// CHR-RAM and palette writes affect only pixels not yet emitted.
    fn render_sprite_pixel(&mut self, x: usize, y: usize, cartridge: Option<&Cartridge>) {
        if x >= 256 || y >= 240 || self.mask & 0x10 == 0
            || (x < 8 && self.mask & 0x04 == 0)
        {
            return;
        }

        // Candidates and their OAM bytes are latched once per scanline. This avoids
        // rescanning all 64 entries for every pixel and prevents mid-line OAM writes
        // from retroactively changing the selected secondary-OAM sprites.
        self.evaluate_sprite_line_if_needed(y);
        for slot in 0..self.sprite_line_count as usize {
            let sprite = self.sprite_line_oam[slot];
            let top = sprite_top(sprite[0]);
            let left = sprite[3] as usize;
            if x < left || x >= left + 8 { continue; }
            let tile = sprite[1] as usize;
            let attributes = sprite[2];
            let pixel = self.sprite_pixel(tile, attributes, y.saturating_sub(top), x - left, cartridge);
            if pixel == 0 { continue; }
            // A hidden opaque sprite still blocks lower-priority sprites.
            if attributes & 0x20 == 0 || !self.background_opaque[y * 256 + x] {
                let palette_index = 0x10 + ((attributes as usize & 3) * 4) + pixel as usize;
                let color = self.palette[Self::palette_index(0x3F00 + palette_index as u16)];
                self.framebuffer[y * 256 + x] = self.output_color(color);
            }
            return;
        }
    }

    /// Latches the first eight sprites intersecting a visible scanline.
    /// The hardware evaluates them on the preceding line; this renderer snapshots
    /// on first use of the target line to keep OAM order and line contents stable.
    fn evaluate_sprite_line_if_needed(&mut self, y: usize) {
        if y >= 240 || self.sprite_line == y as u16 { return; }
        self.sprite_line = y as u16;
        self.sprite_line_count = 0;
        let height = if self.ctrl & 0x20 != 0 { 16usize } else { 8usize };
        for index in 0..64usize {
            let base = index * 4;
            let top = sprite_top(self.oam[base]);
            if y < top || y >= top + height { continue; }
            let slot = self.sprite_line_count as usize;
            if slot == 8 { break; }
            self.sprite_line_oam[slot].copy_from_slice(&self.oam[base..base + 4]);
            self.sprite_line_indices[slot] = index as u8;
            self.sprite_line_count += 1;
        }
    }
    /// Emulates the sprite-overflow scan for the following scanline.
    ///
    /// After the first eight in-range sprites, the 2C02's overflow logic does not
    /// keep checking only Y bytes: its n/m counters can walk diagonally through
    /// tile, attribute and X bytes. This intentionally reproduces the resulting
    /// false positives and false negatives instead of using a simple sprite count.
    fn update_sprite_overflow(&mut self, y: usize) {
        // Sprite evaluation at dot 256 selects sprites for the following scanline.
        let target_y = if y == 261 { 0 } else { y + 1 };
        let height = if self.ctrl & 0x20 != 0 { 16usize } else { 8usize };
        let in_range = |oam_y: u8| {
            let top = sprite_top(oam_y);
            target_y >= top && target_y < top + height
        };

        let mut found = 0usize;
        let mut next_sprite = 0usize;
        for sprite in 0..64usize {
            if in_range(self.oam[sprite * 4]) {
                found += 1;
                if found == 8 {
                    next_sprite = sprite + 1;
                    break;
                }
            }
        }
        if found < 8 || next_sprite >= 64 { return; }

        // The hardware's post-eight-sprite scan advances n and m differently
        // depending on whether the examined byte looks like an in-range Y value.
        let mut n = next_sprite;
        let mut m = 0usize;
        while n < 64 {
            if in_range(self.oam[n * 4 + m]) {
                self.status |= 0x20;
                m = (m + 1) & 3;
                if m == 0 { n += 1; }
            } else {
                n += 1;
                m = (m + 1) & 3;
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
        if self.mask & 0x18 == 0 { return; }
        let sprite_height: usize = if self.ctrl & 0x20 != 0 { 16 } else { 8 };

        // Sprite evaluation still runs when background rendering is enabled but
        // sprite pixels are masked off. Preserve overflow status in snapshot mode too.
        if self.mask & 0x10 == 0 {
            let mut rows_per_scanline = [0u8; 240];
            for sprite in 0..64usize {
                let top = sprite_top(self.oam[sprite * 4]);
                for sy in 0..sprite_height {
                    let y = top + sy;
                    if y >= 240 { continue; }
                    rows_per_scanline[y] = rows_per_scanline[y].saturating_add(1);
                    if rows_per_scanline[y] > 8 { self.status |= 0x20; }
                }
            }
            return;
        }

        let mut rows_per_scanline = [0u8; 240];
        // The first opaque sprite in OAM owns each pixel, even if hidden behind BG.
        let mut sprite_claimed = vec![false; 256 * 240];
        for sprite in 0..64usize {
            let base = sprite * 4;
            let top = sprite_top(self.oam[base]);
            let tile = self.oam[base + 1] as usize;
            let attributes = self.oam[base + 2];
            let left = self.oam[base + 3] as usize;
            let behind_background = attributes & 0x20 != 0;
            for sy in 0..sprite_height {
                let y = top + sy;
                if y >= 240 { continue; }
                rows_per_scanline[y] = rows_per_scanline[y].saturating_add(1);
                if rows_per_scanline[y] > 8 { self.status |= 0x20; continue; }
                for sx in 0..8usize {
                    let x = left + sx;
                    if x >= 256 || (x < 8 && self.mask & 0x04 == 0) { continue; }
                    let pixel = self.sprite_pixel(tile, attributes, sy, sx, cartridge);
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

    /// Keeps the approximate pixel sampler aligned when CPU PPUDATA accesses
    /// advance the live VRAM address during a visible scanline.
    fn sync_visible_scroll_after_vram_access(&mut self) {
        if self.scanline < 240 && (1..=256).contains(&self.dot) && self.mask & 0x18 != 0 {
            self.line_scroll_v = self.v;
            self.line_scroll_start_x = self.dot as usize;
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
mod mid_scanline_scroll_regressions {
    use super::*;

    #[test]
    fn second_ppu_address_write_rebases_only_the_unrendered_scanline() {
        let mut ppu = Ppu::new();
        ppu.scanline = 40;
        ppu.dot = 80;
        ppu.mask = 0x08;

        // Set VRAM address to $2100 with the two-write $2006 protocol.
        ppu.cpu_write_register(6, 0x21, None);
        ppu.cpu_write_register(6, 0x00, None);

        assert_eq!(ppu.v, 0x2100);
        assert_eq!(ppu.line_scroll_v, 0x2100,
            "the next background pixel must use the newly written VRAM address");
        assert_eq!(ppu.line_scroll_start_x, 80,
            "the new address must start at the next unrendered pixel, not pixel zero");
    }

    #[test]
    fn ppudata_increment_rebases_visible_scroll_for_future_pixels() {
        let mut ppu = Ppu::new();
        ppu.scanline = 12;
        ppu.dot = 100;
        ppu.mask = 0x08;
        ppu.v = 0x2000;

        ppu.cpu_write_register(7, 0x55, None);

        assert_eq!(ppu.v, 0x2001);
        assert_eq!(ppu.line_scroll_v, 0x2001,
            "PPUDATA increments must affect the subsequent visible fetch address");
        assert_eq!(ppu.line_scroll_start_x, 100,
            "the updated address applies from the next unrendered pixel");
    }

    #[test]
    fn ppudata_does_not_rebase_scroll_when_rendering_is_disabled() {
        let mut ppu = Ppu::new();
        ppu.scanline = 12;
        ppu.dot = 100;
        ppu.mask = 0;
        ppu.v = 0x2000;
        ppu.line_scroll_v = 0x2123;
        ppu.line_scroll_start_x = 24;

        ppu.cpu_write_register(7, 0x55, None);

        assert_eq!(ppu.v, 0x2001);
        assert_eq!(ppu.line_scroll_v, 0x2123);
        assert_eq!(ppu.line_scroll_start_x, 24);
    }

    #[test]
    fn ppudata_does_not_rebase_scroll_during_vblank() {
        let mut ppu = Ppu::new();
        ppu.scanline = 250;
        ppu.dot = 100;
        ppu.mask = 0x08;
        ppu.v = 0x2000;
        ppu.line_scroll_v = 0x2123;
        ppu.line_scroll_start_x = 24;

        ppu.cpu_write_register(7, 0x55, None);

        assert_eq!(ppu.v, 0x2001);
        assert_eq!(ppu.line_scroll_v, 0x2123);
        assert_eq!(ppu.line_scroll_start_x, 24);
    }

    #[test]
    fn new_scanline_resets_the_scroll_sampling_origin() {
        let mut ppu = Ppu::new();
        ppu.scanline = 40;
        ppu.dot = 80;
        ppu.mask = 0x08;
        ppu.line_scroll_start_x = 80;
        ppu.tick(1);

        assert_eq!(ppu.position(), (40, 81));
        assert_eq!(ppu.line_scroll_start_x, 80,
            "the origin stays fixed until the next scanline starts");
        ppu.tick(261);

        assert_eq!(ppu.position(), (41, 1));
        assert_eq!(ppu.line_scroll_start_x, 0,
            "each visible scanline begins with a fresh scroll origin");
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
    fn mapper_a12_events_follow_pattern_fetch_slots_and_use_monotonic_dot_timestamps() {
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.ctrl = 0x10; // Background pattern table at $1000.
        ppu.mask = 0x08; // Background rendering enabled.
        ppu.tick(10);
        let events = ppu.take_mapper_bus_events();
        assert_eq!(events, vec![(0, 0), (0x1000, 5), (0x0000, 9)]);
        ppu.tick(4);
        assert_eq!(ppu.take_mapper_bus_events(), vec![(0x1000, 13)]);
    }

    #[test]
    fn mapper_a12_uses_the_actual_pattern_table_address() {
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.mask = 0x08;
        ppu.ctrl = 0;
        ppu.nametables[0] = 0x80;
        ppu.tick(5);
        assert_eq!(ppu.take_mapper_bus_events(), vec![(0, 0)], "initial low baseline is published once; tile $80 must not raise A12");
        ppu.dot = 8;
        ppu.ppu_cycle = 8;
        ppu.nametables[1] = 0;
        ppu.v = 1;
        ppu.tick(1);
        assert!(ppu.take_mapper_bus_events().is_empty());
    }

    #[test]
    fn mapper_a12_is_forced_low_when_rendering_is_disabled() {
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.ctrl = 0x10;
        ppu.mask = 0x08;
        ppu.tick(5);
        assert_eq!(ppu.take_mapper_bus_events(), vec![(0, 0), (0x1000, 5)]);

        ppu.mask = 0;
        ppu.tick(1);
        assert_eq!(ppu.take_mapper_bus_events(), vec![(0x0000, 6)]);
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
            ppu.oam[sprite * 4] = 0;
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
        ppu.mem_write(0x0001, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0011, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x3F01, 0x2B, Some(&mut cartridge));
        ppu.mem_write(0x3F11, 0x17, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[0, 1, 0, 16]);
        ppu.render_frame(Some(&cartridge));
        assert_eq!(ppu.framebuffer()[256 + 16], 0x10);
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
    fn mid_scanline_fine_x_write_only_affects_pixels_not_yet_emitted() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.mask = 0x0A;
        // Every tile row has an opaque first plane, so fine-X is visible in
        // the transition from the first tile to the second tile.
        cartridge.ppu_write(0, 0x80);
        cartridge.ppu_write(16, 0xFF);
        ppu.palette[1] = 0x21;
        ppu.palette[0] = 0x0F;

        ppu.tick_with_cartridge(4, Some(&cartridge));
        let emitted = ppu.framebuffer()[0..4].to_vec();
        ppu.cpu_write_register(5, 7, Some(&mut cartridge));
        ppu.tick_with_cartridge(4, Some(&cartridge));

        assert_eq!(&ppu.framebuffer()[0..4], emitted.as_slice(),
            "a scroll write must not repaint pixels already emitted");
        assert_eq!(ppu.line_scroll_start_x, 4,
            "the new fine-X origin starts at the current beam");
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
        ppu.scanline = 1;
        ppu.dot = 0;
        ppu.mask = 0x14; // Sprites enabled, including the leftmost eight pixels.
        ppu.oam[0..4].copy_from_slice(&[0, 1, 0, 0]);
        ppu.palette[0x11] = 0x21;

        ppu.tick_with_cartridge(4, Some(&cartridge));
        ppu.palette[0x11] = 0x22;
        ppu.tick_with_cartridge(4, Some(&cartridge));

        assert_eq!(&ppu.framebuffer()[256..260], &[0x21; 4]);
        assert_eq!(&ppu.framebuffer()[260..264], &[0x22; 4]);
    }

    #[test]
    fn first_opaque_sprite_keeps_priority_even_when_behind_background() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        cartridge.ppu_write(0, 0xFF); // Opaque background tile 0.
        cartridge.ppu_write(1, 0xFF); // Row used at visible scanline one.
        cartridge.ppu_write(16, 0xFF); // Sprite tile 1.
        cartridge.ppu_write(17, 0xFF);
        cartridge.ppu_write(32, 0xFF); // Sprite tile 2.
        let mut ppu = Ppu::new();
        ppu.scanline = 1;
        ppu.dot = 0;
        ppu.mask = 0x1E; // Background/sprites and both left-edge enables.
        ppu.oam[0..8].copy_from_slice(&[0, 1, 0x20, 0, 0, 2, 0, 0]);
        ppu.palette[1] = 0x09; // Visible background color.
        ppu.palette[0x11] = 0x21;
        ppu.palette[0x12] = 0x22;

        ppu.tick_with_cartridge(1, Some(&cartridge));

        assert_eq!(ppu.framebuffer()[256], 0x09, "sprite 1 must not show through the higher-priority sprite 0");
    }

    #[test]
    fn sprite_overflow_models_diagonal_oam_byte_scan() {
        let mut ppu = Ppu::new();
        // First eight sprites overlap target scanline 10 (OAM Y=9).
        for sprite in 0..8usize {
            ppu.oam[sprite * 4] = 9;
        }
        // Sprite 8 is out of range. The buggy n/m walk skips its tile byte,
        // then interprets sprite 9's tile byte as a Y coordinate.
        ppu.oam[8 * 4] = 200;
        ppu.oam[9 * 4 + 1] = 9;
        ppu.update_sprite_overflow(9);
        assert_ne!(ppu.status & 0x20, 0,
            "the 2C02 diagonal scan can raise overflow from a non-Y OAM byte");
    }

    #[test]
    fn sprite_y_ff_counts_for_overflow_on_the_first_visible_scanline() {
        let mut ppu = Ppu::new();
        for sprite in 0..8usize {
            ppu.oam[sprite * 4] = 0;
        }
        ppu.oam[8 * 4] = 0xFF;
        ppu.update_sprite_overflow(0);
        assert_ne!(
            ppu.status & 0x20,
            0,
            "wrapped OAM Y=$FF must be evaluated on scanline zero/one like sprite rendering"
        );
    }

    #[test]
    fn sprite_overflow_stays_clear_with_eight_or_fewer_matching_sprites() {
        let mut ppu = Ppu::new();
        for sprite in 0..8usize {
            ppu.oam[sprite * 4] = 9;
        }
        ppu.oam[8 * 4] = 200;
        ppu.oam[9 * 4 + 1] = 200;
        ppu.update_sprite_overflow(9);
        assert_eq!(ppu.status & 0x20, 0);
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
        ppu.scanline = 1;
        ppu.dot = 0;
        ppu.frame_scroll_valid = true;
        ppu.frame_scroll_v = 0;
        ppu.frame_fine_x = 0;
        ppu.mem_write(0x0000, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0001, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0011, 0x80, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[0, 1, 0, 0]);
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
        // OAM Y stores the top row minus one; Y=0 starts at visible scanline one.
        ppu.mask = 0x1E;
        ppu.mem_write(0x0000, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0001, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0011, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x3F01, 0x22, Some(&mut cartridge));
        ppu.mem_write(0x3F11, 0x30, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[0, 1, 0, 0]);
        ppu.mem_write(0x2000, 0x00, Some(&mut cartridge));
        ppu.render_frame(Some(&cartridge));
        assert_ne!(ppu.status & 0x40, 0);
        assert_eq!(ppu.framebuffer()[256], 0x30);
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
        ppu.mem_write(0x0001, 0x80, Some(&mut cartridge)); // Same pixel on the next tile row.
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge)); // Sprite tile 1, pixel x=0.
        ppu.mem_write(0x0011, 0x80, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[0, 1, 0, 0]); // Sprite 0 begins at screen (0, 1).

        // Scanline zero is sprite-free; OAM Y=0 starts on scanline one.
        ppu.scanline = 1;
        ppu.dot = 0;
        assert_eq!(ppu.position(), (1, 0));
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
        background_disabled.mem_write(0x0001, 0x80, Some(&mut cartridge));
        background_disabled.mem_write(0x0010, 0x80, Some(&mut cartridge));
        background_disabled.mem_write(0x0011, 0x80, Some(&mut cartridge));
        background_disabled.oam[0..4].copy_from_slice(&[0, 1, 0, 0]);
        background_disabled.render_frame(Some(&cartridge));
        assert_eq!(background_disabled.status & 0x40, 0);

        let mut rightmost_pixel = Ppu::new();
        rightmost_pixel.mask = 0x1E;
        rightmost_pixel.mem_write(0x0000, 0x01, Some(&mut cartridge));
        rightmost_pixel.mem_write(0x0001, 0x01, Some(&mut cartridge));
        rightmost_pixel.mem_write(0x0010, 0x01, Some(&mut cartridge));
        rightmost_pixel.mem_write(0x0011, 0x01, Some(&mut cartridge));
        rightmost_pixel.mem_write(0x3F01, 0x22, Some(&mut cartridge));
        rightmost_pixel.mem_write(0x3F11, 0x30, Some(&mut cartridge));
        // OAM X=248 puts the pattern's rightmost set bit at screen x=255.
        rightmost_pixel.oam[0..4].copy_from_slice(&[0, 1, 0, 248]);
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
        ppu.oam[0..4].copy_from_slice(&[0, 3, 0, 16]);

        ppu.render_frame(Some(&cartridge));

        assert_eq!(ppu.framebuffer()[256 + 16], 0x2A);
        assert_eq!(ppu.framebuffer()[16 + 9 * 256], 0x2A);
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
        ppu.oam[0..4].copy_from_slice(&[0, 1, 0x40, 16]);

        ppu.render_frame(Some(&cartridge));

        assert_eq!(ppu.framebuffer()[256 + 16], 0x2C, "horizontal flip moves the source rightmost pixel to the left");
        assert_eq!(ppu.framebuffer()[256 + 23], 0x0F, "the opposite output edge remains transparent");
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
        ppu.oam[0..4].copy_from_slice(&[0, 1, 0x80, 16]);

        ppu.render_frame(Some(&cartridge));

        assert_eq!(ppu.framebuffer()[256 + 16], 0x2B);
        assert_eq!(ppu.framebuffer()[16 + 2 * 256], 0x0F);
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
        ppu.mem_write(0x0001, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0011, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x3F00, 0x0F, Some(&mut cartridge));
        ppu.mem_write(0x3F01, 0x21, Some(&mut cartridge));
        ppu.mem_write(0x3F11, 0x30, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[0, 1, 0, 0]);

        ppu.render_frame(Some(&cartridge));

        assert_eq!(ppu.framebuffer()[256], 0x30, "sprite pixel must remain visible in the leftmost eight pixels");
        assert_eq!(ppu.framebuffer()[257], 0x0F, "transparent sprite pixels show the clipped background color");
        assert_eq!(ppu.status & 0x40, 0, "sprite-zero hit is suppressed where the background is clipped");
    }

    #[test]
    fn sprite_priority_preserves_lower_oam_index() {
        let mut ppu = Ppu::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();

        // Two opaque sprites overlap at (0, 1), with distinct sprite palettes.
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x0020, 0x80, Some(&mut cartridge));
        ppu.palette[0x11] = 0x30;
        ppu.palette[0x15] = 0x27;
        ppu.mask = 0x14; // Show sprites and the leftmost eight sprite pixels.
        ppu.oam[0..8].copy_from_slice(&[0, 1, 0, 16, 0, 2, 1, 16]);
        assert_eq!(cartridge.ppu_read(0x10), Some(0x80));
        assert_eq!(ppu.palette[0x11], 0x30);

        ppu.render_frame(Some(&cartridge));

        assert_eq!(ppu.framebuffer()[256 + 16], 0x30);
    }

    #[test]
    fn oam_y_values_ef_through_fe_remain_below_the_visible_frame() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x14;
        ppu.palette[0x11] = 0x30;
        let mut image = vec![0; 16 + 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1;
        image[16 + 16_384] = 0x80; // Tile zero, row zero, leftmost pixel opaque.
        let cart = Cartridge::parse(&image).unwrap();

        for oam_y in 0xEF..=0xFE {
            ppu.framebuffer.fill(0);
            ppu.oam[0] = oam_y;
            ppu.oam[1] = 0;
            ppu.oam[2] = 0;
            ppu.oam[3] = 0;
            ppu.render_sprite_pixel(0, 0, Some(&cart));
            assert_ne!(
                ppu.framebuffer[0], 0x30,
                "OAM Y={oam_y:#04X} must remain below the visible frame"
            );
        }
    }

    #[test]
    fn sprite_overflow_is_not_evaluated_when_rendering_is_disabled() {
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.mask = 0;
        for sprite in 0..9usize {
            ppu.oam[sprite * 4] = 0; // All nine sprites cover scanline one.
        }
        ppu.tick(256);
        assert_eq!(ppu.status & 0x20, 0);
    }

    #[test]
    fn ninth_sprite_on_scanline_sets_overflow_flag() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x10;
        for sprite in 0..9usize {
            ppu.oam[sprite * 4] = 0; // Every sprite starts at scanline one.
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
        ppu.mem_write(0x0001, 0x80, Some(&mut cartridge)); // Row rendered at scanline one.
        ppu.mem_write(0x0010, 0x80, Some(&mut cartridge)); // Sprite tile one.
        ppu.mem_write(0x0011, 0x80, Some(&mut cartridge));
        ppu.mem_write(0x3F01, 0x22, Some(&mut cartridge));
        ppu.mem_write(0x3F11, 0x30, Some(&mut cartridge));
        ppu.oam[0..4].copy_from_slice(&[0, 1, 0x20, 16]);
        ppu.render_frame(Some(&cartridge));
        assert_eq!(ppu.framebuffer()[256 + 16], 0x22);
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
    fn enabling_nmi_during_vblank_raises_one_edge_immediately() {
        let mut ppu = Ppu::new();
        ppu.scanline = 241;
        ppu.dot = 0;
        ppu.tick(1);
        assert_ne!(ppu.status & 0x80, 0);
        assert!(!ppu.take_nmi_request(), "vblank with NMI disabled must not raise an edge");

        ppu.cpu_write_register(0, 0x80, None);
        assert!(ppu.take_nmi_request(), "enabling NMI while vblank is active creates a rising edge");
        assert!(!ppu.take_nmi_request(), "the edge is consumed exactly once");
        ppu.cpu_write_register(0, 0x80, None);
        assert!(!ppu.take_nmi_request(), "writing the same control value must not retrigger NMI");
    }

    #[test]
    fn odd_frame_skips_one_prerender_dot_only_when_rendering_is_enabled() {
        let mut ppu = Ppu::new();
        ppu.scanline = 261;
        ppu.dot = 339;
        ppu.odd_frame = true;
        ppu.mask = 0x08;
        ppu.tick(1);
        assert_eq!(ppu.position(), (0, 0));
        assert!(!ppu.odd_frame);

        ppu.scanline = 261;
        ppu.dot = 339;
        ppu.odd_frame = true;
        ppu.mask = 0;
        ppu.tick(1);
        assert_eq!(ppu.position(), (261, 340));
        assert!(ppu.odd_frame);
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
    fn palette_ram_writes_apply_six_bit_mask_and_universal_color_aliases() {
        let mut ppu = Ppu::new();
        ppu.mem_write(0x3F10, 0xFF, None);
        assert_eq!(ppu.mem_read(0x3F00, None), 0x3F);
        assert_eq!(ppu.mem_read(0x3F10, None), 0x3F);

        ppu.mem_write(0x3F24, 0x2A, None);
        assert_eq!(ppu.mem_read(0x3F04, None), 0x2A);
        assert_eq!(ppu.mem_read(0x3F24, None), 0x2A);
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


#[cfg(test)]
mod sprite_pattern_address_tests {
    use super::*;

    #[test]
    fn eight_by_sixteen_sprite_tile_bit_selects_pattern_table_and_pair() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x20; // 8x16 sprites; sprite tile bit 0 selects table.
        assert_eq!(ppu.sprite_pattern_address(0x02, 0, 0), 0x0020);
        assert_eq!(ppu.sprite_pattern_address(0x03, 0, 0), 0x1020);
        assert_eq!(ppu.sprite_pattern_address(0x03, 0, 8), 0x1030);
        assert_eq!(ppu.sprite_pattern_address(0x02, 0, 8), 0x0030);
    }

    #[test]
    fn vertical_flip_reverses_the_entire_sixteen_pixel_sprite() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x20;
        assert_eq!(ppu.sprite_pattern_address(0x03, 0x80, 0), 0x1037);
        assert_eq!(ppu.sprite_pattern_address(0x03, 0x80, 7), 0x1030);
        assert_eq!(ppu.sprite_pattern_address(0x03, 0x80, 8), 0x1027);
        assert_eq!(ppu.sprite_pattern_address(0x03, 0x80, 15), 0x1020);
    }

    #[test]
    fn eight_by_eight_sprite_table_still_comes_from_control_register() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x08;
        assert_eq!(ppu.sprite_pattern_address(5, 0, 3), 0x1053);
        ppu.ctrl = 0;
        assert_eq!(ppu.sprite_pattern_address(5, 0, 3), 0x0053);
    }

    #[test]
    fn sprite_fetch_a12_uses_tile_table_bit_in_eight_by_sixteen_mode() {
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.ctrl = 0x20;
        ppu.mask = 0x10;
        ppu.oam[1] = 1; // Odd tile selects $1000 in 8x16 mode.
        ppu.tick(261);
        assert_eq!(ppu.take_mapper_bus_events(), vec![(0, 0), (0x1000, 261)]);

        ppu.oam[1] = 0;
        ppu.dot = 264; // Next sprite's fetch window starts with A12 low.
        ppu.ppu_cycle = 264;
        ppu.tick(1);
        assert_eq!(ppu.take_mapper_bus_events(), vec![(0x0000, 265)]);
    }
}


#[cfg(test)]
mod sprite_overflow_regression_tests {
    use super::*;

    #[test]
    fn ninth_in_range_sprite_sets_overflow_and_prerender_clears_it() {
        let mut ppu = Ppu::new();
        ppu.scanline = 1;
        ppu.dot = 255;
        ppu.mask = 0x18;
        for sprite in 0..9 {
            ppu.oam[sprite * 4] = 0; // Y=$00 places each 8x8 sprite on scanline 1.
        }

        ppu.tick(1);
        assert_ne!(ppu.status & 0x20, 0, "the ninth in-range sprite must set overflow");

        ppu.scanline = 261;
        ppu.dot = 0;
        ppu.tick(1);
        assert_eq!(ppu.status & 0xE0, 0, "pre-render dot 1 clears vblank, sprite-zero hit and overflow");
    }
}


#[cfg(test)]
mod sprite_pixel_regression_tests {
    use super::*;

    #[test]
    fn sprite_zero_hit_and_composited_pixel_use_the_same_sprite_sample() {
        let mut image = vec![0; 16 + 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1;
        let chr = 16 + 16_384;
        image[chr] = 0b1000_0000;
        let cart = Cartridge::parse(&image).unwrap();

        let mut ppu = Ppu::new();
        ppu.mask = 0x1E;
        ppu.oam[0] = 0;
        ppu.oam[1] = 0;
        ppu.oam[2] = 0;
        ppu.oam[3] = 16;
        ppu.palette[0x11] = 0x2A;
        ppu.background_opaque[272] = true;

        assert!(ppu.sprite_zero_hit_at(16, 1, Some(&cart)));
        ppu.render_sprite_pixel(16, 1, Some(&cart));
        assert_eq!(ppu.framebuffer[272], ppu.output_color(0x2A));
    }

    #[test]
    fn sprite_pixel_sampler_applies_horizontal_flip_consistently() {
        let mut image = vec![0; 16 + 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1;
        let chr = 16 + 16_384;
        image[chr] = 0b1000_0000;
        let cart = Cartridge::parse(&image).unwrap();
        let ppu = Ppu::new();

        assert_eq!(ppu.sprite_pixel(0, 0, 0, 0, Some(&cart)), 1);
        assert_eq!(ppu.sprite_pixel(0, 0, 0, 7, Some(&cart)), 0);
        assert_eq!(ppu.sprite_pixel(0, 0x40, 0, 0, Some(&cart)), 0);
        assert_eq!(ppu.sprite_pixel(0, 0x40, 0, 7, Some(&cart)), 1);
    }

    #[test]
    fn sprite_pixel_sampler_flips_entire_8x16_sprite_before_selecting_tile() {
        let mut image = vec![0; 16 + 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1;
        let chr = 16 + 16_384;
        image[chr + 0x1000] = 0b1000_0000; // Odd tile selects pattern table $1000.
        image[chr + 0x17] = 0b0000_0001; // Tile 0 pair's final row (tile 1, row 7).
        let cart = Cartridge::parse(&image).unwrap();
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x20; // 8x16 sprites.
        assert_eq!(ppu.sprite_pixel(0, 0, 0, 0, Some(&cart)), 0);
        assert_eq!(ppu.sprite_pixel(0, 0x80, 0, 7, Some(&cart)), 1);
        assert_eq!(ppu.sprite_pixel(1, 0, 0, 0, Some(&cart)), 1);
    }


    #[test]
    fn sprite_overflow_is_clear_with_exactly_eight_sprites_on_target_scanline() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x18;
        ppu.oam.fill(0x80); // Out-of-range Y and data bytes for the remaining sprites.
        for sprite in 0..8 {
            ppu.oam[sprite * 4] = 0; // Y=0 covers target scanline 1.
        }

        ppu.update_sprite_overflow(0);
        assert_eq!(ppu.status & 0x20, 0);
    }

    #[test]
    fn sprite_overflow_sets_when_ninth_sprite_is_in_range() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x18;
        ppu.oam.fill(0x80);
        for sprite in 0..9 {
            ppu.oam[sprite * 4] = 0;
        }

        ppu.update_sprite_overflow(0);
        assert_ne!(ppu.status & 0x20, 0);
    }

    #[test]
    fn sprite_zero_hit_requires_an_opaque_background_pixel() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x1E;
        ppu.oam[0] = 0;
        ppu.oam[1] = 0;
        ppu.oam[3] = 16;
        ppu.background_opaque[16] = false;
        assert!(!ppu.sprite_zero_hit_at(16, 0, None));
    }

    #[test]
    fn sprite_zero_hit_obeys_both_left_edge_enable_bits() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x18; // Both layers enabled, but leftmost pixels are masked.
        ppu.oam[0] = 0;
        ppu.oam[1] = 0;
        ppu.oam[3] = 0;
        ppu.background_opaque[0] = true;
        assert!(!ppu.sprite_zero_hit_at(0, 0, None));
        ppu.mask |= 0x06;
        assert!(!ppu.sprite_zero_hit_at(0, 0, None), "an empty sprite pattern must not hit");
    }

    #[test]
    fn oam_y_ff_places_sprite_on_visible_scanline_zero() {
        assert_eq!(sprite_top(0xFF), 0);
        let mut ppu = Ppu::new();
        ppu.mask = 0x10;
        ppu.oam[0] = 0xFF;
        ppu.oam[1] = 0;
        ppu.oam[3] = 12;
        ppu.palette[0x11] = 0x2A;
        let mut image = vec![0; 16 + 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1;
        image[16 + 16_384] = 0b1000_0000;
        let cart = Cartridge::parse(&image).unwrap();
        ppu.render_sprite_pixel(12, 0, Some(&cart));
        assert_eq!(ppu.framebuffer[12], ppu.output_color(0x2A));
    }

    #[test]
    fn opaque_high_priority_sprite_blocks_lower_priority_sprite_even_behind_background() {
        let mut image = vec![0; 16 + 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1;
        let chr = 16 + 16_384;
        image[chr] = 0b1000_0000; // Tile 0: opaque at left edge.
        image[chr + 16] = 0b1000_0000; // Tile 1: opaque at left edge too.
        let cart = Cartridge::parse(&image).unwrap();
        let mut ppu = Ppu::new();
        ppu.mask = 0x14; // Sprites enabled.
        ppu.oam[0..4].copy_from_slice(&[0, 0, 0x20, 24]); // First sprite behind BG.
        ppu.oam[4..8].copy_from_slice(&[0, 1, 0, 24]); // Second sprite would otherwise show.
        ppu.background_opaque[24] = true;
        ppu.palette[0x15] = 0x12;
        ppu.palette[0x11] = 0x2A;
        ppu.framebuffer[24] = ppu.output_color(0x0F);
        ppu.render_sprite_pixel(24, 0, Some(&cart));
        assert_eq!(ppu.framebuffer[24], ppu.output_color(0x0F));
    }

    #[test]
    fn sprite_zero_hit_is_suppressed_at_x_255() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x1E;
        ppu.oam[0] = 0;
        ppu.oam[1] = 0;
        ppu.oam[2] = 0;
        ppu.oam[3] = 248;
        ppu.background_opaque[255] = true;
        ppu.nametables[0] = 1;
        ppu.palette[1] = 1;
        assert!(!ppu.sprite_zero_hit_at(255, 0, None), "the NES never sets sprite-zero hit at x=255");
    }

    #[test]
    fn left_edge_mask_hides_background_and_marks_it_transparent_for_sprite_hit() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x08; // Background enabled, leftmost eight pixels disabled.
        ppu.palette[0] = 0x2A;
        ppu.render_background_pixel(0, 0, None);
        assert_eq!(ppu.framebuffer[0], ppu.output_color(0x2A));
        assert!(!ppu.background_opaque[0]);
    }
    #[test]
    fn disabled_rendering_clears_background_opacity_for_reused_pixels() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x08;
        ppu.background_opaque[17] = true;
        ppu.mask = 0;
        ppu.render_background_pixel(17, 0, None);
        assert!(!ppu.background_opaque[17], "disabled background must not leave stale opaque pixels");
    }

}


#[cfg(test)]
mod ppu_nametable_bus_regression_tests {
    use super::*;

    fn vertical_mirroring_cartridge() -> Cartridge {
        let mut image = vec![0; 16 + 16 * 1024];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[6] = 0x01; // Vertical nametable mirroring.
        Cartridge::parse(&image).unwrap()
    }

    #[test]
    fn nametable_reads_and_writes_follow_cartridge_vertical_mirroring() {
        let mut ppu = Ppu::new();
        let mut cartridge = vertical_mirroring_cartridge();

        ppu.mem_write(0x2000, 0x5A, Some(&mut cartridge));
        assert_eq!(ppu.mem_read(0x2800, Some(&cartridge)), 0x5A);
        assert_eq!(ppu.mem_read(0x2400, Some(&cartridge)), 0);
        ppu.mem_write(0x2C00, 0xA5, Some(&mut cartridge));
        assert_eq!(ppu.mem_read(0x2400, Some(&cartridge)), 0xA5);
        assert_eq!(ppu.mem_read(0x2000, Some(&cartridge)), 0x5A);
    }

    #[test]
    fn palette_data_register_masks_written_values_and_wraps_vram_address() {
        let mut ppu = Ppu::new();
        ppu.cpu_write_register(6, 0x3F, None);
        ppu.cpu_write_register(6, 0x1F, None);
        ppu.cpu_write_register(7, 0xFF, None);

        assert_eq!(ppu.mem_read(0x3F1F, None), 0x3F);
        assert_eq!(ppu.v, 0x3F20);
        assert_eq!(ppu.mem_read(0x3F00, None), 0);
    }
}


#[cfg(test)]
mod sprite_priority_regression_tests {
    use super::*;

    #[test]
    fn first_opaque_sprite_claims_pixel_even_when_hidden_behind_background() {
        let mut image = vec![0; 16 + 16 * 1024];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut cartridge = Cartridge::parse(&image).unwrap();
        // Sprite tile 0, row 0 has its leftmost pixel set.
        assert!(cartridge.ppu_write(0, 0x80));
        // Sprite tile 1 also has a visible leftmost pixel; it must not show through
        // the higher-priority sprite 0 when sprite 0 is behind opaque background.
        assert!(cartridge.ppu_write(16, 0x80));

        let mut ppu = Ppu::new();
        ppu.mask = 0x14; // Sprites enabled; show sprites in the leftmost eight pixels.
        ppu.palette[0] = 0x01;
        ppu.palette[0x11] = 0x16;
        ppu.palette[0x12] = 0x27;
        ppu.oam[0..8].copy_from_slice(&[
            0, 0, 0x20, 0, // Sprite 0: behind opaque background.
            0, 1, 0x00, 0, // Sprite 1: foreground, lower priority.
        ]);
        ppu.background_opaque[256] = true;
        ppu.framebuffer[256] = ppu.output_color(0x01);

        ppu.render_sprite_pixel(0, 1, Some(&cartridge));

        assert_eq!(ppu.framebuffer[256], ppu.output_color(0x01),
            "the lower-priority foreground sprite must not leak through sprite 0");
    }
}


#[cfg(test)]
mod cpu_data_port_regressions {
    use super::*;

    #[test]
    fn non_palette_data_reads_are_buffered_and_advance_vram() {
        let mut ppu = Ppu::new();
        ppu.mem_write(0x2000, 0xAB, None);
        ppu.mem_write(0x2001, 0xCD, None);
        ppu.v = 0x2000;

        assert_eq!(ppu.cpu_read_register(7, None), 0x00);
        assert_eq!(ppu.v, 0x2001);
        assert_eq!(ppu.cpu_read_register(7, None), 0xAB);
        assert_eq!(ppu.v, 0x2002);
        assert_eq!(ppu.cpu_read_register(7, None), 0xCD);
    }

    #[test]
    fn palette_data_reads_are_immediate_and_preserve_open_bus_high_bits() {
        let mut ppu = Ppu::new();
        ppu.mem_write(0x3F00, 0x2B, None);
        ppu.cpu_write_register(0, 0xC0, None);
        ppu.v = 0x3F00;

        assert_eq!(ppu.cpu_read_register(7, None), 0xEB);
        assert_eq!(ppu.v, 0x3F01);
    }
}

#[cfg(test)]
mod scroll_boundary_regressions {
    use super::*;

    #[test]
    fn coarse_x_wrap_toggles_horizontal_nametable_and_clears_fine_coarse_x() {
        let mut ppu = Ppu::new();
        ppu.v = 0x041F;
        ppu.increment_coarse_x();
        assert_eq!(ppu.v & 0x001F, 0);
        assert_eq!(ppu.v & 0x0400, 0, "horizontal nametable bit toggles from one to zero");
        assert_eq!(ppu.v & !0x041F, 0, "coarse-X increment must preserve unrelated scroll fields");
        ppu.increment_coarse_x();
        assert_eq!(ppu.v & 0x001F, 1);
        assert_eq!(ppu.v & 0x0400, 0);
    }

    #[test]
    fn coarse_y_rollover_distinguishes_row_29_and_attribute_row_31() {
        let mut ppu = Ppu::new();
        ppu.v = (7 << 12) | (29 << 5) | 0x0800;
        ppu.increment_y();
        assert_eq!(ppu.v & 0x7000, 0);
        assert_eq!(ppu.v & 0x03E0, 0);
        assert_eq!(ppu.v & 0x0800, 0, "row 29 wraps and toggles vertical nametable");

        ppu.v = (7 << 12) | (31 << 5);
        ppu.increment_y();
        assert_eq!(ppu.v & 0x7000, 0);
        assert_eq!(ppu.v & 0x03E0, 0, "coarse row 31 wraps without toggling nametable");
        assert_eq!(ppu.v & 0x0800, 0);
    }

    
    #[test]
    fn ppudata_increment_by_32_and_wraps_at_vram_boundary() {
        let mut ppu = Ppu::new();
        ppu.cpu_write_register(0, 0x04, None); // PPUCTRL: increment by 32.
        ppu.v = 0x2100;
        ppu.cpu_write_register(7, 0x5A, None);
        assert_eq!(ppu.v, 0x2120);
        ppu.cpu_write_register(7, 0xA5, None);
        assert_eq!(ppu.v, 0x2140);

        // v is a 15-bit internal register; each memory access is masked to 14 bits.
        ppu.v = 0x3FFF;
        ppu.cpu_write_register(7, 0x77, None);
        assert_eq!(ppu.v, 0x401F, "increment-by-32 must preserve the 15-bit v-register behavior");
    }

    #[test]
    fn ppustatus_read_resets_the_two_write_latch() {
        let mut ppu = Ppu::new();
        ppu.cpu_write_register(5, 0x2D, None);
        assert!(ppu.write_latch);
        let _ = ppu.cpu_read_register(2, None);
        assert!(!ppu.write_latch, "reading PPUSTATUS resets the shared scroll/address write toggle");
        ppu.cpu_write_register(5, 0x13, None);
        assert!(ppu.write_latch, "the next scroll write must be treated as the first write");
    }
}

#[cfg(test)]
mod mapper_a12_bus_regressions {
    use super::*;

    #[test]
    fn background_pattern_fetches_emit_timestamped_a12_edges() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x10; // Background pattern table at $1000.
        ppu.mask = 0x08; // Enable background rendering.
        ppu.tick_with_cartridge(9, None);

        assert_eq!(
            ppu.take_mapper_bus_events(),
            vec![(0, 0), (0x1000, 5), (0, 9)],
            "A12 must rise on the pattern-fetch slot and fall when the next nametable slot begins"
        );
    }

    #[test]
    fn disabling_rendering_emits_a_low_edge_for_mapper_irq_filters() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x10;
        ppu.mask = 0x08;
        ppu.tick_with_cartridge(5, None);
        assert_eq!(ppu.take_mapper_bus_events(), vec![(0, 0), (0x1000, 5)]);

        ppu.mask = 0;
        ppu.tick_with_cartridge(1, None);
        assert_eq!(
            ppu.take_mapper_bus_events(),
            vec![(0, 6)],
            "turning rendering off must release A12 low rather than leave MMC3 filters high"
        );
    }
}


#[cfg(test)]
mod sprite_pattern_address_regressions {
    use super::*;

    #[test]
    fn eight_by_sixteen_sprite_tile_bit_selects_pattern_table() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x20; // 8x16 sprites.
        assert_eq!(ppu.sprite_pattern_address(0x02, 0, 0), 0x0020);
        assert_eq!(ppu.sprite_pattern_address(0x03, 0, 0), 0x1020);
        assert_eq!(ppu.sprite_pattern_address(0x03, 0, 8), 0x1030);
    }

    #[test]
    fn vertical_flip_reverses_the_full_sixteen_pixel_sprite() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x20;
        let normal_top = ppu.sprite_pattern_address(0x03, 0, 0);
        let flipped_top = ppu.sprite_pattern_address(0x03, 0x80, 0);
        let normal_bottom = ppu.sprite_pattern_address(0x03, 0, 15);
        assert_eq!(flipped_top, normal_bottom);
        assert_eq!(normal_top, 0x1020);
        assert_eq!(normal_bottom, 0x1037);
        assert_eq!(ppu.sprite_pattern_address(0x03, 0x80, 15), normal_top);
    }
    #[test]
    fn enabling_nmi_during_vblank_generates_an_nmi_edge() {
        let mut ppu = Ppu::new();
        ppu.scanline = 241;
        ppu.dot = 2;
        ppu.status |= 0x80; // Vblank already active before NMI output is enabled.
        ppu.ctrl = 0;
        ppu.update_nmi_line();
        assert!(!ppu.nmi_pending);

        ppu.cpu_write_register(0, 0x80, None);
        assert!(ppu.nmi_pending, "raising PPUCTRL bit 7 during vblank must create an NMI edge");
        assert!(ppu.take_nmi_request());
        assert!(!ppu.nmi_pending);

        // Rewriting the same enabled value must not retrigger while the line stays high.
        ppu.cpu_write_register(0, 0x80, None);
        assert!(!ppu.nmi_pending);
    }

    #[test]
    fn disabling_nmi_output_during_vblank_drops_the_line_without_a_new_edge() {
        let mut ppu = Ppu::new();
        ppu.scanline = 241;
        ppu.dot = 2;
        ppu.status |= 0x80;
        ppu.cpu_write_register(0, 0x80, None);
        assert!(ppu.take_nmi_request());

        ppu.cpu_write_register(0, 0x00, None);
        assert!(!ppu.nmi_line);
        assert!(!ppu.nmi_pending);
        ppu.cpu_write_register(0, 0x80, None);
        assert!(ppu.nmi_pending, "re-enabling output while vblank remains set creates a fresh rising edge");
    }

    #[test]
    fn odd_frame_skip_only_occurs_when_rendering_is_enabled() {
        let mut rendered = Ppu::new();
        rendered.scanline = 261;
        rendered.dot = 339;
        rendered.odd_frame = true;
        rendered.mask = 0x08;
        rendered.tick(1);
        assert_eq!(rendered.position(), (0, 0));
        assert!(!rendered.odd_frame);

        let mut blanked = Ppu::new();
        blanked.scanline = 261;
        blanked.dot = 339;
        blanked.odd_frame = true;
        blanked.mask = 0;
        blanked.tick(1);
        assert_eq!(blanked.position(), (261, 340));
        assert!(blanked.odd_frame);
    }

}


#[cfg(test)]
mod palette_address_regressions {
    use super::*;

    #[test]
    fn sprite_universal_palette_entries_mirror_background_universal_entries() {
        let mut ppu = Ppu::new();
        ppu.mem_write(0x3F00, 0x09, None);
        assert_eq!(ppu.mem_read(0x3F10, None), 0x09);
        ppu.mem_write(0x3F14, 0x16, None);
        assert_eq!(ppu.mem_read(0x3F04, None), 0x16);
        ppu.mem_write(0x3F08, 0x27, None);
        assert_eq!(ppu.mem_read(0x3F18, None), 0x27);
        ppu.mem_write(0x3F0C, 0x31, None);
        assert_eq!(ppu.mem_read(0x3F1C, None), 0x31);
    }

    #[test]
    fn palette_ram_mirrors_every_thirty_two_bytes_and_masks_to_six_bits() {
        let mut ppu = Ppu::new();
        ppu.mem_write(0x3F21, 0xFF, None);
        assert_eq!(ppu.mem_read(0x3F01, None), 0x3F);
        ppu.mem_write(0x3F41, 0x12, None);
        assert_eq!(ppu.mem_read(0x3F01, None), 0x12);
        assert_eq!(ppu.mem_read(0x3F21, None), 0x12);
    }
}


#[cfg(test)]
mod sprite_y_wrap_regression_tests {
    use super::*;

    fn nrom_with_sprite_pixel() -> Cartridge {
        let mut image = vec![0; 16 + 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1;
        // Tile 0, row 0: leftmost pixel is opaque.
        image[16 + 16_384] = 0x80;
        Cartridge::parse(&image).unwrap()
    }

    #[test]
    fn oam_y_ff_places_sprite_on_scanline_zero_for_rendering() {
        let cart = nrom_with_sprite_pixel();
        let mut ppu = Ppu::new();
        ppu.mask = 0x14; // Sprites enabled, including the leftmost eight pixels.
        ppu.oam[0] = 0xFF;
        ppu.oam[1] = 0;
        ppu.oam[2] = 0;
        ppu.oam[3] = 0;
        ppu.palette[0x11] = 0x21;

        ppu.render_sprite_pixel(0, 0, Some(&cart));

        assert_eq!(ppu.framebuffer[0], 0x21,
            "OAM Y=$FF must wrap to the top scanline instead of placing the sprite off-screen");
    }

    #[test]
    fn oam_y_ff_sprite_zero_hit_uses_the_same_wrapped_top_row() {
        let cart = nrom_with_sprite_pixel();
        let mut ppu = Ppu::new();
        ppu.mask = 0x1E; // Background/sprites and both left-edge enables.
        ppu.oam[0] = 0xFF;
        ppu.oam[1] = 0;
        ppu.oam[2] = 0;
        ppu.oam[3] = 0;
        ppu.background_opaque[0] = true;

        assert!(ppu.sprite_zero_hit_at(0, 0, Some(&cart)),
            "sprite-zero hit testing must use the same wrapped Y coordinate as rendering");
    }
}


#[cfg(test)]
mod sprite_composition_regressions {
    use super::*;

    fn cart_with_opaque_tile_zero() -> Cartridge {
        let mut image = vec![0; 16 + 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1;
        image[16 + 16_384] = 0x80;
        Cartridge::parse(&image).unwrap()
    }

    #[test]
    fn behind_background_sprite_zero_blocks_lower_priority_sprite() {
        let cart = cart_with_opaque_tile_zero();
        let mut ppu = Ppu::new();
        ppu.mask = 0x14;
        ppu.background_opaque[0] = true;
        ppu.framebuffer[0] = 0x05;
        ppu.palette[0x11] = 0x21;
        // Sprite 0 is opaque but behind the background; sprite 1 is in front.
        ppu.oam[0..4].copy_from_slice(&[0xFF, 0, 0x20, 0]);
        ppu.oam[4..8].copy_from_slice(&[0xFF, 0, 0, 0]);

        ppu.render_sprite_pixel(0, 0, Some(&cart));

        assert_eq!(ppu.framebuffer[0], 0x05,
            "a hidden opaque sprite still claims the pixel and blocks lower-priority sprites");
    }

    #[test]
    fn ninth_in_range_sprite_is_not_rendered_even_if_first_eight_are_transparent() {
        let cart = cart_with_opaque_tile_zero();
        let mut ppu = Ppu::new();
        ppu.mask = 0x14;
        ppu.framebuffer[0] = 0x06;
        // Eight transparent sprites occupy the scanline before the opaque ninth.
        for sprite in 0..8 {
            ppu.oam[sprite * 4..sprite * 4 + 4].copy_from_slice(&[0xFF, 1, 0, 0]);
        }
        ppu.oam[32..36].copy_from_slice(&[0xFF, 0, 0, 0]);
        ppu.palette[0x11] = 0x21;

        ppu.render_sprite_pixel(0, 0, Some(&cart));

        assert_eq!(ppu.framebuffer[0], 0x06,
            "the scanline sprite limit counts transparent sprites too");
    }
}


#[cfg(test)]
mod sprite_8x16_top_wrap_regressions {
    use super::*;

    #[test]
    fn wrapped_y_sprite_uses_8x16_tile_selection_on_scanline_zero() {
        let mut rom = vec![0; 16 + 16_384 + 8_192];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[5] = 1;
        rom[16 + 16_384] = 0x80; // CHR tile 0 has an opaque top-left pixel.
        let cart = Cartridge::parse(&rom).unwrap();
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x20;
        ppu.mask = 0x14;
        ppu.palette[0x11] = 0x2A;
        ppu.oam[0..4].copy_from_slice(&[0xFF, 0, 0, 0]);

        ppu.render_sprite_pixel(0, 0, Some(&cart));
        assert_eq!(ppu.framebuffer[0], ppu.output_color(0x2A));
    }
}


#[cfg(test)]
mod beam_timed_sprite_zero_regressions {
    use super::*;

    fn opaque_test_cartridge() -> Cartridge {
        let mut rom = vec![0; 16 + 16_384 + 8_192];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[5] = 1;
        // Tile zero has opaque pixels across its first row for both background and sprite.
        rom[16 + 16_384] = 0xFF;
        Cartridge::parse(&rom).unwrap()
    }

    #[test]
    fn sprite_zero_hit_is_visible_at_the_overlap_dot_not_after_the_scanline() {
        let cartridge = opaque_test_cartridge();
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 0;
        ppu.mask = 0x1E; // Background, sprites, and both left-edge enables.
        ppu.oam[0..4].copy_from_slice(&[0xFF, 0, 0, 0]);

        ppu.tick_with_cartridge(1, Some(&cartridge));
        assert_ne!(ppu.status & 0x40, 0,
            "sprite-zero hit must become visible as soon as the first overlapping pixel is emitted");
        assert_eq!(ppu.position(), (0, 1));
    }

    #[test]
    fn sprite_zero_hit_is_never_set_on_the_last_screen_pixel() {
        let cartridge = opaque_test_cartridge();
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 255;
        ppu.mask = 0x1E;
        ppu.oam[0..4].copy_from_slice(&[0xFF, 0, 0, 255]);

        ppu.tick_with_cartridge(1, Some(&cartridge));
        assert_eq!(ppu.status & 0x40, 0,
            "the 2C02 suppresses sprite-zero hit at screen X=255");
    }
}


#[cfg(test)]
mod prerender_sprite_overflow_regressions {
    use super::*;

    #[test]
    fn prerender_scanline_evaluates_sprite_overflow_for_visible_line_zero() {
        let mut ppu = Ppu::new();
        ppu.scanline = 261;
        ppu.dot = 255;
        ppu.mask = 0x18;
        for sprite in 0..9usize {
            ppu.oam[sprite * 4..sprite * 4 + 4]
                .copy_from_slice(&[0xFF, 0, 0, (sprite * 8) as u8]);
        }

        ppu.tick(1);

        assert_ne!(ppu.status & 0x20, 0,
            "more than eight sprites on scanline zero must overflow during pre-render evaluation");
    }
}


#[cfg(test)]
mod background_only_sprite_evaluation_regressions {
    use super::*;

    #[test]
    fn background_only_snapshot_still_reports_sprite_overflow() {
        let mut ppu = Ppu::new();
        ppu.mask = 0x08; // Background on, sprite pixels off.
        for sprite in 0..9usize {
            // All nine sprites overlap scanline zero; X positions do not affect
            // scanline evaluation, but keeping them distinct avoids pixel overlap.
            ppu.oam[sprite * 4..sprite * 4 + 4]
                .copy_from_slice(&[0, 0, 0, (sprite * 16) as u8]);
        }

        ppu.render_frame(None);

        assert_ne!(ppu.status & 0x20, 0,
            "sprite evaluation continues while background rendering alone is enabled");
    }
}


#[cfg(test)]
mod vblank_nmi_race_regressions {
    use super::*;

    #[test]
    fn status_read_on_dot_zero_suppresses_vblank_and_its_nmi_edge() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x80; // Enable NMI on vblank.
        ppu.scanline = 241;
        ppu.dot = 0;
        ppu.status = 0x20; // Keep sprite overflow to check unrelated status bits.
        ppu.suppress_vblank = false;

        let status = ppu.cpu_read_register(2, None);
        assert_eq!(status & 0x80, 0);
        assert_eq!(ppu.status & 0x20, 0x20);
        assert!(ppu.suppress_vblank);

        ppu.tick(1); // Advance to the vblank-set dot.
        assert_eq!(ppu.status & 0x80, 0);
        assert!(!ppu.take_nmi_request());
        assert!(!ppu.suppress_vblank, "the suppression latch is consumed at dot 1");
    }
}

#[cfg(test)]
mod nmi_control_edge_regressions {
    use super::*;

    #[test]
    fn enabling_nmi_during_vblank_raises_one_edge_until_the_line_drops() {
        let mut ppu = Ppu::new();
        ppu.scanline = 241;
        ppu.dot = 10;
        ppu.status |= 0x80;
        ppu.ctrl = 0;
        ppu.nmi_line = false;
        ppu.nmi_pending = false;

        ppu.cpu_write_register(0, 0x80, None);
        assert!(ppu.take_nmi_request(), "enabling NMI while vblank is active raises an edge");
        ppu.cpu_write_register(0, 0x80, None);
        assert!(!ppu.take_nmi_request(), "keeping the NMI output high must not create duplicate edges");

        let _ = ppu.cpu_read_register(2, None);
        assert!(!ppu.nmi_line, "reading PPUSTATUS lowers the vblank NMI output");
        ppu.cpu_write_register(0, 0x80, None);
        assert!(!ppu.nmi_line, "NMI cannot rise again after status read cleared vblank");
        assert!(!ppu.take_nmi_request());
    }
}


#[cfg(test)]
mod nmi_enable_during_vblank_regressions {
    use super::*;

    #[test]
    fn enabling_nmi_during_active_vblank_latches_one_rising_edge() {
        let mut ppu = Ppu::new();
        ppu.status |= 0x80;
        assert!(!ppu.nmi_line);
        ppu.cpu_write_register(0, 0x80, None);
        assert!(ppu.take_nmi_request(), "raising NMI enable while VBlank is active must raise an NMI edge");
        assert!(!ppu.take_nmi_request(), "the same high line must not create duplicate NMI edges");

        ppu.cpu_read_register(2, None);
        assert!(!ppu.nmi_line, "reading PPUSTATUS clears VBlank and lowers the NMI line");
        assert!(!ppu.take_nmi_request(), "lowering the line must not create another edge");
    }
}


#[cfg(test)]
mod vblank_suppression_recovery_regressions {
    use super::*;

    #[test]
    fn suppressing_one_vblank_does_not_disable_the_next_vblank_nmi() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x80;
        ppu.scanline = 241;
        ppu.dot = 0;

        let _ = ppu.cpu_read_register(2, None);
        ppu.tick(1);
        assert_eq!(ppu.status & 0x80, 0);
        assert!(!ppu.take_nmi_request());

        // Model the following frame's VBlank entry.
        ppu.scanline = 241;
        ppu.dot = 0;
        ppu.tick(1);
        assert_ne!(ppu.status & 0x80, 0);
        assert!(ppu.take_nmi_request(), "the suppression latch must not leak into a later frame");
        assert!(!ppu.take_nmi_request(), "one VBlank entry must generate only one NMI edge");
    }
}


#[cfg(test)]
mod coarse_y_rollover_regressions {
    use super::*;

    #[test]
    fn coarse_y_29_wraps_and_toggles_vertical_nametable() {
        assert_eq!(Ppu::scroll_coarse_y(29, 0, 1), (0, 1));
        assert_eq!(Ppu::scroll_coarse_y(29, 1, 1), (0, 0));
    }

    #[test]
    fn coarse_y_31_wraps_without_toggling_vertical_nametable() {
        assert_eq!(Ppu::scroll_coarse_y(31, 0, 1), (0, 0));
        assert_eq!(Ppu::scroll_coarse_y(31, 1, 1), (0, 1));
    }

    #[test]
    fn coarse_y_rollover_applies_each_row_in_a_multi_row_advance() {
        assert_eq!(Ppu::scroll_coarse_y(28, 0, 3), (1, 1));
        assert_eq!(Ppu::scroll_coarse_y(30, 1, 2), (0, 1));
    }
}


#[cfg(test)]
mod increment_y_register_regressions {
    use super::*;

    #[test]
    fn fine_y_rollover_from_coarse_y_29_toggles_vertical_nametable() {
        let mut ppu = Ppu::new();
        ppu.v = 0x7000 | (29 << 5) | (1 << 10) | 3;

        ppu.increment_y();

        assert_eq!(ppu.v & 0x7000, 0, "fine Y wraps from seven to zero");
        assert_eq!(ppu.v & 0x03E0, 0, "coarse Y 29 wraps to row zero");
        assert_ne!(ppu.v & 0x0800, 0, "coarse Y 29 toggles the vertical nametable");
        assert_ne!(ppu.v & 0x0400, 0, "horizontal nametable bit is preserved");
        assert_eq!(ppu.v & 0x001F, 3, "coarse X is preserved");
    }

    #[test]
    fn fine_y_rollover_from_coarse_y_31_wraps_without_toggling_nametable() {
        let mut ppu = Ppu::new();
        ppu.v = 0x7000 | (31 << 5) | (1 << 11) | 7;

        ppu.increment_y();

        assert_eq!(ppu.v & 0x7000, 0);
        assert_eq!(ppu.v & 0x03E0, 0);
        assert_ne!(ppu.v & 0x0800, 0, "vertical nametable remains unchanged at coarse Y 31");
        assert_eq!(ppu.v & 0x001F, 7);
    }

    #[test]
    fn fine_y_increment_does_not_touch_coarse_y_before_fine_y_rollover() {
        let mut ppu = Ppu::new();
        ppu.v = (3 << 12) | (12 << 5) | (1 << 11) | 9;

        ppu.increment_y();

        assert_eq!((ppu.v >> 12) & 7, 4);
        assert_eq!((ppu.v >> 5) & 31, 12);
        assert_ne!(ppu.v & 0x0800, 0);
        assert_eq!(ppu.v & 0x001F, 9);
    }
}


#[cfg(test)]
mod nmi_enable_and_coarse_y_regressions {
    use super::*;

    #[test]
    fn enabling_nmi_during_vblank_generates_only_one_rising_edge() {
        let mut ppu = Ppu::new();
        ppu.scanline = 241;
        ppu.dot = 20;
        ppu.status |= 0x80; // VBlank is already active.
        ppu.ctrl = 0;
        ppu.update_nmi_line();

        ppu.cpu_write_register(0, 0x80, None);
        assert!(ppu.take_nmi_request(), "raising PPUCTRL.NMI during VBlank must assert NMI");
        ppu.cpu_write_register(0, 0x80, None);
        assert!(!ppu.take_nmi_request(), "keeping the NMI line high must not create a second edge");
    }

    #[test]
    fn fine_y_rollover_from_coarse_y_30_advances_to_row_31_without_toggling_nametable() {
        let mut ppu = Ppu::new();
        ppu.v = 0x7000 | (30 << 5) | (1 << 10) | 5;

        ppu.increment_y();

        assert_eq!(ppu.v & 0x7000, 0);
        assert_eq!((ppu.v & 0x03E0) >> 5, 31);
        assert_ne!(ppu.v & 0x0400, 0, "horizontal nametable bit is preserved");
        assert_eq!(ppu.v & 0x0800, 0, "coarse Y 30 does not toggle vertical nametable");
        assert_eq!(ppu.v & 0x001F, 5, "coarse X is preserved");
    }
}


#[cfg(test)]
mod sprite_zero_boundary_regressions {
    use super::*;

    fn opaque_sprite_cartridge() -> Cartridge {
        let mut rom = vec![0; 16 + 16_384 + 8_192];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[5] = 1;
        let chr = 16 + 16_384;
        rom[chr..chr + 16].fill(0xFF);
        Cartridge::parse(&rom).unwrap()
    }

    #[test]
    fn sprite_zero_hit_can_occur_at_x254_but_never_at_x255() {
        let cart = opaque_sprite_cartridge();
        let mut ppu = Ppu::new();
        ppu.mask = 0x1E;
        ppu.oam[0] = 0xFF;
        ppu.oam[1] = 0;
        ppu.oam[3] = 248;
        ppu.background_opaque[254] = true;
        ppu.background_opaque[255] = true;

        assert!(ppu.sprite_zero_hit_at(254, 0, Some(&cart)));
        assert!(!ppu.sprite_zero_hit_at(255, 0, Some(&cart)),
            "the hardware suppresses sprite-zero hit on the final visible pixel");
    }

    #[test]
    fn sprite_zero_hit_is_independent_of_sprite_behind_background_priority() {
        let cart = opaque_sprite_cartridge();
        let mut ppu = Ppu::new();
        ppu.mask = 0x1E;
        ppu.oam[0] = 0xFF;
        ppu.oam[1] = 0;
        ppu.oam[2] = 0x20; // Sprite priority: behind the background.
        ppu.oam[3] = 16;
        ppu.background_opaque[16] = true;

        assert!(ppu.sprite_zero_hit_at(16, 0, Some(&cart)),
            "sprite-zero hit depends on opaque overlap, not visual priority");
    }
}


#[cfg(test)]
mod sprite_zero_flip_regressions {
    use super::*;

    fn sprite_with_only_leftmost_pattern_bit() -> Cartridge {
        let mut rom = vec![0; 16 + 16_384 + 8_192];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[5] = 1;
        // Tile zero, row zero: only bit 0 is opaque. Horizontal flip moves
        // this pixel from the rightmost sprite column to the leftmost column.
        rom[16 + 16_384] = 0x01;
        Cartridge::parse(&rom).unwrap()
    }

    #[test]
    fn sprite_zero_hit_uses_the_same_horizontal_flip_as_sprite_rendering() {
        let cart = sprite_with_only_leftmost_pattern_bit();
        let mut ppu = Ppu::new();
        ppu.mask = 0x1E;
        ppu.oam[0] = 0xFF;
        ppu.oam[1] = 0;
        ppu.oam[3] = 16;
        ppu.background_opaque[16] = true;

        assert!(!ppu.sprite_zero_hit_at(16, 0, Some(&cart)),
            "without flipping, the sprite's first screen pixel reads pattern bit 7");
        ppu.oam[2] = 0x40; // Horizontal flip.
        assert!(ppu.sprite_zero_hit_at(16, 0, Some(&cart)),
            "horizontal flip must move pattern bit 0 onto screen X=16");
    }
}


#[cfg(test)]
mod sprite_pattern_address_boundary_regressions {
    use super::*;

    #[test]
    fn vertical_flip_in_8x16_sprites_crosses_tile_pair_in_reverse() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x20; // 8x16 sprites.
        // An odd tile selects pattern table $1000; the even tile starts the pair.
        assert_eq!(ppu.sprite_pattern_address(3, 0, 0), 0x1020);
        assert_eq!(ppu.sprite_pattern_address(3, 0, 7), 0x1027);
        assert_eq!(ppu.sprite_pattern_address(3, 0, 8), 0x1030);

        assert_eq!(ppu.sprite_pattern_address(3, 0x80, 0), 0x1037);
        assert_eq!(ppu.sprite_pattern_address(3, 0x80, 7), 0x1030);
        assert_eq!(ppu.sprite_pattern_address(3, 0x80, 8), 0x1027);
        assert_eq!(ppu.sprite_pattern_address(3, 0x80, 15), 0x1020);
    }

    #[test]
    fn vertical_flip_in_8x8_sprites_reverses_rows_without_changing_table() {
        let mut ppu = Ppu::new();
        ppu.ctrl = 0x08; // Sprite pattern table at $1000.
        assert_eq!(ppu.sprite_pattern_address(5, 0, 0), 0x1050);
        assert_eq!(ppu.sprite_pattern_address(5, 0, 7), 0x1057);
        assert_eq!(ppu.sprite_pattern_address(5, 0x80, 0), 0x1057);
        assert_eq!(ppu.sprite_pattern_address(5, 0x80, 7), 0x1050);
    }
}


#[cfg(test)]
mod sprite_8x16_render_regressions {
    use super::*;

    #[test]
    fn vertical_flip_composes_pixels_from_both_tiles_in_the_reversed_pair() {
        let mut image = vec![0; 16 + 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1;
        let chr_start = 16 + 16_384;
        // Odd tile selects pattern table $1000. With vertical flip, output rows
        // 0 and 8 source rows 15 and 7 respectively, crossing the tile boundary.
        image[chr_start + 0x1017] = 0x80;
        image[chr_start + 0x1007] = 0x80;
        let cartridge = Cartridge::parse(&image).unwrap();

        let mut ppu = Ppu::new();
        ppu.ctrl = 0x20; // 8x16 sprites.
        ppu.mask = 0x14; // Sprites enabled and visible in the left eight pixels.
        ppu.palette[0x11] = 0x2B;
        ppu.oam[0..4].copy_from_slice(&[0, 1, 0x80, 16]);

        ppu.render_frame(Some(&cartridge));

        let sprite_color = ppu.output_color(0x2B);
        // OAM Y=0 places the sprite's first visible row on scanline 1.
        assert_eq!(ppu.framebuffer[256 + 16], sprite_color, "flipped output row 0 must use tile 1 row 7");
        assert_eq!(ppu.framebuffer[9 * 256 + 16], sprite_color, "flipped output row 8 must use tile 0 row 7");
        assert_ne!(ppu.framebuffer[2 * 256 + 16], sprite_color, "other source rows stay transparent");
        assert_ne!(ppu.framebuffer[10 * 256 + 16], sprite_color, "the opposite rows in each tile stay transparent");
    }
}


#[cfg(test)]
mod ppudata_palette_buffer_regressions {
    use super::*;

    #[test]
    fn palette_read_is_immediate_preserves_open_bus_high_bits_and_refills_buffer() {
        let mut ppu = Ppu::new();
        ppu.open_bus = 0xC0;
        ppu.palette[5] = 0x2A;
        // $2F05 mirrors nametable 1 in horizontal mirroring.
        ppu.nametables[0x705] = 0x5A;
        ppu.v = 0x3F05;

        let palette_value = ppu.cpu_read_register(7, None);
        assert_eq!(palette_value, 0xEA, "palette drives bits 0-5 while bits 6-7 retain the I/O latch");
        assert_eq!(ppu.read_buffer, 0x5A, "palette reads refill the delayed buffer from address - $1000");

        ppu.v = 0x2000;
        assert_eq!(ppu.cpu_read_register(7, None), 0x5A, "the next non-palette read returns the refilled buffer");
    }
}


#[cfg(test)]
mod ppudata_palette_mirror_regressions {
    use super::*;

    #[test]
    fn palette_alias_read_refills_buffer_from_corresponding_nametable_address() {
        let mut ppu = Ppu::new();
        ppu.open_bus = 0x80;
        ppu.mem_write(0x2F10, 0x71, None);
        ppu.mem_write(0x3F00, 0x23, None);
        ppu.v = 0x3F10; // Mirrors universal background entry $3F00.

        assert_eq!(ppu.cpu_read_register(7, None), 0xA3);
        assert_eq!(ppu.read_buffer, 0x71, "the hidden buffer fetch must use $2F10, not palette RAM");
        ppu.v = 0x2000;
        assert_eq!(ppu.cpu_read_register(7, None), 0x71);
    }
}


#[cfg(test)]
mod nametable_address_bus_regressions {
    use super::*;

    #[test]
    fn four_screen_cartridge_keeps_all_four_physical_nametables_distinct() {
        let mut image = vec![0; 16 + 2 * 16 * 1024];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 2;
        image[6] = 0x48; // Mapper 4 with hardwired four-screen mirroring.
        let cart = Cartridge::parse(&image).unwrap();
        assert_eq!(cart.mirroring(), Mirroring::FourScreen);

        let mut ppu = Ppu::new();
        let mut cart = cart;
        for (address, value) in [
            (0x2000, 0x11),
            (0x2400, 0x22),
            (0x2800, 0x33),
            (0x2C00, 0x44),
        ] {
            ppu.mem_write(address, value, Some(&mut cart));
        }
        for (address, expected) in [
            (0x2000, 0x11),
            (0x2400, 0x22),
            (0x2800, 0x33),
            (0x2C00, 0x44),
        ] {
            assert_eq!(ppu.mem_read(address, Some(&cart)), expected);
        }
        assert_eq!(ppu.mem_read(0x3000, Some(&cart)), 0x11, "$3000 mirrors $2000 even with four-screen wiring");
    }

    #[test]
    fn horizontal_and_vertical_nametable_mirrors_keep_their_expected_aliases() {
        let mut horizontal = Ppu::new();
        horizontal.mem_write(0x2000, 0x5A, None);
        assert_eq!(horizontal.mem_read(0x2400, None), 0x5A);
        horizontal.mem_write(0x2800, 0xA5, None);
        assert_eq!(horizontal.mem_read(0x2C00, None), 0xA5);

        let mut vertical_image = vec![0; 16 + 2 * 16 * 1024];
        vertical_image[0..4].copy_from_slice(b"NES\x1A");
        vertical_image[4] = 2;
        vertical_image[6] = 0x41; // Mapper 4, vertical mirroring.
        let vertical_cart = Cartridge::parse(&vertical_image).unwrap();
        let mut vertical = Ppu::new();
        let mut vertical_cart = vertical_cart;
        vertical.mem_write(0x2000, 0x3C, Some(&mut vertical_cart));
        assert_eq!(vertical.mem_read(0x2800, Some(&vertical_cart)), 0x3C);
    }
}


#[cfg(test)]
mod mid_scanline_scroll_latch_regressions {
    use super::*;

    #[test]
    fn second_ppuscroll_write_changes_only_pixels_from_the_current_beam_x() {
        let mut ppu = Ppu::new();
        ppu.scanline = 12;
        ppu.dot = 40;
        ppu.mask = 0x18;
        ppu.line_scroll_v = 0x0123;
        ppu.line_scroll_start_x = 0;
        ppu.write_latch = true;
        ppu.cpu_write_register(5, 0b1011_0110, None);

        assert_eq!(ppu.fine_x, 0, "the second $2005 write must not overwrite fine X");
        assert_eq!(ppu.t, 0x62C0, "the second $2005 write updates the temporary scroll address");
        assert_eq!(ppu.line_scroll_v, 0x0123, "the current line keeps its active scroll address");
        assert_eq!(ppu.line_scroll_start_x, 0,
            "a second $2005 write updates t but does not immediately replace v on the current line");
    }

    #[test]
    fn second_ppuaddr_write_latches_scroll_at_the_current_beam_without_rewriting_prior_pixels() {
        let mut ppu = Ppu::new();
        ppu.scanline = 12;
        ppu.dot = 41;
        ppu.mask = 0x18;
        ppu.write_latch = true;
        ppu.t = 0x2000;
        ppu.cpu_write_register(6, 0x45, None);

        assert_eq!(ppu.v, 0x2045);
        assert_eq!(ppu.line_scroll_v, 0x2045);
        assert_eq!(ppu.line_scroll_start_x, 41,
            "the new VRAM address takes effect at the current visible beam coordinate");
    }

    #[test]
    fn ppuaddr_write_outside_visible_pixel_dots_does_not_move_line_sampling_origin() {
        let mut ppu = Ppu::new();
        ppu.scanline = 12;
        ppu.dot = 257;
        ppu.mask = 0x18;
        ppu.write_latch = true;
        ppu.line_scroll_start_x = 17;
        ppu.cpu_write_register(6, 0x00, None);

        assert_eq!(ppu.line_scroll_start_x, 17,
            "writes after visible pixels must not create an out-of-range pixel origin");
    }
}



#[cfg(test)]
mod beam_sprite_composition_regressions {
    use super::*;

    fn sprite_pattern_cartridge() -> Cartridge {
        let mut rom = vec![0; 16 + 16_384 + 8_192];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[5] = 1;
        let chr = 16 + 16_384;
        rom[chr..chr + 16].fill(0xFF);
        Cartridge::parse(&rom).unwrap()
    }

    #[test]
    fn sprite_pixels_are_composited_as_the_beam_reaches_them() {
        let cartridge = sprite_pattern_cartridge();
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 16;
        ppu.mask = 0x14; // Sprites enabled, background disabled, left-edge sprites visible.
        ppu.palette[0x13] = 0x2B;
        ppu.oam[0..4].copy_from_slice(&[0xFF, 0, 0, 16]);

        ppu.tick_with_cartridge(1, Some(&cartridge));

        assert_eq!(ppu.framebuffer[16], ppu.output_color(0x2B),
            "sprite zero's first pixel must be visible immediately at beam X=16");
        assert_eq!(ppu.framebuffer[15], ppu.output_color(0),
            "pixels before the sprite must remain untouched");
    }

    #[test]
    fn behind_background_sprite_claims_pixel_without_overwriting_opaque_background() {
        let cartridge = sprite_pattern_cartridge();
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 16;
        ppu.mask = 0x1E;
        ppu.palette[0] = 0x01;
        ppu.palette[0x11] = 0x2B;
        ppu.background_opaque[16] = true;
        ppu.framebuffer[16] = ppu.output_color(0x01);
        ppu.oam[0..4].copy_from_slice(&[0xFF, 0, 0x20, 16]);

        ppu.render_sprite_pixel(16, 0, Some(&cartridge));

        assert_eq!(ppu.framebuffer[16], ppu.output_color(0x01),
            "behind-background sprites must not overwrite an opaque background pixel");
    }

    #[test]
    fn transparent_sprite_does_not_claim_pixel_before_later_opaque_sprite() {
        let mut rom = vec![0; 16 + 16_384 + 8_192];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[5] = 1;
        let chr = 16 + 16_384;
        rom[chr + 16..chr + 32].fill(0xFF); // Tile 1 opaque.
        let cartridge = Cartridge::parse(&rom).unwrap();
        let mut ppu = Ppu::new();
        ppu.mask = 0x14;
        ppu.palette[0x15] = 0x2C;
        ppu.oam[0..4].copy_from_slice(&[0xFF, 0, 0, 16]); // Tile 0 transparent.
        ppu.oam[4..8].copy_from_slice(&[0xFF, 1, 1, 16]); // Tile 1 uses sprite palette 1.

        ppu.render_sprite_pixel(16, 0, Some(&cartridge));

        assert_eq!(ppu.framebuffer[16], ppu.output_color(0x2C),
            "transparent sprite zero must allow sprite one to supply the visible pixel");
    }
}


#[cfg(test)]
mod secondary_oam_latch_regressions {
    use super::*;

    fn solid_sprite_cartridge() -> Cartridge {
        let mut rom = vec![0; 16 + 16_384 + 8_192];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[5] = 1;
        let chr = 16 + 16_384;
        rom[chr..chr + 16].fill(0xFF);
        Cartridge::parse(&rom).unwrap()
    }

    #[test]
    fn mid_scanline_oam_write_does_not_move_already_evaluated_sprite() {
        let cartridge = solid_sprite_cartridge();
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 16;
        ppu.mask = 0x14;
        ppu.palette[0x13] = 0x2B;
        ppu.oam[0..4].copy_from_slice(&[0xFF, 0, 0, 16]);

        ppu.tick_with_cartridge(1, Some(&cartridge));
        ppu.oam[3] = 40;
        ppu.tick_with_cartridge(1, Some(&cartridge));

        let sprite_color = ppu.output_color(0x2B);
        assert_eq!(ppu.framebuffer[16], sprite_color);
        assert_eq!(ppu.framebuffer[17], sprite_color,
            "changing primary OAM after evaluation must not move this scanline's sprite");
        assert_ne!(ppu.framebuffer[40], sprite_color,
            "the new X coordinate is not visible until a later scanline evaluation");
    }

    #[test]
    fn sprite_line_snapshot_is_refreshed_for_the_next_scanline() {
        let cartridge = solid_sprite_cartridge();
        let mut ppu = Ppu::new();
        ppu.scanline = 0;
        ppu.dot = 16;
        ppu.mask = 0x14;
        ppu.palette[0x13] = 0x2B;
        ppu.oam[0..4].copy_from_slice(&[0xFF, 0, 0, 16]);

        ppu.tick_with_cartridge(1, Some(&cartridge));
        ppu.oam[3] = 40;
        ppu.scanline = 1;
        ppu.dot = 0;
        ppu.tick_with_cartridge(1, Some(&cartridge));
        for _ in 0..40 { ppu.tick_with_cartridge(1, Some(&cartridge)); }

        let sprite_color = ppu.output_color(0x2B);
        assert_ne!(ppu.framebuffer[256 + 16], sprite_color);
        assert_eq!(ppu.framebuffer[256 + 40], sprite_color,
            "a fresh scanline evaluation must observe the updated primary OAM");
    }
}
