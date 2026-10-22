//! Initial Ricoh 2A03 CPU instruction interpreter.
//!
//! This milestone implements a tested subset of official 6502 instructions.
//! Unsupported opcodes are errors, not silent NOPs. Cycle-level bus timing,
//! interrupts beyond BRK, and the remaining instruction set are not complete.

use crate::bus::CpuBus;
use core::fmt;

/// Processor status flag masks.
pub mod flags {
    /// Carry flag.
    pub const CARRY: u8 = 0x01;
    /// Zero flag.
    pub const ZERO: u8 = 0x02;
    /// Interrupt disable flag.
    pub const INTERRUPT_DISABLE: u8 = 0x04;
    /// Decimal mode flag; the 2A03 ignores decimal arithmetic.
    pub const DECIMAL: u8 = 0x08;
    /// Break bit in pushed status.
    pub const BREAK: u8 = 0x10;
    /// Unused status bit, kept set internally.
    pub const UNUSED: u8 = 0x20;
    /// Overflow flag.
    pub const OVERFLOW: u8 = 0x40;
    /// Negative flag.
    pub const NEGATIVE: u8 = 0x80;
}

/// CPU execution error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuError {
    /// Opcode has not been implemented.
    UnsupportedOpcode {
        /// Unimplemented opcode byte.
        opcode: u8,
        /// Address where the opcode was fetched.
        pc: u16,
    },
}
impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedOpcode { opcode, pc } => write!(f, "unsupported 2A03 opcode {opcode:#04X} at {pc:#06X}"),
        }
    }
}
impl std::error::Error for CpuError {}

/// Public CPU register snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuState {
    /// Accumulator.
    pub a: u8,
    /// X index register.
    pub x: u8,
    /// Y index register.
    pub y: u8,
    /// Stack pointer.
    pub sp: u8,
    /// Program counter.
    pub pc: u16,
    /// Processor status.
    pub status: u8,
}

/// Result of executing one instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StepResult {
    /// Instruction address.
    pub pc: u16,
    /// Opcode byte.
    pub opcode: u8,
    /// Nominal CPU cycles consumed.
    pub cycles: u8,
}

/// Ricoh 2A03 CPU state.
#[derive(Debug)]
pub struct Cpu {
    state: CpuState,
    cycles: u64,
}
impl Default for Cpu {
    fn default() -> Self { Self::new() }
}
impl Cpu {
    /// Creates CPU state before reset-vector initialization.
    pub fn new() -> Self {
        Self { state: CpuState { a: 0, x: 0, y: 0, sp: 0xFD, pc: 0, status: flags::INTERRUPT_DISABLE | flags::UNUSED }, cycles: 0 }
    }
    /// Returns the current registers and flags.
    pub fn state(&self) -> CpuState { self.state }
    /// Returns accumulated nominal CPU cycles.
    pub fn total_cycles(&self) -> u64 { self.cycles }
    /// Adds CPU stall cycles consumed by DMA or another bus-level device.
    pub fn add_stall_cycles(&mut self, cycles: u16) { self.cycles = self.cycles.saturating_add(u64::from(cycles)); }
    /// Sets the program counter; useful for reset initialization and unit tests.
    pub fn set_pc(&mut self, pc: u16) { self.state.pc = pc; }
    /// Performs the hardware reset sequence and loads the reset vector at $FFFC.
    pub fn reset(&mut self, bus: &mut CpuBus) {
        self.state.a = 0;
        self.state.x = 0;
        self.state.y = 0;
        self.state.sp = 0xFD;
        self.state.status = flags::INTERRUPT_DISABLE | flags::UNUSED;
        self.state.pc = self.read_word(bus, 0xFFFC);
        self.cycles = self.cycles.saturating_add(7);
    }

    /// Executes one instruction. Unsupported opcodes return an error.
    pub fn step(&mut self, bus: &mut CpuBus) -> Result<StepResult, CpuError> {
        let pc = self.state.pc;
        if bus.take_nmi_request() {
            self.push(bus, (self.state.pc >> 8) as u8);
            self.push(bus, self.state.pc as u8);
            self.push(bus, (self.state.status | flags::UNUSED) & !flags::BREAK);
            self.set_flag(flags::INTERRUPT_DISABLE, true);
            self.state.pc = self.read_word(bus, 0xFFFA);
            self.cycles = self.cycles.saturating_add(7);
            self.state.status |= flags::UNUSED;
            return Ok(StepResult { pc, opcode: 0xFF, cycles: 7 });
        }
        if bus.irq_pending() && !self.flag(flags::INTERRUPT_DISABLE) {
            self.push(bus, (self.state.pc >> 8) as u8);
            self.push(bus, self.state.pc as u8);
            self.push(bus, (self.state.status | flags::UNUSED) & !flags::BREAK);
            self.set_flag(flags::INTERRUPT_DISABLE, true);
            self.state.pc = self.read_word(bus, 0xFFFE);
            self.cycles = self.cycles.saturating_add(7);
            self.state.status |= flags::UNUSED;
            return Ok(StepResult { pc, opcode: 0xFE, cycles: 7 });
        }
        let opcode = self.fetch(bus);
        let cycles = match opcode {
            0xEA => 2,
            0xA9 => { let v = self.fetch(bus); self.state.a = v; self.set_nz(v); 2 }
            0xA5 => { let a = self.fetch(bus) as u16; let v = bus.read(a); self.state.a = v; self.set_nz(v); 3 }
            0xAD => { let a = self.fetch_word(bus); let v = bus.read(a); self.state.a = v; self.set_nz(v); 4 }
            0xB5 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; let v = bus.read(a); self.state.a = v; self.set_nz(v); 4 }
            0xBD => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); let v = bus.read(a); self.state.a = v; self.set_nz(v); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xB9 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); let v = bus.read(a); self.state.a = v; self.set_nz(v); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xA1 => { let a = self.indirect_x(bus); let v = bus.read(a); self.state.a = v; self.set_nz(v); 6 }
            0xB1 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); let v = bus.read(a); self.state.a = v; self.set_nz(v); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xA2 => { let v = self.fetch(bus); self.state.x = v; self.set_nz(v); 2 }
            0xA0 => { let v = self.fetch(bus); self.state.y = v; self.set_nz(v); 2 }
            0xA6 => { let a = self.fetch(bus) as u16; let v = bus.read(a); self.state.x = v; self.set_nz(v); 3 }
            0xAE => { let a = self.fetch_word(bus); let v = bus.read(a); self.state.x = v; self.set_nz(v); 4 }
            0xB6 => { let a = self.fetch(bus).wrapping_add(self.state.y) as u16; let v = bus.read(a); self.state.x = v; self.set_nz(v); 4 }
            0xBE => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); let v = bus.read(a); self.state.x = v; self.set_nz(v); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xA4 => { let a = self.fetch(bus) as u16; let v = bus.read(a); self.state.y = v; self.set_nz(v); 3 }
            0xAC => { let a = self.fetch_word(bus); let v = bus.read(a); self.state.y = v; self.set_nz(v); 4 }
            0xB4 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; let v = bus.read(a); self.state.y = v; self.set_nz(v); 4 }
            0xBC => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); let v = bus.read(a); self.state.y = v; self.set_nz(v); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x85 => { let a = self.fetch(bus) as u16; bus.write(a, self.state.a); 3 }
            0x8D => { let a = self.fetch_word(bus); bus.write(a, self.state.a); 4 }
            0x86 => { let a = self.fetch(bus) as u16; bus.write(a, self.state.x); 3 }
            0x84 => { let a = self.fetch(bus) as u16; bus.write(a, self.state.y); 3 }
            0x95 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; bus.write(a, self.state.a); 4 }
            0x9D => { let a = self.fetch_word(bus).wrapping_add(self.state.x as u16); bus.write(a, self.state.a); 5 }
            0x99 => { let a = self.fetch_word(bus).wrapping_add(self.state.y as u16); bus.write(a, self.state.a); 5 }
            0x81 => { let a = self.indirect_x(bus); bus.write(a, self.state.a); 6 }
            0x91 => { let a = self.indirect_y(bus).wrapping_add(self.state.y as u16); bus.write(a, self.state.a); 6 }
            0x96 => { let a = self.fetch(bus).wrapping_add(self.state.y) as u16; bus.write(a, self.state.x); 4 }
            0x8E => { let a = self.fetch_word(bus); bus.write(a, self.state.x); 4 }
            0x94 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; bus.write(a, self.state.y); 4 }
            0x8C => { let a = self.fetch_word(bus); bus.write(a, self.state.y); 4 }
            0xAA => { self.state.x = self.state.a; self.set_nz(self.state.x); 2 }
            0xA8 => { self.state.y = self.state.a; self.set_nz(self.state.y); 2 }
            0x8A => { self.state.a = self.state.x; self.set_nz(self.state.a); 2 }
            0x98 => { self.state.a = self.state.y; self.set_nz(self.state.a); 2 }
            0xE8 => { self.state.x = self.state.x.wrapping_add(1); self.set_nz(self.state.x); 2 }
            0xE6 => { let a = self.fetch(bus) as u16; let v = bus.read(a).wrapping_add(1); bus.write(a, v); self.set_nz(v); 5 }
            0xEE => { let a = self.fetch_word(bus); let v = bus.read(a).wrapping_add(1); bus.write(a, v); self.set_nz(v); 6 }
            0xF6 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; let v = bus.read(a).wrapping_add(1); bus.write(a, v); self.set_nz(v); 6 }
            0xFE => { let a = self.fetch_word(bus).wrapping_add(self.state.x as u16); let v = bus.read(a).wrapping_add(1); bus.write(a, v); self.set_nz(v); 7 }
            0xC6 => { let a = self.fetch(bus) as u16; let v = bus.read(a).wrapping_sub(1); bus.write(a, v); self.set_nz(v); 5 }
            0xCE => { let a = self.fetch_word(bus); let v = bus.read(a).wrapping_sub(1); bus.write(a, v); self.set_nz(v); 6 }
            0xD6 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; let v = bus.read(a).wrapping_sub(1); bus.write(a, v); self.set_nz(v); 6 }
            0xDE => { let a = self.fetch_word(bus).wrapping_add(self.state.x as u16); let v = bus.read(a).wrapping_sub(1); bus.write(a, v); self.set_nz(v); 7 }
            0xC8 => { self.state.y = self.state.y.wrapping_add(1); self.set_nz(self.state.y); 2 }
            0xCA => { self.state.x = self.state.x.wrapping_sub(1); self.set_nz(self.state.x); 2 }
            0x88 => { self.state.y = self.state.y.wrapping_sub(1); self.set_nz(self.state.y); 2 }
            0x0A => { self.state.a = self.shift(self.state.a, 0); 2 }
            0x2A => { self.state.a = self.shift(self.state.a, 1); 2 }
            0x4A => { self.state.a = self.shift(self.state.a, 2); 2 }
            0x6A => { self.state.a = self.shift(self.state.a, 3); 2 }
            0x06 | 0x26 | 0x46 | 0x66 => { let a = self.fetch(bus) as u16; let k = (opcode >> 5) & 3; let v = self.shift(bus.read(a), k); bus.write(a, v); 5 }
            0x16 | 0x36 | 0x56 | 0x76 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; let k = (opcode >> 5) & 3; let v = self.shift(bus.read(a), k); bus.write(a, v); 6 }
            0x0E | 0x2E | 0x4E | 0x6E => { let a = self.fetch_word(bus); let k = (opcode >> 5) & 3; let v = self.shift(bus.read(a), k); bus.write(a, v); 6 }
            0x1E | 0x3E | 0x5E | 0x7E => { let a = self.fetch_word(bus).wrapping_add(self.state.x as u16); let k = (opcode >> 5) & 3; let v = self.shift(bus.read(a), k); bus.write(a, v); 7 }
            0x69 => { let v = self.fetch(bus); self.adc(v); 2 }
            0x65 => { let a = self.fetch(bus) as u16; self.adc(bus.read(a)); 3 }
            0x75 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.adc(bus.read(a)); 4 }
            0x6D => { let a = self.fetch_word(bus); self.adc(bus.read(a)); 4 }
            0x7D => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.adc(bus.read(a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x79 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.adc(bus.read(a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x61 => { let a = self.indirect_x(bus); self.adc(bus.read(a)); 6 }
            0x71 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.adc(bus.read(a)); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xE9 | 0xEB => { let v = self.fetch(bus); self.adc(!v); 2 }
            0xE5 => { let a = self.fetch(bus) as u16; self.adc(!bus.read(a)); 3 }
            0xF5 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.adc(!bus.read(a)); 4 }
            0xED => { let a = self.fetch_word(bus); self.adc(!bus.read(a)); 4 }
            0xFD => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.adc(!bus.read(a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xF9 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.adc(!bus.read(a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xE1 => { let a = self.indirect_x(bus); self.adc(!bus.read(a)); 6 }
            0xF1 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.adc(!bus.read(a)); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x29 => { let v = self.fetch(bus); self.state.a &= v; self.set_nz(self.state.a); 2 }
            0x25 => { let a = self.fetch(bus) as u16; self.state.a &= bus.read(a); self.set_nz(self.state.a); 3 }
            0x2D => { let a = self.fetch_word(bus); self.state.a &= bus.read(a); self.set_nz(self.state.a); 4 }
            0x35 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.state.a &= bus.read(a); self.set_nz(self.state.a); 4 }
            0x3D => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.state.a &= bus.read(a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x39 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a &= bus.read(a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x21 => { let a = self.indirect_x(bus); self.state.a &= bus.read(a); self.set_nz(self.state.a); 6 }
            0x31 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a &= bus.read(a); self.set_nz(self.state.a); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x05 => { let a = self.fetch(bus) as u16; self.state.a |= bus.read(a); self.set_nz(self.state.a); 3 }
            0x15 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.state.a |= bus.read(a); self.set_nz(self.state.a); 4 }
            0x19 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a |= bus.read(a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x1D => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.state.a |= bus.read(a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x01 => { let a = self.indirect_x(bus); self.state.a |= bus.read(a); self.set_nz(self.state.a); 6 }
            0x11 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a |= bus.read(a); self.set_nz(self.state.a); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x0D => { let a = self.fetch_word(bus); self.state.a |= bus.read(a); self.set_nz(self.state.a); 4 }
            0x45 => { let a = self.fetch(bus) as u16; self.state.a ^= bus.read(a); self.set_nz(self.state.a); 3 }
            0x55 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.state.a ^= bus.read(a); self.set_nz(self.state.a); 4 }
            0x59 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a ^= bus.read(a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x5D => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.state.a ^= bus.read(a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x41 => { let a = self.indirect_x(bus); self.state.a ^= bus.read(a); self.set_nz(self.state.a); 6 }
            0x51 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a ^= bus.read(a); self.set_nz(self.state.a); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x4D => { let a = self.fetch_word(bus); self.state.a ^= bus.read(a); self.set_nz(self.state.a); 4 }
            0x09 => { let v = self.fetch(bus); self.state.a |= v; self.set_nz(self.state.a); 2 }
            0x49 => { let v = self.fetch(bus); self.state.a ^= v; self.set_nz(self.state.a); 2 }
            0xC1 => { let a = self.indirect_x(bus); self.compare(self.state.a, bus.read(a)); 6 }
            0xC5 => { let a = self.fetch(bus) as u16; self.compare(self.state.a, bus.read(a)); 3 }
            0xD1 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.compare(self.state.a, bus.read(a)); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xC9 => { let v = self.fetch(bus); self.compare(self.state.a, v); 2 }
            0xE0 => { let v = self.fetch(bus); self.compare(self.state.x, v); 2 }
            0xC0 => { let v = self.fetch(bus); self.compare(self.state.y, v); 2 }
            0xCD => { let a = self.fetch_word(bus); self.compare(self.state.a, bus.read(a)); 4 }
            0xD5 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.compare(self.state.a, bus.read(a)); 4 }
            0xDD => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.compare(self.state.a, bus.read(a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xD9 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.compare(self.state.a, bus.read(a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xE4 => { let a = self.fetch(bus) as u16; self.compare(self.state.x, bus.read(a)); 3 }
            0xEC => { let a = self.fetch_word(bus); self.compare(self.state.x, bus.read(a)); 4 }
            0xC4 => { let a = self.fetch(bus) as u16; self.compare(self.state.y, bus.read(a)); 3 }
            0xCC => { let a = self.fetch_word(bus); self.compare(self.state.y, bus.read(a)); 4 }
            0x4C => { self.state.pc = self.fetch_word(bus); 3 }
            0x6C => {
                let pointer = self.fetch_word(bus);
                let lo = bus.read(pointer) as u16;
                // The original 6502 wraps the high-byte read within the pointer's page.
                let high_address = (pointer & 0xFF00) | (pointer.wrapping_add(1) & 0x00FF);
                let hi = bus.read(high_address) as u16;
                self.state.pc = (hi << 8) | lo;
                5
            }
            0x20 => {
                let target = self.fetch_word(bus);
                let ret = self.state.pc.wrapping_sub(1);
                self.push(bus, (ret >> 8) as u8); self.push(bus, ret as u8);
                self.state.pc = target; 6
            }
            0x60 => {
                let lo = self.pop(bus) as u16; let hi = self.pop(bus) as u16;
                self.state.pc = ((hi << 8) | lo).wrapping_add(1); 6
            }
            0x40 => {
                self.state.status = (self.pop(bus) | flags::UNUSED) & !flags::BREAK;
                let lo = self.pop(bus) as u16;
                let hi = self.pop(bus) as u16;
                self.state.pc = (hi << 8) | lo;
                6
            }
            0x48 => { self.push(bus, self.state.a); 3 }
            0x68 => { self.state.a = self.pop(bus); self.set_nz(self.state.a); 4 }
            0x08 => { self.push(bus, self.state.status | flags::BREAK | flags::UNUSED); 3 }
            0x28 => { self.state.status = (self.pop(bus) | flags::UNUSED) & !flags::BREAK; 4 }
            0x18 => { self.set_flag(flags::CARRY, false); 2 }
            0x38 => { self.set_flag(flags::CARRY, true); 2 }
            0x58 => { self.set_flag(flags::INTERRUPT_DISABLE, false); 2 }
            0x78 => { self.set_flag(flags::INTERRUPT_DISABLE, true); 2 }
            0xD8 => { self.set_flag(flags::DECIMAL, false); 2 }
            0xF8 => { self.set_flag(flags::DECIMAL, true); 2 }
            0xB8 => { self.set_flag(flags::OVERFLOW, false); 2 }
            0x24 => { let a = self.fetch(bus) as u16; let v = bus.read(a); self.set_flag(flags::ZERO, self.state.a & v == 0); self.set_flag(flags::OVERFLOW, v & 0x40 != 0); self.set_flag(flags::NEGATIVE, v & 0x80 != 0); 3 }
            0x2C => { let a = self.fetch_word(bus); let v = bus.read(a); self.set_flag(flags::ZERO, self.state.a & v == 0); self.set_flag(flags::OVERFLOW, v & 0x40 != 0); self.set_flag(flags::NEGATIVE, v & 0x80 != 0); 4 }
            0x10 => self.branch(bus, !self.flag(flags::NEGATIVE)),
            0x30 => self.branch(bus, self.flag(flags::NEGATIVE)),
            0x50 => self.branch(bus, !self.flag(flags::OVERFLOW)),
            0x70 => self.branch(bus, self.flag(flags::OVERFLOW)),
            0x90 => self.branch(bus, !self.flag(flags::CARRY)),
            0xB0 => self.branch(bus, self.flag(flags::CARRY)),
            0xD0 => self.branch(bus, !self.flag(flags::ZERO)),
            0xF0 => self.branch(bus, self.flag(flags::ZERO)),
            0x9A => { self.state.sp = self.state.x; 2 }
            0xBA => { self.state.x = self.state.sp; self.set_nz(self.state.x); 2 }
            0x00 => {
                self.state.pc = self.state.pc.wrapping_add(1);
                self.push(bus, (self.state.pc >> 8) as u8); self.push(bus, self.state.pc as u8);
                self.push(bus, self.state.status | flags::BREAK | flags::UNUSED);
                self.set_flag(flags::INTERRUPT_DISABLE, true);
                self.state.pc = self.read_word(bus, 0xFFFE); 7
            }
            _ => return Err(CpuError::UnsupportedOpcode { opcode, pc }),
        };
        self.state.status |= flags::UNUSED;
        self.cycles += cycles as u64;
        Ok(StepResult { pc, opcode, cycles })
    }

    fn indirect_x(&mut self, bus: &mut CpuBus) -> u16 {
        let pointer = self.fetch(bus).wrapping_add(self.state.x);
        let lo = bus.read(pointer as u16) as u16;
        let hi = bus.read(pointer.wrapping_add(1) as u16) as u16;
        (hi << 8) | lo
    }
    fn indirect_y(&mut self, bus: &mut CpuBus) -> u16 {
        let pointer = self.fetch(bus);
        let lo = bus.read(pointer as u16) as u16;
        let hi = bus.read(pointer.wrapping_add(1) as u16) as u16;
        (hi << 8) | lo
    }
    fn fetch(&mut self, bus: &mut CpuBus) -> u8 {
        let v = bus.read(self.state.pc); self.state.pc = self.state.pc.wrapping_add(1); v
    }
    fn fetch_word(&mut self, bus: &mut CpuBus) -> u16 {
        let lo = self.fetch(bus) as u16; let hi = self.fetch(bus) as u16; (hi << 8) | lo
    }
    fn read_word(&mut self, bus: &mut CpuBus, a: u16) -> u16 {
        let lo = bus.read(a) as u16; let hi = bus.read(a.wrapping_add(1)) as u16; (hi << 8) | lo
    }
    fn push(&mut self, bus: &mut CpuBus, v: u8) {
        bus.write(0x0100 | self.state.sp as u16, v); self.state.sp = self.state.sp.wrapping_sub(1);
    }
    fn pop(&mut self, bus: &mut CpuBus) -> u8 {
        self.state.sp = self.state.sp.wrapping_add(1); bus.read(0x0100 | self.state.sp as u16)
    }
    fn flag(&self, mask: u8) -> bool { self.state.status & mask != 0 }
    fn set_flag(&mut self, mask: u8, on: bool) {
        if on { self.state.status |= mask; } else { self.state.status &= !mask; }
    }
    fn set_nz(&mut self, v: u8) {
        self.set_flag(flags::ZERO, v == 0); self.set_flag(flags::NEGATIVE, v & 0x80 != 0);
    }
    fn adc(&mut self, v: u8) {
        let a = self.state.a; let sum = a as u16 + v as u16 + u16::from(self.flag(flags::CARRY));
        let r = sum as u8;
        self.set_flag(flags::CARRY, sum > 0xFF);
        self.set_flag(flags::OVERFLOW, (!(a ^ v) & (a ^ r) & 0x80) != 0);
        self.state.a = r; self.set_nz(r);
    }
    fn shift(&mut self, value: u8, kind: u8) -> u8 {
        let carry_in = u8::from(self.flag(flags::CARRY));
        let result = match kind {
            0 => { self.set_flag(flags::CARRY, value & 0x80 != 0); value.wrapping_shl(1) }
            1 => { self.set_flag(flags::CARRY, value & 0x80 != 0); (value << 1) | carry_in }
            2 => { self.set_flag(flags::CARRY, value & 0x01 != 0); value >> 1 }
            _ => { self.set_flag(flags::CARRY, value & 0x01 != 0); (value >> 1) | (carry_in << 7) }
        };
        self.set_nz(result);
        result
    }
    fn compare(&mut self, a: u8, b: u8) {
        self.set_flag(flags::CARRY, a >= b); self.set_nz(a.wrapping_sub(b));
    }
    fn branch(&mut self, bus: &mut CpuBus, take: bool) -> u8 {
        let offset = self.fetch(bus) as i8;
        if !take { return 2; }
        let before = self.state.pc;
        self.state.pc = self.state.pc.wrapping_add_signed(offset as i16);
        if before & 0xFF00 != self.state.pc & 0xFF00 { 4 } else { 3 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup(program: &[u8]) -> (Cpu, CpuBus) {
        let mut bus = CpuBus::new();
        for (i, v) in program.iter().copied().enumerate() { bus.write(0x0600 + i as u16, v); }
        let mut cpu = Cpu::new(); cpu.set_pc(0x0600); (cpu, bus)
    }
    #[test] fn lda_adc_sta_sets_flags_and_cycles() {
        let (mut c, mut b) = setup(&[0xA9, 0x7F, 0x69, 1, 0x8D, 0, 2]);
        assert_eq!(c.step(&mut b).unwrap().cycles, 2); c.step(&mut b).unwrap();
        assert_eq!(c.state().a, 0x80); assert_ne!(c.state().status & flags::OVERFLOW, 0);
        assert_ne!(c.state().status & flags::NEGATIVE, 0); c.step(&mut b).unwrap();
        assert_eq!(b.read(0x0200), 0x80); assert_eq!(c.total_cycles(), 8);
    }
    #[test] fn compare_sets_zero_and_carry() {
        let (mut c, mut b) = setup(&[0xA9, 0x10, 0xC9, 0x10]);
        c.step(&mut b).unwrap(); c.step(&mut b).unwrap();
        assert_ne!(c.state().status & flags::ZERO, 0); assert_ne!(c.state().status & flags::CARRY, 0);
    }
    #[test] fn branch_uses_signed_relative_offset() {
        let (mut c, mut b) = setup(&[0xD0, 2, 0xEA, 0xEA, 0xA9, 0x44]);
        c.step(&mut b).unwrap(); assert_eq!(c.state().pc, 0x0604);
        c.step(&mut b).unwrap(); assert_eq!(c.state().a, 0x44);
    }
    #[test]
    fn status_stack_and_transfer_opcodes_work() {
        let (mut cpu, mut bus) = setup(&[
            0x38, // SEC
            0x08, // PHP
            0x18, // CLC
            0x28, // PLP
            0xA9, 0x42, // LDA #$42
            0x48, // PHA
            0xA9, 0x00, // LDA #0
            0x68, // PLA
            0xAA, // TAX
            0x9A, // TXS
            0xBA, // TSX
        ]);
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        assert_ne!(cpu.state().status & flags::CARRY, 0);
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.state().a, 0x42);
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.state().x, cpu.state().sp);
    }

    #[test]
    fn conditional_branches_cover_negative_carry_and_overflow() {
        let (mut cpu, mut bus) = setup(&[
            0xA9, 0x80, // LDA #$80 => N set
            0x30, 0x02, // BMI skip two bytes
            0xA9, 0x00,
            0x38, // SEC
            0xB0, 0x02, // BCS skip two bytes
            0xA9, 0x00,
            0xB8, // CLV
            0x50, 0x02, // BVC skip two bytes
            0xA9, 0x00,
        ]);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 3);
        assert_eq!(cpu.state().pc, 0x0606);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 3);
        assert_eq!(cpu.state().pc, 0x060B);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 3);
        assert_eq!(cpu.state().pc, 0x0610);
    }

    #[test]
    fn branch_page_crossing_adds_cycle_in_both_directions() {
        let mut bus = CpuBus::new();
        bus.write(0x06FD, 0xD0); // BNE from $06FD; offset is at $06FE, next PC=$06FF
        bus.write(0x06FE, 0x02); // target $0701 crosses page
        let mut cpu = Cpu::new();
        cpu.set_pc(0x06FD);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 4);
        assert_eq!(cpu.state().pc, 0x0701);

        bus.write(0x0700, 0xD0); // branch backward across page from $0702 to $06FE
        bus.write(0x0701, 0xFC);
        cpu.set_pc(0x0700);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 4);
        assert_eq!(cpu.state().pc, 0x06FE);
    }

    #[test]
    fn untaken_branch_always_uses_two_cycles_even_near_page_boundary() {
        let mut bus = CpuBus::new();
        bus.write(0x06FD, 0xD0);
        bus.write(0x06FE, 0x7F);
        let mut cpu = Cpu::new();
        cpu.set_pc(0x06FD);
        // Z is set, so BNE is not taken.
        // Set status through a small instruction sequence rather than exposing a mutator.
        bus.write(0x0600, 0xA9);
        bus.write(0x0601, 0x00);
        cpu.set_pc(0x0600);
        cpu.step(&mut bus).unwrap();
        cpu.set_pc(0x06FD);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2);
        assert_eq!(cpu.state().pc, 0x06FF);
    }

    #[test]
    fn irq_is_deferred_while_interrupt_disable_is_set_and_pushes_state_when_enabled() {
        let (mut cpu, mut bus) = setup(&[0x58, 0xEA]); // CLI; NOP
        bus.write(0xFFFE, 0x00);
        bus.write(0xFFFF, 0x07);
        bus.write(0x4017, 0x00);
        bus.tick_apu(14_915);
        assert!(bus.irq_pending());
        // IRQ is masked before CLI, so CLI executes first.
        assert_eq!(cpu.step(&mut bus).unwrap().opcode, 0x58);
        let irq = cpu.step(&mut bus).unwrap();
        assert_eq!(irq.cycles, 7);
        assert_eq!(cpu.state().pc, 0x0700);
        assert_ne!(cpu.state().status & flags::INTERRUPT_DISABLE, 0);
        assert_eq!(bus.read(0x01FB), 0x06); // return PC high byte
        assert_eq!(bus.read(0x01FA), 0x01); // return PC low byte
        assert_eq!(bus.read(0x01F9) & flags::BREAK, 0);
    }

    #[test]
    fn irq_is_not_taken_when_interrupt_disable_flag_is_set() {
        let (mut cpu, mut bus) = setup(&[0xEA]);
        bus.write(0xFFFE, 0x00);
        bus.write(0xFFFF, 0x07);
        bus.tick_apu(14_915);
        assert_eq!(cpu.step(&mut bus).unwrap().opcode, 0xEA);
        assert_eq!(cpu.state().pc, 0x0601);
    }

    #[test] fn unknown_opcode_is_explicit() {
        let (mut c, mut b) = setup(&[0x02]);
        assert_eq!(c.step(&mut b), Err(CpuError::UnsupportedOpcode { opcode: 2, pc: 0x0600 }));
        assert_eq!(c.total_cycles(), 0);
    }
    #[test]
    fn nmi_pushes_cpu_state_and_jumps_to_nmi_vector() {
        use crate::cartridge::Nrom;
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x3FFA] = 0x00; // $FFFA points to the $9000 NMI handler
        image[16 + 0x3FFB] = 0x90;
        image[16 + 0x0600] = 0xEA;
        let mut bus = CpuBus::new();
        bus.insert_cartridge(Nrom::parse(&image).unwrap());
        let mut cpu = Cpu::new();
        cpu.set_pc(0x8600);
        bus.tick_ppu(243 * 341 + 1);
        bus.write(0x2000, 0x80);
        let result = cpu.step(&mut bus).unwrap();
        assert_eq!(result.cycles, 7);
        assert_eq!(result.opcode, 0xFF);
        assert_eq!(cpu.state().pc, 0x9000);
        assert_eq!(bus.read(0x01FD), 0x86);
        assert_eq!(bus.read(0x01FC), 0x00);
        assert_ne!(cpu.state().status & flags::INTERRUPT_DISABLE, 0);
    }

    #[test] fn reset_loads_vector_from_nrom_cartridge() {
        use crate::cartridge::Nrom;
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x3FFC] = 0x34;
        image[16 + 0x3FFD] = 0x12;
        let cartridge = Nrom::parse(&image).unwrap();
        let mut bus = CpuBus::new();
        bus.insert_cartridge(cartridge);
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus);
        assert_eq!(cpu.state().pc, 0x1234);
        assert_eq!(cpu.state().sp, 0xFD);
        assert_eq!(cpu.total_cycles(), 7);
    }
    #[test] fn indexed_zero_page_and_absolute_reads_work() {
        let (mut cpu, mut bus) = setup(&[0xA2, 0x01, 0xB5, 0x20, 0xBD, 0xFF, 0x02]);
        bus.write(0x0021, 0x35);
        bus.write(0x0300, 0xA7);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 4);
        assert_eq!(cpu.state().a, 0x35);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(cpu.state().a, 0xA7);
    }
    #[test] fn indexed_indirect_load_and_store_work() {
        let (mut cpu, mut bus) = setup(&[0xA2, 0x04, 0xA1, 0x20, 0x81, 0x20]);
        bus.write(0x0024, 0x00);
        bus.write(0x0025, 0x02);
        bus.write(0x0200, 0x6C);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 6);
        assert_eq!(cpu.state().a, 0x6C);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 6);
        assert_eq!(bus.read(0x0200), 0x6C);
    }
    #[test] fn shifts_rotate_and_bit_set_expected_flags() {
        let (mut c, mut b) = setup(&[0xA9, 0x81, 0x0A, 0x6A, 0xA9, 0xC0, 0x85, 0x10, 0x24, 0x10]);
        c.step(&mut b).unwrap();
        assert_eq!(c.state().a, 0x81);
        c.step(&mut b).unwrap();
        assert_eq!(c.state().a, 0x02);
        assert_ne!(c.state().status & flags::CARRY, 0);
        c.step(&mut b).unwrap();
        assert_eq!(c.state().a, 0x81);
        assert_eq!(c.state().status & flags::CARRY, 0);
        c.step(&mut b).unwrap();
        c.step(&mut b).unwrap();
        c.step(&mut b).unwrap();
        assert_eq!(c.state().a, 0xC0);
        assert_ne!(c.state().status & flags::NEGATIVE, 0);
        assert_ne!(c.state().status & flags::OVERFLOW, 0);
    }

    #[test] fn adc_sbc_and_indexed_logical_ops_work() {
        let (mut c, mut b) = setup(&[0xA9, 0x10, 0x69, 0x05, 0xE9, 0x02, 0xA2, 1, 0x15, 0x20, 0x55, 0x20]);
        b.write(0x21, 0x03);
        for _ in 0..6 { c.step(&mut b).unwrap(); }
        assert_eq!(c.state().a, 0x10);
    }

    #[test]
    fn and_absolute_y_and_indirect_y_apply_page_cross_penalty() {
        let (mut cpu, mut bus) = setup(&[
            0xA0, 0x01,             // LDY #1
            0xA9, 0xFF,             // LDA #$FF
            0x39, 0xFF, 0x02,       // AND $02FF,Y -> $0300 (cross)
            0x31, 0x20,             // AND ($20),Y -> $0300 (cross)
        ]);
        bus.write(0x0300, 0x7F);
        bus.write(0x20, 0xFF);
        bus.write(0x21, 0x02);
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(cpu.state().a, 0x7F);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 6);
        assert_eq!(cpu.state().a, 0x7F);
    }

    #[test]
    fn cmp_zero_page_and_indirect_y_update_flags_and_cycles() {
        let (mut cpu, mut bus) = setup(&[
            0xA9, 0x80,       // LDA #$80
            0xC5, 0x20,       // CMP $20
            0xA0, 0x01,       // LDY #1
            0xD1, 0x30,       // CMP ($30),Y
        ]);
        bus.write(0x20, 0x80);
        bus.write(0x30, 0xFF);
        bus.write(0x31, 0x02);
        bus.write(0x0300, 0x7F);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 3);
        assert_ne!(cpu.state().status & flags::ZERO, 0);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 6);
        assert_ne!(cpu.state().status & flags::CARRY, 0);
        assert_eq!(cpu.state().a, 0x80);
    }

    #[test]
    fn bit_sets_zero_negative_and_overflow_from_memory() {
        let (mut cpu, mut bus) = setup(&[0xA9, 0x40, 0x24, 0x10, 0xA9, 0x00, 0x2C, 0x10, 0x02]);
        bus.write(0x10, 0xC0);
        bus.write(0x0210, 0x80);
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.state().status & flags::ZERO, 0);
        assert_ne!(cpu.state().status & flags::NEGATIVE, 0);
        assert_ne!(cpu.state().status & flags::OVERFLOW, 0);
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        assert_ne!(cpu.state().status & flags::ZERO, 0);
        assert_ne!(cpu.state().status & flags::NEGATIVE, 0);
        assert_eq!(cpu.state().status & flags::OVERFLOW, 0);
    }

    #[test] fn indirect_jmp_preserves_original_6502_page_wrap_bug() {
        let (mut cpu, mut bus) = setup(&[0x6C, 0xFF, 0x02]);
        bus.write(0x02FF, 0x34);
        bus.write(0x0200, 0x12);
        bus.write(0x0300, 0x99);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(cpu.state().pc, 0x1234);
    }

    #[test]
    fn rti_restores_status_and_program_counter_from_stack() {
        let (mut cpu, mut bus) = setup(&[0x40]);
        bus.write(0x01FE, flags::CARRY | flags::OVERFLOW | flags::BREAK);
        bus.write(0x01FF, 0x34);
        bus.write(0x0100, 0x12);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 6);
        assert_eq!(cpu.state().pc, 0x1234);
        assert_ne!(cpu.state().status & flags::CARRY, 0);
        assert_ne!(cpu.state().status & flags::OVERFLOW, 0);
        assert_eq!(cpu.state().status & flags::BREAK, 0);
        assert_ne!(cpu.state().status & flags::UNUSED, 0);
        assert_eq!(cpu.state().sp, 0x00);
    }

    #[test] fn jsr_and_rts_round_trip() {
        let (mut c, mut b) = setup(&[0x20, 6, 6, 0xA9, 0x55, 0xEA, 0xA9, 0x77, 0x60]);
        c.step(&mut b).unwrap(); assert_eq!(c.state().pc, 0x0606);
        c.step(&mut b).unwrap(); c.step(&mut b).unwrap();
        assert_eq!(c.state().pc, 0x0603);
        c.step(&mut b).unwrap(); assert_eq!(c.state().a, 0x55);
    }
}
