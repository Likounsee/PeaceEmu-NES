//! Small top-level NES machine wrapper for loading and stepping cartridge programs.

use crate::bus::CpuBus;
use crate::cartridge::{Cartridge, CartridgeError};
use crate::cpu::{Cpu, CpuError, CpuState, StepResult};
use core::fmt;

/// Error raised while creating or executing a basic NES machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MachineError {
    /// Cartridge image could not be parsed or is unsupported.
    Cartridge(CartridgeError),
    /// CPU encountered an unsupported instruction.
    Cpu(CpuError),
    /// A frame could not be reached within the safety instruction budget.
    FrameTimeout,
    /// The loaded cartridge does not declare battery-backed save RAM.
    NoBatteryRam,
    /// Save RAM payload does not match the cartridge's exact RAM size.
    InvalidSaveRamLength {
        /// Number of bytes required by this cartridge.
        expected: usize,
        /// Number of bytes supplied by the caller.
        actual: usize,
    },
}

impl fmt::Display for MachineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cartridge(error) => write!(f, "invalid NES cartridge: {error}"),
            Self::Cpu(error) => write!(f, "NES CPU error: {error}"),
            Self::FrameTimeout => write!(f, "NES frame step exceeded its instruction budget"),
            Self::NoBatteryRam => write!(f, "NES cartridge does not declare battery-backed RAM"),
            Self::InvalidSaveRamLength { expected, actual } => write!(f, "invalid NES save RAM length: expected {expected} bytes, got {actual}"),
        }
    }
}

impl std::error::Error for MachineError {}

impl From<CartridgeError> for MachineError {
    fn from(value: CartridgeError) -> Self { Self::Cartridge(value) }
}

impl From<CpuError> for MachineError {
    fn from(value: CpuError) -> Self { Self::Cpu(value) }
}

/// CPU plus the current minimal bus and a mapper-selected cartridge.
///
/// The wrapper supports instruction stepping, frame execution, snapshot rendering, controller
/// input, and queued PCM output. The PPU renderer and hardware timing remain approximate,
/// so commercial-game compatibility is not yet established.
#[derive(Debug)]
pub struct NesMachine {
    cpu: Cpu,
    bus: CpuBus,
    battery_backed: bool,
}

impl NesMachine {
    /// Parses an iNES image, attaches its mapper-selected cartridge, and performs CPU reset.
    pub fn from_ines(image: &[u8]) -> Result<Self, MachineError> {
        let cartridge = Cartridge::parse(image)?;
        let battery_backed = image.get(6).is_some_and(|flags| flags & 0x02 != 0);
        let mut bus = CpuBus::new();
        bus.insert_cartridge(cartridge);
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus);
        Ok(Self { cpu, bus, battery_backed })
    }

    /// Updates controller 1 or 2 button state.
    ///
    /// Button bits, from least to most significant: A, B, Select, Start, Up, Down, Left,
    /// Right. The CPU reads latched bits serially through $4016/$4017.
    pub fn set_controller_state(&mut self, port: usize, buttons: u8) {
        self.bus.set_controller_state(port, buttons);
    }

    /// Renders a background snapshot for frontends that need an indexed 256x240 image.
    /// Call after updating PPU scroll/register state; per-dot rendering is not implemented yet.
    pub fn render_background(&mut self) { self.bus.render_background(); }

    /// Renders a background snapshot and composites sprites; timing remains frame-based.
    pub fn render_frame(&mut self) { self.bus.render_frame(); }

    /// Returns the most recently rendered 256x240 background as 6-bit NES palette indices.
    pub fn framebuffer(&self) -> &[u8] { self.bus.framebuffer() }

    /// Converts the indexed framebuffer to RGBA8 using a frontend-provided 64-color palette.
    ///
    /// Supplying the palette keeps regional/host palette choices outside the emulation core.
    /// The returned buffer is tightly packed as R, G, B, A bytes for each of 256x240 pixels.
    pub fn framebuffer_rgba(&self, palette: &[[u8; 4]; 64]) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(self.bus.framebuffer().len() * 4);
        for &index in self.bus.framebuffer() {
            rgba.extend_from_slice(&palette[(index & 0x3F) as usize]);
        }
        rgba
    }

    /// Returns a snapshot of the CPU registers and flags.
    pub fn cpu_state(&self) -> CpuState { self.cpu.state() }

    /// Returns the accumulated nominal CPU cycle count, including reset.
    pub fn total_cycles(&self) -> u64 { self.cpu.total_cycles() }

    /// Returns one approximate normalized mono audio sample (0.0..1.0).
    pub fn audio_sample(&self) -> f32 { self.bus.apu_sample() }

    /// Configures audio output sampling rate (8,000..=192,000 samples/second).
    ///
    /// Returns false for unsupported rates. A successful change clears queued samples
    /// and resets the fractional resampling phase.
    pub fn set_audio_sample_rate(&mut self, sample_rate: u32) -> bool {
        self.bus.set_audio_sample_rate(sample_rate)
    }

    /// Drains mono PCM samples generated since the previous drain.
    ///
    /// The default output rate is 44.1 kHz. Frontends should drain this queue regularly;
    /// the queue is bounded and discards the oldest samples if a consumer falls behind.
    pub fn drain_audio_samples(&mut self) -> Vec<f32> {
        self.bus.drain_audio_samples()
    }

    /// Returns whether the loaded iNES image declares battery-backed save RAM.
    pub fn has_battery_backed_ram(&self) -> bool { self.battery_backed }

    /// Copies battery-backed PRG-RAM for persistence by the frontend.
    ///
    /// The returned payload is the raw RAM region, not a full emulator save state.
    pub fn save_battery_ram(&self) -> Result<Vec<u8>, MachineError> {
        if !self.battery_backed { return Err(MachineError::NoBatteryRam); }
        self.bus.cartridge_prg_ram_snapshot().ok_or(MachineError::NoBatteryRam)
    }

    /// Restores battery-backed PRG-RAM after validating its entire payload.
    pub fn load_battery_ram(&mut self, data: &[u8]) -> Result<(), MachineError> {
        if !self.battery_backed { return Err(MachineError::NoBatteryRam); }
        let expected = self.bus.cartridge_prg_ram_snapshot()
            .ok_or(MachineError::NoBatteryRam)?.len();
        if data.len() != expected {
            return Err(MachineError::InvalidSaveRamLength { expected, actual: data.len() });
        }
        if !self.bus.restore_cartridge_prg_ram(data) {
            return Err(MachineError::InvalidSaveRamLength { expected, actual: data.len() });
        }
        Ok(())
    }

    /// Executes instructions until the next complete PPU frame boundary.
    ///
    /// On a freshly reset machine, the initial pre-render-to-visible transition is skipped
    /// so this method advances a full frame instead of returning after startup scanline 261.
    /// Returns nominal CPU cycles consumed; instruction granularity means the boundary may
    /// be crossed by a few dots. The current snapshot renderer runs once at the boundary.
    pub fn step_frame(&mut self) -> Result<u32, MachineError> {
        let start_cycles = self.cpu.total_cycles();
        let (start_scanline, _) = self.bus.ppu_position();
        let mut previous_scanline = start_scanline;
        let mut skipped_initial_prerender = start_scanline == 261;
        let mut iterations = 0u32;
        loop {
            self.step()?;
            iterations += 1;
            let (scanline, _) = self.bus.ppu_position();
            let crossed_frame = previous_scanline > scanline;
            if crossed_frame {
                if skipped_initial_prerender {
                    skipped_initial_prerender = false;
                } else {
                    self.bus.render_frame();
                    return Ok(self.cpu.total_cycles().saturating_sub(start_cycles) as u32);
                }
            }
            previous_scanline = scanline;
            // Prevent a malformed ROM from trapping a host frontend forever.
            if iterations >= 100_000 {
                return Err(MachineError::FrameTimeout);
            }
        }
    }

    /// Executes several complete frames for frontend loops and returns total CPU cycles.
    ///
    /// Each frame is rendered before continuing. A zero frame count is a no-op; if a
    /// later frame fails, earlier frames remain executed and the error is returned.
    pub fn run_frames(&mut self, frames: usize) -> Result<u64, MachineError> {
        let mut total_cycles = 0u64;
        for _ in 0..frames {
            total_cycles = total_cycles.saturating_add(u64::from(self.step_frame()?));
        }
        Ok(total_cycles)
    }

    /// Executes one CPU instruction and services any OAM DMA it requests.
    pub fn step(&mut self) -> Result<StepResult, MachineError> {
        let result = self.cpu.step(&mut self.bus)?;
        let mut elapsed_cycles = u16::from(result.cycles);
        if let Some(page) = self.bus.take_oam_dma_request() {
            // DMA occupies 513 CPU cycles, plus one alignment cycle on odd parity.
            let dma_cycles = if self.cpu.total_cycles() & 1 == 0 { 513 } else { 514 };
            self.bus.transfer_oam_dma(page);
            self.cpu.add_stall_cycles(dma_cycles);
            elapsed_cycles = elapsed_cycles.saturating_add(dma_cycles);
        }
        // Count every scanline boundary, not just whether the final scanline differs.
        // This matters for OAM DMA stalls, which advance the PPU across several scanlines.
        // MMC3 still uses scanline-boundary approximation until real PPU A12 fetch edges
        // are exposed by the renderer.
        let crossed_scanlines = self.bus.tick_ppu(u32::from(elapsed_cycles) * 3);
        for _ in 0..crossed_scanlines {
            self.bus.clock_mapper_irq();
        }
        self.bus.tick_apu(u32::from(elapsed_cycles));
        Ok(result)
    }

    /// Reads a byte through the CPU address bus.
    pub fn read_cpu(&mut self, address: u16) -> u8 { self.bus.read(address) }

    /// Writes a byte through the CPU address bus.
    pub fn write_cpu(&mut self, address: u16, value: u8) { self.bus.write(address, value); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nrom_with_program(program: &[u8]) -> Vec<u8> {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16..16 + program.len()].copy_from_slice(program);
        image[16 + 0x3FFC] = 0x00;
        image[16 + 0x3FFD] = 0x80;
        image
    }

    #[test]
    fn machine_loads_uxrom_and_switches_prg_bank_through_cpu_bus() {
        const BANK_LEN: usize = 16 * 1024;
        let mut image = vec![0; 16 + 3 * BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 3;
        image[6] = 0x20; // Mapper 2 (UxROM).
        for bank in 0..3 {
            image[16 + bank * BANK_LEN..16 + (bank + 1) * BANK_LEN].fill(bank as u8);
        }
        // Reset vector lives in the fixed final bank at CPU $FFFC-$FFFD.
        image[16 + 2 * BANK_LEN + 0x3FFC] = 0x00;
        image[16 + 2 * BANK_LEN + 0x3FFD] = 0xC0;

        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.cpu_state().pc, 0xC000);
        assert_eq!(machine.read_cpu(0x8000), 0);
        assert_eq!(machine.read_cpu(0xC000), 2);

        machine.write_cpu(0x8000, 1);
        assert_eq!(machine.read_cpu(0x8000), 1);
        assert_eq!(machine.read_cpu(0xBFFF), 1);
        assert_eq!(machine.read_cpu(0xC000), 2);
    }

    #[test]
    fn machine_loads_mmc1_and_routes_serial_mapper_writes() {
        const BANK_LEN: usize = 16 * 1024;
        let mut image = vec![0; 16 + 4 * BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 4;
        image[6] = 0x10; // Mapper 1 (MMC1), horizontal initial mirroring.
        for bank in 0..4 {
            image[16 + bank * BANK_LEN..16 + (bank + 1) * BANK_LEN].fill(bank as u8);
        }
        image[16 + 3 * BANK_LEN + 0x3FFC] = 0x00;
        image[16 + 3 * BANK_LEN + 0x3FFD] = 0xC0;

        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.cpu_state().pc, 0xC000);
        assert_eq!(machine.read_cpu(0x8000), 0);
        assert_eq!(machine.read_cpu(0xC000), 3);

        // MMC1 serial register commits bit 0 first after five writes.
        for bit in [0, 1, 0, 0, 0] {
            machine.write_cpu(0xE000, bit);
        }
        assert_eq!(machine.read_cpu(0x8000), 2);
        assert_eq!(machine.read_cpu(0xC000), 3);
    }

    #[test]
    fn machine_loads_gxrom_and_routes_prg_chr_bank_selection() {
        const PRG_BANK_LEN: usize = 16 * 1024;
        const CHR_BANK_LEN: usize = 8 * 1024;
        let mut image = vec![0; 16 + 4 * PRG_BANK_LEN + 3 * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 4;
        image[5] = 3;
        image[6] = 0x20; // Mapper 66 low nibble.
        image[7] = 0x40; // Mapper 66 high nibble.
        for bank in 0..2 {
            let start = 16 + bank * 2 * PRG_BANK_LEN;
            image[start..start + 2 * PRG_BANK_LEN].fill((0x30 + bank) as u8);
        }
        let chr_start = 16 + 4 * PRG_BANK_LEN;
        for bank in 0..3 {
            let start = chr_start + bank * CHR_BANK_LEN;
            image[start..start + CHR_BANK_LEN].fill((0x60 + bank) as u8);
        }
        // The initial 32 KiB bank pair contains the reset vector at $FFFC.
        image[16 + 2 * PRG_BANK_LEN - 4] = 0x00;
        image[16 + 2 * PRG_BANK_LEN - 3] = 0xC0;

        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.cpu_state().pc, 0xC000);
        assert_eq!(machine.read_cpu(0x8000), 0x30);
        assert_eq!(machine.read_cpu(0xC000), 0x30);

        machine.write_cpu(0x8000, 0x11); // PRG bank 1 and CHR bank 1.
        assert_eq!(machine.read_cpu(0x8000), 0x31);
        assert_eq!(machine.read_cpu(0xC000), 0x31);
    }


    #[test]
    fn machine_loads_mmc3_and_routes_prg_bank_registers() {
        const PRG_BANK_LEN: usize = 16 * 1024;
        let mut image = vec![0; 16 + 4 * PRG_BANK_LEN + 8 * 1024];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 4;
        image[5] = 1;
        image[6] = 0x40; // Mapper 4 (MMC3).
        for bank in 0..8 {
            let start = 16 + bank * 8 * 1024;
            image[start..start + 8 * 1024].fill(bank as u8);
        }
        // Reset vector must live in the fixed final 8 KiB bank.
        image[16 + 4 * PRG_BANK_LEN - 4] = 0x00;
        image[16 + 4 * PRG_BANK_LEN - 3] = 0xE0;

        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.cpu_state().pc, 0xE000);
        assert_eq!(machine.read_cpu(0x8000), 0);
        assert_eq!(machine.read_cpu(0xC000), 6);
        assert_eq!(machine.read_cpu(0xE000), 7);
        machine.write_cpu(0x8000, 6);
        machine.write_cpu(0x8001, 3);
        assert_eq!(machine.read_cpu(0x8000), 3);
        assert_eq!(machine.read_cpu(0xC000), 6);
        assert_eq!(machine.read_cpu(0xE000), 7);
    }

    #[test]
    fn machine_loads_reset_vector_and_executes_program() {
        let image = nrom_with_program(&[0xA9, 0x42, 0x8D, 0x00, 0x02, 0xEA]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.cpu_state().pc, 0x8000);
        assert_eq!(machine.total_cycles(), 7);
        assert_eq!(machine.step().unwrap().cycles, 2);
        assert_eq!(machine.cpu_state().a, 0x42);
        assert_eq!(machine.step().unwrap().cycles, 4);
        assert_eq!(machine.read_cpu(0x0200), 0x42);
        assert_eq!(machine.total_cycles(), 13);
    }

    #[test]
    fn machine_services_oam_dma_and_counts_stall_cycles() {
        let image = nrom_with_program(&[0xA9, 0x02, 0x8D, 0x14, 0x40, 0xEA]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        for i in 0..=u8::MAX {
            machine.write_cpu(0x0200 | u16::from(i), i ^ 0x5A);
        }
        machine.step().unwrap(); // LDA #$02
        machine.step().unwrap(); // STA $4014 starts DMA
        assert_eq!(machine.total_cycles(), 7 + 2 + 4 + 514);
        for i in 0..=u8::MAX {
            machine.write_cpu(0x2003, i);
            assert_eq!(machine.read_cpu(0x2004), i ^ 0x5A);
        }
    }

    #[test]
    fn machine_step_frame_advances_ppu_and_populates_framebuffer() {
        let image = nrom_with_program(&[0xEA, 0x4C, 0x00, 0x80]); // NOP; JMP $8000
        let mut machine = NesMachine::from_ines(&image).unwrap();
        let before = machine.total_cycles();
        let elapsed = machine.step_frame().unwrap();
        assert!(elapsed > 29_000 && elapsed < 31_000, "one NTSC frame should take about 29.8k CPU cycles");
        assert_eq!(machine.total_cycles() - before, u64::from(elapsed));
        assert_eq!(machine.framebuffer().len(), 256 * 240);
        assert_eq!(machine.framebuffer()[0], 0);
    }

    #[test]
    fn run_frames_batches_rendered_frames_and_reports_total_cycles() {
        let image = nrom_with_program(&[0xEA, 0x4C, 0x00, 0x80]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        let before = machine.total_cycles();
        assert_eq!(machine.run_frames(0).unwrap(), 0);
        let elapsed = machine.run_frames(3).unwrap();
        assert!(elapsed > 89_000 && elapsed < 93_000, "three NTSC frames should take about 89.4k cycles");
        assert_eq!(machine.total_cycles() - before, elapsed);
        assert_eq!(machine.framebuffer().len(), 256 * 240);
        assert!(machine.drain_audio_samples().len() > 2_000);
    }

    #[test]
    fn consecutive_frame_steps_keep_advancing_and_return_frame_sized_cycle_counts() {
        let image = nrom_with_program(&[0xEA, 0x4C, 0x00, 0x80]); // NOP; JMP $8000
        let mut machine = NesMachine::from_ines(&image).unwrap();
        let first = machine.step_frame().unwrap();
        let second = machine.step_frame().unwrap();
        assert!(first > 29_000 && first < 31_000);
        assert!(second > 29_000 && second < 31_000);
        assert_eq!(machine.total_cycles(), 7 + u64::from(first) + u64::from(second));
        assert_eq!(machine.framebuffer().len(), 256 * 240);
    }

    #[test]
    fn oam_dma_uses_513_cycles_when_the_write_finishes_on_even_parity() {
        // JMP takes 3 cycles, making the absolute STA to $4014 finish at cycle 16.
        let image = nrom_with_program(&[
            0x4C, 0x04, 0x80, // JMP $8004
            0xEA,             // skipped
            0xA9, 0x02,       // LDA #$02
            0x8D, 0x14, 0x40, // STA $4014
            0xEA,
        ]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        machine.step().unwrap();
        machine.step().unwrap();
        machine.step().unwrap();
        assert_eq!(machine.total_cycles(), 7 + 3 + 2 + 4 + 513);
    }

    #[test]
    fn cpu_instruction_cycles_advance_ppu_by_three_dots_each() {
        let image = nrom_with_program(&[0xEA, 0xEA]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.bus.ppu_position(), (261, 0));
        let result = machine.step().unwrap();
        assert_eq!(result.cycles, 2);
        assert_eq!(machine.bus.ppu_position(), (261, 6));
    }

    #[test]
    fn machine_exposes_controller_input_to_cpu_reads() {
        let image = nrom_with_program(&[
            0xA9, 0x01,             // LDA #1: strobe high
            0x8D, 0x16, 0x40,       // STA $4016
            0xA9, 0x00,             // LDA #0: strobe low, latch buttons
            0x8D, 0x16, 0x40,       // STA $4016
            0xAD, 0x16, 0x40,       // LDA $4016
        ]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        machine.set_controller_state(0, 0x01);
        for _ in 0..5 {
            machine.step().unwrap();
        }
        assert_eq!(machine.cpu_state().a, 0x41);
    }

    #[test]
    fn machine_exposes_apu_sample_output() {
        let image = nrom_with_program(&[0xEA]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.audio_sample(), 0.0);
        machine.write_cpu(0x4015, 0x01);
        machine.write_cpu(0x4000, 0x9F);
        machine.write_cpu(0x4002, 0x20);
        machine.write_cpu(0x4003, 0xF8);
        assert!(machine.audio_sample() > 0.0);
    }

    #[test]
    fn homebrew_rom_renders_chr_rom_tiles_through_the_full_machine() {
        const PRG_LEN: usize = 16 * 1024;
        const CHR_LEN: usize = 8 * 1024;
        let mut image = vec![0; 16 + PRG_LEN + CHR_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1; // One 8 KiB CHR-ROM bank.

        // Tile 1 has every pixel set in its low bitplane for all eight rows.
        let chr_start = 16 + PRG_LEN;
        image[chr_start + 16..chr_start + 24].fill(0xFF);
        // The ROM loops forever at $8000 while the machine advances real PPU timing.
        image[16..20].copy_from_slice(&[0xEA, 0x4C, 0x00, 0x80]);
        image[16 + 0x3FFC] = 0x00;
        image[16 + 0x3FFD] = 0x80;

        let mut machine = NesMachine::from_ines(&image).unwrap();

        // Set universal backdrop and background palette through the CPU-visible PPU port.
        machine.write_cpu(0x2006, 0x3F);
        machine.write_cpu(0x2006, 0x00);
        machine.write_cpu(0x2007, 0x0F);
        machine.write_cpu(0x2007, 0x21);

        // Fill the visible nametable with tile 1, then enable background rendering.
        machine.write_cpu(0x2006, 0x20);
        machine.write_cpu(0x2006, 0x00);
        for _ in 0..(32 * 30) {
            machine.write_cpu(0x2007, 1);
        }
        machine.write_cpu(0x2001, 0x0A); // Background + leftmost 8 background pixels.

        let cycles = machine.step_frame().unwrap();
        assert!(cycles > 29_000 && cycles < 31_000);
        let frame = machine.framebuffer();
        assert_eq!(frame.len(), 256 * 240);
        assert_eq!(frame[0], 0x21, "tile pattern and palette must reach the output image");
        assert_eq!(frame[8 * 256 + 8], 0x21, "the rendered tile grid must fill the image");
        assert!(frame.iter().any(|&pixel| pixel != 0x0F), "framebuffer must not remain the blank backdrop");
        let mut palette = [[0, 0, 0, 255]; 64];
        palette[0x21] = [12, 34, 56, 255];
        let rgba = machine.framebuffer_rgba(&palette);
        assert_eq!(rgba.len(), 256 * 240 * 4);
        assert_eq!(&rgba[0..4], &[12, 34, 56, 255]);
    }

    #[test]
    fn battery_ram_round_trips_and_rejects_wrong_size_without_mutation() {
        let mut image = nrom_with_program(&[0xEA]);
        image[6] |= 0x02;
        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert!(machine.has_battery_backed_ram());
        machine.write_cpu(0x6000, 0xA5);
        machine.write_cpu(0x7FFF, 0x5A);

        let saved = machine.save_battery_ram().unwrap();
        assert_eq!(saved.len(), 8 * 1024);
        machine.write_cpu(0x6000, 0x11);
        assert!(matches!(
            machine.load_battery_ram(&saved[..saved.len() - 1]),
            Err(MachineError::InvalidSaveRamLength { expected: 8192, actual: 8191 })
        ));
        assert_eq!(machine.read_cpu(0x6000), 0x11, "invalid save must not partially mutate RAM");

        machine.load_battery_ram(&saved).unwrap();
        assert_eq!(machine.read_cpu(0x6000), 0xA5);
        assert_eq!(machine.read_cpu(0x7FFF), 0x5A);
    }

    #[test]
    fn cartridges_without_battery_flag_reject_persistent_save_operations() {
        let image = nrom_with_program(&[0xEA]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert!(!machine.has_battery_backed_ram());
        assert_eq!(machine.save_battery_ram(), Err(MachineError::NoBatteryRam));
        assert_eq!(machine.load_battery_ram(&vec![0; 8192]), Err(MachineError::NoBatteryRam));
    }

    #[test]
    fn machine_reports_invalid_cartridges() {
        assert!(matches!(NesMachine::from_ines(&[0; 8]), Err(MachineError::Cartridge(CartridgeError::TruncatedHeader))));
    }
    #[test]
    fn machine_controller_api_latches_both_ports() {
        let image = nrom_with_program(&[0xEA, 0x4C, 0x00, 0x80]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        machine.set_controller_state(0, 0b1000_0001);
        machine.set_controller_state(1, 0b0000_0010);
        machine.write_cpu(0x4016, 1);
        machine.write_cpu(0x4016, 0);
        let a: Vec<u8> = (0..8).map(|_| machine.read_cpu(0x4016) & 1).collect();
        let b: Vec<u8> = (0..8).map(|_| machine.read_cpu(0x4017) & 1).collect();
        assert_eq!(a, [1, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(b, [0, 1, 0, 0, 0, 0, 0, 0]);
    }

}


#[cfg(test)]
mod pcm_frontend_tests {
    use super::*;

    #[test]
    fn machine_exposes_configurable_pcm_batches_while_emulating() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        // Infinite JMP $8000 loop so stepping remains deterministic.
        image[16] = 0x4C;
        image[17] = 0x00;
        image[18] = 0x80;
        image[16 + 0x3FFC] = 0x00;
        image[16 + 0x3FFD] = 0x80;

        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert!(!machine.set_audio_sample_rate(1));
        assert!(machine.set_audio_sample_rate(48_000));
        for _ in 0..200 {
            machine.step().unwrap();
        }
        let samples = machine.drain_audio_samples();
        assert!(!samples.is_empty());
        assert!(samples.iter().all(|sample| sample.is_finite() && (0.0..=1.0).contains(sample)));
        assert!(machine.drain_audio_samples().is_empty());
    }
}
