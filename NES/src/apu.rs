//! Initial APU register, length-counter, and frame-sequencer model for the NES 2A03.
//!
//! CPU-cycle APU model with pulse, triangle, noise, DMC, frame IRQ and queued PCM output.
//!
//! Frame-counter writes include the delayed 3/4-cycle reset window. Envelope and sweep
//! dividers, DMC DMA stalls, regional timing, and nonlinear mixing remain approximate.

use std::collections::VecDeque;

const CPU_CLOCK_HZ: u64 = 1_789_773;
const AUDIO_QUEUE_LIMIT: usize = 8_192;

const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20,  2, 40,  4, 80,  6, 160,  8, 60, 10, 14, 12, 26, 14,
    12, 16, 24, 18, 48, 20, 96, 22, 192, 24, 72, 26, 16, 28, 32, 30,
];

/// CPU-visible register state for the NES audio processing unit.
#[derive(Clone, Debug)]
pub struct Apu {
    registers: [u8; 0x18],
    enabled_channels: u8,
    length_counters: [u8; 4],
    frame_cycles: u32,
    half_frame_cycles: u32,
    frame_irq: bool,
    frame_irq_inhibit: bool,
    five_step_mode: bool,
    pending_five_step_mode: bool,
    // $4017 resets the frame sequencer after a hardware-delayed 3/4 CPU cycles.
    frame_reset_delay: u8,
    frame_reset_pending: bool,
    pulse_timer_counter: [u16; 2],
    pulse_sequence: [u8; 2],
    triangle_timer_counter: u16,
    triangle_sequence: u8,
    triangle_has_clocked: bool,
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

impl Default for Apu {
    fn default() -> Self {
        Self {
            registers: [0; 0x18],
            enabled_channels: 0,
            length_counters: [0; 4],
            frame_cycles: 0,
            half_frame_cycles: 0,
            frame_irq: false,
            frame_irq_inhibit: false,
            five_step_mode: false,
            pending_five_step_mode: false,
            frame_reset_delay: 0,
            frame_reset_pending: false,
            pulse_timer_counter: [0; 2],
            pulse_sequence: [0; 2],
            triangle_timer_counter: 0,
            triangle_sequence: 0,
            triangle_has_clocked: false,
            noise_timer_counter: 0,
            noise_shift: 0,
            cpu_cycle_odd: false,
            quarter_frame_cycles: 0,
            pulse_envelope_start: [false; 2],
            pulse_envelope_divider: [0; 2],
            pulse_envelope_decay: [0; 2],
            noise_envelope_start: false,
            noise_envelope_divider: 0,
            noise_envelope_decay: 0,
            pulse_sweep_divider: [0; 2],
            pulse_sweep_reload: [false; 2],
            triangle_linear_counter: 0,
            triangle_reload_flag: false,
            dmc_irq: false,
            dmc_enabled: false,
            dmc_bytes_remaining: 0,
            dmc_current_address: 0,
            dmc_sample_buffer: None,
            dmc_shift_register: 0,
            dmc_bits_remaining: 8,
            dmc_silence: true,
            dmc_timer_counter: 0,
            dmc_output_level: 0,
            sample_rate: 44_100,
            sample_phase: 0,
            pcm_low_pass: 0.0,
            audio_samples: VecDeque::with_capacity(AUDIO_QUEUE_LIMIT),
        }
    }
}

impl Apu {
    /// Creates an APU register model with all channels disabled.
    pub fn new() -> Self { Self::default() }

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
            self.pending_five_step_mode = value & 0x80 != 0;
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
                    0 | 1 => {
                        // Writing the high timer register restarts the pulse duty
                        // sequencer at its first step, as on the 2A03.
                        self.pulse_sequence[channel] = 0;
                        self.pulse_envelope_start[channel] = true;
                    }
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

    /// Advances oscillator timers and the frame sequencer in CPU-cycle-sized steps.
    /// The $4017 reset delay is modelled; individual oscillator and regional timings remain approximate.
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
            }
            // Unlike the pulse timers, the noise timer is clocked every CPU cycle.
            // Its period table is already expressed in CPU cycles, so keeping this
            // inside the alternating-cycle pulse block would halve the noise pitch.
            let noise_period = NOISE_PERIODS[(self.registers[0x0E] & 0x0F) as usize];
            if self.noise_timer_counter == 0 {
                self.noise_timer_counter = noise_period.saturating_sub(1);
                let tap = if self.registers[0x0E] & 0x80 != 0 { 6 } else { 1 };
                let feedback = (self.noise_shift ^ (self.noise_shift >> tap)) & 1;
                self.noise_shift = (self.noise_shift >> 1) | (feedback << 14);
            } else { self.noise_timer_counter -= 1; }
            let triangle_period = u16::from(self.registers[0x0A])
                | (u16::from(self.registers[0x0B] & 7) << 8);
            if self.triangle_timer_counter == 0 {
                self.triangle_timer_counter = triangle_period;
                if self.length_counters[2] > 0 && self.triangle_linear_counter > 0 {
                    self.triangle_sequence = (self.triangle_sequence + 1) & 31;
                    self.triangle_has_clocked = true;
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
                    self.five_step_mode = self.pending_five_step_mode;
                    self.frame_cycles = 0;
                    self.half_frame_cycles = 0;
                    self.quarter_frame_cycles = 0;
                    // The five-step sequence clocks both unit groups immediately
                    // when its delayed reset takes effect.
                    if self.five_step_mode {
                        self.clock_quarter_units_once();
                        self.clock_half_units_once();
                    }
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
                self.clock_quarter_units_once();
            }
            self.half_frame_cycles = self.half_frame_cycles.saturating_add(1);
            while self.half_frame_cycles >= HALF_FRAME_PERIOD {
                self.half_frame_cycles -= HALF_FRAME_PERIOD;
                self.clock_half_units_once();
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

    /// Applies one quarter-frame clock to envelope and triangle linear-counter units.
    /// Used by the immediate clock accompanying a five-step frame-counter reset.
    fn clock_quarter_units_once(&mut self) {
        for channel in 0..2 {
            let control = self.registers[channel * 4];
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
        let control = self.registers[0x0C];
        if self.noise_envelope_start {
            self.noise_envelope_start = false;
            self.noise_envelope_decay = 15;
            self.noise_envelope_divider = control & 0x0F;
        } else if self.noise_envelope_divider == 0 {
            self.noise_envelope_divider = control & 0x0F;
            if self.noise_envelope_decay > 0 {
                self.noise_envelope_decay -= 1;
            } else if control & 0x20 != 0 {
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

    /// Applies one half-frame length-counter clock on a five-step reset.
    fn clock_half_units_once(&mut self) {
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
        let triangle_value = if self.triangle_sequence < 16 { 15 - self.triangle_sequence } else { self.triangle_sequence - 16 };
        // The triangle channel has no volume envelope: when its length or linear
        // counter expires, the sequencer stops but the DAC holds its last level.
        // Keep that held level in the mixer instead of incorrectly forcing silence.
        // The timer period controls how quickly the sequencer advances, not whether
        // the DAC holds a value. Periods 0 and 1 are ultrasonic but still leave
        // the last sequencer value on the triangle output when its counters stop.
        let triangle = if self.triangle_has_clocked { f32::from(triangle_value) } else { 0.0 };
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
    fn default_and_new_apu_start_with_a_valid_pcm_configuration() {
        for mut apu in [Apu::default(), Apu::new()] {
            assert_eq!(apu.sample_rate(), 44_100);
            apu.tick_cpu_cycles(2_000);
            assert!(!apu.drain_audio_samples().is_empty(), "default construction must produce PCM");
        }
    }

    #[test]
    fn noise_lfsr_uses_distinct_feedback_taps_for_long_and_short_modes() {
        let mut short_mode = Apu::new();
        short_mode.registers[0x0E] = 0x00; // Tap bit 1.
        short_mode.noise_shift = 0b10;
        short_mode.noise_timer_counter = 0;
        short_mode.tick_cpu_cycles(1);

        let mut long_mode = Apu::new();
        long_mode.registers[0x0E] = 0x80; // Tap bit 6.
        long_mode.noise_shift = 0b10;
        long_mode.noise_timer_counter = 0;
        long_mode.tick_cpu_cycles(1);

        assert_eq!(short_mode.noise_shift, 0x4001,
            "short mode XORs bit 0 with bit 1 before shifting");
        assert_eq!(long_mode.noise_shift, 0x0001,
            "long mode XORs bit 0 with bit 6 before shifting");
    }

    #[test]
    fn noise_timer_honors_every_ntsc_period_table_entry() {
        const PERIODS: [u16; 16] = [
            4, 8, 16, 32, 64, 96, 128, 160,
            202, 254, 380, 508, 762, 1016, 2034, 4068,
        ];
        for (index, period) in PERIODS.into_iter().enumerate() {
            let mut apu = Apu::new();
            apu.registers[0x0E] = index as u8;
            apu.noise_shift = 1;
            // Start one tick before expiry so the first LFSR step is exactly
            // the selected period away, independent of the power-on zero state.
            apu.noise_timer_counter = period - 1;
            apu.tick_cpu_cycles(u32::from(period - 1));
            assert_eq!(apu.noise_shift, 1, "period {period} expired too early");
            apu.tick_cpu_cycles(1);
            assert_ne!(apu.noise_shift, 1, "period {period} failed to clock the LFSR");
        }
    }

    #[test]
    fn noise_timer_clocks_each_cpu_cycle_instead_of_alternating_cycles() {
        let mut apu = Apu::new();
        apu.registers[0x0E] = 0; // Noise period table entry = 4 CPU cycles.
        apu.noise_shift = 1;
        apu.noise_timer_counter = 0;

        apu.tick_cpu_cycles(1);
        let first_shift = apu.noise_shift;
        assert_ne!(first_shift, 1, "the initial timer expiration advances the LFSR");

        apu.tick_cpu_cycles(3);
        assert_eq!(apu.noise_shift, first_shift, "the timer waits three cycles after reload");

        apu.tick_cpu_cycles(1);
        assert_ne!(apu.noise_shift, first_shift, "the next LFSR step occurs after four CPU cycles");
    }

    #[test]
    fn frame_counter_write_resets_sequencer_after_hardware_delay() {
        let mut apu = Apu::new();
        apu.tick_cpu_cycles(20);
        assert_eq!(apu.frame_cycles, 20);

        apu.write(0x4017, 0x80); // Five-step mode; reset is not immediate.
        assert!(apu.frame_reset_pending);
        assert!(!apu.five_step_mode, "the mode switch waits for the delayed reset");
        assert_eq!(apu.frame_cycles, 20);
        let delay = u32::from(apu.frame_reset_delay);
        apu.tick_cpu_cycles(delay - 1);
        assert!(apu.frame_reset_pending);
        apu.tick_cpu_cycles(1);
        assert!(!apu.frame_reset_pending);
        assert!(apu.five_step_mode, "the pending mode takes effect with the reset");
        assert_eq!(apu.frame_cycles, 1, "the sequencer resumes on the cycle after reset");
        assert_eq!(apu.half_frame_cycles, 1);
        assert_eq!(apu.quarter_frame_cycles, 1);
    }

    #[test]
    fn frame_counter_reset_delay_tracks_cpu_cycle_parity() {
        let mut even = Apu::new();
        even.write(0x4017, 0x80);
        assert_eq!(even.frame_reset_delay, 4);

        let mut odd = Apu::new();
        odd.tick_cpu_cycles(1);
        odd.write(0x4017, 0x80);
        assert_eq!(odd.frame_reset_delay, 3);
    }

    #[test]
    fn frame_counter_reset_occurs_on_exact_final_delay_cycle() {
        let mut apu = Apu::new();
        apu.frame_cycles = 321;
        apu.write(0x4017, 0x80);
        let delay = u32::from(apu.frame_reset_delay);

        apu.tick_cpu_cycles(delay - 1);
        assert!(apu.frame_reset_pending);
        assert_eq!(apu.frame_cycles, 321 + delay - 1, "phase must not reset early");

        apu.tick_cpu_cycles(1);
        assert!(!apu.frame_reset_pending);
        assert!(apu.five_step_mode);
        assert_eq!(apu.frame_cycles, 1, "the sequencer resumes on the cycle after reset");
    }

    #[test]
    fn frame_irq_inhibit_clears_irq_before_delayed_sequencer_reset() {
        let mut apu = Apu::new();
        apu.frame_irq = true;
        apu.frame_cycles = 123;
        apu.write(0x4017, 0x40);

        assert!(!apu.frame_irq, "IRQ inhibit takes effect on the register write");
        assert!(apu.frame_reset_pending, "sequencer reset remains delayed");
        assert_eq!(apu.frame_cycles, 123, "frame phase is preserved until reset");
    }

    #[test]
    fn replacing_frame_counter_write_restarts_the_pending_delay() {
        let mut apu = Apu::new();
        apu.write(0x4017, 0x80);
        apu.tick_cpu_cycles(2);
        apu.write(0x4017, 0x00);

        assert!(apu.frame_reset_pending);
        assert!(!apu.pending_five_step_mode);
        assert_eq!(apu.frame_reset_delay, if apu.cpu_cycle_odd { 3 } else { 4 });
        let delay = u32::from(apu.frame_reset_delay);
        apu.tick_cpu_cycles(delay);
        assert!(!apu.five_step_mode, "the most recent write wins");
        assert!(!apu.frame_reset_pending);
    }

    #[test]
    fn five_step_reset_clocks_envelopes_and_length_units_immediately() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x09); // Enable pulse 1 and noise.
        apu.write(0x4000, 0x00); // Pulse envelope, period zero.
        apu.write(0x4001, 0x81); // Enable pulse sweep with shift one.
        apu.write(0x4002, 0x20); // Timer period 32.
        apu.write(0x4003, 0xF8); // Reload pulse length.
        apu.write(0x400C, 0x00); // Noise envelope, period zero.
        apu.write(0x400F, 0xF8); // Reload noise length and restart envelope.
        let pulse_length = apu.length_counters[0];
        let noise_length = apu.length_counters[3];

        apu.write(0x4017, 0x80);
        let reset_delay = u32::from(apu.frame_reset_delay);
        apu.tick_cpu_cycles(reset_delay);

        assert_eq!(apu.pulse_envelope_decay[0], 15);
        assert_eq!(apu.noise_envelope_decay, 15);
        assert_eq!(apu.length_counters[0], pulse_length - 1);
        assert_eq!(apu.length_counters[3], noise_length - 1);
        assert_eq!(apu.read(0x4002), 0x30, "five-step reset also clocks pulse sweep");
    }

    #[test]
    fn reading_status_clears_frame_irq_without_disabling_the_sequencer() {
        let mut apu = Apu::new();
        apu.write(0x4017, 0x00); // Four-step mode, frame IRQ enabled.
        apu.tick_cpu_cycles(14_919);

        assert_ne!(apu.read(0x4015) & 0x40, 0, "frame IRQ should be observable in $4015");
        assert_eq!(apu.read(0x4015) & 0x40, 0, "reading $4015 acknowledges frame IRQ");
        assert!(!apu.irq_pending(), "the acknowledged frame IRQ should release the IRQ line");
    }

    #[test]
    fn setting_frame_irq_inhibit_clears_a_pending_frame_irq() {
        let mut apu = Apu::new();
        apu.write(0x4017, 0x00);
        apu.tick_cpu_cycles(14_919);
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
        apu.tick_cpu_cycles(18_645);
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
        assert_eq!(apu.sample(), 0.0, "the triangle sequencer never started because its linear counter stayed at zero");
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
        apu.tick_cpu_cycles(14_919);
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
        apu.tick_cpu_cycles(14_919);
        assert!(apu.frame_irq_pending(), "four-step mode should resume frame IRQ generation");
    }

}


#[cfg(test)]
mod pcm_output_tests {
    use super::*;

    #[test]
    fn noise_lfsr_uses_the_selected_feedback_tap_and_never_stays_zero() {
        let mut apu = Apu::new();
        apu.noise_shift = 0x0002;
        apu.noise_timer_counter = 0;
        apu.cpu_cycle_odd = false;
        apu.write(0x400E, 0x00); // Long mode: tap bit 1.
        apu.tick_cpu_cycles(1);
        assert_eq!(apu.noise_shift, 0x4001, "bit 0 XOR bit 1 is fed into bit 14");

        apu.noise_shift = 0x0002;
        apu.noise_timer_counter = 0;
        apu.cpu_cycle_odd = false;
        apu.write(0x400E, 0x80); // Short mode: tap bit 6.
        apu.tick_cpu_cycles(1);
        assert_eq!(apu.noise_shift, 0x0001, "bit 0 XOR bit 6 is zero for this seed");

        apu.noise_shift = 0;
        apu.noise_timer_counter = 0;
        apu.cpu_cycle_odd = false;
        apu.tick_cpu_cycles(1);
        assert_eq!(apu.noise_shift, 0x4000, "the LFSR must recover from an invalid all-zero state");
    }

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
    #[test]
    fn dmc_loop_restarts_sample_without_raising_terminal_irq() {
        let mut apu = Apu::new();
        apu.write(0x4010, 0x40); // Loop enabled, IRQ disabled.
        apu.write(0x4012, 0x00); // Start at $C000.
        apu.write(0x4013, 0x00); // One-byte sample.
        apu.write(0x4015, 0x10);
        assert_eq!(apu.take_dmc_dma_request(), Some(0xC000));
        apu.supply_dmc_byte(0xA5);
        assert_eq!(apu.dmc_bytes_remaining, 1, "loop mode immediately reloads the one-byte sample length");
        assert_eq!(apu.dmc_current_address, 0xC000, "loop mode restarts from the configured sample address");
        assert!(!apu.dmc_irq, "loop mode without IRQ enable must not raise DMC IRQ");
        apu.dmc_sample_buffer = None;
        assert_eq!(apu.take_dmc_dma_request(), Some(0xC000));
    }

    #[test]
    fn dmc_address_wraps_from_ffff_to_8000_after_a_sample_fetch() {
        let mut apu = Apu::new();
        apu.write(0x4010, 0x00);
        apu.write(0x4012, 0xFF);
        apu.write(0x4013, 0xFF);
        apu.write(0x4015, 0x10);
        apu.dmc_current_address = 0xFFFF;
        apu.dmc_bytes_remaining = 2;
        assert_eq!(apu.take_dmc_dma_request(), Some(0xFFFF));
        apu.supply_dmc_byte(0x12);
        assert_eq!(apu.dmc_current_address, 0x8000);
        assert_eq!(apu.dmc_bytes_remaining, 1);
    }

    #[test]
    fn invalid_pcm_rate_does_not_change_active_rate_or_queued_samples() {
        let mut apu = Apu::new();
        assert!(apu.set_sample_rate(48_000));
        apu.tick_cpu_cycles(2_000);
        let queued = apu.audio_samples.len();
        assert!(queued > 0);
        assert!(!apu.set_sample_rate(7_999));
        assert_eq!(apu.sample_rate(), 48_000);
        assert_eq!(apu.audio_samples.len(), queued, "rejected rate must not clear queued PCM");
    }

}


#[cfg(test)]
mod status_irq_regression_tests {
    use super::*;

    #[test]
    fn reading_status_acknowledges_frame_irq_without_clearing_dmc_irq() {
        let mut apu = Apu::new();
        apu.frame_irq = true;
        apu.dmc_irq = true;

        assert_eq!(apu.read(0x4015) & 0xC0, 0xC0);
        assert!(!apu.frame_irq, "$4015 read acknowledges the frame IRQ");
        assert!(apu.dmc_irq, "$4015 read must not acknowledge the DMC IRQ");
        assert_eq!(apu.read(0x4015) & 0xC0, 0x80);
    }
}


#[cfg(test)]
mod channel_enable_regression_tests {
    use super::*;

    #[test]
    fn disabling_a_channel_clears_its_length_counter_and_status_bit() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x01);
        apu.write(0x4003, 0xF8); // Load pulse 1 length while enabled.
        assert!(apu.length_counter(0).unwrap() > 0);
        assert_ne!(apu.read(0x4015) & 1, 0);

        apu.write(0x4015, 0x00);
        assert_eq!(apu.length_counter(0), Some(0));
        assert_eq!(apu.read(0x4015) & 1, 0);
    }

    #[test]
    fn dmc_restart_uses_register_encoded_address_and_length() {
        let mut apu = Apu::new();
        apu.write(0x4012, 0x23);
        apu.write(0x4013, 0x04);
        apu.write(0x4015, 0x10);
        assert_eq!(apu.dmc_current_address, 0xC8C0);
        assert_eq!(apu.dmc_bytes_remaining, 65);
    }
}


#[cfg(test)]
mod dmc_address_regression_tests {
    use super::*;

    #[test]
    fn dmc_sample_address_wraps_from_ffff_to_8000() {
        let mut apu = Apu::new();
        apu.dmc_current_address = 0xFFFF;
        apu.dmc_bytes_remaining = 2;
        apu.dmc_sample_buffer = None;

        apu.supply_dmc_byte(0xA5);

        assert_eq!(apu.dmc_sample_buffer, Some(0xA5));
        assert_eq!(apu.dmc_current_address, 0x8000);
        assert_eq!(apu.dmc_bytes_remaining, 1);
    }

    #[test]
    fn looping_dmc_restarts_the_sample_after_the_last_byte() {
        let mut apu = Apu::new();
        apu.registers[0x10] = 0x40; // Loop enabled, IRQ disabled.
        apu.registers[0x12] = 0x04; // $C100.
        apu.registers[0x13] = 0x01; // 17-byte sample.
        apu.dmc_enabled = true;
        apu.dmc_current_address = 0xC110;
        apu.dmc_bytes_remaining = 1;
        apu.dmc_sample_buffer = None;

        apu.supply_dmc_byte(0x7E);

        assert_eq!(apu.dmc_sample_buffer, Some(0x7E));
        assert_eq!(apu.dmc_current_address, 0xC100);
        assert_eq!(apu.dmc_bytes_remaining, 17);
        assert!(!apu.dmc_irq);
    }
}


#[cfg(test)]
mod pulse_output_regression_tests {
    use super::*;

    #[test]
    fn pulse_output_requires_enabled_nonzero_length_and_valid_period() {
        let mut apu = Apu::new();
        apu.write(0x4000, 0x5F); // Duty 1, constant volume 15.
        apu.write(0x4002, 8);
        apu.write(0x4003, 0xF8); // Length table entry 31.
        assert_eq!(apu.sample(), 0.0, "a pulse with a zero length counter is silent");

        apu.write(0x4015, 0x01);
        apu.write(0x4003, 0xF8);
        assert!(apu.sample() > 0.0, "an enabled pulse with a valid timer and active duty bit is audible");

        apu.write(0x4015, 0x00);
        assert_eq!(apu.length_counter(0), Some(0));
        assert_eq!(apu.sample(), 0.0, "disabling the pulse channel silences its output");
    }

    #[test]
    fn changing_sample_rate_discards_old_batch_and_resets_filter_phase() {
        let mut apu = Apu::new();
        apu.tick_cpu_cycles(2_000);
        assert!(!apu.drain_audio_samples().is_empty());
        apu.tick_cpu_cycles(200);
        assert!(!apu.audio_samples.is_empty());

        assert!(apu.set_sample_rate(48_000));
        assert_eq!(apu.sample_rate(), 48_000);
        assert!(apu.audio_samples.is_empty());
        assert_eq!(apu.sample_phase, 0);
        assert_eq!(apu.pcm_low_pass, 0.0);
    }
}


#[cfg(test)]
mod pcm_queue_regressions {
    use super::*;

    #[test]
    fn pcm_queue_drops_oldest_sample_when_capacity_is_reached() {
        let mut apu = Apu::new();
        apu.audio_samples.extend((0..AUDIO_QUEUE_LIMIT).map(|n| n as f32 / AUDIO_QUEUE_LIMIT as f32));
        let first = apu.audio_samples[1];
        apu.sample_phase = CPU_CLOCK_HZ - u64::from(apu.sample_rate);
        apu.tick_cpu_cycles(1);
        assert_eq!(apu.audio_samples.len(), AUDIO_QUEUE_LIMIT);
        assert_eq!(apu.audio_samples.front().copied(), Some(first),
            "overrun should discard the oldest sample, not block or grow the queue");
    }

    #[test]
    fn pcm_sample_phase_is_stable_across_single_cycle_boundaries() {
        let mut apu = Apu::new();
        assert!(apu.set_sample_rate(48_000));
        apu.tick_cpu_cycles(37_311);
        let phase_after_batch = apu.sample_phase;
        let batch_samples = apu.drain_audio_samples();
        let mut split = Apu::new();
        assert!(split.set_sample_rate(48_000));
        for _ in 0..37_311 { split.tick_cpu_cycles(1); }
        assert_eq!(split.sample_phase, phase_after_batch);
        assert_eq!(split.drain_audio_samples(), batch_samples);
    }
    #[test]
    fn changing_sample_rate_clears_stale_pcm_and_invalid_rate_is_atomic() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0x01);
        apu.write(0x4000, 0x9F);
        apu.write(0x4002, 0x20);
        apu.write(0x4003, 0xF8);
        apu.tick_cpu_cycles(2_000);
        assert!(!apu.audio_samples.is_empty(), "fixture should produce queued PCM");

        let old_rate = apu.sample_rate();
        let old_len = apu.audio_samples.len();
        assert!(!apu.set_sample_rate(7_999));
        assert_eq!(apu.sample_rate(), old_rate);
        assert_eq!(apu.audio_samples.len(), old_len, "invalid rate must not discard queued PCM");

        assert!(apu.set_sample_rate(48_000));
        assert_eq!(apu.sample_rate(), 48_000);
        assert!(apu.audio_samples.is_empty(), "valid rate changes must clear samples produced at the previous rate");
    }

}


#[cfg(test)]
mod frame_and_dmc_irq_regressions {
    use super::*;

    #[test]
    fn frame_irq_inhibit_clears_pending_irq_immediately() {
        let mut apu = Apu::new();
        apu.tick_cpu_cycles(14_915);
        assert!(apu.irq_pending(), "four-step frame sequence should raise frame IRQ");

        apu.write(0x4017, 0x40); // Inhibit frame IRQ.
        assert!(!apu.irq_pending(), "setting inhibit must clear an already-pending frame IRQ immediately");
        apu.tick_cpu_cycles(14_915);
        assert!(!apu.irq_pending(), "frame IRQ must stay suppressed while inhibit is set");
    }

    #[test]
    fn dmc_terminal_irq_is_raised_only_after_final_sample_byte() {
        let mut apu = Apu::new();
        apu.write(0x4010, 0x80); // IRQ enabled, loop disabled.
        apu.write(0x4012, 0x00);
        apu.write(0x4013, 0x00); // One-byte sample.
        apu.write(0x4015, 0x10);

        assert_eq!(apu.take_dmc_dma_request(), Some(0xC000));
        assert!(!apu.irq_pending(), "requesting the final byte is not itself terminal completion");
        apu.supply_dmc_byte(0xA5);
        assert!(apu.irq_pending(), "the last byte raises DMC IRQ when IRQ is enabled");
        assert_eq!(apu.read(0x4015) & 0x80, 0x80);
        assert!(apu.irq_pending(), "reading status must not acknowledge DMC IRQ");
        apu.write(0x4010, 0x00);
        assert!(!apu.irq_pending(), "clearing DMC IRQ enable also clears the pending DMC IRQ");
    }
}


#[cfg(test)]
mod pulse_sequencer_restart_regressions {
    use super::*;

    #[test]
    fn writing_pulse_high_timer_restarts_each_duty_sequencer() {
        let mut apu = Apu::new();
        apu.pulse_sequence = [5, 7];

        apu.write(0x4003, 0xF8);
        assert_eq!(apu.pulse_sequence[0], 0, "pulse 1 high timer write restarts its duty sequence");
        assert_eq!(apu.pulse_sequence[1], 7, "pulse 1 must not disturb pulse 2");

        apu.write(0x4007, 0xF8);
        assert_eq!(apu.pulse_sequence[1], 0, "pulse 2 high timer write restarts its duty sequence");
    }

    #[test]
    fn pulse_high_timer_write_restarts_sequence_even_when_channel_is_disabled() {
        let mut apu = Apu::new();
        apu.pulse_sequence[0] = 6;
        apu.write(0x4003, 0xF8);
        assert_eq!(apu.pulse_sequence[0], 0);
        assert_eq!(apu.length_counter(0), Some(0), "disabled channel must not load its length counter");
    }
}


#[cfg(test)]
mod triangle_dac_hold_regressions {
    use super::*;

    #[test]
    fn triangle_dac_holds_last_level_when_length_and_linear_counters_stop() {
        let mut low = Apu::new();
        low.registers[0x0A] = 2; // Valid timer period; no timer sequence advance required.
        low.triangle_sequence = 0; // DAC level 15.
        low.triangle_has_clocked = true; // Sequencer previously drove this DAC level.
        low.length_counters[2] = 0;
        low.triangle_linear_counter = 0;

        let mut high = low.clone();
        high.triangle_sequence = 15; // DAC level 0.

        assert!(low.sample() > high.sample(),
            "stopping the triangle sequencer must hold its DAC level rather than mute it");
    }

    #[test]
    fn disabling_triangle_through_status_register_preserves_held_dac_level() {
        let mut apu = Apu::new();
        apu.registers[0x0A] = 2;
        apu.triangle_sequence = 0; // Held DAC level 15.
        apu.triangle_has_clocked = true;
        apu.length_counters[2] = 10;
        apu.enabled_channels = 0x04;
        let before = apu.sample();

        apu.write(0x4015, 0x00);

        assert_eq!(apu.length_counter(2), Some(0));
        assert_eq!(apu.sample(), before,
            "disabling the triangle clears its length counter but does not reset its DAC latch");
    }

    #[test]
    fn triangle_dac_still_outputs_zero_when_sequence_is_at_zero_level() {
        let mut apu = Apu::new();
        apu.registers[0x0A] = 2;
        apu.triangle_sequence = 15;
        apu.length_counters[2] = 0;
        apu.triangle_linear_counter = 0;
        assert_eq!(apu.sample(), 0.0);
    }
}


#[cfg(test)]
mod triangle_dac_period_regressions {
    use super::*;

    #[test]
    fn triangle_dac_keeps_held_value_for_zero_and_one_timer_periods() {
        for period in [0u8, 1u8] {
            let mut apu = Apu::new();
            apu.registers[0x0A] = period;
            apu.triangle_sequence = 0; // Held DAC level 15.
            apu.triangle_has_clocked = true;
            apu.length_counters[2] = 0;
            apu.triangle_linear_counter = 0;
            assert!(apu.sample() > 0.0, "timer period {period} must not mute a previously clocked triangle DAC");
        }
    }

    #[test]
    fn triangle_dac_is_silent_until_the_sequencer_has_started() {
        for period in [0u8, 1u8, 2u8] {
            let mut apu = Apu::new();
            apu.registers[0x0A] = period;
            apu.triangle_sequence = 0;
            apu.triangle_has_clocked = false;
            assert_eq!(apu.sample(), 0.0, "unstarted triangle must be silent at timer period {period}");
        }
    }
}
