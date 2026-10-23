//! Optional adapter implementing PeaceEmu's shared host/core contract.
use emulation_core::{
    core_io::{InputControl, InputEvent, PixelFormat, VideoFrame},
    CoreApiVersion, CoreCapabilities, CoreDescriptor, CoreError, CoreEvent, EmulationCore,
    CURRENT_CORE_API_VERSION,
};
use crate::{machine::NesMachine, CORE_VERSION};
use std::sync::Arc;

const NES_PALETTE: [[u8; 4]; 64] = [
    [84,84,84,255],[0,30,116,255],[8,16,144,255],[48,0,136,255],
    [68,0,100,255],[92,0,48,255],[84,4,0,255],[60,24,0,255],
    [32,42,0,255],[8,58,0,255],[0,64,0,255],[0,60,0,255],
    [0,50,60,255],[0,0,0,255],[0,0,0,255],[0,0,0,255],
    [152,150,152,255],[8,76,196,255],[48,50,236,255],[92,30,228,255],
    [136,20,176,255],[160,20,100,255],[152,34,32,255],[120,60,0,255],
    [84,90,0,255],[40,114,0,255],[8,124,0,255],[0,118,40,255],
    [0,102,120,255],[0,0,0,255],[0,0,0,255],[0,0,0,255],
    [236,238,236,255],[76,154,236,255],[120,124,236,255],[176,98,236,255],
    [228,84,236,255],[236,88,180,255],[236,106,100,255],[212,136,32,255],
    [160,170,0,255],[116,196,0,255],[76,208,32,255],[56,204,108,255],
    [56,180,204,255],[60,60,60,255],[0,0,0,255],[0,0,0,255],
    [236,238,236,255],[168,204,236,255],[188,188,236,255],[212,178,236,255],
    [236,174,236,255],[236,174,212,255],[236,180,176,255],[228,196,144,255],
    [204,210,120,255],[180,222,120,255],[168,226,144,255],[152,226,180,255],
    [160,214,228,255],[160,162,160,255],[0,0,0,255],[0,0,0,255],
];

/// NES implementation of the PeaceEmu shared emulation-core contract.
pub struct PeaceEmuNesCore {
    rom: Option<Arc<[u8]>>,
    machine: Option<NesMachine>,
    pending_frame: Option<VideoFrame>,
    controller_buttons: u8,
}

impl PeaceEmuNesCore {
    /// Creates an unloaded NES core instance for registration in the host.
    pub fn new() -> Self {
        Self { rom: None, machine: None, pending_frame: None, controller_buttons: 0 }
    }

    fn machine(&self) -> Result<&NesMachine, CoreError> {
        self.machine.as_ref().ok_or(CoreError::NotInitialized)
    }

    fn machine_mut(&mut self) -> Result<&mut NesMachine, CoreError> {
        self.machine.as_mut().ok_or(CoreError::NotInitialized)
    }
}

impl Default for PeaceEmuNesCore {
    fn default() -> Self { Self::new() }
}

impl EmulationCore for PeaceEmuNesCore {
    fn descriptor(&self) -> CoreDescriptor {
        CoreDescriptor {
            api_version: CURRENT_CORE_API_VERSION,
            system_id: emulation_core::SystemId::new("nes").expect("static system ID is valid"),
            display_name: "Nintendo Entertainment System",
            core_version: CORE_VERSION,
            capabilities: CoreCapabilities {
                content_loading: true,
                persistent_saves: false,
                save_states: false,
                video_output: true,
                audio_output: false,
                input: true,
            },
        }
    }

    fn load_content(&mut self, content: &[u8]) -> Result<(), CoreError> {
        // Parse before replacing current state so failed loads preserve the active game.
        let machine = NesMachine::from_ines(content)
            .map_err(|_| CoreError::InvalidContent("unsupported or malformed iNES ROM"))?;
        self.rom = Some(Arc::from(content));
        self.machine = Some(machine);
        self.pending_frame = None;
        self.controller_buttons = 0;
        Ok(())
    }

    fn reset(&mut self) -> Result<(), CoreError> {
        let rom = self.rom.as_ref().ok_or(CoreError::NotInitialized)?;
        let machine = NesMachine::from_ines(rom)
            .map_err(|_| CoreError::InvalidContent("stored iNES ROM can no longer be loaded"))?;
        self.machine = Some(machine);
        self.pending_frame = None;
        self.controller_buttons = 0;
        Ok(())
    }

    fn run_until_event(&mut self) -> Result<CoreEvent, CoreError> {
        let machine = self.machine_mut()?;
        machine.step_frame().map_err(|_| CoreError::Execution("NES frame execution failed"))?;
        let mut rgba = Vec::with_capacity(256 * 240 * 4);
        for &index in machine.framebuffer() {
            rgba.extend_from_slice(&NES_PALETTE[(index & 0x3F) as usize]);
        }
        self.pending_frame = Some(
            VideoFrame::new(256, 240, PixelFormat::Rgba8888, rgba)
                .map_err(|_| CoreError::Execution("invalid NES framebuffer dimensions"))?,
        );
        Ok(CoreEvent::FrameReady)
    }

    fn send_input(&mut self, event: InputEvent) -> Result<(), CoreError> {
        let bit = match event.control {
            InputControl::Action1 => 0,
            InputControl::Action2 => 1,
            InputControl::Select => 2,
            InputControl::Start => 3,
            InputControl::Up => 4,
            InputControl::Down => 5,
            InputControl::Left => 6,
            InputControl::Right => 7,
            InputControl::Action3 | InputControl::Action4 => {
                return Err(CoreError::Unsupported("NES has only A and B action buttons"));
            }
        };
        if event.pressed {
            self.controller_buttons |= 1 << bit;
        } else {
            self.controller_buttons &= !(1 << bit);
        }
        self.machine_mut()?.set_controller_state(0, self.controller_buttons);
        Ok(())
    }

    fn take_video_frame(&mut self) -> Result<Option<VideoFrame>, CoreError> {
        self.machine()?;
        Ok(self.pending_frame.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nrom() -> Vec<u8> {
        let mut rom = vec![0; 16 + 16_384];
        rom[..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[16..20].copy_from_slice(&[0xEA, 0x4C, 0x00, 0x80]);
        rom[16 + 0x3FFC] = 0x00;
        rom[16 + 0x3FFD] = 0x80;
        rom
    }

    #[test]
    fn host_contract_loads_rom_emits_owned_rgba_frame_and_maps_input() {
        let mut core = PeaceEmuNesCore::new();
        assert!(core.take_video_frame().is_err());
        core.load_content(&nrom()).unwrap();
        assert_eq!(core.descriptor().system_id.as_str(), "nes");
        assert_eq!(core.run_until_event().unwrap(), CoreEvent::FrameReady);
        let frame = core.take_video_frame().unwrap().unwrap();
        assert_eq!((frame.width(), frame.height()), (256, 240));
        assert_eq!(frame.format(), PixelFormat::Rgba8888);
        assert_eq!(frame.pixels().len(), 256 * 240 * 4);
        assert!(core.take_video_frame().unwrap().is_none());
        core.send_input(InputEvent { control: InputControl::Action1, pressed: true }).unwrap();
        assert_eq!(core.controller_buttons & 1, 1);
        core.reset().unwrap();
        assert_eq!(core.controller_buttons, 0);
    }

    #[test]
    fn failed_rom_load_does_not_replace_the_running_game() {
        let mut core = PeaceEmuNesCore::new();
        core.load_content(&nrom()).unwrap();
        assert!(core.load_content(b"not a ROM").is_err());
        assert_eq!(core.run_until_event().unwrap(), CoreEvent::FrameReady);
    }
}
