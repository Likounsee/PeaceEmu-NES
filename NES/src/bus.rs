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
    pending_oam_dma: Option<u8>,
}
impl Default for CpuBus {
    fn default() -> Self { Self { ram: [0; 0x800], cartridge: None, open_bus: 0, ppu: Ppu::new(), pending_oam_dma: None } }
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
            0x4014 => self.pending_oam_dma = Some(value),
            _ => {}
        }
    }
    /// Advances the PPU by the specified number of dots.
    pub fn tick_ppu(&mut self, dots: u32) { self.ppu.tick(dots); }
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
        bus.write(0x2003, 0x80);
        for i in 0..=u8::MAX { assert_eq!(bus.read(0x2004), i.wrapping_mul(3)); }
        assert_eq!(bus.take_oam_dma_request(), None);
    }
    #[test] fn attaches_cartridge() {
        let mut image = vec![0; 16 + 16384]; image[0..4].copy_from_slice(b"NES\x1A"); image[4] = 1;
        let mut b = CpuBus::new(); b.insert_cartridge(Nrom::parse(&image).unwrap()); assert!(b.has_cartridge());
    }
}
