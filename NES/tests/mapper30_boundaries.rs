//! Boundary regressions for UNROM-512 (iNES mapper 30).
//!
//! These tests deliberately use synthetic ROM images; no copyrighted game ROMs
//! or third-party emulator implementation is included.

use peaceemu_nes::cartridge::{Cartridge, CartridgeError, INES_HEADER_LEN};

const PRG_BANK_LEN: usize = 16 * 1024;
const TRAINER_LEN: usize = 512;

fn mapper30_image(with_trainer: bool) -> Vec<u8> {
    let trainer_len = if with_trainer { TRAINER_LEN } else { 0 };
    let mut image = vec![0; INES_HEADER_LEN + trainer_len + 16 * PRG_BANK_LEN];
    image[0..4].copy_from_slice(b"NES\x1A");
    image[4] = 16; // 16 x 16 KiB PRG-ROM.
    image[5] = 0; // CHR-RAM.
    image[6] = 0xE8 | if with_trainer { 0x04 } else { 0 };
    image[7] = 0x10; // Mapper 30 high bits in legacy iNES.
    if with_trainer {
        image[INES_HEADER_LEN..INES_HEADER_LEN + TRAINER_LEN].fill(0xA5);
    }
    let prg_start = INES_HEADER_LEN + trainer_len;
    for bank in 0..16 {
        let start = prg_start + bank * PRG_BANK_LEN;
        image[start..start + PRG_BANK_LEN].fill(bank as u8);
    }
    image
}

#[test]
fn mapper30_rejects_a_truncated_trainer_before_reading_prg() {
    let mut image = mapper30_image(true);
    image.truncate(INES_HEADER_LEN + TRAINER_LEN - 1);

    assert!(matches!(
        Cartridge::parse(&image),
        Err(CartridgeError::TruncatedImage { .. })
    ));
}

#[test]
fn mapper30_rejects_prg_truncated_after_a_complete_trainer() {
    let mut image = mapper30_image(true);
    image.pop();

    assert!(matches!(
        Cartridge::parse(&image),
        Err(CartridgeError::TruncatedImage { .. })
    ));
}

#[test]
fn mapper30_trainer_does_not_shift_the_fixed_last_prg_bank() {
    let image = mapper30_image(true);
    let cartridge = Cartridge::parse(&image).expect("valid synthetic mapper 30 image");

    assert_eq!(cartridge.cpu_read(0x8000), Some(0));
    assert_eq!(cartridge.cpu_read(0xC000), Some(15));
    assert_eq!(cartridge.cpu_read(0xFFFF), Some(15));
    assert_eq!(cartridge.cpu_read(0x7000), None);
}
