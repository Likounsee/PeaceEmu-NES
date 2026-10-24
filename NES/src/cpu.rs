//! Initial Ricoh 2A03 CPU instruction interpreter.
//!
//! Implements a broad subset of official Ricoh 2A03/6502 instructions plus common
//! unofficial NOP encodings used by commercial software. Other unsupported opcodes
//! return errors rather than silently acting as NOPs. Interrupt entry and nominal
//! instruction cycles are modeled; bus-cycle-exact execution remains incomplete.

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
    // CLI and PLP defer maskable IRQ recognition for one instruction on the 2A03.
    irq_poll_delay: bool,
    // KIL/JAM unofficial opcodes halt the 2A03 until reset.
    jammed: bool,
    jam_opcode: u8,
}
impl Default for Cpu {
    fn default() -> Self { Self::new() }
}
impl Cpu {
    /// Creates CPU state before reset-vector initialization.
    pub fn new() -> Self {
        Self { state: CpuState { a: 0, x: 0, y: 0, sp: 0xFD, pc: 0, status: flags::INTERRUPT_DISABLE | flags::UNUSED }, cycles: 0, irq_poll_delay: false, jammed: false, jam_opcode: 0x02 }
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
        self.irq_poll_delay = false;
        self.jammed = false;
        self.jam_opcode = 0x02;
        self.state.pc = self.read_word(bus, 0xFFFC);
        self.cycles = self.cycles.saturating_add(7);
    }

    /// Executes one instruction. Unsupported opcodes return an error.
    pub fn step(&mut self, bus: &mut CpuBus) -> Result<StepResult, CpuError> {
        let pc = self.state.pc;
        if self.jammed {
            // A jammed CPU does not fetch, change registers, or respond to IRQ/NMI.
            self.cycles = self.cycles.saturating_add(1);
            return Ok(StepResult { pc, opcode: self.jam_opcode, cycles: 1 });
        }
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
        let defer_irq_poll = self.irq_poll_delay;
        self.irq_poll_delay = false;
        if bus.irq_pending() && !self.flag(flags::INTERRUPT_DISABLE) && !defer_irq_poll {
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
            // Common stable unofficial NOPs. Addressed forms perform the read, and
            // absolute-X forms charge the page-cross penalty used by the 2A03.
            0xEA | 0x1A | 0x3A | 0x5A | 0x7A | 0xDA | 0xFA => 2,
            0x80 | 0x82 | 0x89 | 0xC2 | 0xE2 => { self.fetch(bus); 2 }
            0x04 | 0x44 | 0x64 => { let a = self.fetch(bus) as u16; let _ = bus.read(a); 3 }
            0x14 | 0x34 | 0x54 | 0x74 | 0xD4 | 0xF4 => {
                let base = self.fetch(bus);
                let _ = bus.read(base as u16);
                let a = base.wrapping_add(self.state.x) as u16;
                let _ = bus.read(a);
                4
            }
            0x0C => { let a = self.fetch_word(bus); let _ = bus.read(a); 4 }
            0x1C | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => {
                let base = self.fetch_word(bus);
                let address = base.wrapping_add(self.state.x as u16);
                let _ = Self::read_indexed(bus, base, address);
                4 + u8::from(base & 0xFF00 != address & 0xFF00)
            }
            // Stable unofficial immediate operations, adapted to this interpreter's
            // register/flag helpers from established 2A03 opcode tables.
            0x0B | 0x2B => { // ANC #imm
                self.state.a &= self.fetch(bus);
                self.set_nz(self.state.a);
                self.set_flag(flags::CARRY, self.state.a & 0x80 != 0);
                2
            }
            0x4B => { // ALR #imm: AND followed by logical shift right.
                self.state.a &= self.fetch(bus);
                self.set_flag(flags::CARRY, self.state.a & 1 != 0);
                self.state.a >>= 1;
                self.set_nz(self.state.a);
                2
            }
            0x6B => { // ARR #imm: AND followed by rotate right through carry.
                self.state.a &= self.fetch(bus);
                self.state.a = (self.state.a >> 1)
                    | if self.flag(flags::CARRY) { 0x80 } else { 0 };
                self.set_nz(self.state.a);
                self.set_flag(flags::CARRY, self.state.a & 0x40 != 0);
                self.set_flag(flags::OVERFLOW, ((self.state.a >> 6) ^ (self.state.a >> 5)) & 1 != 0);
                2
            }
            0xCB => { // AXS #imm: X = (A & X) - imm.
                let lhs = self.state.a & self.state.x;
                let operand = self.fetch(bus);
                self.set_flag(flags::CARRY, lhs >= operand);
                self.state.x = lhs.wrapping_sub(operand);
                self.set_nz(self.state.x);
                2
            }
            0x8B => { // XAA #imm is unstable; model the commonly observed A & X & immediate form.
                self.state.a &= self.state.x & self.fetch(bus);
                self.set_nz(self.state.a);
                2
            }
            0xA9 => { let v = self.fetch(bus); self.state.a = v; self.set_nz(v); 2 }
            0xA5 => { let a = self.fetch(bus) as u16; let v = bus.read(a); self.state.a = v; self.set_nz(v); 3 }
            0xAD => { let a = self.fetch_word(bus); let v = bus.read(a); self.state.a = v; self.set_nz(v); 4 }
            0xB5 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; let v = bus.read(a); self.state.a = v; self.set_nz(v); 4 }
            0xBD => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); let v = Self::read_indexed(bus, base, a); self.state.a = v; self.set_nz(v); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xB9 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); let v = Self::read_indexed(bus, base, a); self.state.a = v; self.set_nz(v); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xA1 => { let a = self.indirect_x(bus); let v = bus.read(a); self.state.a = v; self.set_nz(v); 6 }
            0xB1 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); let v = Self::read_indexed(bus, base, a); self.state.a = v; self.set_nz(v); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xA3 => { let a = self.indirect_x(bus); let v = bus.read(a); self.state.a = v; self.state.x = v; self.set_nz(v); 6 }
            0xB3 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); let v = Self::read_indexed(bus, base, a); self.state.a = v; self.state.x = v; self.set_nz(v); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xA2 => { let v = self.fetch(bus); self.state.x = v; self.set_nz(v); 2 }
            // LAX immediate ($AB) is unstable on NMOS 6502 revisions, but is used
            // by real NES software; model the common observed A=X=operand behavior.
            0xAB => { let v = self.fetch(bus); self.state.a = v; self.state.x = v; self.set_nz(v); 2 }
            0xA0 => { let v = self.fetch(bus); self.state.y = v; self.set_nz(v); 2 }
            0xA6 => { let a = self.fetch(bus) as u16; let v = bus.read(a); self.state.x = v; self.set_nz(v); 3 }
            0xAE => { let a = self.fetch_word(bus); let v = bus.read(a); self.state.x = v; self.set_nz(v); 4 }
            0xB6 => { let a = self.fetch(bus).wrapping_add(self.state.y) as u16; let v = bus.read(a); self.state.x = v; self.set_nz(v); 4 }
            0xBE => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); let v = Self::read_indexed(bus, base, a); self.state.x = v; self.set_nz(v); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            // Stable unofficial LAX loads one value into both A and X.
            0xA7 => { let a = self.fetch(bus) as u16; let v = bus.read(a); self.state.a = v; self.state.x = v; self.set_nz(v); 3 }
            0xAF => { let a = self.fetch_word(bus); let v = bus.read(a); self.state.a = v; self.state.x = v; self.set_nz(v); 4 }
            0xB7 => { let a = self.fetch(bus).wrapping_add(self.state.y) as u16; let v = bus.read(a); self.state.a = v; self.state.x = v; self.set_nz(v); 4 }
            0xBF => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); let v = Self::read_indexed(bus, base, a); self.state.a = v; self.state.x = v; self.set_nz(v); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xBB => { // LAS: A, X and SP share memory & stack pointer.
                let base = self.fetch_word(bus);
                let address = base.wrapping_add(self.state.y as u16);
                let value = bus.read(address) & self.state.sp;
                self.state.a = value;
                self.state.x = value;
                self.state.sp = value;
                self.set_nz(value);
                4 + u8::from(base & 0xFF00 != address & 0xFF00)
            }
            0xA4 => { let a = self.fetch(bus) as u16; let v = bus.read(a); self.state.y = v; self.set_nz(v); 3 }
            0xAC => { let a = self.fetch_word(bus); let v = bus.read(a); self.state.y = v; self.set_nz(v); 4 }
            0xB4 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; let v = bus.read(a); self.state.y = v; self.set_nz(v); 4 }
            0xBC => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); let v = Self::read_indexed(bus, base, a); self.state.y = v; self.set_nz(v); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            // Unstable NMOS 2A03 stores. Following established opcode-table behavior,
            // the data mask uses the base address high byte + 1; crossing a page also
            // corrupts the destination high byte to the value written.
            0x93 => {
                let base = self.indirect_y(bus);
                let address = base.wrapping_add(self.state.y as u16);
                Self::dummy_indexed_store_read(bus, base, address);
                let value = self.state.a & self.state.x & ((base >> 8) as u8).wrapping_add(1);
                let target = if base & 0xFF00 != address & 0xFF00 {
                    (u16::from(value) << 8) | (address & 0x00FF)
                } else { address };
                bus.write(target, value);
                6
            }
            0x9F => {
                let base = self.fetch_word(bus);
                let address = base.wrapping_add(self.state.y as u16);
                Self::dummy_indexed_store_read(bus, base, address);
                let value = self.state.a & self.state.x & ((base >> 8) as u8).wrapping_add(1);
                let target = if base & 0xFF00 != address & 0xFF00 {
                    (u16::from(value) << 8) | (address & 0x00FF)
                } else { address };
                bus.write(target, value);
                5
            }
            0x9B => {
                let base = self.fetch_word(bus);
                let address = base.wrapping_add(self.state.y as u16);
                Self::dummy_indexed_store_read(bus, base, address);
                self.state.sp = self.state.a & self.state.x;
                let value = self.state.sp & ((base >> 8) as u8).wrapping_add(1);
                let target = if base & 0xFF00 != address & 0xFF00 {
                    (u16::from(value) << 8) | (address & 0x00FF)
                } else { address };
                bus.write(target, value);
                5
            }
            0x9C => {
                let base = self.fetch_word(bus);
                let address = base.wrapping_add(self.state.x as u16);
                Self::dummy_indexed_store_read(bus, base, address);
                let value = self.state.y & ((base >> 8) as u8).wrapping_add(1);
                let target = if base & 0xFF00 != address & 0xFF00 {
                    (u16::from(value) << 8) | (address & 0x00FF)
                } else { address };
                bus.write(target, value);
                5
            }
            0x9E => {
                let base = self.fetch_word(bus);
                let address = base.wrapping_add(self.state.y as u16);
                Self::dummy_indexed_store_read(bus, base, address);
                let value = self.state.x & ((base >> 8) as u8).wrapping_add(1);
                let target = if base & 0xFF00 != address & 0xFF00 {
                    (u16::from(value) << 8) | (address & 0x00FF)
                } else { address };
                bus.write(target, value);
                5
            }
            0x85 => { let a = self.fetch(bus) as u16; bus.write(a, self.state.a); 3 }
            0x8D => { let a = self.fetch_word(bus); bus.write(a, self.state.a); 4 }
            0x86 => { let a = self.fetch(bus) as u16; bus.write(a, self.state.x); 3 }
            0x84 => { let a = self.fetch(bus) as u16; bus.write(a, self.state.y); 3 }
            0x95 => { let base = self.fetch(bus) as u16; let _ = bus.read(base); let a = base.wrapping_add(self.state.x as u16) & 0x00FF; bus.write(a, self.state.a); 4 }
            0x9D => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); Self::dummy_indexed_store_read(bus, base, a); bus.write(a, self.state.a); 5 }
            0x99 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); Self::dummy_indexed_store_read(bus, base, a); bus.write(a, self.state.a); 5 }
            0x81 => { let a = self.indirect_x(bus); bus.write(a, self.state.a); 6 }
            0x91 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); Self::dummy_indexed_store_read(bus, base, a); bus.write(a, self.state.a); 6 }
            0x96 => { let base = self.fetch(bus) as u16; let _ = bus.read(base); let a = base.wrapping_add(self.state.y as u16) & 0x00FF; bus.write(a, self.state.x); 4 }
            0x8E => { let a = self.fetch_word(bus); bus.write(a, self.state.x); 4 }
            0x94 => { let base = self.fetch(bus) as u16; let _ = bus.read(base); let a = base.wrapping_add(self.state.x as u16) & 0x00FF; bus.write(a, self.state.y); 4 }
            0x8C => { let a = self.fetch_word(bus); bus.write(a, self.state.y); 4 }
            // Stable unofficial SAX stores A & X without changing flags.
            0x87 => { let a = self.fetch(bus) as u16; bus.write(a, self.state.a & self.state.x); 3 }
            0x8F => { let a = self.fetch_word(bus); bus.write(a, self.state.a & self.state.x); 4 }
            0x97 => { let base = self.fetch(bus) as u16; let _ = bus.read(base); let a = base.wrapping_add(self.state.y as u16) & 0x00FF; bus.write(a, self.state.a & self.state.x); 4 }
            0x83 => { let a = self.indirect_x(bus); bus.write(a, self.state.a & self.state.x); 6 }
            0xAA => { self.state.x = self.state.a; self.set_nz(self.state.x); 2 }
            0xA8 => { self.state.y = self.state.a; self.set_nz(self.state.y); 2 }
            0x8A => { self.state.a = self.state.x; self.set_nz(self.state.a); 2 }
            0x98 => { self.state.a = self.state.y; self.set_nz(self.state.a); 2 }
            0xE8 => { self.state.x = self.state.x.wrapping_add(1); self.set_nz(self.state.x); 2 }
            0xE6 => { let a = self.fetch(bus) as u16; let old = bus.read(a); bus.write(a, old); let v = old.wrapping_add(1); bus.write(a, v); self.set_nz(v); 5 }
            0xEE => { let a = self.fetch_word(bus); let old = bus.read(a); bus.write(a, old); let v = old.wrapping_add(1); bus.write(a, v); self.set_nz(v); 6 }
            0xF6 => { let base = self.fetch(bus); let _ = bus.read(base as u16); let a = base.wrapping_add(self.state.x) as u16; let old = bus.read(a); bus.write(a, old); let v = old.wrapping_add(1); bus.write(a, v); self.set_nz(v); 6 }
            0xFE => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); Self::dummy_indexed_store_read(bus, base, a); let old = bus.read(a); bus.write(a, old); let v = old.wrapping_add(1); bus.write(a, v); self.set_nz(v); 7 }
            0xC6 => { let a = self.fetch(bus) as u16; let old = bus.read(a); bus.write(a, old); let v = old.wrapping_sub(1); bus.write(a, v); self.set_nz(v); 5 }
            0xCE => { let a = self.fetch_word(bus); let old = bus.read(a); bus.write(a, old); let v = old.wrapping_sub(1); bus.write(a, v); self.set_nz(v); 6 }
            0xD6 => { let base = self.fetch(bus); let _ = bus.read(base as u16); let a = base.wrapping_add(self.state.x) as u16; let old = bus.read(a); bus.write(a, old); let v = old.wrapping_sub(1); bus.write(a, v); self.set_nz(v); 6 }
            0xDE => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); Self::dummy_indexed_store_read(bus, base, a); let old = bus.read(a); bus.write(a, old); let v = old.wrapping_sub(1); bus.write(a, v); self.set_nz(v); 7 }
            0xC8 => { self.state.y = self.state.y.wrapping_add(1); self.set_nz(self.state.y); 2 }
            0xCA => { self.state.x = self.state.x.wrapping_sub(1); self.set_nz(self.state.x); 2 }
            0x88 => { self.state.y = self.state.y.wrapping_sub(1); self.set_nz(self.state.y); 2 }
            0x0A => { self.state.a = self.shift(self.state.a, 0); 2 }
            0x2A => { self.state.a = self.shift(self.state.a, 1); 2 }
            0x4A => { self.state.a = self.shift(self.state.a, 2); 2 }
            0x6A => { self.state.a = self.shift(self.state.a, 3); 2 }
            0x06 | 0x26 | 0x46 | 0x66 => { let a = self.fetch(bus) as u16; let old = bus.read(a); let v = self.shift(old, (opcode >> 5) & 3); bus.write(a, old); bus.write(a, v); 5 }
            0x16 | 0x36 | 0x56 | 0x76 => { let base = self.fetch(bus); let _ = bus.read(base as u16); let a = base.wrapping_add(self.state.x) as u16; let old = bus.read(a); let v = self.shift(old, (opcode >> 5) & 3); bus.write(a, old); bus.write(a, v); 6 }
            0x0E | 0x2E | 0x4E | 0x6E => { let a = self.fetch_word(bus); let old = bus.read(a); let v = self.shift(old, (opcode >> 5) & 3); bus.write(a, old); bus.write(a, v); 6 }
            0x1E | 0x3E | 0x5E | 0x7E => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); Self::dummy_indexed_store_read(bus, base, a); let old = bus.read(a); let v = self.shift(old, (opcode >> 5) & 3); bus.write(a, old); bus.write(a, v); 7 }
            0x69 => { let v = self.fetch(bus); self.adc(v); 2 }
            0x65 => { let a = self.fetch(bus) as u16; self.adc(bus.read(a)); 3 }
            0x75 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.adc(bus.read(a)); 4 }
            0x6D => { let a = self.fetch_word(bus); self.adc(bus.read(a)); 4 }
            0x7D => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.adc(Self::read_indexed(bus, base, a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x79 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.adc(Self::read_indexed(bus, base, a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x61 => { let a = self.indirect_x(bus); self.adc(bus.read(a)); 6 }
            0x71 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.adc(Self::read_indexed(bus, base, a)); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xE9 | 0xEB => { let v = self.fetch(bus); self.adc(!v); 2 }
            0xE5 => { let a = self.fetch(bus) as u16; self.adc(!bus.read(a)); 3 }
            0xF5 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.adc(!bus.read(a)); 4 }
            0xED => { let a = self.fetch_word(bus); self.adc(!bus.read(a)); 4 }
            0xFD => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.adc(!Self::read_indexed(bus, base, a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xF9 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.adc(!Self::read_indexed(bus, base, a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xE1 => { let a = self.indirect_x(bus); self.adc(!bus.read(a)); 6 }
            0xF1 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.adc(!Self::read_indexed(bus, base, a)); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x29 => { let v = self.fetch(bus); self.state.a &= v; self.set_nz(self.state.a); 2 }
            0x25 => { let a = self.fetch(bus) as u16; self.state.a &= bus.read(a); self.set_nz(self.state.a); 3 }
            0x2D => { let a = self.fetch_word(bus); self.state.a &= bus.read(a); self.set_nz(self.state.a); 4 }
            0x35 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.state.a &= bus.read(a); self.set_nz(self.state.a); 4 }
            0x3D => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.state.a &= Self::read_indexed(bus, base, a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x39 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a &= Self::read_indexed(bus, base, a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x21 => { let a = self.indirect_x(bus); self.state.a &= bus.read(a); self.set_nz(self.state.a); 6 }
            0x31 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a &= Self::read_indexed(bus, base, a); self.set_nz(self.state.a); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x05 => { let a = self.fetch(bus) as u16; self.state.a |= bus.read(a); self.set_nz(self.state.a); 3 }
            0x15 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.state.a |= bus.read(a); self.set_nz(self.state.a); 4 }
            0x19 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a |= Self::read_indexed(bus, base, a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x1D => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.state.a |= Self::read_indexed(bus, base, a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x01 => { let a = self.indirect_x(bus); self.state.a |= bus.read(a); self.set_nz(self.state.a); 6 }
            0x11 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a |= Self::read_indexed(bus, base, a); self.set_nz(self.state.a); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x0D => { let a = self.fetch_word(bus); self.state.a |= bus.read(a); self.set_nz(self.state.a); 4 }
            0x45 => { let a = self.fetch(bus) as u16; self.state.a ^= bus.read(a); self.set_nz(self.state.a); 3 }
            0x55 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.state.a ^= bus.read(a); self.set_nz(self.state.a); 4 }
            0x59 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a ^= Self::read_indexed(bus, base, a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x5D => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.state.a ^= Self::read_indexed(bus, base, a); self.set_nz(self.state.a); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x41 => { let a = self.indirect_x(bus); self.state.a ^= bus.read(a); self.set_nz(self.state.a); 6 }
            0x51 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.state.a ^= Self::read_indexed(bus, base, a); self.set_nz(self.state.a); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0x4D => { let a = self.fetch_word(bus); self.state.a ^= bus.read(a); self.set_nz(self.state.a); 4 }
            0x09 => { let v = self.fetch(bus); self.state.a |= v; self.set_nz(self.state.a); 2 }
            0x49 => { let v = self.fetch(bus); self.state.a ^= v; self.set_nz(self.state.a); 2 }
            0xC1 => { let a = self.indirect_x(bus); self.compare(self.state.a, bus.read(a)); 6 }
            0xC5 => { let a = self.fetch(bus) as u16; self.compare(self.state.a, bus.read(a)); 3 }
            0xD1 => { let base = self.indirect_y(bus); let a = base.wrapping_add(self.state.y as u16); self.compare(self.state.a, Self::read_indexed(bus, base, a)); 5 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xC9 => { let v = self.fetch(bus); self.compare(self.state.a, v); 2 }
            0xE0 => { let v = self.fetch(bus); self.compare(self.state.x, v); 2 }
            0xC0 => { let v = self.fetch(bus); self.compare(self.state.y, v); 2 }
            0xCD => { let a = self.fetch_word(bus); self.compare(self.state.a, bus.read(a)); 4 }
            0xD5 => { let a = self.fetch(bus).wrapping_add(self.state.x) as u16; self.compare(self.state.a, bus.read(a)); 4 }
            0xDD => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.x as u16); self.compare(self.state.a, Self::read_indexed(bus, base, a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
            0xD9 => { let base = self.fetch_word(bus); let a = base.wrapping_add(self.state.y as u16); self.compare(self.state.a, Self::read_indexed(bus, base, a)); 4 + u8::from(base & 0xFF00 != a & 0xFF00) }
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
            0x28 => { self.state.status = (self.pop(bus) | flags::UNUSED) & !flags::BREAK; self.irq_poll_delay = true; 4 }
            0x18 => { self.set_flag(flags::CARRY, false); 2 }
            0x38 => { self.set_flag(flags::CARRY, true); 2 }
            0x58 => { self.set_flag(flags::INTERRUPT_DISABLE, false); self.irq_poll_delay = true; 2 }
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
            // Stable composite unofficial RMW instructions adapted to local CPU helpers.
            _ if Self::is_unofficial_rmw(opcode) => self.unofficial_rmw(bus, opcode),
            // NMOS 6502 KIL/JAM encodings halt the processor indefinitely until reset.
            0x02 | 0x12 | 0x22 | 0x32 | 0x42 | 0x52 | 0x62 | 0x72 |
            0x92 | 0xB2 | 0xD2 | 0xF2 => {
                self.jammed = true;
                self.jam_opcode = opcode;
                2
            }
            _ => return Err(CpuError::UnsupportedOpcode { opcode, pc }),
        };
        self.state.status |= flags::UNUSED;
        self.cycles += cycles as u64;
        Ok(StepResult { pc, opcode, cycles })
    }

    fn is_unofficial_rmw(opcode: u8) -> bool {
        matches!(opcode,
            0x03 | 0x07 | 0x0F | 0x13 | 0x17 | 0x1B | 0x1F |
            0x23 | 0x27 | 0x2F | 0x33 | 0x37 | 0x3B | 0x3F |
            0x43 | 0x47 | 0x4F | 0x53 | 0x57 | 0x5B | 0x5F |
            0x63 | 0x67 | 0x6F | 0x73 | 0x77 | 0x7B | 0x7F |
            0xC3 | 0xC7 | 0xCF | 0xD3 | 0xD7 | 0xDB | 0xDF |
            0xE3 | 0xE7 | 0xEF | 0xF3 | 0xF7 | 0xFB | 0xFF)
    }

    /// Executes stable unofficial read-modify-write opcodes with nominal instruction timing.
    /// Dummy bus writes and per-cycle address glitches are not yet modeled.
    fn unofficial_rmw(&mut self, bus: &mut CpuBus, opcode: u8) -> u8 {
        // (operation, addressing mode, nominal cycles)
        // Modes: (indirect,X), zero page, absolute, (indirect),Y, zero page,X,
        // absolute,Y and absolute,X.
        let (operation, mode, cycles) = match opcode {
            0x03 => (0, 0, 8), 0x07 => (0, 1, 5), 0x0F => (0, 2, 6), 0x13 => (0, 3, 8), 0x17 => (0, 4, 6), 0x1B => (0, 5, 7), 0x1F => (0, 6, 7),
            0x23 => (1, 0, 8), 0x27 => (1, 1, 5), 0x2F => (1, 2, 6), 0x33 => (1, 3, 8), 0x37 => (1, 4, 6), 0x3B => (1, 5, 7), 0x3F => (1, 6, 7),
            0x43 => (2, 0, 8), 0x47 => (2, 1, 5), 0x4F => (2, 2, 6), 0x53 => (2, 3, 8), 0x57 => (2, 4, 6), 0x5B => (2, 5, 7), 0x5F => (2, 6, 7),
            0x63 => (3, 0, 8), 0x67 => (3, 1, 5), 0x6F => (3, 2, 6), 0x73 => (3, 3, 8), 0x77 => (3, 4, 6), 0x7B => (3, 5, 7), 0x7F => (3, 6, 7),
            0xC3 => (4, 0, 8), 0xC7 => (4, 1, 5), 0xCF => (4, 2, 6), 0xD3 => (4, 3, 8), 0xD7 => (4, 4, 6), 0xDB => (4, 5, 7), 0xDF => (4, 6, 7),
            0xE3 => (5, 0, 8), 0xE7 => (5, 1, 5), 0xEF => (5, 2, 6), 0xF3 => (5, 3, 8), 0xF7 => (5, 4, 6), 0xFB => (5, 5, 7), 0xFF => (5, 6, 7),
            _ => unreachable!("guarded by is_unofficial_rmw"),
        };
        let address = match mode {
            0 => self.indirect_x(bus),
            1 => self.fetch(bus) as u16,
            2 => self.fetch_word(bus),
            3 => {
                let base = self.indirect_y(bus);
                let address = base.wrapping_add(self.state.y as u16);
                Self::dummy_indexed_store_read(bus, base, address);
                address
            }
            4 => {
                let base = self.fetch(bus);
                let _ = bus.read(base as u16);
                base.wrapping_add(self.state.x) as u16
            }
            5 => {
                let base = self.fetch_word(bus);
                let address = base.wrapping_add(self.state.y as u16);
                Self::dummy_indexed_store_read(bus, base, address);
                address
            }
            _ => {
                let base = self.fetch_word(bus);
                let address = base.wrapping_add(self.state.x as u16);
                Self::dummy_indexed_store_read(bus, base, address);
                address
            }
        };
        let value = bus.read(address);
        let modified = match operation {
            0 => { let v = self.shift(value, 0); self.state.a |= v; self.set_nz(self.state.a); v } // SLO
            1 => { let v = self.shift(value, 1); self.state.a &= v; self.set_nz(self.state.a); v } // RLA
            2 => { let v = self.shift(value, 2); self.state.a ^= v; self.set_nz(self.state.a); v } // SRE
            3 => { let v = self.shift(value, 3); self.adc(v); v } // RRA: ROR carry feeds ADC
            4 => { let v = value.wrapping_sub(1); self.compare(self.state.a, v); v } // DCP
            _ => { let v = value.wrapping_add(1); self.adc(!v); v } // ISC/ISB
        };
        // NMOS 6502 RMW instructions write the original byte before the result.
        // The first write can trigger memory-mapped register and mapper side effects.
        bus.write(address, value);
        bus.write(address, modified);
        cycles
    }

    fn indirect_x(&mut self, bus: &mut CpuBus) -> u16 {
        let zero_page = self.fetch(bus);
        // The 6502 performs a dummy read of the unindexed zero-page operand before
        // adding X and reading the effective pointer.
        let _ = bus.read(zero_page as u16);
        let pointer = zero_page.wrapping_add(self.state.x);
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
    /// Performs the extra wrong-page read used by indexed read instructions when
    /// the effective address crosses a 256-byte page boundary.
    fn read_indexed(bus: &mut CpuBus, base: u16, address: u16) -> u8 {
        if base & 0xFF00 != address & 0xFF00 {
            let dummy = (base & 0xFF00) | (address & 0x00FF);
            let _ = bus.read(dummy);
        }
        bus.read(address)
    }

    /// Performs the mandatory wrong-page/dummy read before an indexed store.
    /// Unlike indexed loads, stores always perform this bus access, even without page crossing.
    fn dummy_indexed_store_read(bus: &mut CpuBus, base: u16, address: u16) {
        let dummy = (base & 0xFF00) | (address & 0x00FF);
        let _ = bus.read(dummy);
    }

    fn branch(&mut self, bus: &mut CpuBus, take: bool) -> u8 {
        let offset = self.fetch(bus) as i8;
        if !take { return 2; }

        // A taken 6502 branch performs a dummy read at the sequential PC. When
        // crossing a page, it then reads the intermediate address with the old
        // high byte and target low byte before the target instruction executes.
        let sequential_pc = self.state.pc;
        let _ = bus.read(sequential_pc);
        self.state.pc = sequential_pc.wrapping_add_signed(offset as i16);
        if sequential_pc & 0xFF00 != self.state.pc & 0xFF00 {
            let intermediate = (sequential_pc & 0xFF00) | (self.state.pc & 0x00FF);
            let _ = bus.read(intermediate);
            4
        } else {
            3
        }
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
    #[test]
    fn unofficial_lax_loads_accumulator_and_x_with_shared_flags() {
        let (mut cpu, mut bus) = setup(&[0xA7, 0x10, 0xBF, 0xFF, 0x02]);
        bus.write(0x0010, 0x80);
        bus.write(0x0300, 0x00);
        cpu.state.a = 0x11;
        cpu.state.x = 0x22;
        cpu.state.y = 1;
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 3);
        assert_eq!((cpu.state.a, cpu.state.x), (0x80, 0x80));
        assert_ne!(cpu.state.status & flags::NEGATIVE, 0);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5, "LAX absolute,Y adds a cycle on page crossing");
        assert_eq!((cpu.state.a, cpu.state.x), (0x00, 0x00));
        assert_ne!(cpu.state.status & flags::ZERO, 0);
    }

    #[test]
    fn unofficial_lax_immediate_loads_both_registers_and_updates_flags() {
        let (mut cpu, mut bus) = setup(&[0xAB, 0x80, 0xAB, 0x00]);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2);
        assert_eq!((cpu.state().a, cpu.state().x), (0x80, 0x80));
        assert_ne!(cpu.state().status & flags::NEGATIVE, 0);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2);
        assert_eq!((cpu.state().a, cpu.state().x), (0x00, 0x00));
        assert_ne!(cpu.state().status & flags::ZERO, 0);
    }

    #[test]
    fn unofficial_las_loads_a_x_and_stack_pointer_with_page_cross_penalty() {
        let (mut cpu, mut bus) = setup(&[0xBB, 0xFF, 0x02]);
        cpu.state.y = 1;
        cpu.state.sp = 0xF3;
        bus.write(0x0300, 0x8F);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!((cpu.state.a, cpu.state.x, cpu.state.sp), (0x83, 0x83, 0x83));
        assert_ne!(cpu.state.status & flags::NEGATIVE, 0);

        cpu.state.y = 0;
        cpu.state.sp = 0xFF;
        cpu.set_pc(0x0600);
        bus.write(0x0600, 0xBB);
        bus.write(0x0601, 0x00);
        bus.write(0x0602, 0x03);
        bus.write(0x0300, 0x42);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 4);
        assert_eq!((cpu.state.a, cpu.state.x, cpu.state.sp), (0x42, 0x42, 0x42));
    }

    #[test]
    fn unofficial_sax_stores_accumulator_and_x_without_changing_flags() {
        let (mut cpu, mut bus) = setup(&[0x87, 0x10, 0x97, 0x20, 0x8F, 0x00, 0x03]);
        cpu.state.a = 0xF3;
        cpu.state.x = 0x5A;
        cpu.state.status = flags::CARRY | flags::UNUSED;
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 3);
        assert_eq!(bus.read(0x0010), 0x52);
        assert_eq!(cpu.state.status, flags::CARRY | flags::UNUSED);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 4);
        assert_eq!(bus.read(0x0020), 0x52);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 4);
        assert_eq!(bus.read(0x0300), 0x52);
        assert_eq!(cpu.state.status, flags::CARRY | flags::UNUSED);
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
    fn unofficial_sbc_immediate_alias_matches_official_sbc() {
        let (mut official, mut official_bus) = setup(&[0xA9, 0x50, 0x38, 0xE9, 0x10]);
        let (mut unofficial, mut unofficial_bus) = setup(&[0xA9, 0x50, 0x38, 0xEB, 0x10]);
        for _ in 0..3 {
            official.step(&mut official_bus).unwrap();
            unofficial.step(&mut unofficial_bus).unwrap();
        }
        assert_eq!(official.state(), unofficial.state());
        assert_eq!(official.total_cycles(), unofficial.total_cycles());
    }

    #[test]
    fn taken_branch_performs_dummy_reads_and_preserves_target_and_cycles() {
        let (mut cpu, mut bus) = setup(&[0xD0, 0x02, 0xEA, 0xEA, 0xA9, 0x44]);
        let result = cpu.step(&mut bus).unwrap();
        assert_eq!(result.cycles, 3);
        assert_eq!(cpu.state().pc, 0x0604);

        let mut bus = CpuBus::new();
        bus.write(0x06FD, 0xD0);
        bus.write(0x06FE, 0x02);
        let mut cpu = Cpu::new();
        cpu.set_pc(0x06FD);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 4);
        assert_eq!(cpu.state().pc, 0x0701);
    }

    #[test]
    fn indexed_read_performs_wrong_page_bus_access_on_page_crossing() {
        let (mut cpu, mut bus) = setup(&[0xA2, 0x17, 0xBD, 0xFF, 0x40]); // LDX #$17; LDA $40FF,X
        bus.set_controller_state(0, 1);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        cpu.step(&mut bus).unwrap();
        let result = cpu.step(&mut bus).unwrap();
        assert_eq!(result.cycles, 5);
        // The intermediate read hits $4016, consuming A from the controller and
        // leaving its value on the open bus subsequently read from unmapped $4116.
        assert_eq!(cpu.state().a, 0x41);
        assert_eq!(bus.read(0x4016) & 1, 0);
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
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x3FFE] = 0x00;
        image[16 + 0x3FFF] = 0x07;
        bus.insert_cartridge(crate::cartridge::Cartridge::parse(&image).unwrap());
        bus.write(0x4017, 0x00);
        bus.tick_apu(14_915);
        assert!(bus.irq_pending());
        // IRQ is masked before CLI, so CLI executes first.
        assert_eq!(cpu.step(&mut bus).unwrap().opcode, 0x58);
        let following = cpu.step(&mut bus).unwrap();
        assert_eq!(following.opcode, 0xEA, "CLI must allow one following instruction before IRQ entry");
        let irq = cpu.step(&mut bus).unwrap();
        assert_eq!(irq.cycles, 7);
        assert_eq!(cpu.state().pc, 0x0700);
        assert_ne!(cpu.state().status & flags::INTERRUPT_DISABLE, 0);
        assert_eq!(bus.read(0x01FD), 0x06); // return PC high byte
        assert_eq!(bus.read(0x01FC), 0x02); // return PC low byte after the deferred instruction
        assert_eq!(bus.read(0x01FB) & flags::BREAK, 0);
    }

    #[test]
    fn plp_defers_irq_polling_for_one_instruction_when_it_clears_interrupt_disable() {
        let (mut cpu, mut bus) = setup(&[0x28, 0xEA]); // PLP; NOP
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x3FFE] = 0x00;
        image[16 + 0x3FFF] = 0x07;
        bus.insert_cartridge(crate::cartridge::Cartridge::parse(&image).unwrap());
        bus.write(0x01FE, flags::UNUSED); // PLP restores I=0 from the stack.
        bus.write(0x4017, 0x00);
        bus.tick_apu(14_915);
        assert!(bus.irq_pending());

        assert_eq!(cpu.step(&mut bus).unwrap().opcode, 0x28);
        assert_eq!(cpu.step(&mut bus).unwrap().opcode, 0xEA);
        let irq = cpu.step(&mut bus).unwrap();
        assert_eq!(irq.opcode, 0xFE);
        assert_eq!(irq.cycles, 7);
        assert_eq!(cpu.state().pc, 0x0700);
    }

    #[test]
    fn irq_is_not_taken_when_interrupt_disable_flag_is_set() {
        let (mut cpu, mut bus) = setup(&[0xEA]);
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x3FFE] = 0x00;
        image[16 + 0x3FFF] = 0x07;
        bus.insert_cartridge(crate::cartridge::Cartridge::parse(&image).unwrap());
        bus.tick_apu(14_915);
        assert_eq!(cpu.step(&mut bus).unwrap().opcode, 0xEA);
        assert_eq!(cpu.state().pc, 0x0601);
    }

    #[test]
    fn nmi_takes_priority_over_a_pending_apu_irq() {
        let (mut cpu, mut bus) = setup(&[0x58, 0xEA]); // CLI; NOP
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x3FFA] = 0x00;
        image[16 + 0x3FFB] = 0x90;
        image[16 + 0x3FFE] = 0x00;
        image[16 + 0x3FFF] = 0x70;
        image[16 + 0x1000] = 0xEA;
        bus.insert_cartridge(crate::cartridge::Cartridge::parse(&image).unwrap());
        cpu.step(&mut bus).unwrap(); // Clear I before either interrupt.
        bus.tick_ppu(243 * 341 + 1);
        bus.write(0x2000, 0x80);
        bus.tick_apu(14_915);
        assert!(bus.irq_pending());
        let result = cpu.step(&mut bus).unwrap();
        assert_eq!(result.cycles, 7);
        assert_eq!(result.opcode, 0xFF, "NMI must win over maskable IRQ");
        assert_eq!(cpu.state().pc, 0x9000);
        assert!(bus.irq_pending(), "servicing NMI must not acknowledge the APU IRQ");
    }

    #[test]
    fn brk_pushes_incremented_pc_and_break_status_then_uses_irq_vector() {
        let (mut cpu, mut bus) = setup(&[0x00, 0xEA]);
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x3FFE] = 0x34;
        image[16 + 0x3FFF] = 0x12;
        bus.insert_cartridge(crate::cartridge::Cartridge::parse(&image).unwrap());
        let result = cpu.step(&mut bus).unwrap();
        assert_eq!(result.cycles, 7);
        assert_eq!(cpu.state().pc, 0x1234);
        assert_eq!(bus.read(0x01FD), 0x06);
        assert_eq!(bus.read(0x01FC), 0x02);
        assert_ne!(bus.read(0x01FB) & flags::BREAK, 0);
        assert_ne!(bus.read(0x01FB) & flags::UNUSED, 0);
        assert_ne!(cpu.state().status & flags::INTERRUPT_DISABLE, 0);
    }

    #[test]
    fn indirect_jmp_reproduces_the_original_6502_page_wrap_bug() {
        let mut bus = CpuBus::new();
        // JMP ($02FF) reads its high byte from $0200 on the NMOS 6502/2A03,
        // not from $0300. Keep a conflicting value at $0300 to catch regressions.
        bus.write(0x0600, 0x6C);
        bus.write(0x0601, 0xFF);
        bus.write(0x0602, 0x02);
        bus.write(0x02FF, 0x34);
        bus.write(0x0200, 0x12);
        bus.write(0x0300, 0x56);
        let mut cpu = Cpu::new();
        cpu.set_pc(0x0600);

        let result = cpu.step(&mut bus).unwrap();

        assert_eq!(result.cycles, 5);
        assert_eq!(cpu.state().pc, 0x1234);
    }

    #[test]
    fn indexed_store_performs_dummy_read_before_writing_target() {
        let mut bus = CpuBus::new();
        bus.set_controller_state(0, 0b0000_0010); // A=0, B=1.
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        bus.write(0x0600, 0x9D); // STA $40FF,X; dummy read is $4016.
        bus.write(0x0601, 0xFF);
        bus.write(0x0602, 0x40);
        let mut cpu = Cpu::new();
        cpu.set_pc(0x0600);
        cpu.state.x = 0x17;
        cpu.state.a = 0x55;

        let result = cpu.step(&mut bus).unwrap();

        assert_eq!(result.cycles, 5);
        assert_eq!(bus.read(0x4016) & 1, 1, "dummy read must advance the controller shift register");
    }

    #[test]
    fn adc_and_sbc_remain_binary_when_decimal_flag_is_set_on_the_2a03() {
        let (mut cpu, mut bus) = setup(&[
            0xF8,       // SED: the 2A03 stores D but ignores decimal arithmetic.
            0xA9, 0x09, // LDA #$09
            0x18,       // CLC
            0x69, 0x01, // ADC #$01 -> $0A, not BCD $10
            0x38,       // SEC
            0xE9, 0x01, // SBC #$01 -> $09, not BCD $09 via decimal correction
        ]);

        for _ in 0..4 {
            cpu.step(&mut bus).unwrap();
        }
        assert_eq!(cpu.state().a, 0x0A);
        assert_ne!(cpu.state().status & flags::DECIMAL, 0);
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.state().a, 0x09);
        assert_ne!(cpu.state().status & flags::DECIMAL, 0);
    }

    #[test]
    fn status_flag_instructions_and_php_plp_round_trip_are_consistent() {
        let (mut cpu, mut bus) = setup(&[
            0x38, // SEC
            0xF8, // SED (decimal flag is stored, arithmetic remains 2A03 binary)
            0xB8, // CLV
            0x58, // CLI
            0x08, // PHP
            0x18, // CLC
            0xD8, // CLD
            0x78, // SEI
            0x28, // PLP
        ]);
        for _ in 0..5 { cpu.step(&mut bus).unwrap(); }
        assert_ne!(cpu.state().status & flags::CARRY, 0);
        assert_ne!(cpu.state().status & flags::DECIMAL, 0);
        assert_eq!(cpu.state().status & flags::OVERFLOW, 0);
        assert_eq!(cpu.state().status & flags::INTERRUPT_DISABLE, 0);

        for _ in 0..3 { cpu.step(&mut bus).unwrap(); }
        assert_eq!(cpu.state().status & flags::CARRY, 0);
        assert_eq!(cpu.state().status & flags::DECIMAL, 0);
        assert_ne!(cpu.state().status & flags::INTERRUPT_DISABLE, 0);
        cpu.step(&mut bus).unwrap(); // PLP restores the status captured by PHP.
        assert_ne!(cpu.state().status & flags::CARRY, 0);
        assert_ne!(cpu.state().status & flags::DECIMAL, 0);
        assert_eq!(cpu.state().status & flags::INTERRUPT_DISABLE, 0);
        assert_ne!(cpu.state().status & flags::UNUSED, 0);
        assert_eq!(cpu.state().status & flags::BREAK, 0);
    }

    #[test]
    fn all_stable_unofficial_rmw_opcodes_are_recognized() {
        let opcodes = [
            0x03, 0x07, 0x0F, 0x13, 0x17, 0x1B, 0x1F,
            0x23, 0x27, 0x2F, 0x33, 0x37, 0x3B, 0x3F,
            0x43, 0x47, 0x4F, 0x53, 0x57, 0x5B, 0x5F,
            0x63, 0x67, 0x6F, 0x73, 0x77, 0x7B, 0x7F,
            0xC3, 0xC7, 0xCF, 0xD3, 0xD7, 0xDB, 0xDF,
            0xE3, 0xE7, 0xEF, 0xF3, 0xF7, 0xFB, 0xFF,
        ];
        assert_eq!(opcodes.len(), 42);
        for opcode in opcodes {
            assert!(Cpu::is_unofficial_rmw(opcode), "opcode {opcode:#04X} not recognized");
        }
        for opcode in [0x02, 0x04, 0x0B, 0x20, 0x80, 0xA9, 0xEA, 0xFF_u8.wrapping_sub(1)] {
            assert!(!Cpu::is_unofficial_rmw(opcode), "unexpected RMW opcode {opcode:#04X}");
        }
    }

    #[test]
    fn unofficial_composite_rmw_families_update_memory_registers_and_flags() {
        let (mut cpu, mut bus) = setup(&[
            0x07, 0x10, // SLO $10
            0x27, 0x11, // RLA $11
            0x47, 0x12, // SRE $12
            0x67, 0x13, // RRA $13
            0xC7, 0x14, // DCP $14
            0xE7, 0x15, // ISC $15
        ]);
        for (address, value) in [(0x10, 0x40), (0x11, 0x03), (0x12, 0x03), (0x13, 0x02), (0x14, 0x05), (0x15, 0x01)] {
            bus.write(address, value);
        }
        cpu.state.a = 0x01;
        cpu.state.status |= flags::CARRY;
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(bus.read(0x10), 0x80);
        assert_eq!(cpu.state.a, 0x81); // SLO
        assert_ne!(cpu.state.status & flags::NEGATIVE, 0);

        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(bus.read(0x11), 0x06);
        assert_eq!(cpu.state.a, 0x00); // RLA
        assert_ne!(cpu.state.status & flags::ZERO, 0);

        cpu.state.a = 0xFF;
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(bus.read(0x12), 0x01);
        assert_eq!(cpu.state.a, 0xFE); // SRE

        cpu.state.a = 0x10;
        cpu.state.status &= !flags::CARRY;
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(bus.read(0x13), 0x01);
        assert_eq!(cpu.state.a, 0x11); // RRA

        cpu.state.a = 0x04;
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(bus.read(0x14), 0x04);
        assert_ne!(cpu.state.status & flags::ZERO, 0); // DCP compares equal

        cpu.state.a = 0x10;
        cpu.state.status |= flags::CARRY;
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(bus.read(0x15), 0x02);
        assert_eq!(cpu.state.a, 0x0E); // ISC: $10 - $02
    }

    #[test]
    fn unofficial_immediate_anc_alr_arr_and_axs_update_expected_state() {
        // ANC #$80: A=$80, N and C set. ALR #$03: A=$01, C set.
        // ARR #$FF with C set rotates $01 to $80; reload A=$03 before testing AXS.
        let (mut cpu, mut bus) = setup(&[
            0xA9, 0xFF, // LDA #$FF
            0x0B, 0x80, // ANC #$80 -> A=$80, C=N=1
            0xA9, 0x03, // LDA #$03
            0x4B, 0x03, // ALR #$03 -> A=$01, C=1
            0x6B, 0xFF, // ARR #$FF -> A=$80, C=0, N=1, V=0
            0xA9, 0x03, // LDA #$03 (prepare A & X = $03 for AXS)
            0xA2, 0x03, // LDX #$03
            0xCB, 0x01, // AXS #$01 -> X=$02, C=1
        ]);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2);
        assert_eq!(cpu.state().a, 0x80);
        assert_ne!(cpu.state().status & flags::CARRY, 0);
        assert_ne!(cpu.state().status & flags::NEGATIVE, 0);
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.state().a, 0x01);
        assert_ne!(cpu.state().status & flags::CARRY, 0);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.state().a, 0x80);
        assert_eq!(cpu.state().status & flags::CARRY, 0);
        assert_ne!(cpu.state().status & flags::NEGATIVE, 0);
        assert_eq!(cpu.state().status & flags::OVERFLOW, 0);
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap(); // AXS #$01
        assert_eq!(cpu.state().x, 0x02);
        assert_ne!(cpu.state().status & flags::CARRY, 0);
        assert_eq!(cpu.state().status & flags::ZERO, 0);
    }

    #[test] fn kil_opcode_is_modeled_as_a_cpu_jam() {
        let (mut c, mut b) = setup(&[0x02, 0xA9, 0x55]);
        assert_eq!(c.step(&mut b).unwrap().cycles, 2);
        assert_eq!(c.step(&mut b).unwrap().cycles, 1);
        assert_eq!(c.state().pc, 0x0601);
        assert_eq!(c.state().a, 0);
    }
    #[test]
    fn nmi_pushes_cpu_state_and_jumps_to_nmi_vector() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x3FFA] = 0x00; // $FFFA points to the $9000 NMI handler
        image[16 + 0x3FFB] = 0x90;
        image[16 + 0x0600] = 0xEA;
        let mut bus = CpuBus::new();
        bus.insert_cartridge(crate::cartridge::Cartridge::parse(&image).unwrap());
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
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x3FFC] = 0x34;
        image[16 + 0x3FFD] = 0x12;
        let cartridge = crate::cartridge::Cartridge::parse(&image).unwrap();
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

    #[test]
    fn indexed_inc_performs_dummy_read_before_target_access() {
        let (mut cpu, mut bus) = setup(&[
            0xA2, 0x17,             // LDX #$17
            0xFE, 0xFF, 0x40,       // INC $40FF,X: dummy read $4016, target $4116
            0xAD, 0x16, 0x40,       // LDA $4016: should now read controller B
        ]);
        bus.set_controller_state(0, 0b0000_0010);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 7);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.state().a & 1, 1, "indexed INC must consume controller A during its dummy read");
    }

    #[test]
    fn unofficial_xaa_uses_common_and_mask_model() {
        let (mut cpu, mut bus) = setup(&[0x8B, 0xF3]);
        cpu.state.a = 0xCC;
        cpu.state.x = 0x5A;
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2);
        assert_eq!(cpu.state.a, 0x40);
        assert_eq!(cpu.state.status & (flags::NEGATIVE | flags::ZERO), 0);
    }

    #[test]
    fn absolute_x_unofficial_nop_preserves_page_cross_cycles() {
        let (mut cpu, mut bus) = setup(&[0xA2, 0x01, 0x1C, 0xFF, 0x40]);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(cpu.state.pc, 0x0605);
    }

    #[test]
    fn unofficial_rmw_updates_memory_accumulator_and_carry() {
        let (mut cpu, mut bus) = setup(&[
            0xA9, 0x01,       // LDA #$01
            0x07, 0x20,       // SLO $20: ASL memory, then ORA into A
        ]);
        bus.write(0x20, 0x81);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(bus.read(0x20), 0x02);
        assert_eq!(cpu.state().a, 0x03);
        assert_ne!(cpu.state().status & flags::CARRY, 0);
    }

    #[test]
    fn indirect_addressing_wraps_pointer_reads_inside_zero_page() {
        let (mut cpu, mut bus) = setup(&[
            0xA2, 0x01, // LDX #1
            0xA1, 0xFE, // LDA ($FE,X): pointer wraps to $FF, high byte wraps to $00
            0xA0, 0x01, // LDY #1
            0xB1, 0xFF, // LDA ($FF),Y: high byte comes from $00
        ]);
        bus.write(0x00FF, 0x00);
        bus.write(0x0000, 0x02);
        bus.write(0x0001, 0x03);
        bus.write(0x0200, 0x5A);
        bus.write(0x0201, 0xA5);

        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 6);
        assert_eq!(cpu.state().a, 0x5A);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(cpu.state().a, 0xA5);
    }

    #[test]
    fn taken_branches_charge_page_crossing_cycles_in_both_directions() {
        let (mut cpu, mut bus) = setup(&[]);
        // BNE at $06FD: after fetching its operand PC=$06FF, +2 crosses into $07xx.
        bus.write(0x06FD, 0xD0);
        bus.write(0x06FE, 0x02);
        cpu.set_pc(0x06FD);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 4);
        assert_eq!(cpu.state().pc, 0x0701);

        // Put the next branch at $0700 and use a negative offset to cross back to $06xx.
        bus.write(0x0700, 0xD0);
        bus.write(0x0701, 0xFC); // PC=$0702, -4 => $06FE
        cpu.set_pc(0x0700);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 4);
        assert_eq!(cpu.state().pc, 0x06FE);
    }

    #[test]
    fn unofficial_nop_encodings_consume_operands_without_changing_registers_or_flags() {
        let program = [
            0xA9, 0x55,       // LDA #$55
            0xA2, 0x01,       // LDX #1
            0xA0, 0x80,       // LDY #$80
            0x04, 0x20,       // NOP zero page
            0x14, 0x20,       // NOP zero page,X
            0x80, 0xEA,       // NOP immediate (operand itself is an opcode byte)
            0x0C, 0x00, 0x02, // NOP absolute
            0x1A,             // NOP implied
        ];
        let (mut cpu, mut bus) = setup(&program);
        bus.write(0x0020, 0xAA);
        bus.write(0x0021, 0xBB);
        bus.write(0x0200, 0xCC);
        for _ in 0..3 { cpu.step(&mut bus).unwrap(); }
        let before = cpu.state();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 3);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 4);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 4);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2);
        assert_eq!(cpu.state().a, before.a);
        assert_eq!(cpu.state().x, before.x);
        assert_eq!(cpu.state().y, before.y);
        assert_eq!(cpu.state().status, before.status);
    }

    #[test]
    fn unofficial_absolute_x_nop_charges_page_cross_cycle() {
        let (mut cpu, mut bus) = setup(&[0xA2, 0x01, 0x1C, 0xFF, 0x02]);
        bus.write(0x0300, 0x42);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(cpu.state().pc, 0x0605);
    }


    #[test]
    fn unofficial_rmw_slo_covers_all_seven_addressing_modes_and_nominal_cycles() {
        let program = [
            0xA2, 0x01,             // LDX #1
            0xA0, 0x01,             // LDY #1
            0x03, 0x20,             // SLO ($20,X) -> $0200
            0x07, 0x24,             // SLO $24
            0x0F, 0x01, 0x02,       // SLO $0201
            0x13, 0x30,             // SLO ($30),Y -> $0201
            0x17, 0x23,             // SLO $23,X -> $24
            0x1B, 0xFF, 0x01,       // SLO $01FF,Y -> $0200
            0x1F, 0xFF, 0x01,       // SLO $01FF,X -> $0200
        ];
        let (mut cpu, mut bus) = setup(&program);
        bus.write(0x0021, 0x00);
        bus.write(0x0022, 0x02);
        bus.write(0x0030, 0x00);
        bus.write(0x0031, 0x02);
        bus.write(0x0024, 0x01);
        bus.write(0x0200, 0x01);
        bus.write(0x0201, 0x01);

        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2); // LDX
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2); // LDY
        for expected_cycles in [8, 5, 6, 8, 6, 7, 7] {
            let result = cpu.step(&mut bus).unwrap();
            assert_eq!(result.cycles, expected_cycles, "opcode {:#04X}", result.opcode);
        }
        assert_eq!(bus.read(0x0200), 0x08); // Three RMW instructions target this byte.
        assert_eq!(bus.read(0x0201), 0x04); // Two RMW instructions target this byte.
        assert_eq!(bus.read(0x0024), 0x04); // Zero-page and zero-page,X forms.
    }
    #[test]
    fn ahx_indirect_indexed_uses_pointer_base_for_mask_and_page_corruption() {
        let (mut cpu, mut bus) = setup(&[
            0xA9, 0xFF, // LDA #$FF
            0xA2, 0x0F, // LDX #$0F
            0xA0, 0x02, // LDY #$02
            0x93, 0x10, // AHX ($10),Y
        ]);
        bus.write(0x0010, 0xFF);
        bus.write(0x0011, 0x20);
        for _ in 0..3 { cpu.step(&mut bus).unwrap(); }
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 6);
        assert_eq!(bus.read(0x0101), 0x01);
    }

    #[test]
    fn unstable_indexed_stores_mask_data_and_model_page_cross_corruption() {
        let (mut cpu, mut bus) = setup(&[
            0xA2, 0x0F,       // LDX #$0F
            0xA0, 0x02,       // LDY #$02
            0x9E, 0xFF, 0x20, // SHX $20FF,Y -> crossing writes $01 to $0101
            0xA9, 0xFF,       // LDA #$FF
            0x9F, 0x10, 0x02, // AHX $0210,Y -> $0212, mask $03, value $03
            0x9C, 0x10, 0x02, // SHY $0210,X -> $021F, value $03 & Y = $02
        ]);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(bus.read(0x0101), 0x01);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 2);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(bus.read(0x0212), 0x03);
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(bus.read(0x021F), 0x02);
    }

    #[test]
    fn tas_sets_stack_pointer_before_masked_store() {
        let (mut cpu, mut bus) = setup(&[
            0xA9, 0x3F,       // LDA #$3F
            0xA2, 0x1F,       // LDX #$1F
            0xA0, 0x01,       // LDY #$01
            0x9B, 0x00, 0x02, // TAS $0200,Y: SP=A&X=$1F; value=$1F & $03=$03
        ]);
        for _ in 0..3 { cpu.step(&mut bus).unwrap(); }
        assert_eq!(cpu.step(&mut bus).unwrap().cycles, 5);
        assert_eq!(cpu.state().sp, 0x1F);
        assert_eq!(bus.read(0x0201), 0x03);
    }
    #[test]
    fn kil_opcodes_jam_cpu_until_reset() {
        for opcode in [0x02, 0x12, 0x22, 0x32, 0x42, 0x52, 0x62, 0x72, 0x92, 0xB2, 0xD2, 0xF2] {
            let (mut cpu, mut bus) = setup(&[opcode, 0xA9, 0x55]);
            let before_pc = cpu.state().pc;
            let first = cpu.step(&mut bus).unwrap();
            assert_eq!(first.opcode, opcode);
            assert_eq!(first.cycles, 2);
            assert_eq!(cpu.state().pc, before_pc + 1);

            let jammed = cpu.step(&mut bus).unwrap();
            assert_eq!(jammed.opcode, opcode);
            assert_eq!(jammed.cycles, 1);
            assert_eq!(cpu.state().pc, before_pc + 1, "KIL must stop instruction fetch");
            assert_eq!(cpu.state().a, 0, "the following LDA must not execute");

            cpu.reset(&mut bus);
            cpu.set_pc(0x0600);
            assert_eq!(cpu.step(&mut bus).unwrap().opcode, opcode);
        }
    }

}
