//! Dot-accurate regression tests for the NTSC VBlank/NMI boundary.

use peaceemu_nes::ppu::Ppu;

const DOTS_PER_SCANLINE: u32 = 341;
const LINES_BEFORE_VBLANK: u32 = 242;

/// Starts at scanline 241, dot 0 (the dot immediately before VBlank is set).
fn at_vblank_start() -> Ppu {
    let mut ppu = Ppu::new();
    ppu.tick(LINES_BEFORE_VBLANK * DOTS_PER_SCANLINE);
    assert_eq!(ppu.position(), (241, 0));
    ppu
}

#[test]
fn status_read_on_dot_1_observes_the_race_and_cancels_nmi() {
    let mut ppu = at_vblank_start();
    ppu.cpu_write_register(0, 0x80, None);

    ppu.tick(1);
    assert_eq!(ppu.position(), (241, 1));
    assert_eq!(ppu.cpu_read_register(2, None) & 0x80, 0,
        "a status read on the VBlank-set dot must still observe bit 7 low");
    assert!(!ppu.take_nmi_request(),
        "the NMI output must not rise during the one-dot propagation delay");

    // Reading PPUSTATUS clears VBlank, so no NMI edge may survive to dot 2.
    ppu.tick(1);
    assert_eq!(ppu.position(), (241, 2));
    assert!(!ppu.take_nmi_request());
}

#[test]
fn status_read_on_dot_0_suppresses_the_upcoming_vblank_and_nmi() {
    let mut ppu = at_vblank_start();
    ppu.cpu_write_register(0, 0x80, None);

    assert_eq!(ppu.cpu_read_register(2, None) & 0x80, 0);
    ppu.tick(1);
    ppu.tick(1);

    assert_eq!(ppu.position(), (241, 2));
    assert_eq!(ppu.cpu_read_register(2, None) & 0x80, 0);
    assert!(!ppu.take_nmi_request());
}

#[test]
fn enabling_nmi_at_vblank_dot_1_respects_the_propagation_delay() {
    let mut ppu = at_vblank_start();

    ppu.tick(1);
    assert_eq!(ppu.position(), (241, 1));
    ppu.cpu_write_register(0, 0x80, None);
    assert!(!ppu.take_nmi_request(),
        "enabling NMI on the VBlank-set dot must not bypass the PPU delay");

    ppu.tick(1);
    assert_eq!(ppu.position(), (241, 2));
    assert!(ppu.take_nmi_request(),
        "the NMI edge must be generated once the propagation delay expires");
    assert!(!ppu.take_nmi_request());
}
