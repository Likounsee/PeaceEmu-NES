//! Host-facing session API for embedding the NES core inside PeaceEmu.

#[cfg(any(test, feature = "ppu-timing-diagnostics"))]
use crate::bus::CpuBusTraceEntry;
use crate::cpu::CpuState;
use crate::machine::{MachineError, NesMachine};
use bincode::Options;

/// Native NES output dimensions.
pub const NES_FRAME_WIDTH: usize = 256;
/// Native NES output height.
pub const NES_FRAME_HEIGHT: usize = 240;
/// Number of indexed pixels in one NES frame.
pub const NES_FRAME_PIXELS: usize = NES_FRAME_WIDTH * NES_FRAME_HEIGHT;
/// Default audio output rate used by PeaceEmu when no preference is configured.
pub const DEFAULT_AUDIO_SAMPLE_RATE: u32 = 44_100;

/// Host-configurable options for a running NES module.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NesConfig {
    /// Mono PCM sample rate in hertz; supported range is 8,000 through 192,000.
    pub audio_sample_rate: u32,
}

impl Default for NesConfig {
    fn default() -> Self {
        Self { audio_sample_rate: DEFAULT_AUDIO_SAMPLE_RATE }
    }
}

/// Errors returned while creating or advancing an embedded NES session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NesSessionError {
    /// The requested audio sample rate is outside the supported range.
    UnsupportedAudioSampleRate(u32),
    /// The underlying emulation core could not load or advance the game.
    Machine(MachineError),
    /// A saved state is malformed, oversized, or internally inconsistent.
    InvalidSaveState,
}

impl core::fmt::Display for NesSessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedAudioSampleRate(rate) => {
                write!(f, "unsupported NES audio sample rate: {rate}")
            }
            Self::Machine(error) => write!(f, "{error}"),
            Self::InvalidSaveState => f.write_str("invalid or incompatible NES save state"),
        }
    }
}

impl std::error::Error for NesSessionError {}

impl From<MachineError> for NesSessionError {
    fn from(value: MachineError) -> Self {
        Self::Machine(value)
    }
}

/// Video and audio produced by one emulated frame.
///
/// Pixels are 6-bit NES palette indices in row-major 256x240 order. The host renderer
/// chooses its own output palette, scaling, shaders, and presentation surface. Audio is
/// mono floating-point PCM, with samples at the rate configured for the session.
#[derive(Clone, Debug, PartialEq)]
pub struct NesFrameOutput {
    /// Native video width in pixels.
    pub width: usize,
    /// Native video height in pixels.
    pub height: usize,
    /// Row-major 6-bit NES palette indices; length is always 256 * 240.
    pub palette_indices: Vec<u8>,
    /// Mono floating-point PCM samples generated since the previous frame.
    pub audio_samples: Vec<f32>,
    /// Nominal CPU cycles executed while advancing this frame.
    pub cpu_cycles: u32,
}

/// One loaded NES game, controlled and presented by the PeaceEmu host application.
///
/// This type deliberately creates no window, event loop, audio device, or file dialog.
/// PeaceEmu owns presentation, input mapping, configuration UI, file paths, and persistence.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct NesSession {
    machine: NesMachine,
    config: NesConfig,
    rom_fingerprint: u64,
}

impl NesSession {
    fn fingerprint(image: &[u8]) -> u64 {
        // Stable FNV-1a identity check: save states from a different ROM are
        // rejected before the decoded machine can replace the active session.
        image.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3)
        })
    }

    /// Maximum accepted serialized machine state size (ROM plus live hardware state).
    pub const MAX_SAVE_STATE_BYTES: u64 = 64 * 1024 * 1024;
    const SAVE_STATE_HEADER: &'static [u8; 8] = b"PENESST\0";
    const SAVE_STATE_FORMAT_VERSION: u16 = 1;

    /// Serializes the complete CPU, bus, cartridge, PPU, APU, and host audio state.
    ///
    /// Diagnostic traces are intentionally excluded; all state that affects future
    /// emulation is retained. The payload is bounded so malformed host state files
    /// cannot request unbounded allocations during restore.
    pub fn save_state(&self) -> Result<Vec<u8>, NesSessionError> {
        let payload = bincode::DefaultOptions::new()
            .with_limit(Self::MAX_SAVE_STATE_BYTES - 10)
            .reject_trailing_bytes()
            .serialize(self)
            .map_err(|_| NesSessionError::InvalidSaveState)?;
        let mut state = Vec::with_capacity(payload.len() + 10);
        state.extend_from_slice(Self::SAVE_STATE_HEADER);
        state.extend_from_slice(&Self::SAVE_STATE_FORMAT_VERSION.to_le_bytes());
        state.extend_from_slice(&payload);
        if state.len() as u64 > Self::MAX_SAVE_STATE_BYTES {
            return Err(NesSessionError::InvalidSaveState);
        }
        Ok(state)
    }

    /// Restores a complete machine snapshot atomically.
    ///
    /// The current session is not modified unless the entire payload decodes and
    /// its duplicated host audio configuration agrees with the emulated APU state.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), NesSessionError> {
        if data.len() < 10
            || data.len() as u64 > Self::MAX_SAVE_STATE_BYTES
            || &data[..8] != Self::SAVE_STATE_HEADER
            || u16::from_le_bytes([data[8], data[9]]) != Self::SAVE_STATE_FORMAT_VERSION
        {
            return Err(NesSessionError::InvalidSaveState);
        }
        let restored: Self = bincode::DefaultOptions::new()
            .with_limit(Self::MAX_SAVE_STATE_BYTES - 10)
            .reject_trailing_bytes()
            .deserialize(&data[10..])
            .map_err(|_| NesSessionError::InvalidSaveState)?;
        if !(8_000..=192_000).contains(&restored.config.audio_sample_rate)
            || restored.machine.audio_sample_rate() != restored.config.audio_sample_rate
            || restored.rom_fingerprint != self.rom_fingerprint
        {
            return Err(NesSessionError::InvalidSaveState);
        }
        *self = restored;
        Ok(())
    }

    /// Loads an iNES ROM and applies the host-selected audio configuration.
    pub fn load(image: &[u8], config: NesConfig) -> Result<Self, NesSessionError> {
        let mut machine = NesMachine::from_ines(image)?;
        if !machine.set_audio_sample_rate(config.audio_sample_rate) {
            return Err(NesSessionError::UnsupportedAudioSampleRate(config.audio_sample_rate));
        }
        Ok(Self {
            machine,
            config,
            rom_fingerprint: Self::fingerprint(image),
        })
    }

    /// Returns the configuration currently applied to this session.
    pub fn config(&self) -> NesConfig {
        self.config
    }

    /// Applies a new PCM output rate, keeping the public configuration in sync.
    ///
    /// Supported rates are 8,000 through 192,000 samples per second. Changing to a
    /// different valid rate clears queued samples and resets resampling state; applying
    /// the already-active rate is a no-op. An invalid rate leaves configuration unchanged.
    pub fn set_audio_sample_rate(&mut self, sample_rate: u32) -> bool {
        if !self.machine.set_audio_sample_rate(sample_rate) {
            return false;
        }
        self.config.audio_sample_rate = sample_rate;
        true
    }

    /// Resets CPU and volatile devices while retaining the loaded cartridge and audio rate.
    ///
    /// Cartridge RAM is preserved like a console reset. The next frame advances from the reset
    /// vector and returns a fresh frame/audio batch.
    pub fn reset(&mut self) {
        self.machine.reset();
    }

    /// Returns the current CPU register snapshot for frontend diagnostics.
    pub fn cpu_state(&self) -> CpuState {
        self.machine.cpu_state()
    }

    /// Returns the current PPU scanline and dot for frontend diagnostics.
    pub fn ppu_position(&self) -> (u16, u16) {
        self.machine.ppu_position()
    }

    /// Applies the eight NES controller buttons to player port 1 or 2.
    ///
    /// Bits from least to most significant are A, B, Select, Start, Up, Down, Left, Right.
    /// Port numbers are zero-based: 0 is player one, 1 is player two.
    pub fn set_controller_state(&mut self, port: usize, buttons: u8) {
        self.machine.set_controller_state(port, buttons);
    }

    /// Advances the emulation by one frame and returns host-ready video and audio data.
    pub fn advance_frame(&mut self) -> Result<NesFrameOutput, NesSessionError> {
        let cpu_cycles = self.machine.step_frame()?;
        let palette_indices = self.machine.framebuffer().to_vec();
        let audio_samples = self.machine.drain_audio_samples();
        Ok(NesFrameOutput {
            width: NES_FRAME_WIDTH,
            height: NES_FRAME_HEIGHT,
            palette_indices,
            audio_samples,
            cpu_cycles,
        })
    }

    /// Returns recent CPU-bus transactions for frontend debugger and DMA diagnostics.
    ///
    /// The bounded trace contains CPU reads/writes and DMC DMA reads in observation order.
    #[cfg(any(test, feature = "ppu-timing-diagnostics"))]
    pub fn bus_trace(&self) -> Vec<CpuBusTraceEntry> {
        self.machine.bus_trace()
    }

    /// Clears diagnostic bus history without changing the running game.
    #[cfg(any(test, feature = "ppu-timing-diagnostics"))]
    pub fn clear_bus_trace(&mut self) {
        self.machine.clear_bus_trace();
    }

    /// Returns whether the current cartridge declares battery-backed RAM.
    pub fn has_battery_backed_ram(&self) -> bool {
        self.machine.has_battery_backed_ram()
    }

    /// Exports battery-backed RAM for the host application to save to its chosen storage.
    pub fn save_battery_ram(&self) -> Result<Vec<u8>, NesSessionError> {
        Ok(self.machine.save_battery_ram()?)
    }

    /// Restores battery-backed RAM loaded by the host application.
    pub fn load_battery_ram(&mut self, data: &[u8]) -> Result<(), NesSessionError> {
        Ok(self.machine.load_battery_ram(data)?)
    }

    /// Returns the number of nominal CPU cycles since the machine was reset.
    pub fn total_cycles(&self) -> u64 {
        self.machine.total_cycles()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::CpuBusAccessKind;

    fn looping_rom() -> Vec<u8> {
        let mut rom = vec![0; 16 + 16 * 1024];
        rom[..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        // JMP $8000: a deterministic loop while the PPU advances.
        rom[16] = 0x4C;
        rom[17] = 0x00;
        rom[18] = 0x80;
        rom[16 + 0x3FFC] = 0x00;
        rom[16 + 0x3FFD] = 0x80;
        rom
    }

    fn battery_rom() -> Vec<u8> {
        let mut rom = looping_rom();
        rom[6] |= 0x02; // Battery-backed PRG-RAM.
        rom
    }

    #[test]
    fn battery_ram_round_trips_and_rejects_wrong_lengths_atomically() {
        let mut session = NesSession::load(&battery_rom(), NesConfig::default()).unwrap();
        assert!(session.has_battery_backed_ram());
        let initial = session.save_battery_ram().unwrap();
        assert!(!initial.is_empty());

        let replacement = vec![0xA5; initial.len()];
        session.load_battery_ram(&replacement).unwrap();
        assert_eq!(session.save_battery_ram().unwrap(), replacement);

        assert!(matches!(
            session.load_battery_ram(&replacement[..replacement.len() - 1]),
            Err(NesSessionError::Machine(MachineError::InvalidSaveRamLength { .. }))
        ));
        assert_eq!(session.save_battery_ram().unwrap(), replacement, "invalid data must not partially overwrite RAM");
    }

    #[test]
    fn battery_ram_requires_battery_flag_and_survives_console_reset() {
        let ordinary = NesSession::load(&looping_rom(), NesConfig::default()).unwrap();
        assert!(!ordinary.has_battery_backed_ram());
        assert!(matches!(ordinary.save_battery_ram(), Err(NesSessionError::Machine(MachineError::NoBatteryRam))));

        let mut session = NesSession::load(&battery_rom(), NesConfig::default()).unwrap();
        let payload = vec![0x3C; session.save_battery_ram().unwrap().len()];
        session.load_battery_ram(&payload).unwrap();
        session.reset();
        assert_eq!(session.save_battery_ram().unwrap(), payload);
    }

    #[test]
    fn host_session_returns_frame_and_audio_without_frontend_dependencies() {
        let mut session = NesSession::load(&looping_rom(), NesConfig::default()).unwrap();
        let frame = session.advance_frame().unwrap();
        assert_eq!((frame.width, frame.height), (256, 240));
        assert_eq!(frame.palette_indices.len(), NES_FRAME_PIXELS);
        assert!(frame.palette_indices.iter().all(|pixel| *pixel < 64));
        assert!(frame.cpu_cycles > 0);
        // The current APU model may emit no samples for some short test fixtures;
        // audio is nevertheless exposed as a host-owned PCM buffer.
        let _pcm: Vec<f32> = frame.audio_samples;
    }

    #[test]
    fn host_can_configure_audio_rate_and_reject_invalid_rate() {
        let config = NesConfig { audio_sample_rate: 48_000 };
        let session = NesSession::load(&looping_rom(), config).unwrap();
        assert_eq!(session.config(), config);
        assert!(matches!(
            NesSession::load(&looping_rom(), NesConfig { audio_sample_rate: 7_999 }),
            Err(NesSessionError::UnsupportedAudioSampleRate(7_999))
        ));
    }

    #[test]
    fn runtime_audio_rate_updates_config_only_after_validation() {
        let mut session = NesSession::load(&looping_rom(), NesConfig::default()).unwrap();
        assert!(!session.set_audio_sample_rate(7_999));
        assert_eq!(session.config().audio_sample_rate, DEFAULT_AUDIO_SAMPLE_RATE);
        assert!(session.set_audio_sample_rate(48_000));
        assert_eq!(session.config().audio_sample_rate, 48_000);
    }

    #[test]
    fn reset_restores_cpu_and_ppu_startup_state_without_unloading_rom() {
        let mut session = NesSession::load(&looping_rom(), NesConfig { audio_sample_rate: 48_000 }).unwrap();
        session.advance_frame().unwrap();
        let cycles_before_reset = session.total_cycles();
        assert!(cycles_before_reset > 7);

        session.reset();

        assert_eq!(session.cpu_state().pc, 0x8000);
        assert_eq!(session.ppu_position(), (261, 0));
        assert_eq!(session.config().audio_sample_rate, 48_000);
        assert_eq!(session.total_cycles(), cycles_before_reset + 7);
        assert_eq!(session.advance_frame().unwrap().palette_indices.len(), NES_FRAME_PIXELS);
    }

    #[test]
    fn frame_audio_samples_are_finite_and_normalized() {
        let mut session = NesSession::load(&looping_rom(), NesConfig::default()).unwrap();
        let frame = session.advance_frame().unwrap();

        assert!(
            frame.audio_samples.iter().all(|sample| sample.is_finite()),
            "the host PCM buffer must never contain NaN or infinity"
        );
        assert!(
            frame.audio_samples.iter().all(|sample| (-1.0..=1.0).contains(sample)),
            "the host PCM buffer must remain normalized"
        );
    }

    #[test]
    fn supported_audio_rate_boundaries_are_inclusive() {
        for sample_rate in [8_000, 192_000] {
            let config = NesConfig { audio_sample_rate: sample_rate };
            let mut session = NesSession::load(&looping_rom(), config).unwrap();
            assert_eq!(session.config().audio_sample_rate, sample_rate);
            assert!(session.set_audio_sample_rate(sample_rate));
            assert_eq!(session.config().audio_sample_rate, sample_rate);
        }
    }

    #[test]
    fn host_session_exposes_controller_input() {
        let mut session = NesSession::load(&looping_rom(), NesConfig::default()).unwrap();
        session.set_controller_state(0, 0b1000_0001);
        session.set_controller_state(1, 0b0000_0010);
        assert_eq!(session.total_cycles(), 7);
    }

    #[test]
    fn full_machine_save_state_restores_deterministic_execution() {
        let config = NesConfig { audio_sample_rate: 48_000 };
        let mut session = NesSession::load(&looping_rom(), config).unwrap();
        session.set_controller_state(0, 0b1000_0001);
        session.advance_frame().unwrap();
        let snapshot = session.save_state().unwrap();
        let expected_cycles = session.total_cycles();
        let expected_ppu = session.ppu_position();
        let expected_cpu = session.cpu_state();

        let mut expected = NesSession::load(&looping_rom(), config).unwrap();
        expected.load_state(&snapshot).unwrap();
        let expected_frame = expected.advance_frame().unwrap();

        session.advance_frame().unwrap();
        session.load_state(&snapshot).unwrap();
        assert_eq!(session.total_cycles(), expected_cycles);
        assert_eq!(session.ppu_position(), expected_ppu);
        assert_eq!(session.cpu_state(), expected_cpu);
        assert_eq!(session.config(), config);

        let restored_frame = session.advance_frame().unwrap();
        assert_eq!(restored_frame.palette_indices, expected_frame.palette_indices);
        assert_eq!(restored_frame.audio_samples, expected_frame.audio_samples);
        assert_eq!(restored_frame.cpu_cycles, expected_frame.cpu_cycles);
    }

    #[test]
    fn save_state_is_bound_to_the_original_cartridge_image() {
        let mut session = NesSession::load(&looping_rom(), NesConfig::default()).unwrap();
        session.advance_frame().unwrap();
        let snapshot = session.save_state().unwrap();
        assert!(snapshot.starts_with(b"PENESST\0"));

        let mut different_rom = looping_rom();
        different_rom[0x100] ^= 0x5A;
        let mut other = NesSession::load(&different_rom, NesConfig::default()).unwrap();
        assert_eq!(other.load_state(&snapshot), Err(NesSessionError::InvalidSaveState));
    }

    #[test]
    fn malformed_save_state_is_rejected_without_mutating_the_running_session() {
        let mut session = NesSession::load(&looping_rom(), NesConfig::default()).unwrap();
        session.advance_frame().unwrap();
        let cycles = session.total_cycles();
        let ppu = session.ppu_position();
        assert_eq!(
            session.load_state(b"not a valid NES state"),
            Err(NesSessionError::InvalidSaveState)
        );
        assert_eq!(session.total_cycles(), cycles);
        assert_eq!(session.ppu_position(), ppu);
    }
    #[test]
    fn session_exposes_and_clears_machine_bus_trace() {
        let mut session = NesSession::load(&looping_rom(), NesConfig::default()).unwrap();
        session.advance_frame().unwrap();
        let trace = session.bus_trace();
        assert!(!trace.is_empty(), "frontend session should expose CPU fetch traffic");
        assert!(trace.iter().any(|entry| entry.kind == CpuBusAccessKind::Read));
        session.clear_bus_trace();
        assert!(session.bus_trace().is_empty());
        assert!(session.total_cycles() > 0,
            "clearing the trace must not reset emulation progress");
    }

    #[test]
    fn audio_rate_accepts_both_documented_limits_and_rejects_above_maximum() {
        let mut session = NesSession::load(&looping_rom(), NesConfig::default()).unwrap();
        assert!(session.set_audio_sample_rate(8_000));
        assert_eq!(session.config().audio_sample_rate, 8_000);
        assert!(session.set_audio_sample_rate(192_000));
        assert_eq!(session.config().audio_sample_rate, 192_000);
        assert!(!session.set_audio_sample_rate(192_001));
        assert_eq!(session.config().audio_sample_rate, 192_000, "invalid rate must leave the last valid configuration unchanged");
    }

    #[test]
    fn consecutive_frames_have_fixed_video_shape_and_positive_cpu_work() {
        let mut session = NesSession::load(&looping_rom(), NesConfig::default()).unwrap();
        let first = session.advance_frame().unwrap();
        let second = session.advance_frame().unwrap();
        assert_eq!(first.palette_indices.len(), NES_FRAME_PIXELS);
        assert_eq!(second.palette_indices.len(), NES_FRAME_PIXELS);
        assert!(first.cpu_cycles > 0 && second.cpu_cycles > 0);
        assert!(session.total_cycles() > u64::from(first.cpu_cycles));
        assert!(first.palette_indices.iter().chain(&second.palette_indices).all(|pixel| *pixel < 64));
    }

}


#[cfg(test)]
mod audio_rate_noop_regressions {
    use super::*;

    #[test]
    fn applying_current_rate_is_successful_and_keeps_host_config_unchanged() {
        let config = NesConfig { audio_sample_rate: 48_000 };
        let mut session = NesSession::load(&looping_rom_for_audio_rate_test(), config).unwrap();
        assert!(session.set_audio_sample_rate(48_000));
        assert_eq!(session.config(), config);
        assert_eq!(session.machine.audio_sample_rate(), 48_000);
    }

    fn looping_rom_for_audio_rate_test() -> Vec<u8> {
        let mut rom = vec![0; 16 + 16 * 1024];
        rom[..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        rom[4] = 1;
        rom[16..19].copy_from_slice(&[0x4C, 0x00, 0x80]);
        rom[16 + 0x3FFC] = 0x00;
        rom[16 + 0x3FFD] = 0x80;
        rom
    }
}
