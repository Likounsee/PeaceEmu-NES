//! Sprite evaluation boundary regressions for the NES PPU.

use peaceemu_nes::ppu::Ppu;

fn write_oam(ppu: &mut Ppu, entries: &[[u8; 4]; 64]) {
    ppu.cpu_write_register(3, 0, None);
    for entry in entries {
        for &byte in entry {
            ppu.cpu_write_register(4, byte, None);
        }
    }
}

#[test]
fn sprites_with_y_coordinate_255_are_offscreen_not_wrapped_to_scanline_zero() {
    let mut ppu = Ppu::new();
    ppu.cpu_write_register(1, 0x10, None);
    let entries = [[255, 0, 0, 0]; 64];
    write_oam(&mut ppu, &entries);

    ppu.render_frame(None);
    assert_eq!(ppu.cpu_read_register(2, None) & 0x20, 0,
        "Y=255 means the sprite starts below visible scanlines; it must not wrap to row zero");
}

#[test]
fn ninth_sprite_on_a_scanline_sets_overflow_status() {
    let mut ppu = Ppu::new();
    ppu.cpu_write_register(1, 0x10, None);
    let mut entries = [[255, 0, 0, 0]; 64];
    for entry in entries.iter_mut().take(9) {
        *entry = [0, 0, 0, 0];
    }
    write_oam(&mut ppu, &entries);

    ppu.render_frame(None);
    assert_ne!(ppu.cpu_read_register(2, None) & 0x20, 0,
        "the current frame-based evaluator should flag more than eight sprites on a scanline");
}
