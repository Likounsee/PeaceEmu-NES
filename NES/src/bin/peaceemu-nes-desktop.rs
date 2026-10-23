//! Minimal native Windows frontend for playing NES ROMs with the Rust core.
//!
//! Run with: cargo run --release --bin peaceemu-nes-desktop -- path\to\game.nes
#![allow(unsafe_code)]

#[cfg(not(windows))]
fn main() {
    eprintln!("peaceemu-nes-desktop is currently available on Windows only.");
    eprintln!("Build it on Windows with: cargo run --release --bin peaceemu-nes-desktop -- game.nes");
}

#[cfg(windows)]
mod win {
    use peaceemu_nes::machine::NesMachine;
    use std::ffi::c_void;
    use std::fs;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};

    type Handle = *mut c_void;
    type Hwnd = Handle;
    type Hdc = Handle;
    type Hinstance = Handle;
    type Wparam = usize;
    type Lparam = isize;
    type Lresult = isize;
    type Bool = i32;

    const WM_DESTROY: u32 = 0x0002;
    const WM_QUIT: u32 = 0x0012;
    const WS_OVERLAPPEDWINDOW: u32 = 0x00CF0000;
    const CW_USEDEFAULT: i32 = 0x80000000u32 as i32;
    const SW_SHOW: i32 = 5;
    const PM_REMOVE: u32 = 1;
    const DIB_RGB_COLORS: u32 = 0;
    const SRCCOPY: u32 = 0x00CC0020;
    const VK_SHIFT: i32 = 0x10;
    const VK_RETURN: i32 = 0x0D;
    const VK_LEFT: i32 = 0x25;
    const VK_UP: i32 = 0x26;
    const VK_RIGHT: i32 = 0x27;
    const VK_DOWN: i32 = 0x28;
    const VK_X: i32 = 0x58;
    const VK_Z: i32 = 0x5A;
    const WAVE_FORMAT_PCM: u16 = 1;
    const WAVE_MAPPER: u32 = u32::MAX;
    const WHDR_DONE: u32 = 0x0000_0001;

    #[repr(C)]
    struct Point { x: i32, y: i32 }
    #[repr(C)]
    struct Msg {
        hwnd: Hwnd, message: u32, w_param: Wparam, l_param: Lparam,
        time: u32, point: Point, private: u32,
    }
    type WndProc = Option<unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult>;
    #[repr(C)]
    struct WndClassW {
        style: u32, wnd_proc: WndProc, cls_extra: i32, wnd_extra: i32,
        instance: Hinstance, icon: Handle, cursor: Handle, background: Handle,
        menu_name: *const u16, class_name: *const u16,
    }
    #[repr(C)]
    struct BitmapInfoHeader {
        size: u32, width: i32, height: i32, planes: u16, bit_count: u16,
        compression: u32, size_image: u32, x_pels_per_meter: i32,
        y_pels_per_meter: i32, clr_used: u32, clr_important: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RgbQuad { blue: u8, green: u8, red: u8, reserved: u8 }
    #[repr(C)]
    struct BitmapInfo { header: BitmapInfoHeader, colors: [RgbQuad; 1] }
    #[repr(C)]
    struct Rect { left: i32, top: i32, right: i32, bottom: i32 }

    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassW(class: *const WndClassW) -> u16;
        fn CreateWindowExW(ex_style: u32, class_name: *const u16, window_name: *const u16,
            style: u32, x: i32, y: i32, width: i32, height: i32,
            parent: Hwnd, menu: Handle, instance: Hinstance, param: *mut c_void) -> Hwnd;
        fn DefWindowProcW(hwnd: Hwnd, message: u32, w_param: Wparam, l_param: Lparam) -> Lresult;
        fn ShowWindow(hwnd: Hwnd, command: i32) -> Bool;
        fn UpdateWindow(hwnd: Hwnd) -> Bool;
        fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> Bool;
        fn TranslateMessage(msg: *const Msg) -> Bool;
        fn DispatchMessageW(msg: *const Msg) -> Lresult;
        fn PostQuitMessage(exit_code: i32);
        fn GetDC(hwnd: Hwnd) -> Hdc;
        fn ReleaseDC(hwnd: Hwnd, dc: Hdc) -> i32;
        fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> Bool;
        fn GetAsyncKeyState(key: i32) -> i16;
        fn AdjustWindowRect(rect: *mut Rect, style: u32, menu: Bool) -> Bool;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn StretchDIBits(dc: Hdc, x_dest: i32, y_dest: i32, dest_width: i32, dest_height: i32,
            x_src: i32, y_src: i32, src_width: i32, src_height: i32, bits: *const c_void,
            info: *const BitmapInfo, usage: u32, raster_op: u32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(module_name: *const u16) -> Hinstance;
        fn Sleep(milliseconds: u32);
    }

    unsafe extern "system" fn window_proc(hwnd: Hwnd, message: u32, w: Wparam, l: Lparam) -> Lresult {
        if message == WM_DESTROY {
            PostQuitMessage(0);
            return 0;
        }
        DefWindowProcW(hwnd, message, w, l)
    }

    fn key_down(key: i32) -> bool {
        unsafe { GetAsyncKeyState(key) < 0 }
    }

    fn controller_state() -> u8 {
        let mut state = 0u8;
        if key_down(VK_Z) { state |= 1 << 0; }       // A
        if key_down(VK_X) { state |= 1 << 1; }       // B
        if key_down(VK_SHIFT) { state |= 1 << 2; }   // Select
        if key_down(VK_RETURN) { state |= 1 << 3; }  // Start
        if key_down(VK_UP) { state |= 1 << 4; }
        if key_down(VK_DOWN) { state |= 1 << 5; }
        if key_down(VK_LEFT) { state |= 1 << 6; }
        if key_down(VK_RIGHT) { state |= 1 << 7; }
        state
    }

    // Standard 2C02 palette, converted to BGRA for a 32-bit Windows DIB.
    const PALETTE: [[u8; 3]; 64] = [
        [84,84,84],[0,30,116],[8,16,144],[48,0,136],[68,0,100],[92,0,48],[84,4,0],[60,24,0],
        [32,42,0],[8,58,0],[0,64,0],[0,60,0],[0,50,60],[0,0,0],[0,0,0],[0,0,0],
        [152,150,152],[8,76,196],[48,50,236],[92,30,228],[136,20,176],[160,20,100],[152,34,32],[120,60,0],
        [84,90,0],[40,114,0],[8,124,0],[0,118,40],[0,102,120],[0,0,0],[0,0,0],[0,0,0],
        [236,238,236],[76,154,236],[120,124,236],[176,98,236],[228,84,236],[236,88,180],[236,106,100],[212,136,32],
        [160,170,0],[116,196,0],[76,208,32],[56,204,108],[56,180,204],[60,60,60],[0,0,0],[0,0,0],
        [236,238,236],[168,204,236],[188,188,236],[212,178,236],[236,174,236],[236,174,212],[236,180,176],[228,196,144],
        [204,210,120],[180,222,120],[168,226,144],[152,226,180],[160,214,228],[160,162,160],[0,0,0],[0,0,0],
    ];

    fn make_bitmap() -> BitmapInfo {
        BitmapInfo {
            header: BitmapInfoHeader {
                size: size_of::<BitmapInfoHeader>() as u32, width: 256, height: -240,
                planes: 1, bit_count: 32, compression: 0, size_image: 256 * 240 * 4,
                x_pels_per_meter: 0, y_pels_per_meter: 0, clr_used: 0, clr_important: 0,
            },
            colors: [RgbQuad { blue: 0, green: 0, red: 0, reserved: 0 }],
        }
    }

    struct AudioOutput {
        handle: HWaveOut,
        previous_input: f32,
        previous_output: f32,
    }

    impl AudioOutput {
        fn open(sample_rate: u32) -> Result<Self, String> {
            let format = WaveFormatEx {
                format_tag: WAVE_FORMAT_PCM,
                channels: 1,
                samples_per_sec: sample_rate,
                avg_bytes_per_sec: sample_rate.saturating_mul(2),
                block_align: 2,
                bits_per_sample: 16,
                cb_size: 0,
            };
            let mut handle = null_mut();
            let result = unsafe { waveOutOpen(&mut handle, WAVE_MAPPER, &format, 0, 0, 0) };
            if result != 0 || handle.is_null() {
                return Err(format!("Windows audio device could not be opened (waveOut error {result})"));
            }
            Ok(Self { handle, previous_input: 0.0, previous_output: 0.0 })
        }

        fn play(&mut self, samples: &[f32]) -> Result<(), String> {
            if samples.is_empty() { return Ok(()); }
            // Remove the mixer's DC component before converting normalized output to signed PCM.
            let mut pcm = Vec::with_capacity(samples.len());
            for &sample in samples {
                let input = sample.clamp(0.0, 1.0);
                let filtered = input - self.previous_input + 0.995 * self.previous_output;
                self.previous_input = input;
                self.previous_output = filtered;
                pcm.push((filtered.clamp(-1.0, 1.0) * 28_000.0) as i16);
            }
            let mut header = WaveHdr {
                data: pcm.as_mut_ptr().cast(),
                buffer_length: (pcm.len() * size_of::<i16>()) as u32,
                bytes_recorded: 0,
                user: 0,
                flags: 0,
                loops: 0,
                next: null_mut(),
                reserved: 0,
            };
            let header_size = size_of::<WaveHdr>() as u32;
            let prepared = unsafe { waveOutPrepareHeader(self.handle, &mut header, header_size) };
            if prepared != 0 { return Err(format!("waveOutPrepareHeader failed ({prepared})")); }
            let written = unsafe { waveOutWrite(self.handle, &mut header, header_size) };
            if written != 0 {
                unsafe { waveOutUnprepareHeader(self.handle, &mut header, header_size); }
                return Err(format!("waveOutWrite failed ({written})"));
            }
            // Keep the PCM allocation alive until Windows has consumed the whole buffer.
            while unsafe { std::ptr::read_volatile(&header.flags) } & WHDR_DONE == 0 {
                unsafe { Sleep(1); }
            }
            let unprepared = unsafe { waveOutUnprepareHeader(self.handle, &mut header, header_size) };
            if unprepared != 0 { return Err(format!("waveOutUnprepareHeader failed ({unprepared})")); }
            Ok(())
        }
    }

    impl Drop for AudioOutput {
        fn drop(&mut self) {
            unsafe {
                waveOutReset(self.handle);
                waveOutClose(self.handle);
            }
        }
    }

    pub fn run() -> Result<(), String> {
        let mut args = std::env::args_os().skip(1);
        let path = args.next().ok_or_else(|| {
            "usage: peaceemu-nes-desktop <game.nes> [--scale 1..6] [--audio-rate 8000..192000]".to_owned()
        })?;
        let mut scale = 3i32;
        let mut audio_rate = 44_100u32;
        while let Some(argument) = args.next() {
            let value = args.next().ok_or_else(|| format!("missing value for {}", argument.to_string_lossy()))?;
            match argument.to_string_lossy().as_ref() {
                "--scale" => {
                    scale = value.to_string_lossy().parse().map_err(|_| "invalid --scale value".to_owned())?;
                    if !(1..=6).contains(&scale) { return Err("--scale must be between 1 and 6".into()); }
                }
                "--audio-rate" => {
                    audio_rate = value.to_string_lossy().parse().map_err(|_| "invalid --audio-rate value".to_owned())?;
                    if !(8_000..=192_000).contains(&audio_rate) { return Err("--audio-rate must be between 8000 and 192000".into()); }
                }
                _ => return Err(format!("unknown option: {}", argument.to_string_lossy())),
            }
        }
        let image = fs::read(&path).map_err(|e| format!("cannot read ROM: {e}"))?;
        let mut machine = NesMachine::from_ines(&image).map_err(|e| e.to_string())?;
        if !machine.set_audio_sample_rate(audio_rate) {
            return Err(format!("unsupported audio sample rate: {audio_rate}"));
        }
        let mut audio = match AudioOutput::open(audio_rate) {
            Ok(output) => Some(output),
            Err(error) => {
                eprintln!("audio disabled; game will continue silently: {error}");
                None
            }
        };
        let mut save_path = std::path::PathBuf::from(&path);
        save_path.set_extension("sav");
        if machine.has_battery_backed_ram() && save_path.exists() {
            let save = fs::read(&save_path).map_err(|e| format!("cannot read save RAM: {e}"))?;
            machine.load_battery_ram(&save).map_err(|e| format!("invalid save RAM: {e}"))?;
        }

        let class_name: Vec<u16> = "PeaceEmuNesWindow\0".encode_utf16().collect();
        let title: Vec<u16> = format!("PeaceEmu-NES — {}", std::path::Path::new(&path).file_name().unwrap_or_default().to_string_lossy())
            .encode_utf16().chain(Some(0)).collect();
        let instance = unsafe { GetModuleHandleW(null()) };
        let class = WndClassW {
            style: 0, wnd_proc: Some(window_proc), cls_extra: 0, wnd_extra: 0,
            instance, icon: null_mut(), cursor: null_mut(),
            background: 6usize as Handle, menu_name: null(), class_name: class_name.as_ptr(),
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err("RegisterClassW failed".into());
        }
        let mut rect = Rect { left: 0, top: 0, right: 256 * scale, bottom: 240 * scale };
        unsafe { AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, 0); }
        let hwnd = unsafe {
            CreateWindowExW(0, class_name.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT, CW_USEDEFAULT, rect.right - rect.left, rect.bottom - rect.top,
                null_mut(), null_mut(), instance, null_mut())
        };
        if hwnd.is_null() { return Err("CreateWindowExW failed".into()); }
        unsafe { ShowWindow(hwnd, SW_SHOW); UpdateWindow(hwnd); }

        let mut bitmap = make_bitmap();
        let mut pixels = vec![0u8; 256 * 240 * 4];
        let frame_time = Duration::from_micros(16_639);
        let mut next_frame = Instant::now();
        let mut running = true;
        let mut runtime_error = None;
        while running {
            let mut msg: Msg = unsafe { zeroed() };
            while unsafe { PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) } != 0 {
                if msg.message == WM_QUIT { running = false; break; }
                unsafe { TranslateMessage(&msg); DispatchMessageW(&msg); }
            }
            if !running { break; }

            machine.set_controller_state(0, controller_state());
            if let Err(error) = machine.step_frame() {
                runtime_error = Some(format!("emulation stopped: {error}"));
                break;
            }
            let audio_samples = machine.drain_audio_samples();
            if let Some(output) = audio.as_mut() {
                if let Err(error) = output.play(&audio_samples) {
                    eprintln!("audio disabled after playback error: {error}");
                    audio = None;
                }
            }
            for (i, &index) in machine.framebuffer().iter().enumerate() {
                let rgb = PALETTE[(index & 0x3F) as usize];
                let at = i * 4;
                pixels[at] = rgb[2];
                pixels[at + 1] = rgb[1];
                pixels[at + 2] = rgb[0];
                pixels[at + 3] = 0;
            }
            let mut client: Rect = unsafe { zeroed() };
            if unsafe { GetClientRect(hwnd, &mut client) } != 0 {
                let dc = unsafe { GetDC(hwnd) };
                if !dc.is_null() {
                    unsafe {
                        StretchDIBits(dc, 0, 0, client.right, client.bottom, 0, 0, 256, 240,
                            pixels.as_ptr().cast(), &bitmap, DIB_RGB_COLORS, SRCCOPY);
                        ReleaseDC(hwnd, dc);
                    }
                }
            }
            next_frame += frame_time;
            let now = Instant::now();
            if next_frame > now {
                unsafe { Sleep(((next_frame - now).as_millis() as u32).min(16)); }
            } else {
                next_frame = now;
            }
        }
        if machine.has_battery_backed_ram() {
            let save = machine.save_battery_ram().map_err(|e| e.to_string())?;
            fs::write(&save_path, save).map_err(|e| format!("cannot write save RAM: {e}"))?;
        }
        if let Some(error) = runtime_error { return Err(error); }
        Ok(())
    }
}

#[cfg(windows)]
fn main() {
    if let Err(error) = win::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
