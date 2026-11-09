//! Minimal native Windows frontend for playing NES ROMs with the Rust core.
//!
//! Run with: cargo run --release --bin peaceemu-nes-desktop -- path\to\game.nes
#![allow(unsafe_code)]
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(any(windows, test))]
fn aspect_fit(client_width: i32, client_height: i32) -> (i32, i32, i32, i32) {
    if client_width <= 0 || client_height <= 0 {
        return (0, 0, 0, 0);
    }
    // NES pixels are not square: the 256x240 framebuffer is displayed at 4:3.
    // Fit that corrected display ratio into the client area without distorting it.
    let (width, height) = if i64::from(client_width) * 3 <= i64::from(client_height) * 4 {
        (client_width, client_width * 3 / 4)
    } else {
        (client_height * 4 / 3, client_height)
    };
    (
        (client_width - width) / 2,
        (client_height - height) / 2,
        width,
        height,
    )
}


#[cfg(any(windows, test))]
fn adaptive_window_size(screen_width: i32, screen_height: i32) -> (i32, i32) {
    if screen_width <= 0 || screen_height <= 0 {
        return (256 * 3, 256 * 3 * 3 / 4);
    }
    // Keep a comfortable margin around the game and adapt to small and large displays.
    let max_width = (screen_width * 4 / 5).max(256);
    let max_height = (screen_height * 4 / 5).max(240);
    if i64::from(max_width) * 3 <= i64::from(max_height) * 4 {
        (max_width, max_width * 3 / 4)
    } else {
        (max_height * 4 / 3, max_height)
    }
}

#[cfg(any(windows, test))]
fn parse_virtual_keys(value: &str) -> Option<[i32; 8]> {
    let mut result = [0; 8];
    let mut parts = value.split(',');
    for key in &mut result {
        let parsed = parts.next()?.parse::<i32>().ok()?;
        if !(0..=255).contains(&parsed) {
            return None;
        }
        *key = parsed;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(result)
}

#[cfg(any(windows, test))]
fn parse_turbo_speed(value: &str) -> Option<f32> {
    let speed = value.parse::<f32>().ok()?;
    (speed.is_finite() && (0.5..=4.0).contains(&speed)).then_some(speed)
}

#[cfg(any(windows, test))]
fn scaled_frame_interval(base: std::time::Duration, speed: f32) -> Option<std::time::Duration> {
    if !speed.is_finite() || !(0.5..=4.0).contains(&speed) {
        return None;
    }
    Some(std::time::Duration::from_secs_f64(
        base.as_secs_f64() / f64::from(speed),
    ))
}

#[cfg(not(windows))]
fn main() {
    eprintln!("peaceemu-nes-desktop is currently available on Windows only.");
    eprintln!("Build it on Windows with: cargo run --release --bin peaceemu-nes-desktop -- game.nes");
}

#[cfg(windows)]
mod win {
    use super::{adaptive_window_size, aspect_fit, parse_turbo_speed, parse_virtual_keys, scaled_frame_interval};
    use peaceemu_nes::session::{NesConfig, NesSession};
    use std::ffi::c_void;
    use std::fs;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};
    use std::io::{BufRead, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::sync::mpsc;

    type Handle = *mut c_void;
    type HWaveOut = Handle;
    type Hwnd = Handle;
    type Hdc = Handle;
    type Hinstance = Handle;
    type Hmonitor = Handle;
    type Wparam = usize;
    type Lparam = isize;
    type Lresult = isize;
    type Bool = i32;

    const WM_DESTROY: u32 = 0x0002;
    const WM_QUIT: u32 = 0x0012;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_SYSKEYDOWN: u32 = 0x0104;
    const WM_SETICON: u32 = 0x0080;
    const ICON_SMALL: usize = 0;
    const ICON_BIG: usize = 1;
    const WS_OVERLAPPEDWINDOW: u32 = 0x00CF0000;
    const WS_POPUP: u32 = 0x8000_0000;
    const GWL_STYLE: i32 = -16;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const VK_TAB: i32 = 0x09;
    const VK_F1: i32 = 0x70;
    const VK_F5: i32 = 0x74;
    const VK_F8: i32 = 0x77;
    const VK_P: i32 = 0x50;
    const VK_F11: i32 = 0x7A;
    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    const CW_USEDEFAULT: i32 = 0x80000000u32 as i32;
    const SW_SHOW: i32 = 5;
    const PM_REMOVE: u32 = 1;
    const DIB_RGB_COLORS: u32 = 0;
    const COLORONCOLOR: i32 = 3;
    const SRCCOPY: u32 = 0x00CC0020;
    const WAVE_FORMAT_PCM: u16 = 1;
    const WAVE_MAPPER: u32 = u32::MAX;
    const WHDR_DONE: u32 = 0x0000_0001;

    #[repr(C)]
    struct XInputGamepad {
        buttons: u16,
        left_trigger: u8,
        right_trigger: u8,
        thumb_lx: i16,
        thumb_ly: i16,
    }
    #[repr(C)]
    struct XInputState {
        packet_number: u32,
        gamepad: XInputGamepad,
    }

    #[repr(C)]
    struct WaveFormatEx {
        format_tag: u16,
        channels: u16,
        samples_per_sec: u32,
        avg_bytes_per_sec: u32,
        block_align: u16,
        bits_per_sample: u16,
        cb_size: u16,
    }
    #[repr(C)]
    struct WaveHdr {
        data: *mut i8,
        buffer_length: u32,
        bytes_recorded: u32,
        user: usize,
        flags: u32,
        loops: u32,
        next: *mut WaveHdr,
        reserved: usize,
    }

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
    #[repr(C)]
    struct MonitorInfo {
        size: u32,
        monitor: Rect,
        work: Rect,
        flags: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassW(class: *const WndClassW) -> u16;
        fn CreateWindowExW(ex_style: u32, class_name: *const u16, window_name: *const u16,
            style: u32, x: i32, y: i32, width: i32, height: i32,
            parent: Hwnd, menu: Handle, instance: Hinstance, param: *mut c_void) -> Hwnd;
        fn DefWindowProcW(hwnd: Hwnd, message: u32, w_param: Wparam, l_param: Lparam) -> Lresult;
        fn ShowWindow(hwnd: Hwnd, command: i32) -> Bool;
        fn UpdateWindow(hwnd: Hwnd) -> Bool;
        fn SetForegroundWindow(hwnd: Hwnd) -> Bool;
        fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> Bool;
        fn TranslateMessage(msg: *const Msg) -> Bool;
        fn DispatchMessageW(msg: *const Msg) -> Lresult;
        fn PostQuitMessage(exit_code: i32);
        fn GetDC(hwnd: Hwnd) -> Hdc;
        fn GetSystemMetrics(index: i32) -> i32;
        fn ReleaseDC(hwnd: Hwnd, dc: Hdc) -> i32;
        fn FillRect(dc: Hdc, rect: *const Rect, brush: Handle) -> i32;
        fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> Bool;
        fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> Bool;
        fn MonitorFromWindow(hwnd: Hwnd, flags: u32) -> Hmonitor;
        fn GetMonitorInfoW(monitor: Hmonitor, info: *mut MonitorInfo) -> Bool;
        fn GetWindowLongW(hwnd: Hwnd, index: i32) -> i32;
        fn SetWindowLongW(hwnd: Hwnd, index: i32, value: i32) -> i32;
        fn SetWindowPos(hwnd: Hwnd, insert_after: Hwnd, x: i32, y: i32, width: i32, height: i32, flags: u32) -> Bool;
        fn GetAsyncKeyState(key: i32) -> i16;
        fn GetForegroundWindow() -> Hwnd;
        fn AdjustWindowRect(rect: *mut Rect, style: u32, menu: Bool) -> Bool;
        fn MessageBoxW(hwnd: Hwnd, text: *const u16, caption: *const u16, kind: u32) -> i32;
        fn SendMessageW(hwnd: Hwnd, message: u32, w_param: Wparam, l_param: Lparam) -> Lresult;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn SetStretchBltMode(dc: Hdc, mode: i32) -> i32;
        fn StretchDIBits(dc: Hdc, x_dest: i32, y_dest: i32, dest_width: i32, dest_height: i32,
            x_src: i32, y_src: i32, src_width: i32, src_height: i32, bits: *const c_void,
            info: *const BitmapInfo, usage: u32, raster_op: u32) -> i32;
    }
    #[link(name = "xinput9_1_0")]
    extern "system" {
        fn XInputGetState(user_index: u32, state: *mut XInputState) -> u32;
    }
    #[link(name = "winmm")]
    extern "system" {
        fn waveOutOpen(output: *mut Handle, device_id: u32, format: *const WaveFormatEx, callback: usize, instance: usize, flags: u32) -> u32;
        fn waveOutPrepareHeader(output: Handle, header: *mut WaveHdr, size: u32) -> u32;
        fn waveOutWrite(output: Handle, header: *mut WaveHdr, size: u32) -> u32;
        fn waveOutUnprepareHeader(output: Handle, header: *mut WaveHdr, size: u32) -> u32;
        fn waveOutReset(output: Handle) -> u32;
        fn waveOutClose(output: Handle) -> u32;
    }
    #[link(name = "shell32")]
    extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(app_id: *const u16) -> i32;
        fn ExtractIconExW(
            file_name: *const u16,
            icon_index: i32,
            large_icons: *mut Handle,
            small_icons: *mut Handle,
            icon_count: u32,
        ) -> u32;
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

    struct FullscreenState {
        active: bool,
        window_style: i32,
        window_rect: Rect,
    }

    impl FullscreenState {
        fn new(hwnd: Hwnd) -> Self {
            let mut window_rect: Rect = unsafe { zeroed() };
            unsafe { GetWindowRect(hwnd, &mut window_rect); }
            Self {
                active: false,
                window_style: unsafe { GetWindowLongW(hwnd, GWL_STYLE) },
                window_rect,
            }
        }

        fn toggle(&mut self, hwnd: Hwnd) -> Result<(), String> {
            if !self.active {
                let mut window_rect: Rect = unsafe { zeroed() };
                if unsafe { GetWindowRect(hwnd, &mut window_rect) } == 0 {
                    return Err("GetWindowRect failed while entering fullscreen".to_owned());
                }
                let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
                if monitor.is_null() {
                    return Err("MonitorFromWindow could not find the game display".to_owned());
                }
                let mut monitor_info = MonitorInfo {
                    size: size_of::<MonitorInfo>() as u32,
                    monitor: unsafe { zeroed() },
                    work: unsafe { zeroed() },
                    flags: 0,
                };
                if unsafe { GetMonitorInfoW(monitor, &mut monitor_info) } == 0 {
                    return Err("GetMonitorInfoW failed while entering fullscreen".to_owned());
                }
                let window_style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
                let fullscreen_style =
                    (window_style & !(WS_OVERLAPPEDWINDOW as i32)) | WS_POPUP as i32;
                unsafe {
                    SetWindowLongW(hwnd, GWL_STYLE, fullscreen_style);
                }
                let monitor_rect = monitor_info.monitor;
                let positioned = unsafe {
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        monitor_rect.left,
                        monitor_rect.top,
                        monitor_rect.right - monitor_rect.left,
                        monitor_rect.bottom - monitor_rect.top,
                        SWP_FRAMECHANGED | SWP_SHOWWINDOW,
                    )
                };
                if positioned == 0 {
                    unsafe { SetWindowLongW(hwnd, GWL_STYLE, window_style); }
                    return Err("SetWindowPos failed while entering fullscreen".to_owned());
                }
                self.window_rect = window_rect;
                self.window_style = window_style;
                self.active = true;
            } else {
                unsafe {
                    SetWindowLongW(hwnd, GWL_STYLE, self.window_style);
                }
                let restored = unsafe {
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        self.window_rect.left,
                        self.window_rect.top,
                        self.window_rect.right - self.window_rect.left,
                        self.window_rect.bottom - self.window_rect.top,
                        SWP_FRAMECHANGED | SWP_SHOWWINDOW,
                    )
                };
                if restored == 0 {
                    return Err("SetWindowPos failed while restoring the game window".to_owned());
                }
                self.active = false;
            }
            Ok(())
        }
    }

    fn gamepad_state(index: u32) -> u8 {
        const ERROR_SUCCESS: u32 = 0;
        const DPAD_UP: u16 = 0x0001;
        const DPAD_DOWN: u16 = 0x0002;
        const DPAD_LEFT: u16 = 0x0004;
        const DPAD_RIGHT: u16 = 0x0008;
        const START: u16 = 0x0010;
        const BACK: u16 = 0x0020;
        const A: u16 = 0x1000;
        const B: u16 = 0x2000;
        let mut state: XInputState = unsafe { zeroed() };
        if unsafe { XInputGetState(index, &mut state) } != ERROR_SUCCESS {
            return 0;
        }
        let buttons = state.gamepad.buttons;
        let mut nes = 0u8;
        if buttons & A != 0 { nes |= 1 << 0; }
        if buttons & B != 0 { nes |= 1 << 1; }
        if buttons & BACK != 0 { nes |= 1 << 2; }
        if buttons & START != 0 { nes |= 1 << 3; }
        if buttons & DPAD_UP != 0 || state.gamepad.thumb_ly > 16_000 { nes |= 1 << 4; }
        if buttons & DPAD_DOWN != 0 || state.gamepad.thumb_ly < -16_000 { nes |= 1 << 5; }
        if buttons & DPAD_LEFT != 0 || state.gamepad.thumb_lx < -16_000 { nes |= 1 << 6; }
        if buttons & DPAD_RIGHT != 0 || state.gamepad.thumb_lx > 16_000 { nes |= 1 << 7; }
        nes
    }

    fn controller_state(focused: bool, keys: &[i32; 8]) -> u8 {
        if !focused { return 0; }
        let mut state = 0u8;
        for (button, key) in keys.iter().copied().enumerate() {
            let pressed = key_down(key);
            if pressed {
                state |= 1 << button;
            }
        }
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

    struct AudioBuffer {
        pcm: Vec<i16>,
        header: WaveHdr,
    }

    struct AudioOutput {
        handle: HWaveOut,
        previous_input: f32,
        previous_output: f32,
        queued: VecDeque<Box<AudioBuffer>>,
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
            Ok(Self { handle, previous_input: 0.0, previous_output: 0.0, queued: VecDeque::with_capacity(8) })
        }

        fn reap_completed(&mut self) -> Result<(), String> {
            let header_size = size_of::<WaveHdr>() as u32;
            while self.queued.front().is_some_and(|buffer| {
                (unsafe { std::ptr::read_volatile(&buffer.header.flags) }) & WHDR_DONE != 0
            }) {
                let mut buffer = self.queued.pop_front().expect("front was checked");
                let result = unsafe { waveOutUnprepareHeader(self.handle, &mut buffer.header, header_size) };
                if result != 0 { return Err(format!("waveOutUnprepareHeader failed ({result})")); }
            }
            Ok(())
        }

        fn play(&mut self, samples: &[f32]) -> Result<(), String> {
            self.reap_completed()?;
            if samples.is_empty() { return Ok(()); }
            // Never wait for a whole audio frame on the emulation thread.
            // Bound queued latency to roughly eight frames (about 133 ms).
            if self.queued.len() >= 8 { return Ok(()); }

            let mut pcm = Vec::with_capacity(samples.len());
            for &sample in samples {
                let input = sample.clamp(0.0, 1.0);
                let filtered = input - self.previous_input + 0.995 * self.previous_output;
                self.previous_input = input;
                self.previous_output = filtered;
                pcm.push((filtered.clamp(-1.0, 1.0) * 28_000.0) as i16);
            }
            let mut buffer = Box::new(AudioBuffer {
                pcm,
                header: WaveHdr {
                    data: null_mut(), buffer_length: 0, bytes_recorded: 0, user: 0,
                    flags: 0, loops: 0, next: null_mut(), reserved: 0,
                },
            });
            buffer.header.data = buffer.pcm.as_mut_ptr().cast();
            buffer.header.buffer_length = (buffer.pcm.len() * size_of::<i16>()) as u32;

            // Box keeps the WAVEHDR address stable until Windows finishes playback.
            let header_size = size_of::<WaveHdr>() as u32;
            let prepared = unsafe { waveOutPrepareHeader(self.handle, &mut buffer.header, header_size) };
            if prepared != 0 { return Err(format!("waveOutPrepareHeader failed ({prepared})")); }
            let written = unsafe { waveOutWrite(self.handle, &mut buffer.header, header_size) };
            if written != 0 {
                unsafe { waveOutUnprepareHeader(self.handle, &mut buffer.header, header_size); }
                return Err(format!("waveOutWrite failed ({written})"));
            }
            self.queued.push_back(buffer);
            Ok(())
        }
    }

    impl Drop for AudioOutput {
        fn drop(&mut self) {
            unsafe { waveOutReset(self.handle); }
            let header_size = size_of::<WaveHdr>() as u32;
            while let Some(mut buffer) = self.queued.pop_front() {
                unsafe { waveOutUnprepareHeader(self.handle, &mut buffer.header, header_size); }
            }
            unsafe { waveOutClose(self.handle); }
        }
    }

    /// Displays startup/runtime errors without opening a transient console window.
    pub fn report_error(error: &str) {
        let message: Vec<u16> = format!("Impossible de lancer le jeu NES : {error}\0")
            .encode_utf16()
            .collect();
        let title: Vec<u16> = "PeaceEmu NES — erreur\0".encode_utf16().collect();
        unsafe {
            MessageBoxW(null_mut(), message.as_ptr(), title.as_ptr(), 0x0000_0010);
        }
    }

    fn report_host_event(writer: &mut impl Write, event: &str) {
        let _ = writeln!(writer, "EVENT|{event}");
        let _ = writer.flush();
    }

    fn report_host_error(writer: &mut impl Write, command: &str, error: impl std::fmt::Display) {
        let message = error
            .to_string()
            .replace('|', "/")
            .replace(['\n', '\r'], " ");
        report_host_event(writer, &format!("ERROR|{command}|{message}"));
    }

    pub fn run() -> Result<(), String> {
        let app_id: Vec<u16> = "Likounsee.PeaceEmu\0".encode_utf16().collect();
        // SAFETY: app_id is a stable, NUL-terminated UTF-16 identifier for this call.
        let app_id_result = unsafe { SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr()) };
        if app_id_result < 0 {
            eprintln!("Could not set PeaceEmu Windows taskbar identity (HRESULT 0x{:08X}).", app_id_result as u32);
        }

        let mut args = std::env::args_os().skip(1);
        let path = args.next().ok_or_else(|| {
            "usage: peaceemu-nes-desktop <game.nes> [--scale 1..6] [--audio-rate 8000..192000] [--keys A,B,SELECT,START,UP,DOWN,LEFT,RIGHT] [--keys2 A,B,SELECT,START,UP,DOWN,LEFT,RIGHT] [--host-executable PeaceEmu.exe]".to_owned()
        })?;
        let mut scale: Option<i32> = None;
        let mut audio_rate = 44_100u32;
        let mut keys = [0x5A, 0x58, 0x10, 0x0D, 0x26, 0x28, 0x25, 0x27];
        let mut keys2 = [0x61, 0x63, 0x60, 0x6E, 0x68, 0x62, 0x64, 0x66];
        let mut host_executable: Option<std::path::PathBuf> = None;
        while let Some(argument) = args.next() {
            let value = args.next().ok_or_else(|| format!("missing value for {}", argument.to_string_lossy()))?;
            match argument.to_string_lossy().as_ref() {
                "--scale" => {
                    let parsed = value.to_string_lossy().parse::<i32>().map_err(|_| "invalid --scale value".to_owned())?;
                    if !(1..=6).contains(&parsed) { return Err("--scale must be between 1 and 6".into()); }
                    scale = Some(parsed);
                }
                "--audio-rate" => {
                    audio_rate = value.to_string_lossy().parse().map_err(|_| "invalid --audio-rate value".to_owned())?;
                    if !(8_000..=192_000).contains(&audio_rate) { return Err("--audio-rate must be between 8000 and 192000".into()); }
                }
                "--keys" => {
                    let parsed: Result<Vec<i32>, _> = value.to_string_lossy().split(',').map(str::parse).collect();
                    let parsed = parsed.map_err(|_| "--keys must contain eight comma-separated virtual-key codes".to_owned())?;
                    if parsed.len() != 8 || parsed.iter().any(|key| !(0..=255).contains(key)) {
                        return Err("--keys needs eight virtual-key codes from 0 to 255 in A,B,Select,Start,Up,Down,Left,Right order".into());
                    }
                    keys.copy_from_slice(&parsed);
                }
                "--keys2" => {
                    let parsed: Result<Vec<i32>, _> = value.to_string_lossy().split(',').map(str::parse).collect();
                    let parsed = parsed.map_err(|_| "--keys2 must contain eight comma-separated virtual-key codes".to_owned())?;
                    if parsed.len() != 8 || parsed.iter().any(|key| !(0..=255).contains(key)) {
                        return Err("--keys2 needs eight virtual-key codes from 0 to 255 in A,B,Select,Start,Up,Down,Left,Right order".into());
                    }
                    keys2.copy_from_slice(&parsed);
                }
                "--host-executable" => {
                    host_executable = Some(std::path::PathBuf::from(value));
                }
                _ => return Err(format!("unknown option: {}", argument.to_string_lossy())),
            }
        }
        let image = fs::read(&path).map_err(|e| format!("cannot read ROM: {e}"))?;
        let mut machine = NesSession::load(&image, NesConfig { audio_sample_rate: audio_rate }).map_err(|e| e.to_string())?;
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
        let mut state_path = std::path::PathBuf::from(&path);
        state_path.set_extension("state");
        if state_path.exists() {
            let state = fs::read(&state_path)
                .map_err(|error| format!("cannot read automatic save state: {error}"))?;
            machine
                .load_state(&state)
                .map_err(|error| format!("cannot restore automatic save state: {error}"))?;
        }

        let class_name: Vec<u16> = "PeaceEmuNesWindow\0".encode_utf16().collect();
        let title: Vec<u16> = format!("PeaceEmu-NES — {}", std::path::Path::new(&path).file_name().unwrap_or_default().to_string_lossy())
            .encode_utf16().chain(Some(0)).collect();
        let host_icons = host_executable.as_ref().and_then(|host_executable| {
            let wide_path: Vec<u16> = host_executable.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut large = null_mut();
            let mut small = null_mut();
            // SAFETY: wide_path is a valid NUL-terminated Windows path and both output
            // pointers refer to live local HICON slots for the duration of the call.
            let extracted = unsafe { ExtractIconExW(wide_path.as_ptr(), 0, &mut large, &mut small, 1) };
            (extracted > 0 && (!large.is_null() || !small.is_null())).then_some((large, small))
        });
        let large_icon = host_icons.map(|(large, _)| large).unwrap_or(null_mut());
        let instance = unsafe { GetModuleHandleW(null()) };
        let class = WndClassW {
            style: 0, wnd_proc: Some(window_proc), cls_extra: 0, wnd_extra: 0,
            instance, icon: large_icon, cursor: null_mut(),
            background: 4usize as Handle, menu_name: null(), class_name: class_name.as_ptr(),
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err("RegisterClassW failed".into());
        }
        // Use an explicit --scale only for manual/debug launches. PeaceEmu defaults
        // to a window sized for the current display instead of a fixed pixel size.
        let (initial_width, initial_height) = match scale {
            Some(scale) => (256 * scale, 240 * scale),
            None => adaptive_window_size(
                unsafe { GetSystemMetrics(0) },
                unsafe { GetSystemMetrics(1) },
            ),
        };
        let mut rect = Rect {
            left: 0, top: 0, right: initial_width, bottom: initial_height,
        };
        unsafe { AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, 0); }
        let hwnd = unsafe {
            CreateWindowExW(0, class_name.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT, CW_USEDEFAULT, rect.right - rect.left, rect.bottom - rect.top,
                null_mut(), null_mut(), instance, null_mut())
        };
        if hwnd.is_null() { return Err("CreateWindowExW failed".into()); }
        if let Some((large, small)) = host_icons {
            unsafe {
                if !large.is_null() {
                    SendMessageW(hwnd, WM_SETICON, ICON_BIG, large as isize);
                }
                if !small.is_null() {
                    SendMessageW(hwnd, WM_SETICON, ICON_SMALL, small as isize);
                }
            }
        }
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
            // The module is launched by the PeaceEmu desktop process. Explicitly
            // activate its window so the foreground-window guard below does not
            // discard every keyboard/gamepad input on the first frames.
            SetForegroundWindow(hwnd);
        }

        // The host controls this standalone child over newline-delimited stdin commands.
        let (command_tx, command_rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(command) if command.len() <= 128 => {
                        if command_tx.send(command).is_err() { break; }
                    }
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
        });
        let stdout = std::io::stdout();
        let mut host_events = stdout.lock();
        let mut paused = false;
        let mut remote_turbo = false;
        let mut remote_buttons = 0u8;
        let bitmap = make_bitmap();
        let mut fullscreen = FullscreenState::new(hwnd);
        let mut previous_f1 = false;
        let mut previous_f5 = false;
        let mut previous_f8 = false;
        let mut previous_p = false;
        let mut turbo_speed = 4.0_f32;
        let mut pixels = vec![0u8; 256 * 240 * 4];
        let frame_time = Duration::from_micros(16_639);
        let mut next_frame = Instant::now();
        let mut last_save = Instant::now();
        let mut running = true;
        let mut runtime_error = None;
        while running {
            let paused_before_iteration = paused;
            let mut msg: Msg = unsafe { zeroed() };
            while unsafe { PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) } != 0 {
                if msg.message == WM_QUIT { running = false; break; }
                let is_f11_press = msg.hwnd == hwnd
                    && (msg.message == WM_KEYDOWN || msg.message == WM_SYSKEYDOWN)
                    && msg.w_param == VK_F11 as usize
                    && (msg.l_param as u64 & (1_u64 << 30)) == 0;
                if is_f11_press {
                    match fullscreen.toggle(hwnd) {
                        Ok(()) => report_host_event(&mut host_events, "OK|FULLSCREEN"),
                        Err(error) => report_host_error(&mut host_events, "FULLSCREEN", error),
                    }
                    continue;
                }
                unsafe { TranslateMessage(&msg); DispatchMessageW(&msg); }
            }
            if !running { break; }

            while let Ok(command) = command_rx.try_recv() {
                match command.as_str() {
                    "PAUSE" => {
                        paused = true;
                        report_host_event(&mut host_events, "OK|PAUSE");
                    }
                    "RESUME" => {
                        paused = false;
                        report_host_event(&mut host_events, "OK|RESUME");
                    }
                    "TOGGLE_PAUSE" => {
                        paused = !paused;
                        report_host_event(&mut host_events, "OK|TOGGLE_PAUSE");
                    }
                    "RESET" => {
                        machine.reset();
                        next_frame = Instant::now();
                        report_host_event(&mut host_events, "OK|RESET");
                    }
                    "FULLSCREEN" => match fullscreen.toggle(hwnd) {
                        Ok(()) => report_host_event(&mut host_events, "OK|FULLSCREEN"),
                        Err(error) => report_host_error(&mut host_events, "FULLSCREEN", error),
                    },
                    "TURBO_ON" => {
                        remote_turbo = true;
                        report_host_event(&mut host_events, "OK|TURBO_ON");
                    }
                    "TURBO_OFF" => {
                        remote_turbo = false;
                        report_host_event(&mut host_events, "OK|TURBO_OFF");
                    }
                    "TOGGLE_TURBO" => {
                        remote_turbo = !remote_turbo;
                        report_host_event(
                            &mut host_events,
                            if remote_turbo { "OK|TURBO_ON" } else { "OK|TURBO_OFF" },
                        );
                    }
                    value if value.starts_with("TURBO_SPEED ") => {
                        match parse_turbo_speed(&value[12..]) {
                            Some(speed) => {
                                turbo_speed = speed;
                                report_host_event(&mut host_events, "OK|TURBO_SPEED");
                            }
                            None => report_host_event(
                                &mut host_events,
                                "ERROR|TURBO_SPEED|Expected a speed from 0.5 to 4.0",
                            ),
                        }
                    }
                    "SAVE_STATE" => match machine.save_state() {
                        Ok(bytes) => match fs::write(&state_path, bytes) {
                            Ok(()) => report_host_event(&mut host_events, "OK|SAVE_STATE"),
                            Err(error) => report_host_error(&mut host_events, "SAVE_STATE", error),
                        },
                        Err(error) => report_host_error(&mut host_events, "SAVE_STATE", error),
                    },
                    "LOAD_STATE" => match fs::read(&state_path) {
                        Ok(bytes) => match machine.load_state(&bytes) {
                            Ok(()) => {
                                next_frame = Instant::now();
                                report_host_event(&mut host_events, "OK|LOAD_STATE");
                            }
                            Err(error) => report_host_error(&mut host_events, "LOAD_STATE", error),
                        },
                        Err(error) => report_host_error(&mut host_events, "LOAD_STATE", error),
                    },
                    value if value.starts_with("BUTTONS ") => {
                        match value[8..].parse::<u8>() {
                            Ok(buttons) => {
                                remote_buttons = buttons;
                                report_host_event(&mut host_events, "OK|BUTTONS");
                            }
                            Err(error) => report_host_error(&mut host_events, "BUTTONS", error),
                        }
                    }
                    value if value.starts_with("KEYS ") => match parse_virtual_keys(&value[5..]) {
                        Some(mapping) => {
                            keys = mapping;
                            report_host_event(&mut host_events, "OK|KEYS");
                        }
                        None => report_host_event(&mut host_events, "ERROR|KEYS|Expected eight virtual-key codes from 0 to 255"),
                    },
                    value if value.starts_with("KEYS2 ") => match parse_virtual_keys(&value[6..]) {
                        Some(mapping) => {
                            keys2 = mapping;
                            report_host_event(&mut host_events, "OK|KEYS2");
                        }
                        None => report_host_event(&mut host_events, "ERROR|KEYS2|Expected eight virtual-key codes from 0 to 255"),
                    },
                    "QUIT" => {
                        report_host_event(&mut host_events, "OK|QUIT");
                        running = false;
                    }
                    _ => report_host_event(&mut host_events, "ERROR|COMMAND|Unknown command"),
                }
            }
            if !running { break; }

            let focused = unsafe { GetForegroundWindow() == hwnd };
            let f1_down = focused && key_down(VK_F1);
            let f5_down = focused && key_down(VK_F5);
            let f8_down = focused && key_down(VK_F8);
            let p_down = focused && key_down(VK_P);
            if f1_down && !previous_f1 { machine.reset(); next_frame = Instant::now(); }
            if f5_down && !previous_f5 {
                if let Ok(bytes) = machine.save_state() { let _ = fs::write(&state_path, bytes); }
            }
            if f8_down && !previous_f8 {
                if let Ok(bytes) = fs::read(&state_path) { let _ = machine.load_state(&bytes); }
            }
            if p_down && !previous_p { paused = !paused; }
            previous_f1 = f1_down;
            previous_f5 = f5_down;
            previous_f8 = f8_down;
            previous_p = p_down;
            if paused != paused_before_iteration {
                report_host_event(
                    &mut host_events,
                    if paused { "STATE|PAUSED" } else { "STATE|RUNNING" },
                );
            }

            let player_one = remote_buttons | controller_state(focused, &keys)
                | if focused { gamepad_state(0) } else { 0 };
            let player_two = controller_state(focused, &keys2)
                | if focused { gamepad_state(1) } else { 0 };
            machine.set_controller_state(0, player_one);
            machine.set_controller_state(1, player_two);

            let turbo = remote_turbo || (focused && key_down(VK_TAB));
            let playback_speed = if turbo { turbo_speed } else { 1.0 };
            let mut frame_output = None;
            if !paused {
                match machine.advance_frame() {
                    Ok(frame) => frame_output = Some(frame),
                    Err(error) => {
                        runtime_error = Some(format!("emulation stopped: {error}"));
                    }
                }
            }
            if runtime_error.is_some() { break; }
            let audio_samples = if let Some(frame) = frame_output {
                for (i, &index) in frame.palette_indices.iter().enumerate() {
                    let rgb = PALETTE[(index & 0x3F) as usize];
                    let at = i * 4;
                    pixels[at] = rgb[2];
                    pixels[at + 1] = rgb[1];
                    pixels[at + 2] = rgb[0];
                    pixels[at + 3] = 0;
                }
                frame.audio_samples
            } else {
                // Keep the last rendered frame visible while paused. Replacing it with
                // a zero-filled framebuffer made every pause command look like a crash.
                Vec::new()
            };
            if let Some(output) = audio.as_mut() {
                if let Err(error) = output.play(&audio_samples) {
                    eprintln!("audio disabled after playback error: {error}");
                    audio = None;
                }
            }
            let mut client: Rect = unsafe { zeroed() };
            if unsafe { GetClientRect(hwnd, &mut client) } != 0 {
                let dc = unsafe { GetDC(hwnd) };
                if !dc.is_null() {
                    unsafe {
                        // Clear the entire client area on every frame. The window is
                        // resized when entering/leaving fullscreen; without this clear,
                        // pixels from the previous destination rectangle can remain
                        // visible as a frozen strip along the edge. Use the stock
                        // BLACK_BRUSH so the letterbox/pillarbox areas are always black.
                        FillRect(dc, &client, 4usize as Handle);
                        let (x, y, width, height) = aspect_fit(client.right, client.bottom);
                        SetStretchBltMode(dc, COLORONCOLOR);
                        StretchDIBits(dc, x, y, width, height, 0, 0, 256, 240,
                            pixels.as_ptr().cast(), &bitmap, DIB_RGB_COLORS, SRCCOPY);
                        ReleaseDC(hwnd, dc);
                    }
                }
            }
            if machine.has_battery_backed_ram() && last_save.elapsed() >= Duration::from_secs(5) {
                match machine.save_battery_ram() {
                    Ok(save) => {
                        if let Err(error) = fs::write(&save_path, save) {
                            runtime_error = Some(format!("cannot autosave battery RAM: {error}"));
                            break;
                        }
                        last_save = Instant::now();
                    }
                    Err(error) => {
                        runtime_error = Some(format!("cannot autosave battery RAM: {error}"));
                        break;
                    }
                }
            }
            let target_frame_time = scaled_frame_interval(frame_time, playback_speed)
                .expect("the selected playback speed must remain in the supported range");
            next_frame += target_frame_time;
            let now = Instant::now();
            if next_frame > now {
                let sleep = next_frame - now;
                unsafe { Sleep((sleep.as_millis() as u32).clamp(1, 16)); }
            } else {
                next_frame = now;
            }
        }
        // The module's state file is an automatic resume point, independent of the
        // battery-backed RAM save. Closing the game window must not discard progress.
        let state = machine
            .save_state()
            .map_err(|error| format!("cannot create automatic save state: {error}"))?;
        fs::write(&state_path, state)
            .map_err(|error| format!("cannot write automatic save state: {error}"))?;
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
        win::report_error(&error);
        std::process::exit(1);
    }
}



#[cfg(test)]
mod frontend_geometry_tests {
    use super::{adaptive_window_size, aspect_fit, parse_turbo_speed, parse_virtual_keys, scaled_frame_interval};

    #[test]
    fn turbo_speed_accepts_the_supported_range_and_rejects_invalid_values() {
        assert_eq!(parse_turbo_speed("0.5"), Some(0.5));
        assert_eq!(parse_turbo_speed("1.7"), Some(1.7));
        assert_eq!(parse_turbo_speed("4.0"), Some(4.0));
        assert_eq!(parse_turbo_speed("0.4"), None);
        assert_eq!(parse_turbo_speed("4.1"), None);
        assert_eq!(parse_turbo_speed("NaN"), None);
        assert_eq!(parse_turbo_speed("inf"), None);
        assert_eq!(parse_turbo_speed("fast"), None);
    }

    #[test]
    fn turbo_speed_scales_the_frame_interval_correctly() {
        let frame = std::time::Duration::from_micros(16_639);
        assert_eq!(
            scaled_frame_interval(frame, 0.5).unwrap().as_micros(),
            33_278
        );
        assert_eq!(
            scaled_frame_interval(frame, 1.0).unwrap().as_micros(),
            16_639
        );
        assert_eq!(
            scaled_frame_interval(frame, 4.0).unwrap().as_micros(),
            4_159
        );
        assert_eq!(scaled_frame_interval(frame, 0.0), None);
    }

    #[test]
    fn parse_virtual_keys_requires_eight_valid_virtual_keys() {
        assert_eq!(
            parse_virtual_keys("90,88,32,13,38,40,37,39"),
            Some([90, 88, 32, 13, 38, 40, 37, 39])
        );
        assert_eq!(parse_virtual_keys("1,2,3"), None);
        assert_eq!(parse_virtual_keys("1,2,3,4,5,6,7,8,9"), None);
        assert_eq!(parse_virtual_keys("1,2,3,4,5,6,7,256"), None);
        assert_eq!(parse_virtual_keys("1,2,3,4,5,6,7,no"), None);
    }

    #[test]
    fn aspect_fit_uses_corrected_four_three_display_ratio_in_widescreen() {
        let (x, y, width, height) = aspect_fit(1920, 1080);
        assert_eq!((x, y, width, height), (240, 0, 1440, 1080));
        assert_eq!(i64::from(width) * 3, i64::from(height) * 4);
    }

    #[test]
    fn aspect_fit_letterboxes_tall_client_area() {
        let (x, y, width, height) = aspect_fit(800, 1000);
        assert_eq!((x, y, width, height), (0, 200, 800, 600));
        assert_eq!(i64::from(width) * 3, i64::from(height) * 4);
    }

    #[test]
    fn adaptive_window_size_fits_large_and_small_displays() {
        assert_eq!(adaptive_window_size(1920, 1080), (1152, 864));
        assert_eq!(adaptive_window_size(1280, 720), (768, 576));
        assert_eq!(adaptive_window_size(800, 600), (640, 480));
    }

    #[test]
    fn adaptive_window_size_falls_back_for_invalid_display_metrics() {
        assert_eq!(adaptive_window_size(0, 0), (768, 576));
    }

    #[test]
    fn aspect_fit_handles_zero_sized_client_area() {
        assert_eq!(aspect_fit(0, 720), (0, 0, 0, 0));
    }
}
