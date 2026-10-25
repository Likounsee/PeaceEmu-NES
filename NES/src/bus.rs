//! CPU memory bus for the NES machine.

use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::ppu::Ppu;

/// CPU bus with 2 KiB internal RAM and an optional mapper-backed cartridge.
///
/// PPU registers, frame rendering, NMI, OAM DMA and serial controller ports are routed.
/// APU pulse/triangle/noise synthesis and an approximate DMC sample-fetch path are integrated;
/// audio mixing, DMA stalls and interrupt timing are not cycle-accurate.
#[derive(Debug)]
pub struct CpuBus {
    ram: [u8; 0x800],
    cartridge: Option<Cartridge>,
    open_bus: u8,
    ppu: Ppu,
    apu: Apu,
    pending_oam_dma: Option<u8>,
    controller_state: [u8; 2],
    controller_shift: [u8; 2],
    controller_strobe: bool,
    /// Instruction execution enables one PPU cycle per CPU bus access.
    instruction_timing: bool,
    instruction_bus_cycles: u16,
}
impl Default for CpuBus {
    fn default() -> Self { Self { ram: [0; 0x800], cartridge: None, open_bus: 0, ppu: Ppu::new(), apu: Apu::new(), pending_oam_dma: None, controller_state: [0; 2], controller_shift: [0; 2], controller_strobe: false, instruction_timing: false, instruction_bus_cycles: 0 } }
}
impl CpuBus {
    /// Creates a bus without a cartridge.
    pub fn new() -> Self { Self::default() }

    /// Resets volatile device state while preserving CPU RAM and the inserted cartridge.
    ///
    /// Controller button levels and the configured host audio sample rate are retained;
    /// controller serial latches, pending DMA, PPU timing, and APU state are reset.
    pub fn reset(&mut self) {
        let sample_rate = self.apu.sample_rate();
        self.open_bus = 0;
        self.ppu = Ppu::new();
        self.apu = Apu::new();
        let _ = self.apu.set_sample_rate(sample_rate);
        self.pending_oam_dma = None;
        self.controller_shift = [0; 2];
        self.controller_strobe = false;
    }
    /// Attaches a parsed cartridge.
    pub fn insert_cartridge(&mut self, cartridge: Cartridge) { self.cartridge = Some(cartridge); }

    /// Starts tracking CPU bus cycles during one instruction.
    pub(crate) fn begin_cpu_instruction(&mut self) {
        self.instruction_timing = true;
        self.instruction_bus_cycles = 0;
    }

    /// Stops bus-cycle tracking and returns cycles already advanced through PPU dots.
    pub(crate) fn end_cpu_instruction(&mut self) -> u16 {
        self.instruction_timing = false;
        core::mem::take(&mut self.instruction_bus_cycles)
    }

    /// Advances the PPU to the sampling point of the current CPU bus access.
    fn clock_instruction_bus_access(&mut self) {
        if self.instruction_timing {
            self.instruction_bus_cycles = self.instruction_bus_cycles.saturating_add(1);
            self.tick_ppu(3);
        }
    }

    /// Reads a byte from the CPU address space.
    pub fn read(&mut self, address: u16) -> u8 {
        self.clock_instruction_bus_access();
        let value = match address {
            0x0000..=0x1FFF => self.ram[address as usize & 0x07FF],
            0x6000..=0x7FFF => self.cartridge.as_ref().and_then(|c| c.cpu_read(address)).unwrap_or(self.open_bus),
            0x2000..=0x3FFF => self.ppu.cpu_read_register(address & 7, self.cartridge.as_ref()),
            // Bit 5 of $4015 is open bus on the 2A03; preserve the previous bus value.
            0x4015 => self.apu.read(address) | (self.open_bus & 0x20),
            0x4016..=0x4017 => self.read_controller((address - 0x4016) as usize, self.open_bus),
            0x8000..=0xFFFF => self.cartridge.as_ref().and_then(|c| c.cpu_read(address)).unwrap_or(self.open_bus),
            _ => self.open_bus,
        };
        self.open_bus = value;
        value
    }
    /// Writes a byte to the CPU address space. Cartridge ROM is not writable.
    pub fn write(&mut self, address: u16, value: u8) {
        self.clock_instruction_bus_access();
        self.open_bus = value;
        match address {
            0x0000..=0x1FFF => self.ram[address as usize & 0x07FF] = value,
            0x6000..=0x7FFF => { if let Some(cartridge) = self.cartridge.as_mut() { cartridge.cpu_write(address, value); } },
            0x2000..=0x3FFF => self.ppu.cpu_write_register(address & 7, value, self.cartridge.as_mut()),
            0x4000..=0x4013 | 0x4015 | 0x4017 => self.apu.write(address, value),
            0x4014 => self.pending_oam_dma = Some(value),
            0x8000..=0xFFFF => { if let Some(cartridge) = self.cartridge.as_mut() { cartridge.cpu_write(address, value); } },
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

    fn read_controller(&mut self, port: usize, open_bus: u8) -> u8 {
        // The controller drives D0 and pulls D6 high. Other CPU data-bus bits
        // retain their previous values instead of being forced to zero.
        let driven_bits = (open_bus & !0x41) | 0x40;
        if self.controller_strobe {
            return driven_bits | (self.controller_state[port] & 1);
        }
        let value = self.controller_shift[port] & 1;
        self.controller_shift[port] = (self.controller_shift[port] >> 1) | 0x80;
        driven_bits | value
    }

    /// Rebuilds the background framebuffer from the attached cartridge and current VRAM scroll.
    pub fn render_background(&mut self) { self.ppu.render_background(self.cartridge.as_ref()); }
    /// Renders the background and composites visible sprites into the indexed framebuffer.
    pub fn render_frame(&mut self) { self.ppu.render_frame(self.cartridge.as_ref()); }
    /// Returns the most recently rendered background as 256x240 NES palette indices.
    pub fn framebuffer(&self) -> &[u8] { self.ppu.framebuffer() }
    /// Advances the PPU by the specified number of dots.
    pub fn tick_ppu(&mut self, dots: u32) -> u32 {
        let crossed = self.ppu.tick_with_cartridge(dots, self.cartridge.as_ref());
        let events = self.ppu.take_mapper_bus_events();
        if let Some(cartridge) = self.cartridge.as_mut() {
            for (address, cycle) in events { cartridge.observe_ppu_address(address, cycle); }
        }
        crossed
    }
    /// Legacy scanline IRQ clock retained for diagnostics.
    pub fn clock_mapper_irq(&mut self) { if let Some(cartridge) = self.cartridge.as_mut() { cartridge.clock_irq_counter(); } }
    /// Returns whether background or sprite rendering is enabled in PPUMASK.
    pub fn ppu_rendering_enabled(&self) -> bool { self.ppu.rendering_enabled() }
    /// Returns the current PPU scanline and dot, for frame scheduling and diagnostics.
    pub fn ppu_position(&self) -> (u16, u16) { self.ppu.position() }
    /// Advances the APU by CPU cycles and services pending DMC sample reads.
    ///
    /// Returns the CPU stall cycles caused by DMC DMA. Each sample-buffer refill
    /// occupies four CPU cycles; alignment-specific details remain approximate.
    pub fn tick_apu(&mut self, cycles: u32) -> u32 {
        // Most games do not use DMC. Advance their APU cycles in a single batch
        // instead of checking the CPU bus for DMA on every cycle.
        if !self.apu.needs_dmc_bus_service() {
            self.apu.tick_cpu_cycles(cycles);
            return 0;
        }

        let mut stall_cycles = 0u32;
        for _ in 0..cycles {
            self.apu.tick_cpu_cycles(1);
            if let Some(address) = self.apu.take_dmc_dma_request() {
                let value = self.read(address);
                self.apu.supply_dmc_byte(value);
                stall_cycles = stall_cycles.saturating_add(4);
            }
        }
        stall_cycles
    }
    /// Returns one approximate normalized mono sample from the APU mixer.
    pub fn apu_sample(&self) -> f32 { self.apu.sample() }
    /// Returns the configured host PCM output rate in samples per second.
    pub fn audio_sample_rate(&self) -> u32 { self.apu.sample_rate() }
    /// Configures the APU's PCM output rate in samples per second.
    pub fn set_audio_sample_rate(&mut self, sample_rate: u32) -> bool {
        self.apu.set_sample_rate(sample_rate)
    }
    /// Drains generated mono PCM samples in playback order.
    pub fn drain_audio_samples(&mut self) -> Vec<f32> {
        self.apu.drain_audio_samples()
    }
    /// Returns the level-triggered APU IRQ line without clearing it.
    pub fn irq_pending(&self) -> bool { self.apu.irq_pending() || self.cartridge.as_ref().is_some_and(Cartridge::irq_pending) }
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

    /// Copies cartridge PRG-RAM without applying mapper runtime enable gates.
    pub fn cartridge_prg_ram_snapshot(&self) -> Option<Vec<u8>> {
        self.cartridge.as_ref().and_then(Cartridge::prg_ram_snapshot)
    }

    /// Restores a previously validated cartridge PRG-RAM payload.
    pub fn restore_cartridge_prg_ram(&mut self, data: &[u8]) -> bool {
        self.cartridge.as_mut().is_some_and(|cartridge| cartridge.restore_prg_ram(data))
    }

    /// Returns whether a cartridge is attached.
    pub fn has_cartridge(&self) -> bool { self.cartridge.is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dmc_sample_fetch_reports_four_cpu_stall_cycles() {
        let mut bus = CpuBus::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x1000] = 0xA5;
        bus.insert_cartridge(Cartridge::parse(&image).unwrap());
        bus.write(0x4010, 0x00);
        bus.write(0x4012, 0x00);
        bus.write(0x4013, 0x00);
        bus.write(0x4015, 0x10);
        assert_eq!(bus.tick_apu(1), 4);
        assert_eq!(bus.tick_apu(1), 0, "a filled sample buffer must not repeatedly stall the CPU");
    }

    #[test]
    fn dmc_dma_refills_charge_stall_for_each_sample_buffer_fetch() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16..16 + 16_384].fill(0xA5);
        let mut bus = CpuBus::new();
        bus.insert_cartridge(Cartridge::parse(&image).unwrap());
        bus.write(0x4010, 0x0F); // Fastest DMC rate.
        bus.write(0x4012, 0x00);
        bus.write(0x4013, 0x01); // 17-byte sample, long enough for multiple refills.
        bus.write(0x4015, 0x10);

        let stalls = bus.tick_apu(4_000);
        assert!(stalls >= 8, "multiple sample-buffer refills should each charge a four-cycle stall; got {stalls}");
        assert_eq!(stalls % 4, 0, "each current-model DMC fetch costs four CPU cycles");
    }

    #[test]
    fn mmc3_mapper_irq_is_visible_on_the_cpu_irq_line() {
        let mut image = vec![0; 16 + 2 * 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 2;
        image[5] = 1;
        image[6] = 0x40; // Mapper 4.
        let mut bus = CpuBus::new();
        bus.insert_cartridge(Cartridge::parse(&image).unwrap());
        bus.write(0xC000, 0); // IRQ latch = 0.
        bus.write(0xC001, 0); // Request reload.
        bus.write(0xE001, 0); // Enable mapper IRQ.
        assert!(!bus.irq_pending());
        bus.clock_mapper_irq();
        assert!(bus.irq_pending());
        bus.write(0xE000, 0); // Disable and acknowledge.
        assert!(!bus.irq_pending());
    }

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
    fn controller_strobe_high_tracks_live_a_button_state() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0);
        bus.write(0x4016, 1);
        assert_eq!(bus.read(0x4016) & 1, 0);

        // While strobe is high, reads expose the live A button rather than
        // consuming a latched serial bit.
        bus.set_controller_state(0, 1);
        assert_eq!(bus.read(0x4016) & 1, 1);
        bus.set_controller_state(0, 0);
        assert_eq!(bus.read(0x4016) & 1, 0);
    }

    #[test]
    fn controller_reads_preserve_all_undriven_open_bus_bits() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 1);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        bus.write(0x0000, 0xA4);
        assert_eq!(bus.read(0x4016), 0xE5);
    }

    #[test]
    fn latest_oam_dma_write_replaces_the_pending_page() {
        let mut bus = CpuBus::new();
        bus.write(0x4014, 0x02);
        bus.write(0x4014, 0x07);
        assert_eq!(bus.take_oam_dma_request(), Some(0x07));
        assert_eq!(bus.take_oam_dma_request(), None);
    }

    #[test]
    fn oam_dma_copies_all_256_bytes_and_wraps_from_current_oam_address() {
        let mut bus = CpuBus::new();
        for offset in 0..=u8::MAX {
            bus.write(0x0200 | u16::from(offset), offset.wrapping_mul(3));
        }
        bus.write(0x2003, 0x80);
        bus.transfer_oam_dma(0x02);

        for destination in 0..=u8::MAX {
            bus.write(0x2003, destination);
            let source_offset = destination.wrapping_sub(0x80);
            assert_eq!(bus.read(0x2004), source_offset.wrapping_mul(3));
        }
    }

    #[test]
    fn reset_discards_pending_oam_dma_but_keeps_the_cartridge_attached() {
        let mut bus = CpuBus::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        bus.insert_cartridge(Cartridge::parse(&image).unwrap());
        bus.write(0x4014, 0x03);
        bus.reset();
        assert_eq!(bus.take_oam_dma_request(), None);
        assert!(bus.has_cartridge());
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
        bus.tick_apu(14_919); // Include the delayed $4017 sequencer reset.
        assert!(bus.irq_pending());
        bus.write(0x4000, 0x20); // Put bit 5 high on the CPU data bus before reading status.
        let status = bus.read(0x4015);
        assert_ne!(status & 0x20, 0);
        assert_ne!(status & 0x40, 0);
        assert!(!bus.irq_pending());
    }

    #[test]
    fn ppu_position_advances_and_wraps_through_the_cpu_bus() {
        let mut bus = CpuBus::new();
        assert_eq!(bus.ppu_position(), (261, 0));
        bus.tick_ppu(341);
        assert_eq!(bus.ppu_position(), (0, 0));
        bus.tick_ppu(17);
        assert_eq!(bus.ppu_position(), (0, 17));
    }

    #[test]
    fn dmc_bus_fetches_follow_sample_address_register_and_buffer_refills() {
        let mut bus = CpuBus::new();
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x100] = 0xFF;
        image[16 + 0x101] = 0x00;
        bus.insert_cartridge(Cartridge::parse(&image).unwrap());
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
        bus.insert_cartridge(Cartridge::parse(&image).unwrap());
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
        let mut b = CpuBus::new(); b.insert_cartridge(Cartridge::parse(&image).unwrap()); assert!(b.has_cartridge());
    }
    #[test]
    fn cartridge_prg_ram_is_visible_through_cpu_bus() {
        let mut bus = CpuBus::new();
        let mut image = vec![0; 16 + 16 * 1024];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        bus.insert_cartridge(Cartridge::parse(&image).unwrap());
        bus.write(0x6000, 0x12);
        bus.write(0x7FFF, 0x34);
        assert_eq!(bus.read(0x6000), 0x12);
        assert_eq!(bus.read(0x7FFF), 0x34);
        bus.write(0x8000, 0x99);
        assert_eq!(bus.read(0x8000), 0);
    }


    #[test]
    fn oam_dma_request_tracks_the_latest_page_and_is_consumed_once() {
        let mut bus = CpuBus::new();
        bus.write(0x4014, 0x02);
        bus.write(0x4014, 0x7F);
        assert_eq!(bus.take_oam_dma_request(), Some(0x7F));
        assert_eq!(bus.take_oam_dma_request(), None);
    }

    #[test]
    fn controller_serial_reads_shift_in_ones_after_eight_buttons() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0b1010_0101);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        let bits: Vec<u8> = (0..8).map(|_| bus.read(0x4016) & 1).collect();
        assert_eq!(bits, vec![1, 0, 1, 0, 0, 1, 0, 1]);
        assert_eq!(bus.read(0x4016) & 1, 1, "controller reads return 1 after the eight latched buttons");
        assert_eq!(bus.read(0x4016) & 1, 1, "post-button reads remain high");
    }

    #[test]
    fn controller_reads_preserve_cpu_open_bus_bit_five() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0);
        bus.write(0x4016, 1);

        bus.write(0x4000, 0x20);
        assert_ne!(bus.read(0x4016) & 0x20, 0, "bit 5 should retain the high open-bus value");

        bus.write(0x4000, 0x00);
        assert_eq!(bus.read(0x4016) & 0x20, 0, "bit 5 should retain the low open-bus value");
        assert_ne!(bus.read(0x4016) & 0x40, 0, "controller bit 6 is driven high");
    }
    
    #[test]
    fn cpu_ram_mirrors_every_2_kib_and_open_bus_is_retained_on_unmapped_reads() {
        let mut bus = CpuBus::new();
        bus.write(0x0002, 0x5A);
        assert_eq!(bus.read(0x0802), 0x5A);
        assert_eq!(bus.read(0x1002), 0x5A);
        assert_eq!(bus.read(0x1802), 0x5A);

        bus.write(0x4000, 0xC3);
        assert_eq!(bus.read(0x5000), 0xC3, "unmapped reads preserve the CPU data bus latch");
    }

    #[test]
    fn controller_strobe_high_tracks_live_button_state_without_shifting() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0);
        bus.write(0x4016, 1);
        assert_eq!(bus.read(0x4016) & 1, 0);
        bus.set_controller_state(0, 1);
        assert_eq!(bus.read(0x4016) & 1, 1, "while strobe is high, reads expose the current A button");
        bus.set_controller_state(0, 0);
        assert_eq!(bus.read(0x4016) & 1, 0);
    }

#[test]
    fn invalid_controller_port_updates_are_ignored_without_affecting_valid_ports() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0b0000_0001);
        bus.set_controller_state(1, 0b0000_0010);
        bus.set_controller_state(2, 0xFF);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        assert_eq!(bus.read(0x4016) & 1, 1);
        assert_eq!(bus.read(0x4017) & 1, 0);
        assert_eq!(bus.read(0x4016) & 1, 0);
        assert_eq!(bus.read(0x4017) & 1, 1);
    }

    #[test]
    fn reset_preserves_cartridge_ram_controller_levels_and_audio_rate_but_clears_pending_io() {
        let mut image = vec![0; 16 + 16 * 1024];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        let mut bus = CpuBus::new();
        bus.insert_cartridge(Cartridge::parse(&image).unwrap());
        bus.write(0x6000, 0xA6);
        bus.set_controller_state(0, 0x01);
        bus.write(0x4014, 0x03);
        assert!(bus.set_audio_sample_rate(48_000));

        bus.reset();

        assert_eq!(bus.read(0x6000), 0xA6, "reset must preserve cartridge RAM");
        assert_eq!(bus.audio_sample_rate(), 48_000, "reset must preserve the host audio format");
        assert_eq!(bus.take_oam_dma_request(), None, "reset must discard pending DMA");
        assert_eq!(bus.ppu_position(), (261, 0), "PPU timing returns to pre-render");
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        assert_eq!(bus.read(0x4016) & 1, 1, "live controller button levels survive reset");
    }

}


#[cfg(test)]
mod controller_strobe_regression_tests {
    use super::*;

    #[test]
    fn high_strobe_reads_live_a_button_state_until_strobe_falls() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0);
        bus.write(0x4016, 1);
        assert_eq!(bus.read(0x4016) & 1, 0);

        bus.set_controller_state(0, 1);
        assert_eq!(bus.read(0x4016) & 1, 1, "high strobe exposes the current A-button level");
        bus.set_controller_state(0, 0);
        assert_eq!(bus.read(0x4016) & 1, 0);

        bus.set_controller_state(0, 0b1000_0001);
        bus.write(0x4016, 0);
        let bits: Vec<u8> = (0..8).map(|_| bus.read(0x4016) & 1).collect();
        assert_eq!(bits, [1, 0, 0, 0, 0, 0, 0, 1]);
    }
}


#[cfg(test)]
mod open_bus_regression_tests {
    use super::*;

    #[test]
    fn controller_reads_preserve_unconnected_bus_bits() {
        let mut bus = CpuBus::new();
        bus.write(0x0000, 0xA4);
        bus.write(0x4016, 0xA5); // High strobe with a known open-bus pattern.
        bus.set_controller_state(0, 1);
        let value = bus.read(0x4016);
        assert_eq!(value & 1, 1);
        assert_eq!(value & 0x40, 0x40, "controller drives D6 high");
        assert_eq!(value & 0xBE, 0xA4 & 0xBE, "undriven data-bus bits retain open-bus values");
    }
    #[test]
    fn oam_dma_wraps_from_the_current_oam_address() {
        let mut bus = CpuBus::new();
        for offset in 0..=u8::MAX {
            bus.write(0x0200 | u16::from(offset), offset ^ 0x5A);
        }
        bus.write(0x2003, 0xFE);
        bus.write(0x4014, 0x02);
        let page = bus.take_oam_dma_request().expect("DMA page should be queued");
        bus.transfer_oam_dma(page);

        for (address, offset) in [(0xFEu8, 0u8), (0xFF, 1), (0x00, 2), (0x01, 3)] {
            bus.write(0x2003, address);
            assert_eq!(bus.read(0x2004), offset ^ 0x5A, "OAM wraps at 0xFF");
        }
        assert_eq!(bus.take_oam_dma_request(), None);
    }

}


#[cfg(test)]
mod io_regression_matrix {
    use super::*;

    #[test]
    fn controller_ports_keep_independent_latches_after_repeated_strobes() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0b1010_0101);
        bus.set_controller_state(1, 0b0101_1010);
        for _ in 0..2 {
            bus.write(0x4016, 1);
            bus.write(0x4016, 0);
        }
        let first: Vec<u8> = (0..8).map(|_| bus.read(0x4016) & 1).collect();
        let second: Vec<u8> = (0..8).map(|_| bus.read(0x4017) & 1).collect();
        assert_eq!(first, [1, 0, 1, 0, 0, 1, 0, 1]);
        assert_eq!(second, [0, 1, 0, 1, 1, 0, 1, 0]);
        assert_eq!(bus.read(0x4016) & 1, 1);
        assert_eq!(bus.read(0x4017) & 1, 1);
    }

    #[test]
    fn reset_keeps_live_controller_buttons_but_restarts_serial_latches() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0b0000_0101);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        assert_eq!(bus.read(0x4016) & 1, 1);
        bus.reset();
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        assert_eq!(bus.read(0x4016) & 1, 1);
        assert_eq!(bus.read(0x4016) & 1, 0);
        assert_eq!(bus.read(0x4016) & 1, 1);
    }
}

#[cfg(test)]
mod mmc3_ppu_irq_integration_regressions {
    use super::*;

    fn mmc3_bus() -> CpuBus {
        let mut image = vec![0; 16 + 2 * 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 2;
        image[5] = 1;
        image[6] = 0x40; // Mapper 4.
        let mut bus = CpuBus::new();
        bus.insert_cartridge(Cartridge::parse(&image).unwrap());
        bus.write(0xC000, 0); // IRQ latch = 0.
        bus.write(0xC001, 0); // Reload on the next qualified A12 edge.
        bus.write(0xE001, 0); // Enable mapper IRQ.
        bus
    }

    #[test]
    fn ppu_generated_a12_edge_clocks_mmc3_irq_without_diagnostic_hook() {
        let mut bus = mmc3_bus();
        bus.write(0x2000, 0x08); // Sprite pattern table at $1000; background table at $0000.
        bus.write(0x2001, 0x18); // Enable both rendering layers.
        assert!(!bus.irq_pending());

        // On the pre-render scanline, sprite pattern fetches drive A12 high after
        // a long low interval. The real PPU bus event must clock the mapper IRQ.
        bus.tick_ppu(261);
        assert!(bus.irq_pending(), "the PPU-generated qualified A12 edge should assert MMC3 IRQ");
    }

    #[test]
    fn ppu_reset_rebases_mapper_a12_filter_without_false_irq() {
        let mut bus = mmc3_bus();
        bus.write(0x2000, 0x08);
        bus.write(0x2001, 0x18);
        bus.tick_ppu(261);
        assert!(bus.irq_pending(), "fixture must first clock a real mapper IRQ");

        bus.write(0xE000, 0); // Acknowledge and disable IRQ before resetting the PPU.
        bus.reset(); // Cartridge remains inserted while the PPU dot counter restarts.
        bus.write(0xC001, 0);
        bus.write(0xE001, 0);
        bus.write(0x2000, 0x10);
        bus.write(0x2001, 0x08);
        bus.tick_ppu(9);

        assert!(
            !bus.irq_pending(),
            "a backwards PPU timestamp after reset must rebase the filter, not synthesize a long low interval"
        );
    }

    #[test]
    fn short_a12_pulse_does_not_clock_mmc3_irq() {
        let mut bus = mmc3_bus();
        bus.write(0x2000, 0x10); // Background pattern table at $1000.
        bus.write(0x2001, 0x08); // Background rendering only.
        bus.tick_ppu(9); // First high pulse lasts five dots before A12 falls again.
        assert!(!bus.irq_pending(), "an A12 high edge after fewer than eight low dots must be filtered");
    }

    #[test]
    fn disabling_ppu_rendering_prevents_mmc3_irq_edges() {
        let mut bus = mmc3_bus();
        bus.write(0x2000, 0x08);
        bus.write(0x2001, 0x00);
        bus.tick_ppu(341 * 2);
        assert!(!bus.irq_pending(), "idle PPU dots must not synthesize MMC3 scanline IRQs");
    }
}

#[cfg(test)]
mod instruction_bus_timing_regressions {
    use super::*;

    #[test]
    fn status_read_observes_vblank_reached_during_operand_fetches() {
        let mut bus = CpuBus::new();
        bus.tick_ppu(242 * 341);
        assert_eq!(bus.ppu_position(), (241, 0));
        bus.begin_cpu_instruction();

        // BIT $2002 fetches opcode and its two address bytes before reading status.
        let _opcode = bus.read(0x8000);
        let _low = bus.read(0x8001);
        let _high = bus.read(0x8002);
        let status = bus.read(0x2002);
        let cycles = bus.end_cpu_instruction();

        assert_eq!(cycles, 4);
        assert_ne!(status & 0x80, 0, "vblank begins before the fourth-cycle status sample");
        assert_eq!(bus.read(0x2002) & 0x80, 0, "a second $2002 read sees the cleared vblank flag");
        assert_eq!(bus.ppu_position(), (241, 12));
    }

    #[test]
    fn direct_debug_reads_do_not_advance_emulated_time() {
        let mut bus = CpuBus::new();
        assert_eq!(bus.ppu_position(), (261, 0));
        let _ = bus.read(0x2002);
        assert_eq!(bus.ppu_position(), (261, 0));
    }
}

#[cfg(test)]
mod added_controller_regression_tests {
    use super::*;

    #[test]
    fn controller_ports_latch_and_shift_independently() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0b0000_0101);
        bus.set_controller_state(1, 0b0000_1010);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);

        let port_one: Vec<u8> = (0..4).map(|_| bus.read(0x4016) & 1).collect();
        let port_two: Vec<u8> = (0..4).map(|_| bus.read(0x4017) & 1).collect();
        assert_eq!(port_one, [1, 0, 1, 0]);
        assert_eq!(port_two, [0, 1, 0, 1]);
    }

    #[test]
    fn invalid_controller_port_does_not_panic_or_mutate_existing_ports() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 1);
        bus.set_controller_state(1, 2);
        bus.set_controller_state(usize::MAX, 0xFF);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        assert_eq!(bus.read(0x4016) & 1, 1);
        assert_eq!(bus.read(0x4017) & 1, 0);
    }
}


#[cfg(test)]
mod controller_strobe_regressions {
    use super::*;

    #[test]
    fn high_strobe_keeps_reading_live_a_button_without_shifting() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0b0000_0001);
        bus.write(0x4016, 1);
        assert_eq!(bus.read(0x4016) & 1, 1);
        assert_eq!(bus.read(0x4016) & 1, 1,
            "while strobe is high, reads must repeatedly expose the live A button");

        bus.set_controller_state(0, 0);
        assert_eq!(bus.read(0x4016) & 1, 0,
            "the A button is sampled live until the strobe is lowered");

        bus.set_controller_state(0, 0b0000_0010);
        bus.write(0x4016, 0);
        assert_eq!(bus.read(0x4016) & 1, 0,
            "after the falling edge, the latched A bit is read first");
        assert_eq!(bus.read(0x4016) & 1, 1,
            "the next serial read advances to the latched B bit");
    }
}


#[cfg(test)]
mod controller_serial_tail_regressions {
    use super::*;

    #[test]
    fn controller_returns_one_after_all_eight_latched_buttons_are_read() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0b1010_0101);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);

        let bits: Vec<u8> = (0..8).map(|_| bus.read(0x4016) & 1).collect();
        assert_eq!(bits, [1, 0, 1, 0, 0, 1, 0, 1]);
        for _ in 0..4 {
            assert_eq!(bus.read(0x4016) & 1, 1,
                "unconnected serial bits after the eighth button are pulled high");
        }
    }
}


#[cfg(test)]
mod oam_dma_wrap_regressions {
    use super::*;

    #[test]
    fn oam_dma_respects_start_address_and_wraps_after_256_bytes() {
        let mut bus = CpuBus::new();
        for offset in 0..=255u16 {
            bus.write(0x0200 + offset, offset as u8 ^ 0xA5);
        }
        bus.write(0x2003, 0xFE);

        bus.transfer_oam_dma(0x02);

        // DMA begins at OAMADDR, wraps through $FF to $00, and ends at $FE.
        for (address, expected) in [(0xFEu8, 0xA5u8), (0xFF, 0xA4), (0x00, 0xA7), (0x01, 0xA6)] {
            bus.write(0x2003, address);
            assert_eq!(bus.read(0x2004), expected,
                "OAM DMA must preserve the starting address and wrap around OAM");
        }
    }
}


#[cfg(test)]
mod apu_bus_open_bus_regressions {
    use super::*;

    #[test]
    fn status_read_preserves_open_bus_bit_five() {
        let mut bus = CpuBus::new();
        bus.write(0x0000, 0x20); // Leave bit 5 high on the CPU data bus.

        let status = bus.read(0x4015);
        assert_eq!(status & 0x20, 0x20, "$4015 bit 5 is open bus");
        assert_eq!(bus.read(0x4015) & 0x20, 0x20,
            "the undriven bit remains sourced from the previous bus value");
    }
}
