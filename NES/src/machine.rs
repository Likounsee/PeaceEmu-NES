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
}

impl fmt::Display for MachineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cartridge(error) => write!(f, "invalid NES cartridge: {error}"),
            Self::Cpu(error) => write!(f, "NES CPU error: {error}"),
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
/// This wrapper is an instruction-stepping harness, not yet a playable emulator:
/// PPU/APU/controller devices and CPU-to-PPU timing are not implemented.
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

    /// Returns a snapshot of the CPU registers and flags.
    pub fn cpu_state(&self) -> CpuState { self.cpu.state() }

    /// Returns the accumulated nominal CPU cycle count, including reset.
    pub fn total_cycles(&self) -> u64 { self.cpu.total_cycles() }

    /// Executes one CPU instruction.
    pub fn step(&mut self) -> Result<StepResult, MachineError> {
        Ok(self.cpu.step(&mut self.bus)?)
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
        image[0..4].copy_from_slice(b"NES\\x1A");
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
    fn machine_reports_invalid_cartridges() {
        assert!(matches!(NesMachine::from_ines(&[0; 8]), Err(MachineError::Cartridge(CartridgeError::TruncatedHeader))));
    }
}
