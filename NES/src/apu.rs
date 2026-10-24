//! Initial APU register, length-counter, and frame-sequencer model for the NES 2A03.
//!
//! Approximate CPU-cycle APU model with pulse, triangle and noise sample generation.
//!
//! Envelope/sweep units, triangle linear-counter behavior and DMC playback are approximate;
//! DMC fetches are supplied by the bus without cycle-accurate DMA stalls. Exact regional
//! timing and host-rate resampling remain incomplete.

use std::collections::VecDeque;

const CPU_CLOCK_HZ: u64 = 1_789_773;
const AUDIO_QUEUE_LIMIT: usize = 8_192;

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
    // $4017 resets the frame sequencer after a hardware-delayed 3/4 CPU cycles.
    frame_reset_delay: u8,
    frame_reset_pending: bool,
    pulse_timer_counter: [u16; 2],
    pulse_sequence: [u8; 2],
    triangle_timer_counter: u16,
    triangle_sequence: u8,
    noise_timer_counter: u16,
    noise_shift: u16,
    cpu_cycle_odd: bool,
    quarter_frame_cycles: u32,
    pulse_envelope_start: [bool; 2],
    pulse_envelope_divider: [u8; 2],
    pulse_envelope_decay: [u8; 2],
    noise_envelope_start: bool,
    noise_envelope_divider: u8,
    noise_envelope_decay: u8,
    pulse_sweep_divider: [u8; 2],
    pulse_sweep_reload: [bool; 2],
    triangle_linear_counter: u8,
    triangle_reload_flag: bool,
    dmc_irq: bool,
    dmc_enabled: bool,
    dmc_bytes_remaining: u16,
    dmc_current_address: u16,
    dmc_sample_buffer: Option<u8>,
    dmc_shift_register: u8,
    dmc_bits_remaining: u8,
    dmc_silence: bool,
    dmc_timer_counter: u16,
    dmc_output_level: u8,
    sample_rate: u32,
    sample_phase: u64,
    // One-pole reconstruction filter applied at the host PCM rate to soften aliasing.
    pcm_low_pass: f32,
    audio_samples: VecDeque<f32>,
}

impl Apu {
    /// Creates an APU register model with all channels disabled.
    pub fn new() -> Self {
        Self {
            dmc_bits_remaining: 8,
            dmc_silence: true,
            sample_rate: 44_100,
            audio_samples: VecDeque::with_capacity(AUDIO_QUEUE_LIMIT),
            ..Self::default()
        }
    }

    /// Configures the PCM output rate; accepted rates are 8 kHz through 192 kHz.
    ///
    /// Changing the rate resets the fractional resampling phase and discards queued
    /// samples so the next batch has one consistent rate.
    pub fn set_sample_rate(&mut self, sample_rate: u32) -> bool {
        if !(8_000..=192_000).contains(&sample_rate) {
            return false;
        }
        self.sample_rate = sample_rate;
        self.sample_phase = 0;
        self.pcm_low_pass = 0.0;
        self.audio_samples.clear();
        true
    }

    /// Returns the configured PCM output sample rate in samples per second.
    pub fn sample_rate(&self) -> u32 { self.sample_rate }

    /// Removes and returns all queued mono PCM samples in playback order.
    pub fn drain_audio_samples(&mut self) -> Vec<f32> {
        self.audio_samples.drain(..).collect()
    }

    /// Writes a register in the $4000-$4017 range.
    ///
    /// $4014 and $4016 are owned by DMA and controllers. DMC sample fetches are
    /// exposed to the bus as requests; the timing model is approximate.
    pub fn write(&mut self, address: u16, value: u8) {
        if !(0x4000..=0x4017).contains(&address) { return; }
        if address == 0x4015 {
            self.enabled_channels = value & 0x0F;
            self.dmc_irq = false;
            self.dmc_enabled = value & 0x10 != 0;
            if !self.dmc_enabled { self.dmc_bytes_remaining = 0; }
            else if self.dmc_bytes_remaining == 0 { self.restart_dmc_sample(); }
            for channel in 0..4 {
                if self.enabled_channels & (1 << channel) == 0 {
                    self.length_counters[channel] = 0;
                }
            }
            self.registers[(address - 0x4000) as usize] = value;
        } else if address == 0x4010 {
            self.registers[(address - 0x4000) as usize] = value;
            if value & 0x80 == 0 { self.dmc_irq = false; }
        } else if address == 0x4011 {
            self.registers[(address - 0x4000) as usize] = value;
            self.dmc_output_level = value & 0x7F;
        } else if address == 0x4017 {
            self.registers[(address - 0x4000) as usize] = value;
            self.frame_irq_inhibit = value & 0x40 != 0;
            self.five_step_mode = value & 0x80 != 0;
            if self.frame_irq_inhibit { self.frame_irq = false; }
            // IRQ inhibition is immediate; the frame-sequencer reset is delayed.
            self.frame_reset_delay = if self.cpu_cycle_odd { 3 } else { 4 };
            self.frame_reset_pending = true;
        } else if address != 0x4014 && address != 0x4016 {
            self.registers[(address - 0x4000) as usize] = value;
            let channel = match address {
                0x4003 => Some(0),
                0x4007 => Some(1),
                0x400B => Some(2),
                0x400F => Some(3),
                _ => None,
            };
            if address == 0x4001 { self.pulse_sweep_reload[0] = true; }
            if address == 0x4005 { self.pulse_sweep_reload[1] = true; }
            if address == 0x400F { self.noise_envelope_start = true; }
            if let Some(channel) = channel {
                if self.enabled_channels & (1 << channel) != 0 {
                    self.length_counters[channel] = LENGTH_TABLE[(value >> 3) as usize];
                }
                match channel {
                    0 | 1 => self.pulse_envelope_start[channel] = true,
                    2 => self.triangle_reload_flag = true,
                    _ => {},
                }
            }
        }
    }

    /// Returns whether DMC needs per-cycle CPU-bus sample-fetch servicing.
    pub(crate) fn needs_dmc_bus_service(&self) -> bool {
        self.dmc_enabled && self.dmc_bytes_remaining > 0
    }

    /// Advances oscillator timers and the approximate frame sequencer.
    /// Exact regional timing and delayed $4017 reset are not modelled.
    pub fn tick_cpu_cycles(&mut self, cycles: u32) {
        const FOUR_STEP_PERIOD: u32 = 14_915;
        const FIVE_STEP_PERIOD: u32 = 18_641;
        const HALF_FRAME_PERIOD: u32 = 7_457;
        const NOISE_PERIODS: [u16; 16] = [4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068];
        const DMC_PERIODS: [u16; 16] = [428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106,  85,  72,  54];
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
                if self.length_counters[2] > 0 && self.triangle_linear_counter > 0 {
                    self.triangle_sequence = (self.triangle_sequence + 1) & 31;
                }
            } else { self.triangle_timer_counter -= 1; }
            if self.dmc_timer_counter == 0 {
                self.dmc_timer_counter = DMC_PERIODS[(self.registers[0x10] & 0x0F) as usize].saturating_sub(1);
                if !self.dmc_silence {
                    if self.dmc_shift_register & 1 != 0 {
                        if self.dmc_output_level <= 125 { self.dmc_output_level += 2; }
                    } else if self.dmc_output_level >= 2 { self.dmc_output_level -= 2; }
                }
                self.dmc_shift_register >>= 1;
                self.dmc_bits_remaining = self.dmc_bits_remaining.saturating_sub(1);
                if self.dmc_bits_remaining == 0 {
                    self.dmc_bits_remaining = 8;
                    if let Some(sample) = self.dmc_sample_buffer.take() {
                        self.dmc_shift_register = sample;
                        self.dmc_silence = false;
                    } else { self.dmc_silence = true; }
                }
            } else { self.dmc_timer_counter -= 1; }
            self.cpu_cycle_odd = !self.cpu_cycle_odd;
            if self.frame_reset_pending {
                self.frame_reset_delay = self.frame_reset_delay.saturating_sub(1);
                if self.frame_reset_delay == 0 {
                    self.frame_reset_pending = false;
                    self.frame_cycles = 0;
                    self.half_frame_cycles = 0;
                    self.quarter_frame_cycles = 0;
                }
            }
            self.frame_cycles = self.frame_cycles.saturating_add(1);
            while self.frame_cycles >= if self.five_step_mode { FIVE_STEP_PERIOD } else { FOUR_STEP_PERIOD } {
                self.frame_cycles -= if self.five_step_mode { FIVE_STEP_PERIOD } else { FOUR_STEP_PERIOD };
                if !self.five_step_mode && !self.frame_irq_inhibit { self.frame_irq = true; }
            }
            self.quarter_frame_cycles = self.quarter_frame_cycles.saturating_add(1);
            while self.quarter_frame_cycles >= 3_729 {
                self.quarter_frame_cycles -= 3_729;
                for channel in 0..2 {
                    let register = channel * 4;
                    let control = self.registers[register];
                    if self.pulse_envelope_start[channel] {
                        self.pulse_envelope_start[channel] = false;
                        self.pulse_envelope_decay[channel] = 15;
                        self.pulse_envelope_divider[channel] = control & 0x0F;
                    } else if self.pulse_envelope_divider[channel] == 0 {
                        self.pulse_envelope_divider[channel] = control & 0x0F;
                        if self.pulse_envelope_decay[channel] > 0 {
                            self.pulse_envelope_decay[channel] -= 1;
                        } else if control & 0x20 != 0 {
                            self.pulse_envelope_decay[channel] = 15;
                        }
                    } else {
                        self.pulse_envelope_divider[channel] -= 1;
                    }
                }
                // Noise has its own envelope generator controlled by $400C.
                let noise_control = self.registers[0x0C];
                if self.noise_envelope_start {
                    self.noise_envelope_start = false;
                    self.noise_envelope_decay = 15;
                    self.noise_envelope_divider = noise_control & 0x0F;
                } else if self.noise_envelope_divider == 0 {
                    self.noise_envelope_divider = noise_control & 0x0F;
                    if self.noise_envelope_decay > 0 {
                        self.noise_envelope_decay -= 1;
                    } else if noise_control & 0x20 != 0 {
                        self.noise_envelope_decay = 15;
                    }
                } else {
                    self.noise_envelope_divider -= 1;
                }
                let triangle_control = self.registers[0x08];
                if self.triangle_reload_flag {
                    self.triangle_linear_counter = triangle_control & 0x7F;
                } else if self.triangle_linear_counter > 0 {
                    self.triangle_linear_counter -= 1;
                }
                if triangle_control & 0x80 == 0 { self.triangle_reload_flag = false; }
            }
            self.half_frame_cycles = self.half_frame_cycles.saturating_add(1);
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
                // Clock each pulse sweep divider and apply valid period updates.
                for channel in 0..2 {
                    let register = channel * 4;
                    let sweep = self.registers[register + 1];
                    let period = u16::from(self.registers[register + 2])
                        | (u16::from(self.registers[register + 3] & 7) << 8);
                    let shift = sweep & 7;
                    let change = period >> shift;
                    let target = if sweep & 0x08 != 0 {
                        period.wrapping_sub(change).wrapping_sub(u16::from(channel == 0))
                    } else { period.saturating_add(change) };
                    if sweep & 0x80 != 0 && shift != 0 && self.pulse_sweep_divider[channel] == 0
                        && period >= 8 && target <= 0x07FF {
                        self.registers[register + 2] = target as u8;
                        self.registers[register + 3] = (self.registers[register + 3] & 0xF8) | ((target >> 8) as u8 & 7);
                    }
                    if self.pulse_sweep_divider[channel] == 0 || self.pulse_sweep_reload[channel] {
                        self.pulse_sweep_divider[channel] = (sweep >> 4) & 7;
                        self.pulse_sweep_reload[channel] = false;
                    } else {
                        self.pulse_sweep_divider[channel] -= 1;
                    }
                }
            }

            // Resample the running hardware mixer against the configured host PCM rate.
            // The bounded queue prevents a frontend that temporarily stops consuming
            // audio from allowing unbounded memory growth.
            self.sample_phase += u64::from(self.sample_rate);
            if self.sample_phase >= CPU_CLOCK_HZ {
                self.sample_phase -= CPU_CLOCK_HZ;
                if self.audio_samples.len() == AUDIO_QUEUE_LIMIT {
                    self.audio_samples.pop_front();
                }
                // A simple output-stage low-pass filter attenuates harsh Nyquist
                // images from the discrete channel sequencers. Keep the public PCM
                // contract unipolar (0..1) while smoothing at the configured host rate.
                let cutoff_hz = 14_000.0_f32.min(self.sample_rate as f32 * 0.45);
                let alpha = 1.0 - (-core::f32::consts::TAU * cutoff_hz / self.sample_rate as f32).exp();
                self.pcm_low_pass += alpha * (self.sample() - self.pcm_low_pass);
                self.audio_samples.push_back(self.pcm_low_pass.clamp(0.0, 1.0));
            }
        }
    }

    /// Reads APU status at $4015, clearing the frame IRQ flag as a side effect.
    /// Bits 0-3 report nonzero length counters, bit 4 reports DMC bytes remaining,
    /// bit 6 reports and clears frame IRQ, and bit 7 reports DMC IRQ (not cleared by read).
    pub fn read(&mut self, address: u16) -> u8 {
        if address == 0x4015 {
            let mut status = 0;
            for channel in 0..4 {
                if self.length_counters[channel] > 0 { status |= 1 << channel; }
            }
            if self.dmc_bytes_remaining > 0 { status |= 0x10; }
            if self.frame_irq { status |= 0x40; }
            if self.dmc_irq { status |= 0x80; }
            self.frame_irq = false;
            status
        } else if (0x4000..=0x4017).contains(&address) {
            self.registers[(address - 0x4000) as usize]
        } else {
            0
        }
    }

    /// Generates one normalized mono sample (0.0..1.0) from current channel states.
    /// The pulse/TND nonlinear mixer is approximate; DMC output is included in the TND path.
    pub fn sample(&self) -> f32 {
        const DUTY: [u8; 4] = [0b0000_0001, 0b0000_0011, 0b0000_1111, 0b1111_1100];
        let mut pulse = 0.0f32;
        for channel in 0..2 {
            let register = channel * 4;
            let period = u16::from(self.registers[register + 2])
                | (u16::from(self.registers[register + 3] & 7) << 8);
            let duty = usize::from(self.registers[register] >> 6);
            let control = self.registers[register];
            let volume = f32::from(if control & 0x10 != 0 { control & 0x0F } else { self.pulse_envelope_decay[channel] });
            let sweep = self.registers[register + 1];
            let change = period >> (sweep & 7);
            let target = if sweep & 0x08 != 0 {
                period.wrapping_sub(change).wrapping_sub(u16::from(channel == 0))
            } else { period.saturating_add(change) };
            let sweep_muted = period < 8 || (sweep & 0x80 != 0 && sweep & 7 != 0 && target > 0x07FF)
                || (sweep & 0x80 != 0 && sweep & 7 != 0 && sweep & 0x08 != 0 && channel == 0 && change + 1 > period);
            if self.length_counters[channel] > 0 && period >= 8 && !sweep_muted
                && DUTY[duty] & (1 << self.pulse_sequence[channel]) != 0 {
                pulse += volume;
            }
        }
        let pulse_out = if pulse == 0.0 { 0.0 } else { 95.88 / (8128.0 / pulse + 100.0) };
        let triangle_period = u16::from(self.registers[0x0A]) | (u16::from(self.registers[0x0B] & 7) << 8);
        let triangle_value = if self.triangle_sequence < 16 { 15 - self.triangle_sequence } else { self.triangle_sequence - 16 };
        let triangle = if self.length_counters[2] > 0 && self.triangle_linear_counter > 0 && triangle_period > 1 { f32::from(triangle_value) } else { 0.0 };
        let noise_control = self.registers[0x0C];
        let noise_volume = if noise_control & 0x10 != 0 {
            noise_control & 0x0F
        } else {
            self.noise_envelope_decay
        };
        let noise = if self.length_counters[3] > 0 && self.noise_shift & 1 == 0 {
            f32::from(noise_volume)
        } else { 0.0 };
        let dmc = f32::from(self.dmc_output_level);
        let tnd_input = triangle / 8227.0 + noise / 12241.0 + dmc / 22638.0;
        let tnd_out = if tnd_input == 0.0 { 0.0 } else { 159.79 / (1.0 / tnd_input + 100.0) };
        (pulse_out + tnd_out).clamp(0.0, 1.0)
    }

    /// Returns whether the approximate frame sequencer currently asserts frame IRQ.
    pub fn frame_irq_pending(&self) -> bool { self.frame_irq }

    /// Returns whether either the frame sequencer or DMC asserts the CPU IRQ line.
    pub fn irq_pending(&self) -> bool { self.frame_irq || self.dmc_irq }

    /// Returns a pending DMC sample-memory read address, if the sample buffer needs data.
    pub fn take_dmc_dma_request(&mut self) -> Option<u16> {
        if self.dmc_sample_buffer.is_none() && self.dmc_bytes_remaining > 0 {
            Some(self.dmc_current_address)
        } else { None }
    }

    /// Supplies a byte fetched by the bus for the DMC sample buffer.
    pub fn supply_dmc_byte(&mut self, value: u8) {
        if self.dmc_sample_buffer.is_some() || self.dmc_bytes_remaining == 0 { return; }
        self.dmc_sample_buffer = Some(value);
        self.dmc_current_address = if self.dmc_current_address == 0xFFFF { 0x8000 } else { self.dmc_current_address + 1 };
        self.dmc_bytes_remaining -= 1;
        if self.dmc_bytes_remaining == 0 {
            if self.registers[0x10] & 0x40 != 0 && self.dmc_enabled { self.restart_dmc_sample(); }
            else if self.registers[0x10] & 0x80 != 0 { self.dmc_irq = true; }
        }
    }

    fn restart_dmc_sample(&mut self) {
        self.dmc_current_address = 0xC000 | (u16::from(self.registers[0x12]) << 6);
        self.dmc_bytes_remaining = u16::from(self.registers[0x13]) * 16 + 1;
    }

    /// Returns the current length counter for pulse 1, pulse 2, triangle, or noise (0..3).
    pub fn length_counter(&self, channel: usize) -> Option<u8> {
        self.length_counters.get(channel).copied()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn frame_counter_write_resets_sequencer_after_hardware_delay() {
        let mut apu = Apu::new();
        apu.tick_cpu_cycles(20);
        assert_eq!(apu.frame_cycles, 20);

        apu.write(0x4017, 0x80); // Five-step mode; reset is not immediate.
        assert!(apu.frame_reset_pending);
        assert_eq!(apu.frame_cycles, 20);
        let delay = u32::from(apu.frame_reset_delay);
        apu.tick_cpu_cycles(delay - 1);
        assert!(apu.frame_reset_pending);
        apu.tick_cpu_cycles(1);
        assert!(!apu.frame_reset_pending);
        assert_eq!(apu.frame_cycles, 1, "the sequencer resumes on the cycle after reset");
        assert_eq!(apu.half_frame_cycles, 1);
        assert_eq!(apu.quarter_frame_cycles, 1);
    }

    #[test]
    fn reading_status_clears_frame_irq_without_disabling_the_sequencer() {
        let mut apu = Apu::new();
        apu.write(0x4017, 0x00); // Four-step mode, frame IRQ enabled.
        apu.tick_cpu_cycles(14_915);

        assert_ne!(apu.read(0x4015) & 0x40, 0, "frame IRQ should be observable in $4015");
        assert_eq!(apu.read(0x4015) & 0x40, 0, "reading $4015 acknowledges frame IRQ");
        assert!(!apu.irq_pending(), "the acknowledged frame IRQ should release the IRQ line");
    }

    #[test]
    fn setting_frame_irq_inhibit_clears_a_pending_frame_irq() {
        let mut apu = Apu::new();
        apu.write(0x4017, 0x00);
        apu.tick_cpu_cycles(14_915);
        assert!(apu.irq_pending());

        apu.write(0x4017, 0x40); // Inhibit and acknowledge frame IRQ.
        assert_eq!(apu.read(0x4015) & 0x40, 0);
        assert!(!apu.irq_pending());
    }


    use super::*;

    #[test]
    fn dmc_registers_control_sample_address_length_and_irq() {
        let mut apu = Apu::new();
        apu.write(0x4010, 0x80);
        apu.write(0x4012, 0x03);
        apu.write(0x4013, 0x00); // One byte.
        apu.write(0x4015, 0x10);
        assert_eq!(apu.read(0x4015) & 0x10, 0x10);
        assert_eq!(apu.take_dmc_dma_request(), Some(0xC0C0));
        apu.supply_dmc_byte(0xA5);
        assert_eq!(apu.read(0x4015) & 0x10, 0);
        assert_ne!(apu.read(0x4015) & 0x80, 0);
        assert!(apu.irq_pending());
        apu.write(0x4015, 0x00);
        assert!(!apu.irq_pending());
    }

    #[test]
    fn dmc_disable_clears_remaining_bytes_and_write_acknowledges_irq() {
        let mut apu = Apu::new();
        apu.write(0x4010, 0x80); // Enable terminal DMC IRQ.
        apu.write(0x4012, 0x01);
        apu.write(0x4013, 0x00); // One byte, then terminal IRQ.
        apu.write(0x4015, 0x10);
        assert_eq!(apu.take_dmc_dma_request(), Some(0xC040));
        apu.supply_dmc_byte(0x55);
        assert_eq!(apu.read(0x4015) & 0x10, 0);
        assert!(apu.irq_pending());
        apu.write(0x4015, 0x00);
        assert_eq!(apu.read(0x4015) & 0x10, 0);
        assert_eq!(apu.take_dmc_dma_request(), None);
        assert!(!apu.irq_pending(), "writing $4015 acknowledges DMC IRQ");

        apu.write(0x4010, 0x00); // No IRQ; restart a longer sample.
        apu.write(0x4013, 0x02);
        apu.write(0x4015, 0x10);
        assert_ne!(apu.read(0x4015) & 0x10, 0);
        apu.write(0x4015, 0x00);
        assert_eq!(apu.read(0x4015) & 0x10, 0);
        assert_eq!(apu.take_dmc_dma_request(), None);
    }

    #[test]
    fn dmc_stays_silent_until_a_sample_byte_reaches_the_shift_register() {
        let mut apu = Apu::new();
        apu.write(0x4011, 64);
        apu.write(0x4010, 0x0F);
        apu.write(0x4013, 0);
        apu.write(0x4015, 0x10);
        apu.supply_dmc_byte(0xFF);
        let initial = apu.sample();
        apu.tick_cpu_cycles(8 * 54);
        assert_eq!(apu.sample(), initial, "silent startup must not pull the DAC down");
        apu.tick_cpu_cycles(8 * 54);
        assert!(apu.sample() > initial, "loaded sample bits should now affect the DAC");
    }

    #[test]
    fn dmc_delta_unit_changes_dac_level_from_sample_bits() {
        let mut apu = Apu::new();
        apu.write(0x4011, 64);
        apu.write(0x4010, 0x0F); // Fastest DMC timer.
        apu.write(0x4012, 0);
        apu.write(0x4013, 0);
        apu.write(0x4015, 0x10);
        apu.supply_dmc_byte(0xFF);
        let before = apu.sample();
        // The first eight timer clocks finish the current (silent) shift unit;
        // the following eight clocks consume the newly loaded sample byte.
        apu.tick_cpu_cycles(16 * 54);
        let after = apu.sample();
        assert!(after > before, "eight one-bits should raise the 7-bit DMC DAC");
        assert!(after <= 1.0);
    }

    #[test]
    fn dmc_address_wraps_from_ffff_to_8000() {
        let mut apu = Apu::new();
        apu.dmc_current_address = 0xFFFF;
        apu.dmc_bytes_remaining = 2;
        apu.supply_dmc_byte(0);
        assert_eq!(apu.dmc_current_address, 0x8000);
        assert_eq!(apu.dmc_bytes_remaining, 1);
        assert_eq!(apu.take_dmc_dma_request(), None);
        apu.dmc_sample_buffer = None;
        assert_eq!(apu.take_dmc_dma_request(), Some(0x8000));
    }

    #[test]
    fn dmc_loop_restarts_sample_and_output_level_is_mixed() {
        let mut apu = Apu::new();
        apu.write(0x4010, 0x40); // Loop, IRQ disabled.
        apu.write(0x4011, 64);
        apu.write(0x4012, 0x02);
        apu.write(0x4013, 0x00); // One-byte sample.
        apu.write(0x4015, 0x10);
        assert!(apu.sample() > 0.0);
        assert_eq!(apu.take_dmc_dma_request(), Some(0xC080));
        apu.supply_dmc_byte(0x5A);
        assert_eq!(apu.take_dmc_dma_request(), None, "sample buffer is occupied until shifted");
        apu.tick_cpu_cycles(4_000);
        assert_eq!(apu.take_dmc_dma_request(), Some(0xC080), "loop mode restarts the one-byte sample");
        assert!(!apu.irq_pending());
    }

    #[test]
    fn triangle_sequencer_holds_phase_when_length_counter_is_zero() {
        let mut apu = Apu::new();
        apu.triangle_sequence = 11;
        apu.triangle_linear_counter = 7;
        apu.triangle_timer_counter = 0;
        apu.tick_cpu_cycles(1);
        assert_eq!(apu.triangle_sequence, 11);
    }

    #[test]
    fn triangle_sequencer_requires_both_length_and_linear_counters() {
        let mut apu = Apu::new();
        apu.triangle_sequence = 11;
        apu.triangle_linear_counter = 7;
        apu.length_counters[2] = 1;
        apu.triangle_timer_counter = 0;
        apu.tick_cpu_cycles(1);
        assert_eq!(apu.triangle_sequence, 12);

        apu.triangle_sequence = 12;
        apu.triangle_linear_counter = 0;
        apu.triangle_timer_counter = 0;
        apu.tick_cpu_cycles(1);
        assert_eq!(apu.triangle_sequence, 12);
    }

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
    fn frame_irq_status_read_acknowledges_irq_without_disabling_future_events() {
        let mut apu = Apu::new();
        apu.tick_cpu_cycles(14_915);
        assert_ne!(apu.read(0x4015) & 0x40, 0);
        assert!(!apu.frame_irq_pending());
        apu.tick_cpu_cycles(14_915);
        assert!(apu.frame_irq_pending());
    }

    #[test]
    fn status_read_clears_frame_irq_but_preserves_dmc_irq_and_channel_state() {
        let mut apu = Apu::new();
        apu.length_counters[0] = 1;
        apu.dmc_bytes_remaining = 1;
        apu.frame_irq = true;
        apu.dmc_irq = true;

        assert_eq!(apu.read(0x4015), 0xD1);
        assert!(!apu.frame_irq_pending());
        assert!(apu.irq_pending(), "DMC IRQ must survive a $4015 status read");
        assert_eq!(apu.read(0x4015) & 0xC0, 0x80);
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
        apu.write(0x400C, 0x1F); // Constant volume keeps the noise audible without waiting for envelope clocks.
        apu.write(0x400E, 0x00);
        apu.write(0x400F, 0xF8);
        apu.tick_cpu_cycles(200);
        assert!(apu.sample() > 0.0);
        apu.write(0x4015, 0x00);
        assert_eq!(apu.sample(), 0.0);
    }

    #[test]
    fn pulse_envelope_decays_and_constant_volume_bypasses_envelope() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x01);
        apu.write(0x4000, 0x00); // Envelope decay, period zero.
        apu.write(0x4002, 0x20);
        apu.write(0x4003, 0xF8);
        apu.tick_cpu_cycles(3_729);
        let decaying = apu.sample();
        apu.write(0x4000, 0x1F); // Constant volume 15.
        let constant = apu.sample();
        assert!(constant >= decaying);
    }

    #[test]
    fn triangle_linear_counter_must_be_loaded_before_output() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x04);
        apu.write(0x4008, 0x00);
        apu.write(0x400A, 0x40);
        apu.write(0x400B, 0xF8);
        apu.tick_cpu_cycles(100);
        assert_eq!(apu.sample(), 0.0);
        apu.write(0x4008, 0x80 | 0x7F);
        apu.write(0x400B, 0xF8);
        apu.tick_cpu_cycles(3_729);
        assert!(apu.sample() > 0.0);
    }

    #[test]
    fn enabled_pulse_sweep_updates_timer_at_half_frame() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x01);
        apu.write(0x4000, 0x9F);
        apu.write(0x4001, 0x81); // Enable sweep, shift by one.
        apu.write(0x4002, 0x20);
        apu.write(0x4003, 0xF8);
        apu.tick_cpu_cycles(7_457);
        assert_eq!(apu.read(0x4002), 0x30); // 32 + (32 >> 1)
    }

    #[test]
    fn oscillator_state_is_independent_of_cpu_cycle_batching() {
        let mut whole = Apu::new();
        whole.write(0x4015, 0x0F);
        whole.write(0x4000, 0x9A);
        whole.write(0x4002, 0x35);
        whole.write(0x4003, 0xF8);
        whole.write(0x4008, 0xFF);
        whole.write(0x400A, 0x28);
        whole.write(0x400B, 0xF8);
        whole.write(0x400C, 0x0C);
        whole.write(0x400E, 0x04);
        whole.write(0x400F, 0xF8);
        let mut chunked = whole.clone();
        whole.tick_cpu_cycles(15_000);
        for _ in 0..15 { chunked.tick_cpu_cycles(1_000); }
        assert_eq!(whole.sample(), chunked.sample());
        assert_eq!(whole.pulse_timer_counter, chunked.pulse_timer_counter);
        assert_eq!(whole.pulse_sequence, chunked.pulse_sequence);
        assert_eq!(whole.triangle_timer_counter, chunked.triangle_timer_counter);
        assert_eq!(whole.triangle_sequence, chunked.triangle_sequence);
        assert_eq!(whole.noise_timer_counter, chunked.noise_timer_counter);
        assert_eq!(whole.noise_shift, chunked.noise_shift);
        assert_eq!(whole.dmc_timer_counter, chunked.dmc_timer_counter);
        assert_eq!(whole.dmc_bits_remaining, chunked.dmc_bits_remaining);
        assert_eq!(whole.quarter_frame_cycles, chunked.quarter_frame_cycles);
        assert_eq!(whole.half_frame_cycles, chunked.half_frame_cycles);
        for channel in 0..4 {
            assert_eq!(whole.length_counter(channel), chunked.length_counter(channel));
        }
        assert_eq!(whole.frame_irq_pending(), chunked.frame_irq_pending());
    }

    #[test]
    fn does_not_store_dma_or_controller_strobe_writes() {
        let mut apu = Apu::new();
        apu.write(0x4014, 0x22);
        apu.write(0x4016, 0x01);
        assert_eq!(apu.read(0x4014), 0);
        assert_eq!(apu.read(0x4016), 0);
    }
    #[test]
    fn enabling_frame_irq_inhibit_clears_an_already_pending_irq() {
        let mut apu = Apu::new();
        apu.write(0x4017, 0x00);
        apu.tick_cpu_cycles(14_915);
        assert!(apu.frame_irq_pending());
        apu.write(0x4017, 0x40);
        assert!(!apu.frame_irq_pending());
        assert_eq!(apu.read(0x4015) & 0x40, 0);
    }

    #[test]
    fn frame_counter_can_switch_between_four_and_five_step_modes() {
        let mut apu = Apu::new();
        apu.write(0x4017, 0x80);
        apu.tick_cpu_cycles(18_641);
        assert!(!apu.frame_irq_pending(), "five-step mode must not assert frame IRQ");
        apu.write(0x4017, 0x00);
        apu.tick_cpu_cycles(14_915);
        assert!(apu.frame_irq_pending(), "four-step mode should resume frame IRQ generation");
    }

}


#[cfg(test)]
mod pcm_output_tests {
    use super::*;

    #[test]
    fn noise_channel_uses_its_own_decay_envelope_and_constant_volume_mode() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x08);
        apu.write(0x400C, 0x00); // Envelope mode, divider period zero.
        apu.write(0x400E, 0x00);
        apu.write(0x400F, 0xF8); // Reload length and restart noise envelope.
        apu.tick_cpu_cycles(7_458); // First quarter-frame starts the envelope; the next clocks its decay.
        let envelope_level = apu.noise_envelope_decay;
        assert!(envelope_level < 15, "quarter-frame clocks should decay noise volume");
        apu.noise_shift = 0; // Force an audible LFSR output for deterministic mixer assertions.
        let envelope_output = apu.sample();

        apu.write(0x400C, 0x1F); // Constant volume 15.
        let constant_output = apu.sample();
        assert!(constant_output > envelope_output, "constant volume should bypass the quieter decay level");
        assert_eq!(apu.noise_envelope_decay, envelope_level, "constant volume must not overwrite envelope state");
    }

    #[test]
    fn pcm_output_filter_smooths_transitions_and_resets_when_rate_changes() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x01);
        apu.write(0x4000, 0xDF);
        apu.write(0x4002, 0x08);
        apu.write(0x4003, 0xF8);
        apu.tick_cpu_cycles(2_000);
        assert!(apu.pcm_low_pass.is_finite());
        assert!((0.0..=1.0).contains(&apu.pcm_low_pass));

        assert!(apu.set_sample_rate(48_000));
        assert_eq!(apu.pcm_low_pass, 0.0, "changing PCM rate resets filter history");
        assert!(apu.drain_audio_samples().is_empty());
    }

    #[test]
    fn pcm_resampling_produces_expected_frame_sized_batches() {
        let mut apu = Apu::new();
        assert_eq!(apu.sample_rate(), 44_100);
        apu.tick_cpu_cycles(29_781);
        let samples = apu.drain_audio_samples();
        assert!((733..=735).contains(&samples.len()), "unexpected sample count: {}", samples.len());
        assert!(samples.iter().all(|sample| sample.is_finite() && (0.0..=1.0).contains(sample)));
        assert!(apu.drain_audio_samples().is_empty());
    }

    #[test]
    fn pcm_resampling_is_independent_of_cycle_batching() {
        let mut whole = Apu::new();
        let mut chunked = Apu::new();
        for apu in [&mut whole, &mut chunked] {
            apu.write(0x4015, 0x01);
            apu.write(0x4000, 0xDF);
            apu.write(0x4002, 0x30);
            apu.write(0x4003, 0xF8);
        }
        whole.tick_cpu_cycles(50_000);
        for _ in 0..50 { chunked.tick_cpu_cycles(1_000); }
        assert_eq!(whole.drain_audio_samples(), chunked.drain_audio_samples());
    }

    #[test]
    fn pcm_rate_validation_and_queue_bound_are_safe() {
        let mut apu = Apu::new();
        assert!(!apu.set_sample_rate(0));
        assert!(!apu.set_sample_rate(500_000));
        assert!(apu.set_sample_rate(48_000));
        apu.tick_cpu_cycles(1_789_773);
        let samples = apu.drain_audio_samples();
        assert_eq!(samples.len(), AUDIO_QUEUE_LIMIT);
    }
}
