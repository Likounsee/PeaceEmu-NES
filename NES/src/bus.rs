//! CPU memory bus for the initial NROM machine.

use crate::apu::Apu;
use crate::cartridge::Nrom;
use crate::ppu::Ppu;

/// CPU bus with 2 KiB internal RAM and optional NROM cartridge.
///
/// PPU registers, frame rendering, NMI, OAM DMA and serial controller ports are routed.
/// APU pulse/triangle/noise synthesis and an approximate DMC sample-fetch path are integrated;
/// audio mixing, DMA stalls and interrupt timing are not cycle-accurate.
#[derive(Debug)]
pub struct CpuBus {
    ram: [u8; 0x800],
    cartridge: Option<Nrom>,
    open_bus: u8,
    ppu: Ppu,
    apu: Apu,
    pending_oam_dma: Option<u8>,
    controller_state: [u8; 2],
    controller_shift: [u8; 2],
    controller_strobe: bool,
}
impl Default for CpuBus {
    fn default() -> Self { Self { ram: [0; 0x800], cartridge: None, open_bus: 0, ppu: Ppu::new(), apu: Apu::new(), pending_oam_dma: None, controller_state: [0; 2], controller_shift: [0; 2], controller_strobe: false } }
}
impl CpuBus {
    /// Creates a bus without a cartridge.
    pub fn new() -> Self { Self::default() }
    /// Attaches a parsed NROM cartridge.
    pub fn insert_cartridge(&mut self, cartridge: Nrom) { self.cartridge = Some(cartridge); }
    /// Reads a byte from the CPU address space.
    pub fn read(&mut self, address: u16) -> u8 {
        let value = match address {
            0x0000..=0x1FFF => self.ram[address as usize & 0x07FF],
            0x2000..=0x3FFF => self.ppu.cpu_read_register(address & 7, self.cartridge.as_ref()),
            // Bit 5 of $4015 is open bus on the 2A03; preserve the previous bus value.
            0x4015 => self.apu.read(address) | (self.open_bus & 0x20),
            0x4016..=0x4017 => self.read_controller((address - 0x4016) as usize),
            0x8000..=0xFFFF => self.cartridge.as_ref().and_then(|c| c.cpu_read(address)).unwrap_or(self.open_bus),
            _ => self.open_bus,
        };
        self.open_bus = value;
        value
    }
    /// Writes a byte to the CPU address space. Cartridge ROM is not writable.
    pub fn write(&mut self, address: u16, value: u8) {
        self.open_bus = value;
        match address {
            0x0000..=0x1FFF => self.ram[address as usize & 0x07FF] = value,
            0x2000..=0x3FFF => self.ppu.cpu_write_register(address & 7, value, self.cartridge.as_mut()),
            0x4000..=0x4013 | 0x4015 | 0x4017 => self.apu.write(address, value),
            0x4014 => self.pending_oam_dma = Some(value),
            0x4016 => {
                let next_strobe = value & 1 != 0;
                if next_strobe || self.controller_strobe {
                    self.controller_shift = self.controller_state;
                }
                self.controller_strobe = next_strobe;
            },
            _ => {}
        }
    }
    /// Sets the live button bitmap for controller port 1 or 2 (A, B, Select, Start, Up, Down, Left, Right).
    /// The values are latched by the next strobe write to $4016.
    pub fn set_controller_state(&mut self, port: usize, buttons: u8) {
        if let Some(state) = self.controller_state.get_mut(port) {
            *state = buttons;
            if self.controller_strobe {
                self.controller_shift[port] = buttons;
            }
        }
    }

    fn read_controller(&mut self, port: usize) -> u8 {
        if self.controller_strobe {
            return 0x40 | (self.controller_state[port] & 1);
        }
        let value = self.controller_shift[port] & 1;
        self.controller_shift[port] = (self.controller_shift[port] >> 1) | 0x80;
        0x40 | value
    }

    /// Rebuilds the background framebuffer from the attached cartridge and current VRAM scroll.
    pub fn render_background(&mut self) { self.ppu.render_background(self.cartridge.as_ref()); }
    /// Renders the background and composites visible sprites into the indexed framebuffer.
    pub fn render_frame(&mut self) { self.ppu.render_frame(self.cartridge.as_ref()); }
    /// Returns the most recently rendered background as 256x240 NES palette indices.
    pub fn framebuffer(&self) -> &[u8] { self.ppu.framebuffer() }
    /// Advances the PPU by the specified number of dots.
    pub fn tick_ppu(&mut self, dots: u32) { self.ppu.tick(dots); }
    /// Advances the APU by CPU cycles and services pending DMC sample reads.
    /// DMC fetches currently use the bus immediately; their CPU stall timing is approximate.
    pub fn tick_apu(&mut self, cycles: u32) {
        // Service fetch requests between CPU cycles so a DMC buffer refill can affect
        // later timer clocks in the same batch. DMA stall/arbitration cycles are not yet modelled.
        for _ in 0..cycles {
            self.apu.tick_cpu_cycles(1);
            if let Some(address) = self.apu.take_dmc_dma_request() {
                let value = self.read(address);
                self.apu.supply_dmc_byte(value);
            }
        }
    }
    /// Returns one approximate normalized mono sample from the APU mixer.
    pub fn apu_sample(&self) -> f32 { self.apu.sample() }
    /// Returns the level-triggered APU IRQ line without clearing it.
    pub fn irq_pending(&self) -> bool { self.apu.irq_pending() }
    /// Takes a pending NMI edge raised by the PPU.
    pub fn take_nmi_request(&mut self) -> bool { self.ppu.take_nmi_request() }
    /// Takes a pending OAM DMA page requested by a write to $4014.
    pub fn take_oam_dma_request(&mut self) -> Option<u8> { self.pending_oam_dma.take() }
    /// Copies a full 256-byte page into PPU OAM, starting at the current OAM address.
    pub fn transfer_oam_dma(&mut self, page: u8) {
        let base = u16::from(page) << 8;
        for offset in 0..=u8::MAX {
            let value = self.read(base | u16::from(offset));
            self.ppu.cpu_write_register(4, value, self.cartridge.as_mut());
        }
    }
    /// Returns whether a cartridge is attached.
    pub fn has_cartridge(&self) -> bool { self.cartridge.is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn ram_is_mirrored_every_0x800_bytes() {
        let mut b = CpuBus::new(); b.write(2, 0x5A);
        for a in [0x0002, 0x0802, 0x1002, 0x1802] { assert_eq!(b.read(a), 0x5A); }
    }
    #[test] fn unmapped_io_uses_open_bus_latch() {
        let mut b = CpuBus::new(); b.write(0x4000, 0xC3); assert_eq!(b.read(0x4000), 0xC3);
    }
    #[test]
    fn ppu_registers_are_mirrored_through_cpu_bus() {
        let mut bus = CpuBus::new();
        bus.tick_ppu(243 * 341 + 1);
        assert_ne!(bus.read(0x2002) & 0x80, 0);
        assert_eq!(bus.read(0x3FFA) & 0x80, 0);
    }
    #[test]
    fn oam_dma_request_copies_all_256_bytes_and_wraps_oam_address() {
        let mut bus = CpuBus::new();
        for i in 0..=u8::MAX { bus.write(0x0200 | u16::from(i), i.wrapping_mul(3)); }
        bus.write(0x2003, 0x80);
        bus.write(0x4014, 0x02);
        assert_eq!(bus.take_oam_dma_request(), Some(0x02));
        bus.transfer_oam_dma(0x02);
        for i in 0..=u8::MAX {
            bus.write(0x2003, i.wrapping_add(0x80));
            assert_eq!(bus.read(0x2004), i.wrapping_mul(3));
        }
        assert_eq!(bus.take_oam_dma_request(), None);
    }
    #[test]
    fn controllers_latch_and_shift_buttons_in_nes_order() {
        let mut bus = CpuBus::new();
        // A, Select and Up pressed: bit positions 0, 2 and 4.
        bus.set_controller_state(0, 0b0001_0101);
        bus.write(0x4016, 1);
        assert_eq!(bus.read(0x4016) & 1, 1);
        bus.write(0x4016, 0);
        let bits: Vec<u8> = (0..8).map(|_| bus.read(0x4016) & 1).collect();
        assert_eq!(bits, [1, 0, 1, 0, 1, 0, 0, 0]);
        // Reads after the eighth button return 1 on the serial line.
        assert_eq!(bus.read(0x4016) & 1, 1);
    }

    #[test]
    fn second_controller_has_an_independent_shift_register() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0b0000_0001);
        bus.set_controller_state(1, 0b0000_0010);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        assert_eq!(bus.read(0x4016) & 1, 1);
        assert_eq!(bus.read(0x4017) & 1, 0);
        assert_eq!(bus.read(0x4017) & 1, 1);
    }

    #[test]
    fn apu_status_preserves_open_bus_bit_five_and_clears_frame_irq() {
        let mut bus = CpuBus::new();
        bus.write(0x4000, 0x20);
        bus.write(0x4015, 0x00);
        bus.write(0x4017, 0x00);
        bus.tick_apu(14_915);
        assert!(bus.irq_pending());
        bus.write(0x4000, 0x20); // Put bit 5 high on the CPU data bus before reading status.
        let status = bus.read(0x4015);
        assert_ne!(status & 0x20, 0);
        assert_ne!(status & 0x40, 0);
        assert!(!bus.irq_pending());
    }

    #[test]
    fn dmc_bus_fetches_follow_sample_address_register_and_buffer_refills() {
        let mut bus = CpuBus::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x100] = 0xFF;
        image[16 + 0x101] = 0x00;
        bus.insert_cartridge(Nrom::parse(&image).unwrap());
        bus.write(0x4010, 0x0F);
        bus.write(0x4012, 0x04); // $C100, mirrored to $8100 by NROM.
        bus.write(0x4013, 0x01); // 17 bytes.
        bus.write(0x4015, 0x10);
        bus.tick_apu(1);
        assert_eq!(bus.read(0x4015) & 0x10, 0x10);
        bus.tick_apu(16 * 54);
        assert!(bus.apu_sample() > 0.0);
    }

    #[test]
    fn dmc_sample_reads_from_cpu_bus_and_sets_status_irq() {
        let mut bus = CpuBus::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16] = 0x11; // NROM-16K mirrors $8000 at $C000.
        image[17] = 0x22;
        bus.insert_cartridge(Nrom::parse(&image).unwrap());
        bus.write(0x4010, 0x80); // Enable DMC IRQ, fastest-rate selection is not required for fetch.
        bus.write(0x4012, 0x00); // $C000 sample address.
        bus.write(0x4013, 0x00); // One byte.
        bus.write(0x4015, 0x10); // Start DMC.
        bus.tick_apu(1);
        assert!(bus.irq_pending());
        assert_ne!(bus.read(0x4015) & 0x80, 0);
        assert_eq!(bus.read(0x4015) & 0x10, 0);
        assert!(bus.irq_pending(), "reading status must not acknowledge DMC IRQ");
        bus.write(0x4015, 0x00);
        assert!(!bus.irq_pending(), "writing $4015 acknowledges DMC IRQ");
    }

    #[test]
    fn apu_length_counter_status_is_visible_through_cpu_bus() {
        let mut bus = CpuBus::new();
        bus.write(0x4015, 0x01);
        bus.write(0x4003, 0xF8); // Pulse 1 length table entry 31 = 30.
        assert_eq!(bus.read(0x4015) & 0x01, 0x01);
        bus.tick_apu(7_457);
        assert_eq!(bus.read(0x4015) & 0x01, 0x01);
        bus.write(0x4015, 0x00);
        assert_eq!(bus.read(0x4015) & 0x0F, 0);
    }

    #[test]
    fn apu_registers_are_routed_and_status_tracks_channel_enable_bits() {
        let mut bus = CpuBus::new();
        bus.write(0x4000, 0xBF);
        bus.write(0x4015, 0x0D);
        bus.write(0x4003, 0xF8);
        bus.write(0x400B, 0xF8);
        bus.write(0x400F, 0xF8);
        bus.write(0x4017, 0x80);
        // Most APU registers are write-only; $4015 is the readable status register.
        assert_eq!(bus.read(0x4015), 0x0D);
        // Reading the second controller remains independent from APU frame-counter writes.
        bus.set_controller_state(1, 0x02);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        assert_eq!(bus.read(0x4017) & 1, 0);
    }

    #[test] fn attaches_cartridge() {
        let mut image = vec![0; 16 + 16384]; image[0..4].copy_from_slice(b"NES\x1A"); image[4] = 1;
        let mut b = CpuBus::new(); b.insert_cartridge(Nrom::parse(&image).unwrap()); assert!(b.has_cartridge());
    }
}
