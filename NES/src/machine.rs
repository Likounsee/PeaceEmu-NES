//! Small top-level NES machine wrapper for loading and stepping NROM programs.

use crate::bus::CpuBus;
use crate::cartridge::{CartridgeError, Nrom};
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
}

impl fmt::Display for MachineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cartridge(error) => write!(f, "invalid NES cartridge: {error}"),
            Self::Cpu(error) => write!(f, "NES CPU error: {error}"),
            Self::FrameTimeout => write!(f, "NES frame step exceeded its instruction budget"),
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

/// CPU plus the current minimal bus and an NROM cartridge.
///
/// The wrapper supports instruction stepping, PPU register access, snapshot frame
/// rendering and controller input, but is not yet a playable emulator: audio and precise
/// timing remain incomplete.
#[derive(Debug)]
pub struct NesMachine {
    cpu: Cpu,
    bus: CpuBus,
}

impl NesMachine {
    /// Parses an iNES image, attaches its NROM cartridge, and performs CPU reset.
    pub fn from_ines(image: &[u8]) -> Result<Self, MachineError> {
        let cartridge = Nrom::parse(image)?;
        let mut bus = CpuBus::new();
        bus.insert_cartridge(cartridge);
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus);
        Ok(Self { cpu, bus })
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

    /// Returns a snapshot of the CPU registers and flags.
    pub fn cpu_state(&self) -> CpuState { self.cpu.state() }

    /// Returns the accumulated nominal CPU cycle count, including reset.
    pub fn total_cycles(&self) -> u64 { self.cpu.total_cycles() }

    /// Returns one approximate normalized mono audio sample (0.0..1.0).
    pub fn audio_sample(&self) -> f32 { self.bus.apu_sample() }

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
        self.bus.tick_ppu(u32::from(elapsed_cycles) * 3);
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
    fn machine_reports_invalid_cartridges() {
        assert!(matches!(NesMachine::from_ines(&[0; 8]), Err(MachineError::Cartridge(CartridgeError::TruncatedHeader))));
    }
}
