//! Initial APU register model for the NES 2A03.
//!
//! This models CPU-visible register storage and the channel-enable bits in $4015.
//! It does not synthesize audio or yet model frame/DMC IRQ timing.

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
    /// $4014 and $4016 are owned by DMA and controllers, so callers should route
    /// those addresses elsewhere. Writing $4015 updates the four pulse/triangle/noise
    /// enable bits; DMC playback is not implemented yet.
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

    /// Advances the frame sequencer by CPU cycles. Audio synthesis remains unimplemented.\n    pub fn tick_cpu_cycles(&mut self, cycles: u32) {\n        let period = if self.five_step_mode { 18_641 } else { 14_915 };\n        self.frame_cycles = self.frame_cycles.saturating_add(cycles);\n        while self.frame_cycles >= period {\n            self.frame_cycles -= period;\n            if !self.five_step_mode && !self.frame_irq_inhibit { self.frame_irq = true; }\n        }\n    }\n\n    /// Reads APU status at $4015. Other addresses return their stored register value.
    ///
    /// Only channel-enable state is currently emulated; length counters, frame IRQ,
    /// DMC state, and audio generation are intentionally not reported as active.
    pub fn read(&self, address: u16) -> u8 {
        if address == 0x4015 {
            self.enabled_channels
        } else if (0x4000..=0x4017).contains(&address) {
            self.registers[(address - 0x4000) as usize]
        } else {
            0
        }
    }
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
    fn does_not_store_dma_or_controller_strobe_writes() {
        let mut apu = Apu::new();
        apu.write(0x4014, 0x22);
        apu.write(0x4016, 0x01);
        assert_eq!(apu.read(0x4014), 0);
        assert_eq!(apu.read(0x4016), 0);
    }
}
