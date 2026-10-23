//! UNROM-512 banking regressions based on NESdev mapper-30 behavior.
//! Synthetic ROM data keeps these tests independent of commercial software.

use peaceemu_nes::cartridge::{Cartridge, Mirroring, INES_HEADER_LEN};

const PRG_BANK: usize = 16 * 1024;
const CHR_BANK: usize = 8 * 1024;

fn image(prg_banks: u8, flags6: u8) -> Vec<u8> {
    let mut rom = vec![0; INES_HEADER_LEN + prg_banks as usize * PRG_BANK];
    rom[..4].copy_from_slice(b"NES\x1A");
    rom[4] = prg_banks;
    rom[5] = 0; // UNROM-512 CHR-RAM
    rom[6] = 0xE0 | flags6;
    rom[7] = 0x10;
    for bank in 0..prg_banks as usize {
        let start = INES_HEADER_LEN + bank * PRG_BANK;
        rom[start..start + PRG_BANK].fill(bank as u8);
    }
    rom
}

#[test]
fn mapper30_switches_prg_bank_and_keeps_last_bank_fixed() {
    let mut cart = Cartridge::parse(&image(32, 0)).unwrap();
    assert_eq!(cart.cpu_read(0x8000), Some(0));
    assert_eq!(cart.cpu_read(0xBFFF), Some(0));
    assert_eq!(cart.cpu_read(0xC000), Some(31));
    assert_eq!(cart.cpu_read(0xFFFF), Some(31));

    assert!(cart.cpu_write(0x8000, 7));
    assert_eq!(cart.cpu_read(0x8000), Some(7));
    assert_eq!(cart.cpu_read(0xBFFF), Some(7));
    assert_eq!(cart.cpu_read(0xC000), Some(31));
    assert_eq!(cart.cpu_read(0xFFFF), Some(31));
}

#[test]
fn mapper30_chr_ram_banks_are_independent_and_selectable() {
    let mut cart = Cartridge::parse(&image(16, 0)).unwrap();
    assert!(cart.chr_is_ram());
    assert!(cart.ppu_write(0x0123, 0x11));
    assert_eq!(cart.ppu_read(0x0123), Some(0x11));

    // Register bits 5-6 select one of four 8 KiB CHR-RAM banks.
    assert!(cart.cpu_write(0x8000, 0b0010_0000));
    assert_eq!(cart.ppu_read(0x0123), Some(0));
    assert!(cart.ppu_write(0x0123, 0x22));
    assert_eq!(cart.ppu_read(0x0123), Some(0x22));

    assert!(cart.cpu_write(0x8000, 0));
    assert_eq!(cart.ppu_read(0x0123), Some(0x11));
    assert!(cart.cpu_write(0x8000, 0b0100_0000));
    assert_eq!(cart.ppu_read(0x0123), Some(0));
    assert!(cart.ppu_write(0x0123, 0x33));
    assert!(cart.cpu_write(0x8000, 0b0110_0000));
    assert_eq!(cart.ppu_read(0x0123), Some(0));
    assert!(cart.ppu_write(0x0123, 0x44));
    assert!(cart.cpu_write(0x8000, 0b0100_0000));
    assert_eq!(cart.ppu_read(0x0123), Some(0x33));
}

#[test]
fn mapper30_mirroring_register_changes_one_screen_page_when_enabled() {
    // Legacy iNES bit 3 marks the mapper's mirroring-control variant.
    let mut cart = Cartridge::parse(&image(16, 0x08)).unwrap();
    assert_eq!(cart.mirroring(), Mirroring::OneScreenLower);
    assert!(cart.cpu_write(0x8000, 0x80));
    assert_eq!(cart.mirroring(), Mirroring::OneScreenUpper);
    assert!(cart.cpu_write(0x8000, 0x00));
    assert_eq!(cart.mirroring(), Mirroring::OneScreenLower);
}

#[test]
fn mapper30_four_screen_flag_is_not_overridden_by_bank_writes() {
    let mut cart = Cartridge::parse(&image(16, 0x09)).unwrap();
    assert_eq!(cart.mirroring(), Mirroring::FourScreen);
    assert!(cart.cpu_write(0x8000, 0xE0));
    assert_eq!(cart.mirroring(), Mirroring::FourScreen);
    assert_eq!(cart.ppu_read(0), Some(0));
    assert_eq!(cart.ppu_read((CHR_BANK - 1) as u16), Some(0));
}
