//! CPU memory bus for the initial NROM machine.

use crate::cartridge::Nrom;
use crate::ppu::Ppu;

/// CPU bus with 2 KiB internal RAM and optional NROM cartridge.
///
/// PPU/APU/controller register routing is not implemented yet. Unmapped I/O
/// reads return the current open-bus latch; writes update that latch.
#[derive(Debug)]
pub struct CpuBus {
    ram: [u8; 0x800],
    cartridge: Option<Nrom>,
    open_bus: u8,
    ppu: Ppu,
}
impl Default for CpuBus {
    fn default() -> Self { Self { ram: [0; 0x800], cartridge: None, open_bus: 0, ppu: Ppu::new() } }
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
            _ => {}
        }
    }
    /// Advances the PPU by the specified number of dots.
    pub fn tick_ppu(&mut self, dots: u32) { self.ppu.tick(dots); }
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
    #[test] fn attaches_cartridge() {
        let mut image = vec![0; 16 + 16384]; image[0..4].copy_from_slice(b"NES\x1A"); image[4] = 1;
        let mut b = CpuBus::new(); b.insert_cartridge(Nrom::parse(&image).unwrap()); assert!(b.has_cartridge());
    }
}
