//! Headless integration tests for redistributable Blargg NES diagnostic ROMs.
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
    let image = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut machine = NesMachine::from_ines(&image)
        .map_err(|error| format!("{}: cartridge load failed: {error}", path.display()))?;
    let mut reset_delay = None;

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
                    machine.reset();
                    reset_delay = None;
                } else {
                    *remaining -= 1;
                }
            }
            0x00 => return Ok(read_blargg_message(&mut machine)),
            failure @ 0x01..=0x7F => {
                return Err(format!(
                    "{}: Blargg failure code 0x{failure:02X}: {}",
                    path.display(),
                    read_blargg_message(&mut machine)
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
