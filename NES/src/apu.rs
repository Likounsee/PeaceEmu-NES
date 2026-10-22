//! Initial APU register and frame-sequencer model for the NES 2A03.
//!
//! CPU-visible register storage and frame IRQ status are modelled. Audio synthesis,
//! channel length counters, DMC playback, and cycle-exact sequencer events are not.

/// CPU-visible register state for the NES audio processing unit.
#[derive(Clone, Debug, Default)]
pub struct Apu {
    registers: [u8; 0x18],
    enabled_channels: u8,
    frame_cycles: u32,
    frame_irq: bool,
    frame_irq_inhibit: bool,
    five_step_mode: bool,
}

impl Apu {
    /// Creates an APU register model with all channels disabled.
    pub fn new() -> Self { Self::default() }

    /// Writes a register in the $4000-$4017 range.
    ///
    /// $4014 and $4016 are owned by DMA and controllers. Writing $4015 updates the
    /// four implemented channel-enable bits; DMC playback is not implemented yet.
    pub fn write(&mut self, address: u16, value: u8) {
        if !(0x4000..=0x4017).contains(&address) { return; }
        if address == 0x4015 {
            self.enabled_channels = value & 0x0F;
            self.registers[(address - 0x4000) as usize] = value;
        } else if address == 0x4017 {
            self.registers[(address - 0x4000) as usize] = value;
            self.frame_irq_inhibit = value & 0x40 != 0;
            self.five_step_mode = value & 0x80 != 0;
            if self.frame_irq_inhibit { self.frame_irq = false; }
            self.frame_cycles = 0;
        } else if address != 0x4014 && address != 0x4016 {
            self.registers[(address - 0x4000) as usize] = value;
        }
    }

    /// Advances the approximate frame sequencer by CPU cycles.
    pub fn tick_cpu_cycles(&mut self, cycles: u32) {
        let period = if self.five_step_mode { 18_641 } else { 14_915 };
        self.frame_cycles = self.frame_cycles.saturating_add(cycles);
        while self.frame_cycles >= period {
            self.frame_cycles -= period;
            if !self.five_step_mode && !self.frame_irq_inhibit {
                self.frame_irq = true;
            }
        }
    }

    /// Reads APU status at $4015, clearing the frame IRQ flag as a side effect.
    ///
    /// Length-counter bits remain clear because channel envelopes/length counters are
    /// not implemented. Other readable register locations return their stored value.
    pub fn read(&mut self, address: u16) -> u8 {
        if address == 0x4015 {
            let status = self.enabled_channels | (u8::from(self.frame_irq) << 6);
            self.frame_irq = false;
            status
        } else if (0x4000..=0x4017).contains(&address) {
            self.registers[(address - 0x4000) as usize]
        } else {
            0
        }
    }

    /// Returns whether the approximate frame sequencer currently asserts frame IRQ.
    pub fn frame_irq_pending(&self) -> bool { self.frame_irq }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_channel_registers_and_frame_counter() {
        let mut apu = Apu::new();
        apu.write(0x4000, 0xBF);
        apu.write(0x4003, 0xF8);
        apu.write(0x4017, 0xC0);
        assert_eq!(apu.read(0x4000), 0xBF);
        assert_eq!(apu.read(0x4003), 0xF8);
        assert_eq!(apu.read(0x4017), 0xC0);
    }

    #[test]
    fn status_exposes_only_enabled_implemented_channels() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x1F);
        assert_eq!(apu.read(0x4015), 0x0F);
        apu.write(0x4015, 0x04);
        assert_eq!(apu.read(0x4015), 0x04);
        apu.write(0x4015, 0x00);
        assert_eq!(apu.read(0x4015), 0);
    }

    #[test]
    fn four_step_frame_irq_is_raised_cleared_and_can_be_inhibited() {
        let mut apu = Apu::new();
        apu.tick_cpu_cycles(14_914);
        assert!(!apu.frame_irq_pending());
        apu.tick_cpu_cycles(1);
        assert!(apu.frame_irq_pending());
        assert_eq!(apu.read(0x4015) & 0x40, 0x40);
        assert!(!apu.frame_irq_pending());
        apu.tick_cpu_cycles(14_915);
        apu.write(0x4017, 0x40);
        assert!(!apu.frame_irq_pending());
        apu.tick_cpu_cycles(14_915);
        assert!(!apu.frame_irq_pending());
    }

    #[test]
    fn five_step_mode_does_not_raise_frame_irq() {
        let mut apu = Apu::new();
        apu.write(0x4017, 0x80);
        apu.tick_cpu_cycles(18_641);
        assert!(!apu.frame_irq_pending());
    }

    #[test]
    fn does_not_store_dma_or_controller_strobe_writes() {
        let mut apu = Apu::new();
        apu.write(0x4014, 0x22);
        apu.write(0x4016, 0x01);
        assert_eq!(apu.read(0x4014), 0);
        assert_eq!(apu.read(0x4016), 0);
    }
}
