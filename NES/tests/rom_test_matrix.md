# NES ROM validation matrix

This file distinguishes automated headless ROMs from diagnostics whose result
must be observed on a rendered screen or through audio. A timeout from the
headless `$6000` runner is not a valid failure classification for a ROM that
does not implement that protocol.

## Automated headless diagnostics

The CI runner requires the Blargg signature at `$6001-$6003` (`DE B0 61`)
and reads the result from `$6000` (`0x80` running, `0x81` delayed reset,
`0x00` pass, `0x01..=0x7F` failure). ROMs using this protocol are executed
by `NES/tests/rom_conformance.rs`.

## Visual/audio diagnostics — manual validation required

These upstream suites report results on screen and by beeping. They must not
be counted as failed just because the headless status/signature runner times
out.

| Suite | ROMs | Coverage | Validation method |
|---|---:|---|---|
| `branch_timing_tests` | 3 | 6502 branch cycles, page crossings and NMI timing | Inspect displayed result and beeps; run in documented order |
| `sprite_hit_tests_2005.10.05` | 11 | Sprite-zero hit opacity, clipping, alignment and dot timing | Inspect displayed result and beeps; run in documented order |
| `sprite_overflow_tests` | 5 | Sprite overflow flag and 2C02 evaluation quirks | Inspect displayed result and beeps; run in documented order |
| `blargg_ppu_tests_2005.09.15b` | 5 | Palette RAM, power-up palette, sprite RAM/OAM DMA, VBlank clear timing and VRAM buffer | Screen result code and audible result; manual |
| `dmc_dma_during_read4` | 5 | DMC DMA collisions with PPUDATA reads/writes and controller reads | On-screen values and CRC; manual |
| `cpu_dummy_reads` | 1 | CPU dummy reads for indexed and indirect addressing, including PPU register side effects | Screen result and beeps; manual |

The source/readme for each suite in the pinned upstream `nes-test-roms`
checkout documents its expected behavior. Capture the framebuffer and audio
diagnostic output when a GUI/audio-capable test harness is available.

These suites do not expose the headless `$6001-$6003` signature required by
`run_blargg_rom`. They are intentionally marked ignored in the automated Rust
test binary and are not invoked by headless CI. A timeout from that runner
must not be interpreted as an emulator failure or a pass. Keep them in the
manual matrix until a framebuffer/audio-aware assertion harness exists.

## Release evidence rules

- Record pass/fail per ROM, platform and emulator commit.
- Keep protocol-incompatible visual/audio tests in the manual matrix until a
  renderer/audio-aware assertion harness exists.
- Never label an unexecuted or unsupported diagnostic as passed.
- A release candidate must have no unexplained automated failures; manual
  suites must have recorded evidence or be explicitly listed as limitations.
