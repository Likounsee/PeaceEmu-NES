use peaceemu_nes::NesMachine;

fn minimal_nrom() -> Vec<u8> {
    let mut image = vec![0; 16 + 16 * 1024 + 8 * 1024];
    image[..4].copy_from_slice(b"NES\x1A");
    image[4] = 1; // 16 KiB PRG-ROM
    image[5] = 1; // 8 KiB CHR-ROM

    // JMP $8000 keeps the CPU running while the PPU and APU are clocked.
    let prg_start = 16;
    image[prg_start] = 0x4C;
    image[prg_start + 1] = 0x00;
    image[prg_start + 2] = 0x80;

    // NROM reset vector at the end of the 16 KiB PRG image.
    image[prg_start + 0x3FFC] = 0x00;
    image[prg_start + 0x3FFD] = 0x80;
    image
}

#[test]
fn minimal_nrom_loads_and_runs_multiple_frames() {
    let mut machine = NesMachine::from_ines(&minimal_nrom())
        .expect("minimal NROM image should load");

    let start_cycles = machine.total_cycles();
    let elapsed_cycles = machine
        .run_frames(3)
        .expect("minimal NROM should complete three frames");

    assert!(elapsed_cycles > 50_000, "frame execution should advance CPU cycles");
    assert!(machine.total_cycles() > start_cycles);
    assert_eq!(machine.framebuffer().len(), 256 * 240);
    assert_eq!(machine.audio_sample_rate(), 44_100);
}

#[test]
fn minimal_nrom_survives_reset_and_runs_again() {
    let mut machine = NesMachine::from_ines(&minimal_nrom())
        .expect("minimal NROM image should load");

    machine.run_frames(1).expect("first frame should complete");
    machine.reset();
    machine
        .run_frames(1)
        .expect("frame execution should recover after reset");
    assert_eq!(machine.framebuffer().len(), 256 * 240);
}
