//! Headless integration tests for redistributable Blargg NES diagnostic ROMs.
#![cfg(feature = "ppu-timing-diagnostics")]
//!
//! Set PEACEEMU_NES_TEST_ROM_DIR to the checked-out nes-test-roms repository root.
//! The runner uses Blargg's $6001-$6003 signature and $6000 status protocol.

use peaceemu_nes::machine::NesMachine;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_INSTRUCTIONS: u64 = 10_000_000;
const RESET_DELAY_INSTRUCTIONS: u64 = 600_000;

fn read_blargg_message(machine: &mut NesMachine) -> String {
    let mut bytes = Vec::new();
    for address in 0x6004..=0x60FF {
        let byte = machine.read_cpu(address);
        if byte == 0 {
            break;
        }
        if (0x20..=0x7E).contains(&byte) || byte == b'\n' || byte == b'\r' {
            bytes.push(byte);
        }
    }
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

fn run_blargg_rom(path: &Path) -> Result<String, String> {
    run_blargg_rom_with_ppu_phase(path, 0)
}

fn run_blargg_rom_with_ppu_phase(path: &Path, ppu_phase: u8) -> Result<String, String> {
    let image = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut machine = NesMachine::from_ines_with_ppu_startup_phase(&image, ppu_phase)
        .map_err(|error| format!("{}: cartridge load failed: {error}", path.display()))?;
    let mut reset_delay = None;
    // Keep PPU-register transactions across Blargg-requested soft resets. The
    // machine reset intentionally clears its live bus ring, but these ROMs use
    // status 0x81 to restart individual timing subtests before reporting a result.
    let mut ppu_trace_history = Vec::new();
    let mut ppu_timing_history = Vec::new();

    for instruction in 0..MAX_INSTRUCTIONS {
        machine.step().map_err(|error| {
            format!("{}: emulator stopped at instruction {instruction}: {error}", path.display())
        })?;

        if machine.read_cpu(0x6001) != 0xDE
            || machine.read_cpu(0x6002) != 0xB0
            || machine.read_cpu(0x6003) != 0x61
        {
            continue;
        }

        let status = machine.read_cpu(0x6000);
        match status {
            0x80 => {}
            0x81 => {
                let remaining = reset_delay.get_or_insert(RESET_DELAY_INSTRUCTIONS);
                if *remaining == 0 {
                    ppu_trace_history.extend(machine.ppu_bus_trace().into_iter().filter(|entry| {
                        (entry.address & 0xE007) == 0x2000
                            || (((entry.address & 0xE007) == 0x2002)
                                && ((entry.ppu_scanline == 241 && entry.ppu_dot <= 12)
                                    || (entry.ppu_scanline == 261 && entry.ppu_dot <= 3)))
                    }));
                    if ppu_trace_history.len() > 8_000 {
                        let excess = ppu_trace_history.len() - 8_000;
                        ppu_trace_history.drain(..excess);
                    }
                    ppu_timing_history.extend(machine.ppu_timing_events());
                    if ppu_timing_history.len() > 8_000 {
                        let excess = ppu_timing_history.len() - 8_000;
                        ppu_timing_history.drain(..excess);
                    }
                    machine.reset();
                    reset_delay = None;
                } else {
                    *remaining -= 1;
                }
            }
            0x00 => return Ok(read_blargg_message(&mut machine)),
            failure @ 0x01..=0x7F => {
                let message = read_blargg_message(&mut machine);
                let trace = if path.to_string_lossy().contains("ppu_vbl_nmi") {
                    let mut entries = ppu_trace_history.clone();
                    entries.extend(machine.ppu_bus_trace().into_iter().filter(|entry| {
                        (entry.address & 0xE007) == 0x2000
                            || (((entry.address & 0xE007) == 0x2002)
                                && ((entry.ppu_scanline == 241 && entry.ppu_dot <= 12)
                                    || (entry.ppu_scanline == 261 && entry.ppu_dot <= 3)))
                    }));
                    // Keep all PPUCTRL and PPUSTATUS transactions. Their ordering
                    // relative to CPU_NMI_SAMPLE events is what distinguishes the
                    // dot-1/2/3 race; status reads alone omit the NMI-enable strobe.
                    let ppu_accesses: Vec<_> = entries.iter()
                        .filter(|entry| {
                            (entry.address & 0xE007) == 0x2000
                                || ((entry.address & 0xE007) == 0x2002
                                    && ((entry.ppu_scanline == 241 && entry.ppu_dot <= 12)
                                        || (entry.ppu_scanline == 261 && entry.ppu_dot <= 3)))
                        })
                        .rev()
                        .take(4000)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .map(|entry| format!(
                            "cycle={} beam={}:{} {:?} addr=${:04X} value=${:02X} ppu_nmi={} cpu_nmi_pending={}",
                            entry.cycle, entry.ppu_scanline, entry.ppu_dot, entry.kind,
                            entry.address, entry.value, entry.ppu_nmi_line, entry.cpu_nmi_pending
                        ))
                        .collect();
                    let mut timing_events = ppu_timing_history.clone();
                    timing_events.extend(machine.ppu_timing_events());
                    let timing: Vec<_> = timing_events.iter()
                        .filter(|event| {
                            event.kind == "PPUCTRL_WRITE"
                                || (event.scanline == 241 && event.dot <= 30)
                                || (event.scanline == 261 && event.dot <= 3)
                        })
                        .rev()
                        .take(4000)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .map(|event| format!(
                            "cycle={} beam={}:{} {} status=${:02X} ctrl=${:02X} nmi_line={} nmi_pending={} delay={} suppress={}",
                            event.cycle, event.scanline, event.dot, event.kind, event.status,
                            event.ctrl, event.nmi_line, event.nmi_pending,
                            event.nmi_vblank_delay, event.suppress_vblank
                        )).collect();
                    format!("\\nCPU PPUCTRL/PPUSTATUS bus accesses near VBlank/pre-render (including soft-reset history):\\n{}\\nPPU VBlank/NMI edge events (including soft-reset history):\\n{}", ppu_accesses.join("\\n"), timing.join("\\n"))
                } else {
                    String::new()
                };
                return Err(format!(
                    "{}: Blargg failure code 0x{failure:02X}: {}{}",
                    path.display(),
                    message,
                    trace
                ));
            }
            other => {
                return Err(format!(
                    "{}: unexpected Blargg status 0x{other:02X}",
                    path.display()
                ));
            }
        }
    }

    Err(format!(
        "{}: timeout after {MAX_INSTRUCTIONS} instructions (status 0x{:02X})",
        path.display(),
        machine.read_cpu(0x6000)
    ))
}

#[test]
fn blargg_instruction_diagnostics_pass() {
    let Some(root) = std::env::var_os("PEACEEMU_NES_TEST_ROM_DIR") else {
        eprintln!("Skipping external ROM tests: PEACEEMU_NES_TEST_ROM_DIR is not set");
        return;
    };
    let root = PathBuf::from(root);
    // The pinned ROM repository provides instr_test-v5 with 16 single-opcode ROMs.
    let roms = [
        "instr_test-v5/rom_singles/01-basics.nes",
        "instr_test-v5/rom_singles/02-implied.nes",
        "instr_test-v5/rom_singles/03-immediate.nes",
        "instr_test-v5/rom_singles/04-zero_page.nes",
        "instr_test-v5/rom_singles/05-zp_xy.nes",
        "instr_test-v5/rom_singles/06-absolute.nes",
        "instr_test-v5/rom_singles/07-abs_xy.nes",
        "instr_test-v5/rom_singles/08-ind_x.nes",
        "instr_test-v5/rom_singles/09-ind_y.nes",
        "instr_test-v5/rom_singles/10-branches.nes",
        "instr_test-v5/rom_singles/11-stack.nes",
        "instr_test-v5/rom_singles/12-jmp_jsr.nes",
        "instr_test-v5/rom_singles/13-rts.nes",
        "instr_test-v5/rom_singles/14-rti.nes",
        "instr_test-v5/rom_singles/15-brk.nes",
        "instr_test-v5/rom_singles/16-special.nes",
    ];
    for relative in roms {
        let path = root.join(relative);
        assert!(path.is_file(), "missing diagnostic ROM: {}", path.display());
        match run_blargg_rom(&path) {
            Ok(message) => println!("PASS {} — {}", relative, message),
            Err(error) => panic!("{error}"),
        }
    }
}


#[test]
fn blargg_apu_diagnostics_experimental() {
    let Some(root) = std::env::var_os("PEACEEMU_NES_APU_TEST_ROM_DIR") else {
        eprintln!("Skipping experimental APU ROM tests: PEACEEMU_NES_APU_TEST_ROM_DIR is not set");
        return;
    };
    let root = PathBuf::from(root);
    let roms = [
        "apu_test/rom_singles/1-len_ctr.nes",
        "apu_test/rom_singles/2-len_table.nes",
        "apu_test/rom_singles/3-irq_flag.nes",
        "apu_test/rom_singles/4-jitter.nes",
        "apu_test/rom_singles/5-len_timing.nes",
        "apu_test/rom_singles/6-irq_flag_timing.nes",
        "apu_test/rom_singles/7-dmc_basics.nes",
        "apu_test/rom_singles/8-dmc_rates.nes",
    ];

    for relative in roms {
        let path = root.join(relative);
        assert!(path.is_file(), "missing experimental APU ROM: {}", path.display());
        match run_blargg_rom(&path) {
            Ok(message) => println!("EXPERIMENTAL PASS {} — {}", relative, message),
            Err(error) => panic!("experimental APU diagnostic failed: {error}"),
        }
    }
}

#[test]
fn blargg_ppu_diagnostics_experimental() {
    let Some(root) = std::env::var_os("PEACEEMU_NES_PPU_TEST_ROM_DIR") else {
        eprintln!("Skipping experimental PPU ROM tests: PEACEEMU_NES_PPU_TEST_ROM_DIR is not set");
        return;
    };
    let root = PathBuf::from(root);
    let roms = [
        "ppu_vbl_nmi/rom_singles/01-vbl_basics.nes",
        "ppu_vbl_nmi/rom_singles/02-vbl_set_time.nes",
        "ppu_vbl_nmi/rom_singles/03-vbl_clear_time.nes",
        "ppu_vbl_nmi/rom_singles/04-nmi_control.nes",
        "ppu_vbl_nmi/rom_singles/05-nmi_timing.nes",
        "ppu_vbl_nmi/rom_singles/06-suppression.nes",
        "ppu_vbl_nmi/rom_singles/07-nmi_on_timing.nes",
        "ppu_vbl_nmi/rom_singles/08-nmi_off_timing.nes",
        "ppu_vbl_nmi/rom_singles/09-even_odd_frames.nes",
        "ppu_vbl_nmi/rom_singles/10-even_odd_timing.nes",
    ];

    let mut failures = Vec::new();
    for relative in roms {
        let path = root.join(relative);
        assert!(path.is_file(), "missing experimental PPU ROM: {}", path.display());
        match run_blargg_rom(&path) {
            Ok(message) => println!("EXPERIMENTAL PASS phase=0 {} — {}", relative, message),
            Err(error) => {
                eprintln!("EXPERIMENTAL FAIL phase=0 {} — {}", relative, error);
                failures.push(error);
            }
        }
    }

    // Blargg warns that the real console's CPU/PPU divider can start in any
    // of three phases. Re-run only the two remaining phase-sensitive cases
    // with the alternate alignments to distinguish a valid phase offset from
    // a genuine PPU timing defect.
    let phase_sensitive_roms = [
        "ppu_vbl_nmi/rom_singles/02-vbl_set_time.nes",
        "ppu_vbl_nmi/rom_singles/06-suppression.nes",
    ];
    let mut phase_failures = Vec::new();
    for phase in 1..=2 {
        let mut phase_passes = 0;
        for relative in phase_sensitive_roms {
            let path = root.join(relative);
            if !path.is_file() {
                let error = format!("missing phase-sensitive PPU ROM: {}", path.display());
                eprintln!("EXPERIMENTAL PHASE FAIL phase={} {}", phase, error);
                phase_failures.push(error);
                continue;
            }
            match run_blargg_rom_with_ppu_phase(&path, phase) {
                Ok(message) => {
                    phase_passes += 1;
                    println!("EXPERIMENTAL PHASE PASS phase={} {} — {}", phase, relative, message);
                }
                Err(error) => {
                    eprintln!("EXPERIMENTAL PHASE FAIL phase={} {}", phase, error);
                    phase_failures.push(format!("phase {phase}: {error}"));
                }
            }
        }
        println!("EXPERIMENTAL PPU startup phase {}: {}/2 phase-sensitive ROMs passed", phase, phase_passes);
    }

    assert!(failures.is_empty(), "default-phase PPU diagnostics reported {} failure(s)", failures.len());
    assert!(phase_failures.is_empty(),
        "alternate-phase PPU diagnostics reported {} failure(s):\n{}",
        phase_failures.len(), phase_failures.join("\n"));
}

#[test]
fn blargg_extended_hardware_diagnostics() {
    let Some(root) = std::env::var_os("PEACEEMU_NES_TEST_ROM_DIR") else {
        eprintln!("Skipping extended ROM tests: PEACEEMU_NES_TEST_ROM_DIR is not set");
        return;
    };
    let root = PathBuf::from(root);
    // Additional Blargg suites exercise CPU bus timing, interrupts, reset,
    // PPU register behavior, sprite evaluation and sprite-zero hit timing.
    const ROMS: &[&str] = &[
        "branch_timing_tests/1.Branch_Basics.nes",
        "branch_timing_tests/2.Backward_Branch.nes",
        "branch_timing_tests/3.Forward_Branch.nes",
        "instr_misc/rom_singles/01-abs_x_wrap.nes",
        "instr_misc/rom_singles/02-branch_wrap.nes",
        "instr_misc/rom_singles/03-dummy_reads.nes",
        "instr_misc/rom_singles/04-dummy_reads_apu.nes",
        "cpu_interrupts_v2/rom_singles/1-cli_latency.nes",
        "cpu_interrupts_v2/rom_singles/2-nmi_and_brk.nes",
        "cpu_interrupts_v2/rom_singles/3-nmi_and_irq.nes",
        "cpu_interrupts_v2/rom_singles/4-irq_and_dma.nes",
        "cpu_interrupts_v2/rom_singles/5-branch_delays_irq.nes",
        "cpu_reset/ram_after_reset.nes",
        "cpu_reset/registers.nes",
        "ppu_open_bus/ppu_open_bus.nes",
        "ppu_read_buffer/test_ppu_read_buffer.nes",
        "sprite_hit_tests_2005.10.05/01.basics.nes",
        "sprite_hit_tests_2005.10.05/02.alignment.nes",
        "sprite_hit_tests_2005.10.05/03.corners.nes",
        "sprite_hit_tests_2005.10.05/04.flip.nes",
        "sprite_hit_tests_2005.10.05/05.left_clip.nes",
        "sprite_hit_tests_2005.10.05/06.right_edge.nes",
        "sprite_hit_tests_2005.10.05/07.screen_bottom.nes",
        "sprite_hit_tests_2005.10.05/08.double_height.nes",
        "sprite_hit_tests_2005.10.05/09.timing_basics.nes",
        "sprite_hit_tests_2005.10.05/10.timing_order.nes",
        "sprite_hit_tests_2005.10.05/11.edge_timing.nes",
        "sprite_overflow_tests/1.Basics.nes",
        "sprite_overflow_tests/2.Details.nes",
        "sprite_overflow_tests/3.Timing.nes",
        "sprite_overflow_tests/4.Obscure.nes",
        "sprite_overflow_tests/5.Emulator.nes",
    ];

    let mut failures = Vec::new();
    let mut passed = 0;
    for relative in ROMS {
        let path = root.join(relative);
        if !path.is_file() {
            failures.push(format!("missing diagnostic ROM: {}", path.display()));
            continue;
        }
        match run_blargg_rom(&path) {
            Ok(message) => {
                passed += 1;
                println!("EXTENDED PASS {} — {}", relative, message);
            }
            Err(error) => {
                eprintln!("EXTENDED FAIL {}", error);
                failures.push(error);
            }
        }
    }
    println!("Extended Blargg diagnostics: {passed}/{} passed", ROMS.len());
    assert!(
        failures.is_empty(),
        "extended hardware diagnostics reported {} failure(s):\\n{}",
        failures.len(),
        failures.join("\\n")
    );
}

#[test]
fn blargg_ppu_register_diagnostics_experimental() {
    let Some(root) = std::env::var_os("PEACEEMU_NES_PPU_TEST_ROM_DIR") else {
        eprintln!("Skipping experimental PPU register ROM tests: PEACEEMU_NES_PPU_TEST_ROM_DIR is not set");
        return;
    };
    let root = PathBuf::from(root);
    // The 2005.09.15b suite covers PPU palette RAM, power-up state,
    // OAM access, VBlank clearing and PPUDATA/VRAM access.
    const ROMS: &[&str] = &[
        "blargg_ppu_tests_2005.09.15b/palette_ram.nes",
        "blargg_ppu_tests_2005.09.15b/power_up_palette.nes",
        "blargg_ppu_tests_2005.09.15b/sprite_ram.nes",
        "blargg_ppu_tests_2005.09.15b/vbl_clear_time.nes",
        "blargg_ppu_tests_2005.09.15b/vram_access.nes",
    ];

    let mut failures = Vec::new();
    let mut passed = 0;
    for relative in ROMS {
        let path = root.join(relative);
        if !path.is_file() {
            failures.push(format!("missing PPU register ROM: {}", path.display()));
            continue;
        }
        match run_blargg_rom(&path) {
            Ok(message) => {
                passed += 1;
                println!("EXPERIMENTAL PPU REGISTER PASS {} — {}", relative, message);
            }
            Err(error) => {
                eprintln!("EXPERIMENTAL PPU REGISTER FAIL {}", error);
                failures.push(error);
            }
        }
    }
    println!("Blargg PPU register diagnostics: {passed}/{} passed", ROMS.len());
    assert!(
        failures.is_empty(),
        "PPU register diagnostics reported {} failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn blargg_dmc_dma_register_interaction_diagnostics_experimental() {
    let Some(root) = std::env::var_os("PEACEEMU_NES_TEST_ROM_DIR") else {
        eprintln!("Skipping experimental DMC DMA ROM tests: PEACEEMU_NES_TEST_ROM_DIR is not set");
        return;
    };
    let root = PathBuf::from(root);
    // These ROMs exercise DMC DMA collisions with PPU register accesses
    // and controller reads. They use Blargg's $6000 result protocol.
    const ROMS: &[&str] = &[
        "dmc_dma_during_read4/dma_2007_read.nes",
        "dmc_dma_during_read4/dma_2007_write.nes",
        "dmc_dma_during_read4/dma_4016_read.nes",
        "dmc_dma_during_read4/double_2007_read.nes",
        "dmc_dma_during_read4/read_write_2007.nes",
    ];

    let mut failures = Vec::new();
    let mut passed = 0;
    for relative in ROMS {
        let path = root.join(relative);
        if !path.is_file() {
            failures.push(format!("missing DMC DMA ROM: {}", path.display()));
            continue;
        }
        match run_blargg_rom(&path) {
            Ok(message) => {
                passed += 1;
                println!("EXPERIMENTAL DMC DMA PASS {} — {}", relative, message);
            }
            Err(error) => {
                eprintln!("EXPERIMENTAL DMC DMA FAIL {}", error);
                failures.push(error);
            }
        }
    }
    println!("Blargg DMC DMA/register diagnostics: {passed}/{} passed", ROMS.len());
    assert!(
        failures.is_empty(),
        "DMC DMA/register diagnostics reported {} failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn blargg_mmc3_mapper_diagnostics_experimental() {
    let Some(root) = std::env::var_os("PEACEEMU_NES_TEST_ROM_DIR") else {
        eprintln!("Skipping experimental MMC3 ROM tests: PEACEEMU_NES_TEST_ROM_DIR is not set");
        return;
    };
    let root = PathBuf::from(root);
    // MMC3 mapper-4 diagnostics exercise IRQ counter clocking, A12 edges,
    // scanline timing, and board variants. The suite uses the $6000 protocol.
    const ROMS: &[&str] = &[
        "mmc3_test_2/rom_singles/1-clocking.nes",
        "mmc3_test_2/rom_singles/2-details.nes",
        "mmc3_test_2/rom_singles/3-A12_clocking.nes",
        "mmc3_test_2/rom_singles/4-scanline_timing.nes",
        "mmc3_test_2/rom_singles/5-MMC3.nes",
        "mmc3_test_2/rom_singles/6-MMC3_alt.nes",
    ];

    let mut failures = Vec::new();
    let mut passed = 0;
    for relative in ROMS {
        let path = root.join(relative);
        if !path.is_file() {
            failures.push(format!("missing MMC3 ROM: {}", path.display()));
            continue;
        }
        match run_blargg_rom(&path) {
            Ok(message) => {
                passed += 1;
                println!("EXPERIMENTAL MMC3 PASS {} — {}", relative, message);
            }
            Err(error) => {
                eprintln!("EXPERIMENTAL MMC3 FAIL {}", error);
                failures.push(error);
            }
        }
    }
    println!("Blargg MMC3 diagnostics: {passed}/{} passed", ROMS.len());
    assert!(
        failures.is_empty(),
        "MMC3 mapper diagnostics reported {} failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
