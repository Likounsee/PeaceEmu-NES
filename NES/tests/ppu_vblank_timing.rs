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
fn status_read_on_dot_1_returns_old_value_but_keeps_vblank_nmi() {
    let mut ppu = at_vblank_start();
    ppu.cpu_write_register(0, 0x80, None);
    ppu.tick(1);
    assert_eq!(ppu.position(), (241, 1));
    assert_eq!(ppu.cpu_read_register(2, None) & 0x80, 0);
    assert!(!ppu.take_nmi_request(), "NMI output still has its one-dot propagation delay");
    ppu.tick(1);
    assert_eq!(ppu.position(), (241, 2));
    assert!(ppu.take_nmi_request(), "the dot-1 read must not cancel the VBlank set");
    assert!(!ppu.take_nmi_request());
}

#[test]
fn status_read_on_dot_0_suppresses_vblank() {
    let mut ppu = at_vblank_start();
    ppu.cpu_write_register(0, 0x80, None);
    assert_eq!(ppu.cpu_read_register(2, None) & 0x80, 0);
    ppu.tick(2);
    assert_eq!(ppu.position(), (241, 2));
    assert_eq!(ppu.cpu_read_register(2, None) & 0x80, 0);
    assert!(!ppu.take_nmi_request());
}

#[test]
fn status_read_on_dot_2_observes_vblank_and_cancels_nmi() {
    let mut ppu = at_vblank_start();
    ppu.cpu_write_register(0, 0x80, None);
    ppu.tick(2);
    assert_eq!(ppu.position(), (241, 2));
    assert_ne!(ppu.cpu_read_register(2, None) & 0x80, 0);
    assert!(!ppu.take_nmi_request());
}

#[test]
fn status_read_on_dot_3_does_not_retract_an_already_raised_nmi_edge() {
    let mut ppu = at_vblank_start();
    ppu.cpu_write_register(0, 0x80, None);
    ppu.tick(3);
    assert_eq!(ppu.position(), (241, 3));
    assert_ne!(ppu.cpu_read_register(2, None) & 0x80, 0);
    assert!(ppu.take_nmi_request(),
        "a PPUSTATUS read after the NMI edge has risen must not erase the latched edge");
    assert!(!ppu.take_nmi_request());
}

#[test]
fn enabling_nmi_at_vblank_dot_1_respects_the_propagation_delay() {
    let mut ppu = at_vblank_start();
    ppu.tick(1);
    assert_eq!(ppu.position(), (241, 1));
    ppu.cpu_write_register(0, 0x80, None);
    assert!(!ppu.take_nmi_request());
    ppu.tick(1);
    assert_eq!(ppu.position(), (241, 2));
    assert!(ppu.take_nmi_request());
    assert!(!ppu.take_nmi_request());
}
