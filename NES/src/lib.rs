#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Hardware-focused NES emulation components, without a GUI dependency.

/// Audio register model, approximate channel synthesis, and host-rate PCM output.
pub mod apu;
/// Cartridge image parsing and mapper support.
pub mod cartridge;
/// CPU address bus and internal RAM.
pub mod bus;
/// Ricoh 2A03 CPU implementation.
pub mod cpu;
/// Minimal top-level machine wrapper.
pub mod machine;
/// Frontend-independent API for embedding a running game in PeaceEmu.
pub mod session;
/// PPU register and memory model.
pub mod ppu;

/// Current module version.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Stable system identifier for PeaceEmu registration.
pub const SYSTEM_ID: &str = "nes";
