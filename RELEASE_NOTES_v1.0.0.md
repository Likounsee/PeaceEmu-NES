# PeaceEmu-NES v1.0.0 — Alter Release

> Stable 1.0.0 release notes. The full Windows/Linux validation run completed successfully; publication is triggered by the `nes-module-1.0.0` tag.

## Included

- Rust NES emulation core targeting the Ricoh 2A03 CPU.
- iNES cartridge loading and the currently implemented mapper set documented in the project README.
- CPU memory bus, controller input, OAM DMA, PPU register/memory handling, and APU audio generation.
- Machine/session APIs for stepping, frame output, audio samples, controller input, and battery-backed RAM.
- Windows desktop executable, with a SHA-256 checksum distributed alongside it.

## 1.0.0 validation fixes

- Corrected OAM DMA 513/514-cycle alignment parity, including the APU/CPU phase relationship used by the DMA model.
- Corrected the CPU IRQ sampling path across OAM DMA and tightened DMC/OAM arbitration regression coverage.
- Kept both OAM DMA parity paths and DMC GET collision/non-collision cases covered by tests.

## Compatibility boundaries

This release does **not** claim full NES hardware or game compatibility.

- PPU rendering is not yet a complete pixel-by-pixel 2C02 pipeline.
- Audio synthesis and some APU/DMC timing remain approximations.
- NES 2.0 support is partial and limited to explicitly implemented mapper/submapper paths.
- Not every board variant of each implemented mapper is supported.
- Unsupported opcodes and unsupported cartridge configurations are rejected explicitly rather than silently guessed.

See [README.md](README.md) and [NES/README.md](NES/README.md) for the current feature inventory and limitations.

## Validation

The final validation completed successfully on both Windows and Linux.

- Rust formatting, tests, and Clippy (stable and Rust 1.78 core checks).
- Blargg CPU diagnostic ROMs and extended CPU/PPU hardware diagnostics.
- Experimental APU, PPU, MMC3, CPU dummy-write, OAM-read, and APU-reset diagnostics.
- Windows and Linux desktop Release builds.

GitHub Actions runs:
- NES core checks: `42419984378` — success.
- NES module CI: `42419984382` — success.

Local reproduction:

```sh
cargo fmt --manifest-path NES/Cargo.toml -- --check
cargo test --manifest-path NES/Cargo.toml --locked
cargo clippy --manifest-path NES/Cargo.toml --all-targets --locked -- -D warnings
cargo build --manifest-path NES/Cargo.toml --bin peaceemu-nes-desktop --release --locked
```

Only redistributable homebrew/test ROMs may be included in the repository or release assets. Commercial ROMs must not be bundled.
