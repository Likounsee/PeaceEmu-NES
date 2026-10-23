//! Public-interface regressions for PPU vblank/NMI edge timing.
//!
//! Timing references: NESdev's PPU frame timing and NMI documentation.
//! These tests exercise PeaceEmu's implementation; they do not claim dot-exact
//! bus behavior or import code from another emulator.

use peaceemu_nes::ppu::Ppu;

fn advance_to_vblank(ppu: &mut Ppu) {
    // Start at reset's pre-render position and advance through the actual
    // public timing API so this test remains valid if the starting dot changes.
    for _ in 0..(262 * 341) {
        if ppu.position() == (241, 1) {
            return;
        }
        ppu.tick(1);
    }
    panic!("PPU did not reach the vblank-set position");
}

#[test]
fn enabling_nmi_during_vblank_generates_a_rising_edge() {
    let mut ppu = Ppu::new();
    advance_to_vblank(&mut ppu);

    // Move beyond the vblank edge, then wait for the next frame's vblank.
    // Do not read status here: that would clear the flag being tested.
    ppu.tick(1);
    advance_to_vblank(&mut ppu);
    assert!(!ppu.take_nmi_request());

    ppu.cpu_write_register(0, 0x80, None);
    assert!(ppu.take_nmi_request());
    assert!(!ppu.take_nmi_request(), "NMI is a latched edge, not a repeated level");
}

#[test]
fn status_read_during_vblank_cancels_the_pending_nmi_edge() {
    let mut ppu = Ppu::new();
    ppu.cpu_write_register(0, 0x80, None);
    advance_to_vblank(&mut ppu);

    assert!(ppu.take_nmi_request());
    // A status read clears vblank and lowers the NMI line.
    assert_ne!(ppu.cpu_read_register(2, None) & 0x80, 0);
    assert!(!ppu.take_nmi_request());
}

#[test]
fn disabling_and_reenabling_nmi_while_vblank_is_set_creates_a_new_edge() {
    let mut ppu = Ppu::new();
    advance_to_vblank(&mut ppu);
    ppu.cpu_write_register(0, 0x80, None);
    assert!(ppu.take_nmi_request());

    ppu.cpu_write_register(0, 0x00, None);
    ppu.cpu_write_register(0, 0x80, None);
    assert!(ppu.take_nmi_request());
    assert!(!ppu.take_nmi_request());
}

#[test]
fn vblank_flag_is_set_at_scanline_241_dot_1() {
    let mut ppu = Ppu::new();
    advance_to_vblank(&mut ppu);
    assert_eq!(ppu.position(), (241, 1));
    assert_ne!(ppu.cpu_read_register(2, None) & 0x80, 0);
}
