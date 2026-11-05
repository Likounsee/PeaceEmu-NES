//! Small top-level NES machine wrapper for loading and stepping cartridge programs.

use crate::bus::CpuBus;
#[cfg(any(test, feature = "ppu-timing-diagnostics"))]
use crate::bus::CpuBusTraceEntry;
use crate::cartridge::{Cartridge, CartridgeError};
use crate::cpu::{Cpu, CpuError, CpuState, StepResult};
use core::fmt;

/// Error raised while creating or executing a basic NES machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MachineError {
    /// Cartridge image could not be parsed or is unsupported.
    Cartridge(CartridgeError),
    /// CPU encountered an unsupported instruction.
    Cpu(CpuError),
    /// A frame could not be reached within the safety instruction budget.
    FrameTimeout,
    /// The loaded cartridge does not declare battery-backed save RAM.
    NoBatteryRam,
    /// Save RAM payload does not match the cartridge's exact RAM size.
    InvalidSaveRamLength {
        /// Number of bytes required by this cartridge.
        expected: usize,
        /// Number of bytes supplied by the caller.
        actual: usize,
    },
}

impl fmt::Display for MachineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cartridge(error) => write!(f, "invalid NES cartridge: {error}"),
            Self::Cpu(error) => write!(f, "NES CPU error: {error}"),
            Self::FrameTimeout => write!(f, "NES frame step exceeded its instruction budget"),
            Self::NoBatteryRam => write!(f, "NES cartridge does not declare battery-backed RAM"),
            Self::InvalidSaveRamLength { expected, actual } => write!(f, "invalid NES save RAM length: expected {expected} bytes, got {actual}"),
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

/// A CPU-bus slot used while the machine arbitrates OAM and DMC DMA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DmaBusPhase {
    /// OAM's initial CPU halt/alignment slot.
    Halt,
    /// OAM reads one byte from CPU memory; DMC has priority over this GET slot.
    Get,
    /// OAM commits the latched byte to PPU OAM; DMC cannot steal this PUT slot.
    Put,
}

impl DmaBusPhase {
    const fn dmc_can_halt(self) -> bool {
        matches!(self, Self::Halt | Self::Get)
    }
}

/// CPU plus the current minimal bus and a mapper-selected cartridge.
///
/// The wrapper supports instruction stepping, frame execution, snapshot rendering, controller
/// input, and queued PCM output. The PPU renderer and hardware timing remain approximate,
/// so commercial-game compatibility is not yet established.
#[derive(Debug)]
pub struct NesMachine {
    cpu: Cpu,
    bus: CpuBus,
    battery_backed: bool,
    ppu_startup_phase: u8,
}

impl NesMachine {
    /// Parses an iNES image, attaches its mapper-selected cartridge, and performs CPU reset.
    pub fn from_ines(image: &[u8]) -> Result<Self, MachineError> {
        Self::from_ines_with_ppu_startup_phase(image, 0)
    }

    /// Loads an iNES image with a selected CPU/PPU bus-access phase (0, 1, or 2).
    ///
    /// The CPU's register-access point can align with any of the three PPU dots
    /// in a CPU cycle. The default constructor selects phase 0; this variant is
    /// useful for deterministic hardware-timing diagnostics and reset testing.
    pub fn from_ines_with_ppu_startup_phase(
        image: &[u8],
        ppu_startup_phase: u8,
    ) -> Result<Self, MachineError> {
        assert!(ppu_startup_phase < 3, "PPU startup phase must be 0, 1, or 2");
        let cartridge = Cartridge::parse(image)?;
        let battery_backed = image.get(6).is_some_and(|flags| flags & 0x02 != 0);
        let mut bus = CpuBus::new();
        bus.insert_cartridge(cartridge);
        bus.set_ppu_bus_phase(ppu_startup_phase);
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus);
        Ok(Self { cpu, bus, battery_backed, ppu_startup_phase })
    }


    /// Resets CPU and volatile devices while keeping the loaded cartridge and host audio rate.
    ///
    /// CPU RAM and cartridge RAM are preserved, matching a console reset rather than a power cycle.
    pub fn reset(&mut self) {
        self.bus.reset();
        self.bus.set_ppu_bus_phase(self.ppu_startup_phase);
        self.cpu.reset(&mut self.bus);
    }

    /// Returns the bounded CPU-bus transaction history, oldest entry first.
    ///
    /// The trace includes CPU reads/writes and DMC DMA reads. DMC's internal
    /// halt/dummy/alignment phases are visible as transactions, but the current
    /// instruction-granular CPU core does not yet assign each phase an independent
    /// scheduler callback.
    #[cfg(any(test, feature = "ppu-timing-diagnostics"))]
    pub fn bus_trace(&self) -> Vec<CpuBusTraceEntry> {
        self.bus.bus_trace()
    }

    /// Returns the bounded PPU VBlank/NMI timing history, oldest first.
    #[cfg(feature = "ppu-timing-diagnostics")]
    pub fn ppu_timing_events(&self) -> Vec<crate::ppu::PpuTimingEvent> {
        self.bus.ppu_timing_events()
    }

    /// Clears diagnostic bus history without changing emulated machine state.
    #[cfg(any(test, feature = "ppu-timing-diagnostics"))]
    pub fn clear_bus_trace(&mut self) {
        self.bus.clear_bus_trace();
    }

    /// Updates controller 1 or 2 button state.
    ///
    /// Button bits, from least to most significant: A, B, Select, Start, Up, Down, Left,
    /// Right. The CPU reads latched bits serially through $4016/$4017.
    pub fn set_controller_state(&mut self, port: usize, buttons: u8) {
        self.bus.set_controller_state(port, buttons);
    }

    /// Renders a background snapshot for frontends that need an indexed 256x240 image.
    /// Call after updating PPU scroll/register state; per-dot rendering is not implemented yet.
    pub fn render_background(&mut self) { self.bus.render_background(); }

    /// Renders a background snapshot and composites sprites; timing remains frame-based.
    pub fn render_frame(&mut self) { self.bus.render_frame(); }

    /// Returns the most recently rendered 256x240 background as 6-bit NES palette indices.
    pub fn framebuffer(&self) -> &[u8] { self.bus.framebuffer() }

    /// Converts the indexed framebuffer to RGBA8 using a frontend-provided 64-color palette.
    ///
    /// Supplying the palette keeps regional/host palette choices outside the emulation core.
    /// The returned buffer is tightly packed as R, G, B, A bytes for each of 256x240 pixels.
    pub fn framebuffer_rgba(&self, palette: &[[u8; 4]; 64]) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(self.bus.framebuffer().len() * 4);
        let emphasis = self.bus.ppu_color_emphasis();
        for &index in self.bus.framebuffer() {
            let mut color = palette[(index & 0x3F) as usize];
            // On NTSC hardware, emphasizing one primary attenuates the other two.
            // Apply the output-stage effect here so the public indexed framebuffer
            // remains a stable 6-bit palette format for existing frontends.
            if emphasis & 0x06 != 0 {
                color[0] = (u16::from(color[0]) * 3 / 4) as u8;
            }
            if emphasis & 0x05 != 0 {
                color[1] = (u16::from(color[1]) * 3 / 4) as u8;
            }
            if emphasis & 0x03 != 0 {
                color[2] = (u16::from(color[2]) * 3 / 4) as u8;
            }
            rgba.extend_from_slice(&color);
        }
        rgba
    }

    /// Returns a snapshot of the CPU registers and flags.
    pub fn cpu_state(&self) -> CpuState { self.cpu.state() }

    /// Returns the accumulated nominal CPU cycle count, including reset.
    pub fn total_cycles(&self) -> u64 { self.cpu.total_cycles() }

    /// Returns the configured host PCM output rate in samples per second.
    ///
    /// This is the accepted rate used by the APU after construction or reset.
    pub fn audio_sample_rate(&self) -> u32 { self.bus.audio_sample_rate() }

    /// Returns the current PPU scanline and dot for frontend diagnostics.
    ///
    /// The PPU uses NTSC-style 262-scanline timing; rendering and CPU bus accesses
    /// are not yet fully cycle-exact.
    pub fn ppu_position(&self) -> (u16, u16) { self.bus.ppu_position() }

    /// Returns one approximate normalized mono audio sample (0.0..1.0).
    pub fn audio_sample(&self) -> f32 { self.bus.apu_sample() }

    /// Configures audio output sampling rate (8,000..=192,000 samples/second).
    ///
    /// Returns false for unsupported rates. Changing to a different valid rate clears
    /// queued samples and resets resampling state; applying the current rate is a no-op.
    pub fn set_audio_sample_rate(&mut self, sample_rate: u32) -> bool {
        self.bus.set_audio_sample_rate(sample_rate)
    }

    /// Drains mono PCM samples generated since the previous drain.
    ///
    /// The default output rate is 44.1 kHz. Frontends should drain this queue regularly;
    /// the queue is bounded and discards the oldest samples if a consumer falls behind.
    pub fn drain_audio_samples(&mut self) -> Vec<f32> {
        self.bus.drain_audio_samples()
    }

    /// Returns whether the loaded iNES image declares battery-backed save RAM.
    pub fn has_battery_backed_ram(&self) -> bool { self.battery_backed }

    /// Copies battery-backed PRG-RAM for persistence by the frontend.
    ///
    /// The returned payload is the raw RAM region, not a full emulator save state.
    pub fn save_battery_ram(&self) -> Result<Vec<u8>, MachineError> {
        if !self.battery_backed { return Err(MachineError::NoBatteryRam); }
        self.bus.cartridge_prg_ram_snapshot().ok_or(MachineError::NoBatteryRam)
    }

    /// Restores battery-backed PRG-RAM after validating its entire payload.
    pub fn load_battery_ram(&mut self, data: &[u8]) -> Result<(), MachineError> {
        if !self.battery_backed { return Err(MachineError::NoBatteryRam); }
        let expected = self.bus.cartridge_prg_ram_snapshot()
            .ok_or(MachineError::NoBatteryRam)?.len();
        if data.len() != expected {
            return Err(MachineError::InvalidSaveRamLength { expected, actual: data.len() });
        }
        if !self.bus.restore_cartridge_prg_ram(data) {
            return Err(MachineError::InvalidSaveRamLength { expected, actual: data.len() });
        }
        Ok(())
    }

    /// Executes instructions until the next complete PPU frame boundary.
    ///
    /// On a freshly reset machine, the initial pre-render-to-visible transition is skipped
    /// so this method advances a full frame instead of returning after startup scanline 261.
    /// Returns nominal CPU cycles consumed; instruction granularity means the boundary may
    /// be crossed by a few dots. Visible scanlines are rendered as the PPU advances.
    pub fn step_frame(&mut self) -> Result<u32, MachineError> {
        let start_cycles = self.cpu.total_cycles();
        let (start_scanline, _) = self.bus.ppu_position();
        let mut previous_scanline = start_scanline;
        let mut skipped_initial_prerender = start_scanline == 261;
        let mut iterations = 0u32;
        loop {
            self.step()?;
            iterations += 1;
            let (scanline, _) = self.bus.ppu_position();
            let crossed_frame = previous_scanline > scanline;
            if crossed_frame {
                if skipped_initial_prerender {
                    skipped_initial_prerender = false;
                } else {
                    // The PPU has already produced each visible line during the frame.
                    // Re-rendering a snapshot here would erase mid-frame scroll and palette effects.
                    return Ok(self.cpu.total_cycles().saturating_sub(start_cycles) as u32);
                }
            }
            previous_scanline = scanline;
            // Prevent a malformed ROM from trapping a host frontend forever.
            if iterations >= 100_000 {
                return Err(MachineError::FrameTimeout);
            }
        }
    }

    /// Executes several complete frames for frontend loops and returns total CPU cycles.
    ///
    /// Each frame is rendered before continuing. A zero frame count is a no-op; if a
    /// later frame fails, earlier frames remain executed and the error is returned.
    pub fn run_frames(&mut self, frames: usize) -> Result<u64, MachineError> {
        let mut total_cycles = 0u64;
        for _ in 0..frames {
            total_cycles = total_cycles.saturating_add(u64::from(self.step_frame()?));
        }
        Ok(total_cycles)
    }

    /// Executes one CPU instruction and services any OAM DMA it requests.
    pub fn step(&mut self) -> Result<StepResult, MachineError> {
        // Let memory accesses advance the PPU in instruction order. This is especially
        // important for $2002 polling (e.g. SMB's sprite-zero split): the status read
        // must observe PPU events reached by the opcode/operand fetches before it.
        self.bus.begin_cpu_instruction();
        let step_result = self.cpu.step(&mut self.bus);
        let bus_cycles = u32::from(self.bus.end_cpu_instruction());
        let result = step_result?;
        let instruction_cycles = u32::from(result.cycles);
        debug_assert!(
            bus_cycles <= instruction_cycles,
            "CPU performed more bus accesses than instruction cycles"
        );
        debug_assert_eq!(
            bus_cycles, instruction_cycles,
            "NES CPU instruction left unmodeled cycles; add their real dummy/idle bus phases before advancing the next instruction"
        );

        // DMC stall device cycles are clocked at the exact bus access that
        // requested them. Charge only the CPU's elapsed-cycle counter here.
        let dmc_stalls = self.bus.take_instruction_dmc_stalls();
        if dmc_stalls != 0 {
            self.cpu.add_stall_cycles(dmc_stalls.min(u32::from(u16::MAX)) as u16);
        }
        self.clock_devices_for_cpu_cycles(instruction_cycles.saturating_sub(bus_cycles));
        self.bus.finish_cpu_instruction();

        if let Some(page) = self.bus.take_oam_dma_request() {
            self.perform_oam_dma(page);
            self.bus.release_deferred_nmi_after_dma();
        }
        Ok(result)
    }

    /// Runs one OAM DMA transfer through explicit GET/PUT bus phases.
    ///
    /// DMC's setup phases overlap the OAM transfer. A DMC sample GET wins only
    /// against an OAM GET, in which case OAM inserts one alignment cycle and
    /// retries the same byte without advancing its index.
    fn perform_oam_dma(&mut self, page: u8) {
        // OAM DMA is 512 alternating source-read/OAM-write cycles, preceded
        // by one halt cycle and an extra alignment cycle on odd CPU parity.
        let dma_cycles = if self.cpu.total_cycles() & 1 == 0 { 513u16 } else { 514u16 };
        let initial_slots = u32::from(dma_cycles - 512);
        // The CPU has finished the $4014 write and is waiting to fetch its
        // next opcode; preserve that address during OAM's halt/alignment slots.
        self.bus.set_dma_bus_address(self.cpu.state().pc);
        for _ in 0..initial_slots {
            // A DMC transfer has priority over this OAM halt slot. Keep the
            // same OAM slot pending until the DMC transaction releases the bus.
            while self.clock_dma_bus_phase(DmaBusPhase::Halt) {}
        }

        for offset in 0..=u8::MAX {
            let source_address = (u16::from(page) << 8) | u16::from(offset);
            self.bus.set_dma_bus_address(source_address);
            // DMC owns a stolen GET slot; do not advance the OAM byte index or
            // sample source memory until the delayed OAM GET actually occurs.
            while self.clock_dma_bus_phase(DmaBusPhase::Get) {
                // A DMC GET consumed this OAM GET slot. OAM must spend the
                // following PUT half-cycle realigning before retrying the read.
                self.clock_dma_bus_phase(DmaBusPhase::Halt);
                self.cpu.add_stall_cycles(1);
            }
            let value = self.bus.read_oam_dma_source(page, offset);

            self.clock_dma_bus_phase(DmaBusPhase::Put);
            self.bus.write_oam_dma_byte(value);
        }
        self.cpu.add_stall_cycles(dma_cycles);
    }

    /// Advances one OAM DMA slot while allowing DMC's non-GET phases to overlap.
    ///
    /// HALT, dummy and alignment phases consume cycles OAM was already using.
    /// Only a DMC sample GET that collides with an OAM GET postpones that byte;
    /// the caller then inserts OAM's one-cycle PUT alignment before retrying.
    fn clock_dma_bus_phase(&mut self, phase: DmaBusPhase) -> bool {
        self.bus.advance_cpu_cycle();
        let _crossed_scanlines = self.bus.tick_ppu(3);
        let _dmc_phase_cycles = self.bus.tick_apu_with_dma_access(1, phase.dmc_can_halt());
        self.bus.sample_nmi_line();
        let dmc_get = self.bus.take_dmc_dma_get_occurred();
        if dmc_get && phase == DmaBusPhase::Get {
            // The DMC GET itself is one extra CPU cycle beyond the delayed OAM
            // GET. The following OAM alignment cycle is charged by the caller.
            self.cpu.add_stall_cycles(1);
            true
        } else {
            false
        }
    }

    /// Advances PPU and APU time in CPU-cycle-sized quanta.
    ///
    /// This is the central clock path for instruction time and DMA stalls. CPU bus
    /// accesses are still instruction-granular, but device time no longer jumps over
    /// scanline boundaries in one large PPU/APU batch.
    fn clock_devices_for_cpu_cycles(&mut self, cycles: u32) {
        self.clock_devices_for_cpu_cycles_with_dmc_access(cycles, true);
    }

    /// Advances shared device time while controlling whether this bus phase can be
    /// halted by DMC DMA. OAM's put/write phase passes false; get/read and idle
    /// phases pass true.
    fn clock_devices_for_cpu_cycles_with_dmc_access(
        &mut self,
        mut cycles: u32,
        dmc_dma_can_halt: bool,
    ) -> u32 {
        let mut stolen_cycles = 0u32;
        while cycles != 0 {
            self.bus.advance_cpu_cycle();
            let _crossed_scanlines = self.bus.tick_ppu(3);
            // MMC3 IRQs now follow filtered PPU A12 transitions instead of scanline clocks.
            let dmc_stalls = self.bus.tick_apu_with_dma_access(1, dmc_dma_can_halt);
            self.bus.sample_nmi_line();
            cycles -= 1;

            // DMC DMA steals CPU time. Each stolen cycle advances both video and audio
            // clocks through this same path, including any scanline IRQ edges it crosses.
            if dmc_stalls != 0 {
                stolen_cycles = stolen_cycles.saturating_add(dmc_stalls);
                self.cpu.add_stall_cycles(dmc_stalls.min(u32::from(u16::MAX)) as u16);
                cycles = cycles.saturating_add(dmc_stalls);
            }
        }
        stolen_cycles
    }

    /// Reads a byte through the CPU address bus.
    pub fn read_cpu(&mut self, address: u16) -> u8 { self.bus.read(address) }

    /// Writes a byte through the CPU address bus.
    pub fn write_cpu(&mut self, address: u16, value: u8) { self.bus.write(address, value); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::CpuBusAccessKind;

    fn nrom_with_program(program: &[u8]) -> Vec<u8> {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16..16 + program.len()].copy_from_slice(program);
        image[16 + 0x3FFC] = 0x00;
        image[16 + 0x3FFD] = 0x80;
        image
    }

    #[test]
    fn machine_reset_restarts_cpu_and_bus_timing_without_losing_cartridge_ram() {
        let mut image = nrom_with_program(&[0xEA, 0xEA]);
        image[6] |= 0x02; // Battery-backed PRG RAM.
        let mut machine = NesMachine::from_ines(&image).unwrap();
        machine.write_cpu(0x6000, 0xA9);
        machine.step().unwrap();
        machine.step().unwrap();
        assert!(machine.total_cycles() > 7);
        let before_reset = machine.total_cycles();

        machine.reset();

        assert_eq!(machine.total_cycles() - before_reset, 7,
            "reset adds the seven-cycle CPU reset sequence to the accumulated counter");
        assert_eq!(machine.ppu_position(), (261, 0));
        assert_eq!(machine.read_cpu(0x6000), 0xA9, "reset preserves cartridge RAM");
    }

    #[test]
    fn public_device_diagnostics_report_audio_rate_and_ppu_position() {
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[0xEA])).unwrap();
        assert_eq!(machine.audio_sample_rate(), 44_100);
        assert_eq!(machine.ppu_position(), (261, 0));

        assert!(machine.set_audio_sample_rate(48_000));
        machine.clock_devices_for_cpu_cycles(1);
        assert_eq!(machine.audio_sample_rate(), 48_000);
        assert_eq!(machine.ppu_position(), (261, 3));
    }


    #[test]
    fn oam_dma_phase_policy_gives_dmc_priority_only_on_get_and_halt_slots() {
        assert!(DmaBusPhase::Halt.dmc_can_halt());
        assert!(DmaBusPhase::Get.dmc_can_halt());
        assert!(!DmaBusPhase::Put.dmc_can_halt());
    }

    #[test]
    fn oam_dma_uses_513_cycles_when_the_post_instruction_parity_is_even() {
        // LDA $00 (3 cycles); STA $4014 (4 cycles). Including reset's
        // seven cycles, DMA begins at cycle 14 and needs no alignment cycle.
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[
            0xA5, 0x00,
            0x8D, 0x14, 0x40,
            0xEA,
        ])).unwrap();
        machine.write_cpu(0x0000, 0x02);
        for offset in 0..=u8::MAX {
            machine.write_cpu(0x0200 | u16::from(offset), offset ^ 0xA5);
        }

        assert_eq!(machine.step().unwrap().cycles, 3);
        let before = machine.total_cycles();
        assert_eq!(machine.step().unwrap().cycles, 4);
        assert_eq!(machine.total_cycles() - before, 4 + 513);
        for offset in 0..=u8::MAX {
            machine.write_cpu(0x2003, offset);
            assert_eq!(machine.read_cpu(0x2004), offset ^ 0xA5);
        }
    }

    #[test]
    fn oam_dma_adds_alignment_stall_cycles_and_advances_ppu() {
        // LDA #$02; STA $4014. Reset starts at 7 cycles, so six instruction
        // cycles leave odd parity and the DMA takes 514 cycles.
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[
            0xA9, 0x02,
            0x8D, 0x14, 0x40,
            0xEA,
        ])).unwrap();

        machine.step().unwrap();
        let before_cycles = machine.total_cycles();
        let before_position = machine.ppu_position();
        let result = machine.step().unwrap();

        assert_eq!(result.cycles, 4, "STA retains its nominal instruction cycles");
        assert_eq!(machine.total_cycles() - before_cycles, 4 + 514,
            "odd CPU parity adds one alignment cycle to OAM DMA");
        assert_ne!(machine.ppu_position(), before_position,
            "OAM DMA stalls must advance the PPU as well as CPU time");
    }

    #[test]
    fn instruction_bus_accesses_and_remaining_cycles_keep_ppu_clock_synchronized() {
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[0xEA])).unwrap();
        let before_cycles = machine.total_cycles();
        assert_eq!(machine.ppu_position(), (261, 0));

        let result = machine.step().unwrap();

        assert_eq!(result.cycles, 2);
        assert_eq!(machine.total_cycles() - before_cycles, 2);
        assert_eq!(machine.ppu_position(), (261, 6),
            "opcode fetch plus the unmodelled implied-instruction cycle must advance six PPU dots");
    }

    #[test]
    fn oam_dma_services_pending_dmc_fetch_without_corrupting_oam_bytes() {
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[0xEA])).unwrap();
        for offset in 0..=u8::MAX {
            machine.bus.write(0x0200 | u16::from(offset), offset ^ 0x6D);
        }

        // Arm a one-byte DMC sample and let its request become pending on a
        // non-haltable CPU write phase immediately before OAM DMA begins.
        machine.bus.write(0x4010, 0x00);
        machine.bus.write(0x4012, 0x00);
        machine.bus.write(0x4013, 0x00);
        machine.bus.write(0x4015, 0x10);


        let before = machine.total_cycles();
        machine.perform_oam_dma(0x02);

        // The DMC halt and dummy phases overlap OAM. Its sample GET collides
        // with OAM's first GET, costing the DMC GET plus one OAM realignment.
        assert_eq!(machine.total_cycles() - before, 516,
            "a DMC GET collision adds two cycles, not all three DMC setup phases");
        assert_eq!(machine.bus.read(0x4015) & 0x10, 0,
            "the pending one-byte DMC sample must be fetched during OAM DMA");

        for offset in 0..=u8::MAX {
            machine.bus.write(0x2003, offset);
            assert_eq!(machine.bus.read(0x2004), offset ^ 0x6D,
                "OAM DMA byte {offset} must survive DMC arbitration");
        }
    }

    #[test]
    fn repeated_dmc_refills_during_oam_dma_preserve_all_256_transferred_bytes() {
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[0xEA])).unwrap();
        for offset in 0..=u8::MAX {
            machine.bus.write(0x0200 | u16::from(offset), offset.wrapping_mul(37) ^ 0xA6);
        }

        // Use the fastest DMC rate and a multi-byte sample so another refill
        // can become pending while the 256-byte OAM transfer is in progress.
        machine.bus.write(0x4010, 0x0F);
        machine.bus.write(0x4012, 0x00);
        machine.bus.write(0x4013, 0x01); // 17-byte sample.
        machine.bus.write(0x4015, 0x10);


        let before = machine.total_cycles();
        machine.perform_oam_dma(0x02);
        let elapsed = machine.total_cycles() - before;

        assert!(elapsed >= 516,
            "the initial DMC GET must delay the first OAM GET and force realignment; elapsed={elapsed}");
        let sample_gets = machine.bus.bus_trace().iter().filter(|entry| {
            entry.kind == CpuBusAccessKind::DmcRead && entry.address >= 0x8000
        }).count();
        assert!(sample_gets >= 2,
            "the output unit must empty its first byte and request a later sample refill; GETs={sample_gets}");

        for offset in 0..=u8::MAX {
            machine.bus.write(0x2003, offset);
            assert_eq!(machine.bus.read(0x2004), offset.wrapping_mul(37) ^ 0xA6,
                "repeated DMC arbitration must not drop, duplicate, or shift OAM byte {offset}");
        }
    }

    #[test]
    fn looping_dmc_refill_steals_a_four_cycle_oam_get_window() {
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[0xEA])).unwrap();
        for offset in 0..=u8::MAX {
            machine.bus.write(0x0200 | u16::from(offset), offset.wrapping_mul(19) ^ 0x73);
        }

        // A one-byte looping sample refills the buffer again during the OAM
        // transfer. The first fetch costs three cycles; the loop refill costs four.
        machine.bus.write(0x4010, 0x4F); // Loop + fastest DMC rate.
        machine.bus.write(0x4012, 0x00);
        machine.bus.write(0x4013, 0x00);
        machine.bus.write(0x4015, 0x10);


        let before = machine.total_cycles();
        machine.perform_oam_dma(0x02);
        let elapsed = machine.total_cycles() - before;
        assert!(elapsed >= 516,
            "the initial DMC GET collision must delay OAM and trigger realignment; elapsed={elapsed}");
        let sample_gets = machine.bus.bus_trace().iter().filter(|entry| {
            entry.kind == CpuBusAccessKind::DmcRead && entry.address == 0xC000
        }).count();
        assert!(sample_gets >= 2,
            "the looping sample must be fetched again after its first byte is consumed; GETs={sample_gets}");

        for offset in 0..=u8::MAX {
            machine.bus.write(0x2003, offset);
            assert_eq!(machine.bus.read(0x2004), offset.wrapping_mul(19) ^ 0x73,
                "looped DMC arbitration must preserve OAM byte {offset}");
        }
    }

    #[test]
    fn dmc_get_overlapping_oam_put_does_not_extend_the_oam_transfer() {
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[0xEA])).unwrap();
        machine.bus.write(0x4010, 0x00);
        machine.bus.write(0x4012, 0x00);
        machine.bus.write(0x4013, 0x00);
        machine.bus.write(0x4015, 0x10);


        // Reset leaves the CPU at cycle 7. One idle CPU stall makes the
        // OAM halt slot the only initial slot. The APU phase is untouched, so
        // DMC halts on that GET and its sample GET overlaps OAM's PUT.
        machine.cpu.add_stall_cycles(1);
        let before = machine.total_cycles();
        machine.perform_oam_dma(0x02);

        assert_eq!(machine.total_cycles() - before, 513,
            "a DMC sample GET can share an OAM PUT cycle without delaying OAM");
        assert_eq!(machine.bus.read(0x4015) & 0x10, 0,
            "the overlapping DMC sample GET must still complete");
    }

    #[test]
    fn dmc_dma_waits_for_oam_put_phase_to_finish_before_stealing_a_get_phase() {
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[0xEA])).unwrap();
        machine.bus.write(0x4010, 0x00);
        machine.bus.write(0x4012, 0x00);
        machine.bus.write(0x4013, 0x00); // One-byte sample.
        machine.bus.write(0x4015, 0x10);

        // The initial load targets GET. Advance one non-haltable GET phase
        // so the pending request records its failed halt attempt before PUT.
        assert_ne!(machine.bus.read(0x4015) & 0x10, 0,
            "DMC sample must remain pending throughout an OAM put phase");

        // The next OAM get/read phase is a legal DMC halt point.
        let before = machine.cpu.total_cycles();
        machine.clock_devices_for_cpu_cycles_with_dmc_access(1, true);
        assert_eq!(machine.cpu.total_cycles() - before, 3,
            "the deferred initial load reaches its target GET on the next readable slot");
        assert_eq!(machine.bus.read(0x4015) & 0x10, 0,
            "the DMC sample must be fetched during the following OAM get phase");
    }

    #[test]
    fn device_clock_advances_ppu_three_dots_per_cpu_cycle() {
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[0xEA])).unwrap();
        assert_eq!(machine.bus.ppu_position(), (261, 0));

        machine.clock_devices_for_cpu_cycles(1);
        assert_eq!(machine.bus.ppu_position(), (261, 3));

        machine.clock_devices_for_cpu_cycles(2);
        assert_eq!(machine.bus.ppu_position(), (261, 9));
    }

    #[test]
    fn device_clock_crosses_scanline_boundaries_without_losing_dots() {
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[0xEA])).unwrap();
        machine.bus.tick_ppu(338);
        assert_eq!(machine.bus.ppu_position(), (261, 338));

        machine.clock_devices_for_cpu_cycles(1);
        assert_eq!(machine.bus.ppu_position(), (0, 0));
    }

    #[test]
    fn machine_loads_uxrom_and_switches_prg_bank_through_cpu_bus() {
        const BANK_LEN: usize = 16 * 1024;
        let mut image = vec![0; 16 + 3 * BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 3;
        image[6] = 0x20; // Mapper 2 (UxROM).
        for bank in 0..3 {
            image[16 + bank * BANK_LEN..16 + (bank + 1) * BANK_LEN].fill(bank as u8);
        }
        // Reset vector lives in the fixed final bank at CPU $FFFC-$FFFD.
        image[16 + 2 * BANK_LEN + 0x3FFC] = 0x00;
        image[16 + 2 * BANK_LEN + 0x3FFD] = 0xC0;

        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.cpu_state().pc, 0xC000);
        assert_eq!(machine.read_cpu(0x8000), 0);
        assert_eq!(machine.read_cpu(0xC000), 2);

        machine.write_cpu(0x8000, 1);
        assert_eq!(machine.read_cpu(0x8000), 1);
        assert_eq!(machine.read_cpu(0xBFFF), 1);
        assert_eq!(machine.read_cpu(0xC000), 2);
    }

    #[test]
    fn machine_loads_mmc1_and_routes_serial_mapper_writes() {
        const BANK_LEN: usize = 16 * 1024;
        let mut image = vec![0; 16 + 4 * BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 4;
        image[6] = 0x10; // Mapper 1 (MMC1), horizontal initial mirroring.
        for bank in 0..4 {
            image[16 + bank * BANK_LEN..16 + (bank + 1) * BANK_LEN].fill(bank as u8);
        }
        image[16 + 3 * BANK_LEN + 0x3FFC] = 0x00;
        image[16 + 3 * BANK_LEN + 0x3FFD] = 0xC0;

        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.cpu_state().pc, 0xC000);
        assert_eq!(machine.read_cpu(0x8000), 0);
        assert_eq!(machine.read_cpu(0xC000), 3);

        // MMC1 serial register commits bit 0 first after five writes.
        for bit in [0, 1, 0, 0, 0] {
            machine.write_cpu(0xE000, bit);
        }
        assert_eq!(machine.read_cpu(0x8000), 2);
        assert_eq!(machine.read_cpu(0xC000), 3);
    }

    #[test]
    fn machine_loads_gxrom_and_routes_prg_chr_bank_selection() {
        const PRG_BANK_LEN: usize = 16 * 1024;
        const CHR_BANK_LEN: usize = 8 * 1024;
        let mut image = vec![0; 16 + 4 * PRG_BANK_LEN + 3 * CHR_BANK_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 4;
        image[5] = 3;
        image[6] = 0x20; // Mapper 66 low nibble.
        image[7] = 0x40; // Mapper 66 high nibble.
        for bank in 0..2 {
            let start = 16 + bank * 2 * PRG_BANK_LEN;
            image[start..start + 2 * PRG_BANK_LEN].fill((0x30 + bank) as u8);
        }
        let chr_start = 16 + 4 * PRG_BANK_LEN;
        for bank in 0..3 {
            let start = chr_start + bank * CHR_BANK_LEN;
            image[start..start + CHR_BANK_LEN].fill((0x60 + bank) as u8);
        }
        // The initial 32 KiB bank pair contains the reset vector at $FFFC.
        image[16 + 2 * PRG_BANK_LEN - 4] = 0x00;
        image[16 + 2 * PRG_BANK_LEN - 3] = 0xC0;

        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.cpu_state().pc, 0xC000);
        assert_eq!(machine.read_cpu(0x8000), 0x30);
        assert_eq!(machine.read_cpu(0xC000), 0x30);

        machine.write_cpu(0x8000, 0x11); // PRG bank 1 and CHR bank 1.
        assert_eq!(machine.read_cpu(0x8000), 0x31);
        assert_eq!(machine.read_cpu(0xC000), 0x31);
    }


    #[test]
    fn machine_loads_mmc3_and_routes_prg_bank_registers() {
        const PRG_BANK_LEN: usize = 16 * 1024;
        let mut image = vec![0; 16 + 4 * PRG_BANK_LEN + 8 * 1024];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 4;
        image[5] = 1;
        image[6] = 0x40; // Mapper 4 (MMC3).
        for bank in 0..8 {
            let start = 16 + bank * 8 * 1024;
            image[start..start + 8 * 1024].fill(bank as u8);
        }
        // Reset vector must live in the fixed final 8 KiB bank.
        image[16 + 4 * PRG_BANK_LEN - 4] = 0x00;
        image[16 + 4 * PRG_BANK_LEN - 3] = 0xE0;

        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.cpu_state().pc, 0xE000);
        assert_eq!(machine.read_cpu(0x8000), 0);
        assert_eq!(machine.read_cpu(0xC000), 6);
        assert_eq!(machine.read_cpu(0xE000), 7);
        machine.write_cpu(0x8000, 6);
        machine.write_cpu(0x8001, 3);
        assert_eq!(machine.read_cpu(0x8000), 3);
        assert_eq!(machine.read_cpu(0xC000), 6);
        assert_eq!(machine.read_cpu(0xE000), 7);
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
    fn machine_services_oam_dma_and_counts_stall_cycles() {
        let image = nrom_with_program(&[0xA9, 0x02, 0x8D, 0x14, 0x40, 0xEA]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        for i in 0..=u8::MAX {
            machine.write_cpu(0x0200 | u16::from(i), i ^ 0x5A);
        }
        machine.step().unwrap(); // LDA #$02
        machine.step().unwrap(); // STA $4014 starts DMA
        assert_eq!(machine.total_cycles(), 7 + 2 + 4 + 514);
        for i in 0..=u8::MAX {
            machine.write_cpu(0x2003, i);
            assert_eq!(machine.read_cpu(0x2004), i ^ 0x5A);
        }
    }

    #[test]
    fn dmc_dma_delays_oam_get_without_skipping_or_corrupting_bytes() {
        let image = nrom_with_program(&[0xEA]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        for offset in 0..=u8::MAX {
            machine.write_cpu(0x0200 | u16::from(offset), offset ^ 0xA7);
        }

        // One-byte DMC sample: its first request appears immediately, so it
        // competes with OAM's initial halt slot and must complete before OAM GET.
        machine.write_cpu(0x4012, 0x00);
        machine.write_cpu(0x4013, 0x00);
        machine.write_cpu(0x4015, 0x10);
        let before = machine.total_cycles();
        let oam_base_cycles = if before & 1 == 0 { 513 } else { 514 };
        machine.perform_oam_dma(0x02);

        assert_eq!(machine.total_cycles() - before, oam_base_cycles + 2,
            "DMC setup overlaps OAM; only its GET collision and OAM realignment extend the transfer");
        for offset in 0..=u8::MAX {
            machine.write_cpu(0x2003, offset);
            assert_eq!(
                machine.read_cpu(0x2004),
                offset ^ 0xA7,
                "OAM byte {offset} must be copied only after its GET slot is granted",
            );
        }
        assert!(machine.bus.bus_trace().iter().any(|entry| {
            entry.kind == CpuBusAccessKind::DmcRead && entry.address == 0xC000
        }));
    }

    #[test]
    fn nmi_edge_sampled_on_instruction_final_cycle_waits_for_next_instruction() {
        let mut image = nrom_with_program(&[
            0xEA,             // NOP: VBlank begins on this instruction's final cycle.
            0xA9, 0x42,       // LDA #$42 must execute before NMI entry.
            0x85, 0x00,       // STA $00
            0x4C, 0x04, 0x80, // JMP $8004
        ]);
        image[16 + 0x3FFA] = 0x00;
        image[16 + 0x3FFB] = 0x90;
        let mut machine = NesMachine::from_ines(&image).unwrap();

        machine.write_cpu(0x2000, 0x80); // Enable NMI before VBlank.
        machine.bus.tick_ppu(241 * 341 + 336);
        assert_eq!(machine.ppu_position(), (240, 336));

        assert_eq!(machine.step().unwrap().opcode, 0xEA);
        assert_eq!(machine.ppu_position(), (241, 1));
        assert_eq!(machine.step().unwrap().opcode, 0xA9,
            "the instruction after a final-cycle NMI edge must execute first");
        assert_eq!(machine.cpu_state().a, 0x42);

        assert_eq!(machine.step().unwrap().opcode, 0xFF,
            "the deferred NMI must be serviced at the following instruction boundary");
        assert_eq!(machine.cpu_state().pc, 0x9000);
    }

    #[test]
    fn nmi_edge_raised_during_oam_dma_is_delivered_after_dma_finishes() {
        let mut image = nrom_with_program(&[0xEA]);
        // Point the NMI vector at a distinct handler address.
        image[16 + 0x3FFA] = 0x00;
        image[16 + 0x3FFB] = 0x90;
        let mut machine = NesMachine::from_ines(&image).unwrap();

        // Start at VBlank scanline 241 dot 0, one dot before the PPU raises
        // its NMI edge. OAM DMA advances the PPU through that edge while the
        // CPU is stalled, so the request must remain pending until step().
        machine.write_cpu(0x2000, 0x80);
        machine.bus.tick_ppu(242 * 341);
        assert_eq!(machine.ppu_position(), (241, 0));
        machine.perform_oam_dma(0x02);

        assert_eq!(machine.cpu_state().pc, 0x8000,
            "OAM DMA must not execute the NMI handler while the CPU is stalled");
        let interrupt = machine.step().unwrap();
        assert_eq!(interrupt.cycles, 7);
        assert_eq!(machine.cpu_state().pc, 0x9000,
            "the PPU NMI edge raised during DMA must be serviced at the next CPU boundary");
    }

    #[test]
    fn machine_step_frame_advances_ppu_and_populates_framebuffer() {
        let image = nrom_with_program(&[0xEA, 0x4C, 0x00, 0x80]); // NOP; JMP $8000
        let mut machine = NesMachine::from_ines(&image).unwrap();
        let before = machine.total_cycles();
        let elapsed = machine.step_frame().unwrap();
        assert!(elapsed > 29_000 && elapsed < 31_000, "one NTSC frame should take about 29.8k CPU cycles");
        assert_eq!(machine.total_cycles() - before, u64::from(elapsed));
        assert_eq!(machine.framebuffer().len(), 256 * 240);
        assert_eq!(machine.framebuffer()[0], 0);
    }

    #[test]
    fn run_frames_batches_rendered_frames_and_reports_total_cycles() {
        let image = nrom_with_program(&[0xEA, 0x4C, 0x00, 0x80]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        let before = machine.total_cycles();
        assert_eq!(machine.run_frames(0).unwrap(), 0);
        let elapsed = machine.run_frames(3).unwrap();
        assert!(elapsed > 89_000 && elapsed < 93_000, "three NTSC frames should take about 89.4k cycles");
        assert_eq!(machine.total_cycles() - before, elapsed);
        assert_eq!(machine.framebuffer().len(), 256 * 240);
        assert!(machine.drain_audio_samples().len() > 2_000);
    }

    #[test]
    fn consecutive_frame_steps_keep_advancing_and_return_frame_sized_cycle_counts() {
        let image = nrom_with_program(&[0xEA, 0x4C, 0x00, 0x80]); // NOP; JMP $8000
        let mut machine = NesMachine::from_ines(&image).unwrap();
        let first = machine.step_frame().unwrap();
        let second = machine.step_frame().unwrap();
        assert!(first > 29_000 && first < 31_000);
        assert!(second > 29_000 && second < 31_000);
        assert_eq!(machine.total_cycles(), 7 + u64::from(first) + u64::from(second));
        assert_eq!(machine.framebuffer().len(), 256 * 240);
    }

    #[test]
    fn oam_dma_uses_513_cycles_when_the_write_finishes_on_even_parity() {
        // JMP takes 3 cycles, making the absolute STA to $4014 finish at cycle 16.
        let image = nrom_with_program(&[
            0x4C, 0x04, 0x80, // JMP $8004
            0xEA,             // skipped
            0xA9, 0x02,       // LDA #$02
            0x8D, 0x14, 0x40, // STA $4014
            0xEA,
        ]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        machine.step().unwrap();
        machine.step().unwrap();
        machine.step().unwrap();
        assert_eq!(machine.total_cycles(), 7 + 3 + 2 + 4 + 513);
    }

    #[test]
    fn cpu_instruction_cycles_advance_ppu_by_three_dots_each() {
        let image = nrom_with_program(&[0xEA, 0xEA]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.bus.ppu_position(), (261, 0));
        let result = machine.step().unwrap();
        assert_eq!(result.cycles, 2);
        assert_eq!(machine.bus.ppu_position(), (261, 6));
    }

    #[test]
    fn machine_exposes_controller_input_to_cpu_reads() {
        let image = nrom_with_program(&[
            0xA9, 0x01,             // LDA #1: strobe high
            0x8D, 0x16, 0x40,       // STA $4016
            0xA9, 0x00,             // LDA #0: strobe low, latch buttons
            0x8D, 0x16, 0x40,       // STA $4016
            0xAD, 0x16, 0x40,       // LDA $4016
        ]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        machine.set_controller_state(0, 0x01);
        for _ in 0..5 {
            machine.step().unwrap();
        }
        assert_eq!(machine.cpu_state().a, 0x41);
    }

    #[test]
    fn machine_exposes_apu_sample_output() {
        let image = nrom_with_program(&[0xEA]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert_eq!(machine.audio_sample(), 0.0);
        machine.write_cpu(0x4015, 0x01);
        machine.write_cpu(0x4000, 0x9F);
        machine.write_cpu(0x4002, 0x20);
        machine.write_cpu(0x4003, 0xF8);
        machine.clock_devices_for_cpu_cycles(1); // Advance from duty phase 0 to its first active phase.
        assert!(machine.audio_sample() > 0.0);
    }

    #[test]
    fn homebrew_rom_renders_chr_rom_tiles_through_the_full_machine() {
        const PRG_LEN: usize = 16 * 1024;
        const CHR_LEN: usize = 8 * 1024;
        let mut image = vec![0; 16 + PRG_LEN + CHR_LEN];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[5] = 1; // One 8 KiB CHR-ROM bank.

        // Tile 1 has every pixel set in its low bitplane for all eight rows.
        let chr_start = 16 + PRG_LEN;
        image[chr_start + 16..chr_start + 24].fill(0xFF);
        // The ROM loops forever at $8000 while the machine advances real PPU timing.
        image[16..20].copy_from_slice(&[0xEA, 0x4C, 0x00, 0x80]);
        image[16 + 0x3FFC] = 0x00;
        image[16 + 0x3FFD] = 0x80;

        let mut machine = NesMachine::from_ines(&image).unwrap();

        // Set universal backdrop and background palette through the CPU-visible PPU port.
        machine.write_cpu(0x2006, 0x3F);
        machine.write_cpu(0x2006, 0x00);
        machine.write_cpu(0x2007, 0x0F);
        machine.write_cpu(0x2007, 0x21);

        // Fill the visible nametable with tile 1, then enable background rendering.
        machine.write_cpu(0x2006, 0x20);
        machine.write_cpu(0x2006, 0x00);
        for _ in 0..(32 * 30) {
            machine.write_cpu(0x2007, 1);
        }
        machine.write_cpu(0x2001, 0x0A); // Background + leftmost 8 background pixels.

        let cycles = machine.step_frame().unwrap();
        assert!(cycles > 29_000 && cycles < 31_000);
        let frame = machine.framebuffer();
        assert_eq!(frame.len(), 256 * 240);
        assert_eq!(frame[0], 0x21, "tile pattern and palette must reach the output image");
        assert_eq!(frame[8 * 256 + 8], 0x21, "the rendered tile grid must fill the image");
        assert!(frame.iter().any(|&pixel| pixel != 0x0F), "framebuffer must not remain the blank backdrop");
        let mut palette = [[0, 0, 0, 255]; 64];
        palette[0x21] = [12, 34, 56, 255];
        let rgba = machine.framebuffer_rgba(&palette);
        assert_eq!(rgba.len(), 256 * 240 * 4);
        assert_eq!(&rgba[0..4], &[12, 34, 56, 255]);
    }

    #[test]
    fn battery_ram_round_trips_and_rejects_wrong_size_without_mutation() {
        let mut image = nrom_with_program(&[0xEA]);
        image[6] |= 0x02;
        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert!(machine.has_battery_backed_ram());
        machine.write_cpu(0x6000, 0xA5);
        machine.write_cpu(0x7FFF, 0x5A);

        let saved = machine.save_battery_ram().unwrap();
        assert_eq!(saved.len(), 8 * 1024);
        machine.write_cpu(0x6000, 0x11);
        assert!(matches!(
            machine.load_battery_ram(&saved[..saved.len() - 1]),
            Err(MachineError::InvalidSaveRamLength { expected: 8192, actual: 8191 })
        ));
        assert_eq!(machine.read_cpu(0x6000), 0x11, "invalid save must not partially mutate RAM");

        machine.load_battery_ram(&saved).unwrap();
        assert_eq!(machine.read_cpu(0x6000), 0xA5);
        assert_eq!(machine.read_cpu(0x7FFF), 0x5A);
    }

    #[test]
    fn cartridges_without_battery_flag_reject_persistent_save_operations() {
        let image = nrom_with_program(&[0xEA]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert!(!machine.has_battery_backed_ram());
        assert_eq!(machine.save_battery_ram(), Err(MachineError::NoBatteryRam));
        assert_eq!(machine.load_battery_ram(&vec![0; 8192]), Err(MachineError::NoBatteryRam));
    }

    #[test]
    fn machine_reports_invalid_cartridges() {
        assert!(matches!(NesMachine::from_ines(&[0; 8]), Err(MachineError::Cartridge(CartridgeError::TruncatedHeader))));
    }
    #[test]
    fn machine_runs_a_frame_from_cartridge_and_renders_programmed_chr_nametable_and_palette() {
        let image = nrom_with_program(&[0x4C, 0x00, 0x80]); // JMP $8000
        let mut machine = NesMachine::from_ines(&image).unwrap();

        // Program a visible background tile using the CPU-visible PPU registers.
        machine.write_cpu(0x2006, 0x3F);
        machine.write_cpu(0x2006, 0x00);
        machine.write_cpu(0x2007, 0x0F); // universal background color
        machine.write_cpu(0x2007, 0x21); // background palette 0, color 1
        machine.write_cpu(0x2006, 0x20);
        machine.write_cpu(0x2006, 0x00);
        machine.write_cpu(0x2007, 0x01); // nametable tile 0 -> pattern tile 1
        machine.write_cpu(0x2006, 0x00);
        machine.write_cpu(0x2006, 0x10); // pattern tile 1
        for _ in 0..8 {
            machine.write_cpu(0x2007, 0xFF); // make each row visibly use palette color 1
        }
        machine.write_cpu(0x2006, 0x20);
        machine.write_cpu(0x2006, 0x00); // restore the intended scroll address
        machine.write_cpu(0x2001, 0x0A); // enable background and its leftmost 8 pixels

        let cycles = machine.step_frame().unwrap();
        assert!(cycles > 0);
        assert_eq!(machine.framebuffer().len(), 256 * 240);
        assert!(machine.framebuffer().contains(&0x21), "the emulated frame should contain the programmed background palette color");
    }

    #[test]
    fn oam_dma_stalls_cpu_and_copies_the_requested_page_with_oam_wraparound() {
        // LDA #$02; STA $4014; NOP. The store requests DMA after a four-cycle instruction.
        let image = nrom_with_program(&[0xA9, 0x02, 0x8D, 0x14, 0x40, 0xEA]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        for offset in 0..=u8::MAX {
            machine.write_cpu(0x0200 | u16::from(offset), offset.wrapping_mul(3));
        }
        machine.write_cpu(0x2003, 0x80);
        assert_eq!(machine.step().unwrap().opcode, 0xA9);
        let before = machine.total_cycles();
        let result = machine.step().unwrap();
        assert_eq!(result.opcode, 0x8D);
        assert_eq!(machine.total_cycles() - before, 4 + 514,
            "the odd-parity store must include a 514-cycle OAM DMA stall");
        // DMA increments OAMADDR modulo 256, so the starting address is restored.
        assert_eq!(machine.read_cpu(0x2004), 0, "source byte $0200 reaches OAM address $80");
        machine.write_cpu(0x2003, 0xFF);
        assert_eq!(machine.read_cpu(0x2004), 0x7D, "source byte $027F reaches wrapped OAM address $FF");
    }

    #[test]
    fn dmc_stall_is_applied_before_later_ppu_register_accesses_in_the_instruction() {
        // LDA $2002 reads PPUSTATUS on its fourth bus cycle. Place vblank's
        // set edge 15 PPU dots after the initial beam position: without an
        // immediate DMC halt, the read happens before the edge; with the halt
        // after the opcode fetch, it happens after the edge.
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[
            0xAD, 0x02, 0x20,
        ])).unwrap();
        machine.bus.tick_ppu(241 * 341 + 326);
        assert_eq!(machine.bus.ppu_position(), (240, 326));

        machine.bus.write(0x4010, 0x00);
        machine.bus.write(0x4012, 0x00);
        machine.bus.write(0x4013, 0x00);
        machine.bus.write(0x4015, 0x10);

        machine.step().unwrap();
        assert_ne!(machine.cpu_state().a & 0x80, 0,
            "PPUSTATUS must see vblank after the DMC halt delays the operand accesses");
        assert_eq!(machine.total_cycles(), 4 + 3 + 7,
            "the four nominal instruction cycles and three initial DMC load cycles are both charged");
    }

    #[test]
    fn oam_dma_keeps_ppu_nmi_edges_live_while_the_cpu_is_stalled() {
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[0xEA])).unwrap();

        // Start at vblank dot 0 with NMI output enabled. The OAM DMA transfer
        // must keep clocking the PPU, allowing the dot-1 edge to reach the CPU
        // even though the CPU cannot execute an instruction during the transfer.
        machine.bus.tick_ppu(242 * 341);
        assert_eq!(machine.bus.ppu_position(), (241, 0));
        machine.write_cpu(0x2000, 0x80);

        let before = machine.total_cycles();
        machine.perform_oam_dma(0x00);

        assert!(machine.bus.take_nmi_request(),
            "the vblank NMI edge must remain pending when OAM DMA releases the CPU");
        assert!(machine.bus.read(0x2002) & 0x80 != 0,
            "vblank status must be set while OAM DMA advances through scanline 241");
        assert!(machine.total_cycles() - before >= 513,
            "the CPU remains stalled for the full OAM DMA transfer");
    }

    #[test]
    fn dmc_dma_stalls_cpu_and_advances_ppu_time() {
        let mut machine = NesMachine::from_ines(&nrom_with_program(&[0xEA])).unwrap();
        let before_cycles = machine.total_cycles();
        machine.write_cpu(0x4010, 0x00); // No DMC IRQ.
        machine.write_cpu(0x4012, 0x00); // $C000 sample start.
        machine.write_cpu(0x4013, 0x00); // One-byte sample.
        machine.write_cpu(0x4015, 0x10); // Enable DMC; its buffer is empty.
        let before_position = machine.bus.ppu_position();

        let result = machine.step().unwrap();
        assert_eq!(result.cycles, 2, "StepResult reports the opcode's nominal cycles");
        assert_eq!(machine.total_cycles() - before_cycles, 5, "the three-cycle initial DMC DMA load must be included in elapsed CPU time");
        let after_position = machine.bus.ppu_position();
        assert_eq!(after_position.0, before_position.0);
        assert_eq!(after_position.1 - before_position.1, 15, "the PPU must advance three dots for each of five elapsed CPU cycles");
    }

    #[test]
    fn mapper_irq_does_not_advance_when_ppu_rendering_is_disabled() {
        let mut image = vec![0; 16 + 2 * 16_384 + 8_192];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 2;
        image[5] = 1;
        image[6] = 0x40; // MMC3 / mapper 4.
        image[16 + 0x3FFC] = 0x00;
        image[16 + 0x3FFD] = 0x80;
        let mut machine = NesMachine::from_ines(&image).unwrap();

        machine.write_cpu(0xC000, 0); // IRQ latch = 0.
        machine.write_cpu(0xC001, 0); // Reload counter.
        machine.write_cpu(0xE001, 0); // Enable IRQ.
        machine.write_cpu(0x2001, 0); // Rendering disabled.
        machine.clock_devices_for_cpu_cycles(114); // Cross one scanline.
        assert!(!machine.bus.irq_pending(), "idle PPU scanlines must not clock the MMC3 IRQ counter");

        machine.write_cpu(0x2000, 0x10); // Background pattern table at $1000.
        machine.write_cpu(0x2001, 0x08); // Enable background rendering.
        machine.clock_devices_for_cpu_cycles(228); // Allow the filtered A12 edge on the following scanline.
        assert!(machine.bus.irq_pending(), "a qualified PPU A12 rising edge should clock the MMC3 IRQ counter");
    }

    #[test]
    fn machine_controller_api_latches_both_ports() {
        let image = nrom_with_program(&[0xEA, 0x4C, 0x00, 0x80]);
        let mut machine = NesMachine::from_ines(&image).unwrap();
        machine.set_controller_state(0, 0b1000_0001);
        machine.set_controller_state(1, 0b0000_0010);
        machine.write_cpu(0x4016, 1);
        machine.write_cpu(0x4016, 0);
        let a: Vec<u8> = (0..8).map(|_| machine.read_cpu(0x4016) & 1).collect();
        let b: Vec<u8> = (0..8).map(|_| machine.read_cpu(0x4017) & 1).collect();
        assert_eq!(a, [1, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(b, [0, 1, 0, 0, 0, 0, 0, 0]);
    }
    #[test]
    fn selected_ppu_startup_phase_survives_console_reset() {
        let image = nrom_with_program(&[0xEA]);
        let mut machine = NesMachine::from_ines_with_ppu_startup_phase(&image, 2).unwrap();
        assert_eq!(machine.ppu_position(), (261, 0));
        assert_eq!(machine.ppu_startup_phase, 2);
        machine.reset();
        assert_eq!(machine.ppu_position(), (261, 0));
        assert_eq!(machine.ppu_startup_phase, 2);
    }

    #[test]
    fn machine_reset_recovers_jammed_cpu_and_preserves_ram_cartridge_and_audio_rate() {
        let image = nrom_with_program(&[0x02, 0xEA]); // KIL followed by NOP.
        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert!(machine.set_audio_sample_rate(48_000));
        machine.write_cpu(0x0005, 0xA5);
        assert_eq!(machine.step().unwrap().opcode, 0x02);
        assert_eq!(machine.step().unwrap().opcode, 0x02, "KIL keeps the CPU jammed");

        let before_reset_cycles = machine.total_cycles();
        machine.reset();
        assert_eq!(machine.cpu_state().pc, 0x8000, "reset vector is reloaded");
        assert_eq!(machine.total_cycles(), before_reset_cycles + 7);
        assert_eq!(machine.read_cpu(0x0005), 0xA5, "CPU RAM survives a reset");
        assert_eq!(machine.read_cpu(0x8000), 0x02, "the cartridge remains attached");
        assert!(machine.set_audio_sample_rate(48_000), "host audio rate remains usable after reset");
        assert_eq!(machine.audio_sample_rate(), 48_000);
        assert_eq!(machine.ppu_position(), (261, 0), "PPU timing returns to pre-render");
        assert_eq!(machine.step().unwrap().opcode, 0x02);
    }


}


#[cfg(test)]
mod pcm_frontend_tests {
    use super::*;

    #[test]
    fn machine_exposes_configurable_pcm_batches_while_emulating() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        // Infinite JMP $8000 loop so stepping remains deterministic.
        image[16] = 0x4C;
        image[17] = 0x00;
        image[18] = 0x80;
        image[16 + 0x3FFC] = 0x00;
        image[16 + 0x3FFD] = 0x80;

        let mut machine = NesMachine::from_ines(&image).unwrap();
        assert!(!machine.set_audio_sample_rate(1));
        assert!(machine.set_audio_sample_rate(48_000));
        for _ in 0..200 {
            machine.step().unwrap();
        }
        let samples = machine.drain_audio_samples();
        assert!(!samples.is_empty());
        assert!(samples.iter().all(|sample| sample.is_finite() && (0.0..=1.0).contains(sample)));
        assert!(machine.drain_audio_samples().is_empty());
    }


}


#[cfg(test)]
mod battery_ram_persistence_regressions {
    use super::*;

    fn battery_test_rom() -> Vec<u8> {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[6] = 0x02;
        image[16] = 0xEA;
        image[16 + 0x3FFC] = 0x00;
        image[16 + 0x3FFD] = 0x80;
        image
    }

    #[test]
    fn battery_ram_round_trips_and_survives_console_reset() {
        let mut image = battery_test_rom();
        image[6] |= 0x02; // Battery-backed PRG-RAM.
        let mut machine = NesMachine::from_ines(&image).unwrap();

        machine.write_cpu(0x6000, 0xA5);
        machine.write_cpu(0x7FFF, 0x5A);
        let saved = machine.save_battery_ram().unwrap();
        assert_eq!(saved.len(), 8 * 1024);
        assert_eq!(saved[0], 0xA5);
        assert_eq!(saved[0x1FFF], 0x5A);

        machine.write_cpu(0x6000, 0x00);
        machine.load_battery_ram(&saved).unwrap();
        assert_eq!(machine.read_cpu(0x6000), 0xA5);

        machine.reset();
        assert_eq!(machine.read_cpu(0x6000), 0xA5,
            "console reset must preserve battery-backed cartridge RAM");
        assert_eq!(machine.read_cpu(0x7FFF), 0x5A);
    }

    #[test]
    fn battery_ram_rejects_wrong_payload_length_without_partial_restore() {
        let image = battery_test_rom();
        let mut machine = NesMachine::from_ines(&image).unwrap();
        machine.write_cpu(0x6000, 0xCC);

        let error = machine.load_battery_ram(&[0x11, 0x22]).unwrap_err();
        assert_eq!(error, MachineError::InvalidSaveRamLength {
            expected: 8 * 1024,
            actual: 2,
        });
        assert_eq!(machine.read_cpu(0x6000), 0xCC,
            "invalid save payload must not modify cartridge RAM");
    }
}


#[cfg(test)]
mod dmc_clock_integration_regressions {
    use super::*;

    #[test]
    fn dmc_dma_stalls_advance_the_shared_cpu_ppu_timeline() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        image[4] = 1;
        image[16..16 + 16_384].fill(0xA5);
        image[16 + 0x3FFC] = 0x00;
        image[16 + 0x3FFD] = 0x80;

        let mut machine = NesMachine::from_ines(&image).unwrap();
        machine.bus.write(0x4010, 0x00);
        machine.bus.write(0x4012, 0x00); // Sample starts at $C000.
        machine.bus.write(0x4013, 0x01); // 17 bytes.
        machine.bus.write(0x4015, 0x10);

        let before_ppu = machine.ppu_position();
        machine.clock_devices_for_cpu_cycles(1);

        assert_eq!(machine.ppu_position(), (261, 12),
            "one elapsed CPU cycle plus three initial DMC load cycles must advance twelve PPU dots from pre-render");
        assert_ne!(machine.ppu_position(), before_ppu,
            "the DMC stall must advance the same PPU timeline as ordinary CPU cycles");
    }
}



#[cfg(test)]
mod color_emphasis_output_regressions {
    use super::*;

    fn test_machine() -> NesMachine {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16 + 0x3FFC] = 0x00;
        image[16 + 0x3FFD] = 0x80;
        NesMachine::from_ines(&image).unwrap()
    }

    fn palette() -> [[u8; 4]; 64] {
        let mut palette = [[0, 0, 0, 255]; 64];
        palette[0] = [200, 160, 120, 255];
        palette
    }

    #[test]
    fn framebuffer_conversion_preserves_palette_without_emphasis() {
        let machine = test_machine();
        assert_eq!(&machine.framebuffer_rgba(&palette())[..4], &[200, 160, 120, 255]);
    }

    #[test]
    fn red_emphasis_attenuates_green_and_blue_only() {
        let mut machine = test_machine();
        machine.write_cpu(0x2001, 0x20);
        assert_eq!(&machine.framebuffer_rgba(&palette())[..4], &[200, 120, 90, 255]);
    }

    #[test]
    fn combined_emphasis_attenuates_each_channel_affected_by_another_primary() {
        let mut machine = test_machine();
        machine.write_cpu(0x2001, 0x60); // Red + green emphasis.
        assert_eq!(&machine.framebuffer_rgba(&palette())[..4], &[150, 120, 90, 255]);
    }

    #[test]
    fn color_emphasis_does_not_change_the_indexed_framebuffer_contract() {
        let mut machine = test_machine();
        let before = machine.framebuffer().to_vec();
        machine.write_cpu(0x2001, 0xE0);
        assert_eq!(machine.framebuffer(), before,
            "PPUMASK emphasis is a host RGB output effect, not a palette-index mutation");
    }
    #[test]
    fn dmc_irq_raised_during_stolen_cycles_is_polled_at_the_next_instruction_boundary() {
        let mut image = vec![0; 16 + 16_384];
        image[0..4].copy_from_slice(b"NES\x1A");
        image[4] = 1;
        image[16..19].copy_from_slice(&[0x58, 0xEA, 0xEA]); // CLI; NOP; NOP.
        image[16 + 0x3FFC] = 0x00;
        image[16 + 0x3FFD] = 0x80;
        image[16 + 0x3FFE] = 0x00;
        image[16 + 0x3FFF] = 0x90;
        image[16 + 0x1000] = 0xEA;
        let mut machine = NesMachine::from_ines(&image).unwrap();

        assert_eq!(machine.step().unwrap().opcode, 0x58,
            "CLI must clear I before the DMC IRQ is generated");
        machine.write_cpu(0x4010, 0x8F); // Enable DMC IRQ and use the fastest rate.
        machine.write_cpu(0x4012, 0x00);
        machine.write_cpu(0x4013, 0x00); // One-byte sample.
        machine.write_cpu(0x4015, 0x10);

        let interrupted_instruction = machine.step().unwrap();
        assert_eq!(interrupted_instruction.opcode, 0xEA);
        assert!(machine.bus.irq_pending(),
            "the final DMC GET must assert IRQ while the CPU is stalled");

        let irq = machine.step().unwrap();
        assert_eq!(irq.opcode, 0xFE,
            "the CPU must observe the DMC IRQ on the next instruction poll");
        assert_eq!(machine.cpu_state().pc, 0x9000);
        assert_ne!(machine.cpu_state().status & crate::cpu::flags::INTERRUPT_DISABLE, 0);
    }

}
