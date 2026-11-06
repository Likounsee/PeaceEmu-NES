# PeaceEmu-NES v1.0.0

> Release candidate notes — publication is pending the final Windows/Linux diagnostic and release-build checks.

## Included

- Rust NES emulation core targeting the Ricoh 2A03 CPU.
- iNES cartridge loading and the currently implemented mapper set documented in the project README.
- CPU memory bus, controller input, OAM DMA, PPU register/memory handling, and APU audio generation.
- Machine/session APIs for stepping, frame output, audio samples, controller input, and battery-backed RAM.
- Windows desktop test executable.

## Compatibility boundaries

This release does **not** claim full NES hardware or game compatibility.

- PPU rendering is not yet a complete pixel-by-pixel 2C02 pipeline.
- Audio synthesis and some APU/DMC timing remain approximations.
- NES 2.0 support is partial and limited to explicitly implemented mapper/submapper paths.
- Not every board variant of each implemented mapper is supported.
- Unsupported opcodes and unsupported cartridge configurations are rejected explicitly rather than silently guessed.

See [README.md](README.md) and [NES/README.md](NES/README.md) for the current feature inventory and limitations.

## Validation

The release is not considered published until the current CI run has completed and its results have been reviewed. The release build and diagnostic ROM suites must be reported accurately; known failures must not be hidden or described as passing.

Local checks:

```sh
cargo fmt --manifest-path NES/Cargo.toml -- --check
cargo test --manifest-path NES/Cargo.toml --locked
cargo clippy --manifest-path NES/Cargo.toml --all-targets --locked -- -D warnings
cargo build --manifest-path NES/Cargo.toml --bin peaceemu-nes-desktop --release --locked
```

Only redistributable homebrew/test ROMs may be included in the repository or release assets. Commercial ROMs must not be bundled.
