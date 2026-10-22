//! Initial APU register, length-counter, and frame-sequencer model for the NES 2A03.
//!
//! Approximate CPU-cycle APU model with pulse, triangle and noise sample generation.
//!
//! Envelope/sweep units, triangle linear-counter behavior, DMC playback, and exact regional
//! timing remain incomplete; output is suitable for integration and smoke tests, not yet
//! cycle-accurate audio.

const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20,  2, 40,  4, 80,  6, 160,  8, 60, 10, 14, 12, 26, 14,
    12, 16, 24, 18, 48, 20, 96, 22, 192, 24, 72, 26, 16, 28, 32, 30,
];

/// CPU-visible register state for the NES audio processing unit.
#[derive(Clone, Debug, Default)]
pub struct Apu {
    registers: [u8; 0x18],
    enabled_channels: u8,
    length_counters: [u8; 4],
    frame_cycles: u32,
    half_frame_cycles: u32,
    frame_irq: bool,
    frame_irq_inhibit: bool,
    five_step_mode: bool,
    pulse_timer_counter: [u16; 2],
    pulse_sequence: [u8; 2],
    triangle_timer_counter: u16,
    triangle_sequence: u8,
    noise_timer_counter: u16,
    noise_shift: u16,
    cpu_cycle_odd: bool,
}

impl Apu {
    /// Creates an APU register model with all channels disabled.
    pub fn new() -> Self { Self::default() }

    /// Writes a register in the $4000-$4017 range.
    ///
    /// $4014 and $4016 are owned by DMA and controllers. $4015 controls the four
    /// implemented length-counter channels; DMC playback is not implemented.
    pub fn write(&mut self, address: u16, value: u8) {
        if !(0x4000..=0x4017).contains(&address) { return; }
        if address == 0x4015 {
            self.enabled_channels = value & 0x0F;
            for channel in 0..4 {
                if self.enabled_channels & (1 << channel) == 0 {
                    self.length_counters[channel] = 0;
                }
            }
            self.registers[(address - 0x4000) as usize] = value;
        } else if address == 0x4017 {
            self.registers[(address - 0x4000) as usize] = value;
            self.frame_irq_inhibit = value & 0x40 != 0;
            self.five_step_mode = value & 0x80 != 0;
            if self.frame_irq_inhibit { self.frame_irq = false; }
            self.frame_cycles = 0;
            self.half_frame_cycles = 0;
        } else if address != 0x4014 && address != 0x4016 {
            self.registers[(address - 0x4000) as usize] = value;
            let channel = match address {
                0x4003 => Some(0),
                0x4007 => Some(1),
                0x400B => Some(2),
                0x400F => Some(3),
                _ => None,
            };
            if let Some(channel) = channel {
                if self.enabled_channels & (1 << channel) != 0 {
                    self.length_counters[channel] = LENGTH_TABLE[(value >> 3) as usize];
                }
            }
        }
    }

    /// Advances oscillator timers and the approximate frame sequencer.
    /// Exact regional timing and delayed $4017 reset are not modelled.
    pub fn tick_cpu_cycles(&mut self, cycles: u32) {
        const NOISE_PERIODS: [u16; 16] = [4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068];
        if self.noise_shift == 0 { self.noise_shift = 1; }
        for _ in 0..cycles {
            // Pulse/noise timers are clocked on alternating CPU cycles; triangle runs each cycle.
            if !self.cpu_cycle_odd {
                for channel in 0..2 {
                    let register = channel * 4;
                    let period = u16::from(self.registers[register + 2])
                        | (u16::from(self.registers[register + 3] & 7) << 8);
                    if self.pulse_timer_counter[channel] == 0 {
                        self.pulse_timer_counter[channel] = period;
                        self.pulse_sequence[channel] = (self.pulse_sequence[channel] + 1) & 7;
                    } else { self.pulse_timer_counter[channel] -= 1; }
                }
                let noise_period = NOISE_PERIODS[(self.registers[0x0E] & 0x0F) as usize];
                if self.noise_timer_counter == 0 {
                    self.noise_timer_counter = noise_period.saturating_sub(1);
                    let tap = if self.registers[0x0E] & 0x80 != 0 { 6 } else { 1 };
                    let feedback = (self.noise_shift ^ (self.noise_shift >> tap)) & 1;
                    self.noise_shift = (self.noise_shift >> 1) | (feedback << 14);
                } else { self.noise_timer_counter -= 1; }
            }
            let triangle_period = u16::from(self.registers[0x0A])
                | (u16::from(self.registers[0x0B] & 7) << 8);
            if self.triangle_timer_counter == 0 {
                self.triangle_timer_counter = triangle_period;
                self.triangle_sequence = (self.triangle_sequence + 1) & 31;
            } else { self.triangle_timer_counter -= 1; }
            self.cpu_cycle_odd = !self.cpu_cycle_odd;
        }
        const FOUR_STEP_PERIOD: u32 = 14_915;
        const FIVE_STEP_PERIOD: u32 = 18_641;
        const HALF_FRAME_PERIOD: u32 = 7_457;
        self.frame_cycles = self.frame_cycles.saturating_add(cycles);
        while self.frame_cycles >= if self.five_step_mode { FIVE_STEP_PERIOD } else { FOUR_STEP_PERIOD } {
            self.frame_cycles -= if self.five_step_mode { FIVE_STEP_PERIOD } else { FOUR_STEP_PERIOD };
            if !self.five_step_mode && !self.frame_irq_inhibit { self.frame_irq = true; }
        }
        self.half_frame_cycles = self.half_frame_cycles.saturating_add(cycles);
        while self.half_frame_cycles >= HALF_FRAME_PERIOD {
            self.half_frame_cycles -= HALF_FRAME_PERIOD;
            for channel in 0..4 {
                let halt = match channel {
                    0 | 1 => self.registers[if channel == 0 { 0 } else { 4 }] & 0x20 != 0,
                    2 => self.registers[0x08] & 0x80 != 0,
                    _ => self.registers[0x0C] & 0x20 != 0,
                };
                if !halt && self.length_counters[channel] > 0 {
                    self.length_counters[channel] -= 1;
                }
            }
        }
    }

    /// Reads APU status at $4015, clearing the frame IRQ flag as a side effect.
    /// Bits 0-3 report nonzero length counters; bit 6 reports and clears frame IRQ.
    pub fn read(&mut self, address: u16) -> u8 {
        if address == 0x4015 {
            let mut status = 0;
            for channel in 0..4 {
                if self.length_counters[channel] > 0 { status |= 1 << channel; }
            }
            if self.frame_irq { status |= 0x40; }
            self.frame_irq = false;
            status
        } else if (0x4000..=0x4017).contains(&address) {
            self.registers[(address - 0x4000) as usize]
        } else {
            0
        }
    }

    /// Generates one normalized mono sample (0.0..1.0) from current channel states.
    /// Pulse envelope/sweep and triangle linear-counter behavior are approximated; DMC is silent.
    pub fn sample(&self) -> f32 {
        const DUTY: [u8; 4] = [0b0000_0001, 0b0000_0011, 0b0000_1111, 0b1111_1100];
        let mut pulse = 0.0f32;
        for channel in 0..2 {
            let register = channel * 4;
            let period = u16::from(self.registers[register + 2])
                | (u16::from(self.registers[register + 3] & 7) << 8);
            let duty = usize::from(self.registers[register] >> 6);
            let volume = f32::from(self.registers[register] & 0x0F);
            if self.length_counters[channel] > 0 && period >= 8
                && DUTY[duty] & (1 << self.pulse_sequence[channel]) != 0 {
                pulse += volume;
            }
        }
        let pulse_out = if pulse == 0.0 { 0.0 } else { 95.88 / (8128.0 / pulse + 100.0) };
        let triangle_period = u16::from(self.registers[0x0A]) | (u16::from(self.registers[0x0B] & 7) << 8);
        let triangle_value = if self.triangle_sequence < 16 { 15 - self.triangle_sequence } else { self.triangle_sequence - 16 };
        let triangle = if self.length_counters[2] > 0 && triangle_period > 1 { f32::from(triangle_value) } else { 0.0 };
        let noise = if self.length_counters[3] > 0 && self.noise_shift & 1 == 0 {
            f32::from(self.registers[0x0C] & 0x0F)
        } else { 0.0 };
        let tnd_input = triangle / 8227.0 + noise / 12241.0;
        let tnd_out = if tnd_input == 0.0 { 0.0 } else { 159.79 / (1.0 / tnd_input + 100.0) };
        (pulse_out + tnd_out).clamp(0.0, 1.0)
    }

    /// Returns whether the approximate frame sequencer currently asserts frame IRQ.
    pub fn frame_irq_pending(&self) -> bool { self.frame_irq }

    /// Returns the current length counter for pulse 1, pulse 2, triangle, or noise (0..3).
    pub fn length_counter(&self, channel: usize) -> Option<u8> {
        self.length_counters.get(channel).copied()
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
    fn status_reports_enabled_length_counters_and_disable_clears_them() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x0F);
        apu.write(0x4003, 0xF8); // length table index 31 -> 30
        apu.write(0x4007, 0x08); // length table index 1 -> 254
        assert_eq!(apu.length_counter(0), Some(30));
        assert_eq!(apu.length_counter(1), Some(254));
        assert_eq!(apu.read(0x4015) & 0x0F, 0x03);
        apu.write(0x4015, 0x02);
        assert_eq!(apu.length_counter(0), Some(0));
        assert_eq!(apu.read(0x4015) & 0x0F, 0x02);
    }

    #[test]
    fn half_frame_clocks_decrement_unhalted_length_counters() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x01);
        apu.write(0x4003, 0xF8);
        apu.tick_cpu_cycles(7_457);
        assert_eq!(apu.length_counter(0), Some(29));
        apu.write(0x4000, 0x20); // length counter halt
        apu.tick_cpu_cycles(7_457);
        assert_eq!(apu.length_counter(0), Some(29));
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
    fn sample_is_silent_until_a_pulse_is_enabled_and_has_length() {
        let mut apu = Apu::new();
        assert_eq!(apu.sample(), 0.0);
        apu.write(0x4015, 0x01);
        apu.write(0x4000, 0x9F); // 50% duty, volume 15
        apu.write(0x4002, 0x20);
        apu.write(0x4003, 0xF8);
        apu.tick_cpu_cycles(40);
        assert!(apu.sample() > 0.0);
        apu.write(0x4015, 0x00);
        assert_eq!(apu.sample(), 0.0);
    }

    #[test]
    fn triangle_and_noise_registers_contribute_to_audio_output() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x0C);
        apu.write(0x400A, 0x40);
        apu.write(0x400B, 0xF8);
        apu.write(0x400C, 0x0F);
        apu.write(0x400E, 0x00);
        apu.write(0x400F, 0xF8);
        apu.tick_cpu_cycles(200);
        assert!(apu.sample() > 0.0);
        apu.write(0x4015, 0x00);
        assert_eq!(apu.sample(), 0.0);
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
