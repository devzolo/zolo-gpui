//! Development-only Win32 adapter. Test scenarios and process orchestration are Zolo.
//! Every input/capture/close operation checks HWND ownership against the live child PID.
use std::time::Duration;

// Armed only after a DOWN accepted by Win32. Cleanup sends only its matching UP,
// including when ownership/focus checks fail or Rust unwinds during a drag.
struct ReleaseOnDrop<F: FnMut() -> Result<(), String>> {
    release: F,
    armed: bool,
}
impl<F: FnMut() -> Result<(), String>> ReleaseOnDrop<F> {
    fn new(release: F) -> Self {
        Self {
            release,
            armed: true,
        }
    }
    fn release(&mut self) -> Result<(), String> {
        if !self.armed {
            return Ok(());
        }
        (self.release)()?;
        self.armed = false;
        Ok(())
    }
}
impl<F: FnMut() -> Result<(), String>> Drop for ReleaseOnDrop<F> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.release();
        }
    }
}

pub struct JobResult {
    pub token: i64,
    pub error: String,
}
pub struct ActionResult {
    pub error: String,
}
pub struct WindowInfo {
    pub hwnd: i64,
    pub title: String,
}
pub struct DesktopSnapshot {
    pub window: i64,
    pub x: i64,
    pub y: i64,
    pub error: String,
}
pub struct CaptureResult {
    pub path: String,
    pub method: String,
    pub width: i64,
    pub height: i64,
    pub colors: i64,
    pub error: String,
}

pub fn process_job(pid: i64) -> JobResult {
    match os::process_job(pid) {
        Ok(token) => JobResult {
            token,
            error: String::new(),
        },
        Err(error) => JobResult { token: 0, error },
    }
}
pub fn release_job(token: i64) -> ActionResult {
    action(os::release_job(token))
}
pub fn snapshot() -> DesktopSnapshot {
    os::snapshot()
}
pub fn restore(window: i64, x: i64, y: i64) -> ActionResult {
    action(os::restore(window, x, y))
}
pub fn windows(pid: i64, prefix: String) -> Vec<WindowInfo> {
    os::windows(pid, &prefix)
}
pub fn activate(window: i64, pid: i64) -> ActionResult {
    action(os::activate(window, pid))
}
pub fn click(window: i64, pid: i64, x: f64, y: f64) -> ActionResult {
    action(os::click(window, pid, x, y))
}
pub fn dev_click(window: i64, pid: i64, x: f64, y: f64) -> ActionResult {
    action(os::dev_click(window, pid, x, y))
}
pub fn wheel(window: i64, pid: i64, x: f64, y: f64, delta: i64) -> ActionResult {
    action(os::wheel(window, pid, x, y, delta))
}
pub fn drag(window: i64, pid: i64, x1: f64, y1: f64, x2: f64, y2: f64) -> ActionResult {
    action(os::drag(window, pid, x1, y1, x2, y2))
}
pub fn chord(window: i64, pid: i64, key: i64, control: bool, shift: bool) -> ActionResult {
    action(os::chord(window, pid, key, control, shift))
}
pub fn text(window: i64, pid: i64, value: String) -> ActionResult {
    action(os::text(window, pid, &value))
}
pub fn close(window: i64, pid: i64) -> ActionResult {
    action(os::close(window, pid))
}
pub fn capture(window: i64, pid: i64, path: String, print_window: bool) -> CaptureResult {
    match os::capture(window, pid, &path, print_window) {
        Ok((width, height, colors)) => CaptureResult {
            path,
            method: if print_window {
                "owned_client_printwindow"
            } else {
                "owned_client_dc"
            }
            .into(),
            width: width as i64,
            height: height as i64,
            colors: colors as i64,
            error: String::new(),
        },
        Err(error) => CaptureResult {
            path,
            method: String::new(),
            width: 0,
            height: 0,
            colors: 0,
            error,
        },
    }
}
fn action(value: Result<(), String>) -> ActionResult {
    ActionResult {
        error: value.err().unwrap_or_default(),
    }
}

// PNG encoding uses stored DEFLATE blocks: no image/GUI dependencies or deployment DLL.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
fn png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(tag);
    out.extend_from_slice(body);
    out.extend_from_slice(&crc32(&out[start..]).to_be_bytes());
}
fn encode_png(width: usize, height: usize, bgra: &[u8]) -> Result<Vec<u8>, String> {
    let size = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .ok_or("Owned window bitmap overflow")?;
    if width == 0 || height == 0 || size > 128 * 1024 * 1024 || bgra.len() != size {
        return Err("Invalid owned window bitmap dimensions".into());
    }
    let mut raw = Vec::with_capacity(size + height);
    for row in bgra.chunks_exact(width * 4) {
        raw.push(0);
        for pixel in row.chunks_exact(4) {
            raw.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
    }
    let mut zipped = vec![0x78, 0x01];
    let mut a = 1u32;
    let mut b = 0u32;
    for (index, block) in raw.chunks(65535).enumerate() {
        zipped.push(if (index + 1) * 65535 >= raw.len() {
            1
        } else {
            0
        });
        let n = block.len() as u16;
        zipped.extend_from_slice(&n.to_le_bytes());
        zipped.extend_from_slice(&(!n).to_le_bytes());
        zipped.extend_from_slice(block);
        for byte in block {
            a = (a + u32::from(*byte)) % 65521;
            b = (b + a) % 65521;
        }
    }
    zipped.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    png_chunk(&mut out, b"IHDR", &header);
    png_chunk(&mut out, b"IDAT", &zipped);
    png_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

#[cfg(windows)]
mod os {
    use super::*;
    use std::{
        collections::HashSet,
        ffi::c_void,
        mem::{size_of, zeroed},
        ptr::null_mut,
        thread::sleep,
    };
    type Handle = *mut c_void;
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MouseInput {
        x: i32,
        y: i32,
        data: u32,
        flags: u32,
        time: u32,
        extra: usize,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeyboardInput {
        key: u16,
        scan: u16,
        flags: u32,
        time: u32,
        extra: usize,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    union InputUnion {
        mouse: MouseInput,
        keyboard: KeyboardInput,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Input {
        kind: u32,
        data: InputUnion,
    }
    #[repr(C)]
    struct BitmapHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bits: u16,
        compression: u32,
        image_size: u32,
        xppm: i32,
        yppm: i32,
        colors: u32,
        important: u32,
    }
    #[repr(C)]
    struct BitmapInfo {
        header: BitmapHeader,
        color: [u32; 1],
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(Handle, isize) -> i32,
            param: isize,
        ) -> i32;
        fn IsWindowVisible(window: Handle) -> i32;
        fn IsWindow(window: Handle) -> i32;
        fn GetWindowThreadProcessId(window: Handle, pid: *mut u32) -> u32;
        fn GetWindowTextW(window: Handle, text: *mut u16, size: i32) -> i32;
        fn GetClassNameW(window: Handle, text: *mut u16, size: i32) -> i32;
        fn GetClientRect(window: Handle, rect: *mut Rect) -> i32;
        fn GetWindowRect(window: Handle, rect: *mut Rect) -> i32;
        fn ClientToScreen(window: Handle, point: *mut Point) -> i32;
        fn GetDpiForWindow(window: Handle) -> u32;
        fn SetThreadDpiAwarenessContext(context: Handle) -> Handle;
        fn GetForegroundWindow() -> Handle;
        fn WindowFromPoint(point: Point) -> Handle;
        fn GetAncestor(window: Handle, flags: u32) -> Handle;
        fn SetForegroundWindow(window: Handle) -> i32;
        fn BringWindowToTop(window: Handle) -> i32;
        fn IsIconic(window: Handle) -> i32;
        fn ShowWindow(window: Handle, command: i32) -> i32;
        fn AttachThreadInput(thread: u32, other: u32, attach: i32) -> i32;
        fn GetPhysicalCursorPos(point: *mut Point) -> i32;
        fn SetPhysicalCursorPos(x: i32, y: i32) -> i32;
        fn SendInput(count: u32, inputs: *const Input, size: i32) -> u32;
        fn PostMessageW(window: Handle, message: u32, w: usize, l: isize) -> i32;
        fn GetDC(window: Handle) -> Handle;
        fn ReleaseDC(window: Handle, dc: Handle) -> i32;
        fn PrintWindow(window: Handle, dc: Handle, flags: u32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, value: *const c_void, size: u32)
            -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn CloseHandle(value: Handle) -> i32;
        fn WaitForSingleObject(value: Handle, milliseconds: u32) -> u32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, value: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: Handle, value: *mut ProcessEntry) -> i32;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateCompatibleDC(dc: Handle) -> Handle;
        fn CreateCompatibleBitmap(dc: Handle, width: i32, height: i32) -> Handle;
        fn SelectObject(dc: Handle, value: Handle) -> Handle;
        fn DeleteObject(value: Handle) -> i32;
        fn DeleteDC(dc: Handle) -> i32;
        fn BitBlt(
            destination: Handle,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            source: Handle,
            sx: i32,
            sy: i32,
            operation: u32,
        ) -> i32;
        fn GetDIBits(
            dc: Handle,
            bitmap: Handle,
            first: u32,
            count: u32,
            bytes: *mut c_void,
            info: *mut BitmapInfo,
            usage: u32,
        ) -> i32;
    }
    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        pid: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        priority: i32,
        flags: u32,
        exe: [u16; 260],
    }
    #[repr(C)]
    struct BasicLimit {
        process_time: i64,
        job_time: i64,
        flags: u32,
        min_working: usize,
        max_working: usize,
        active: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    struct ExtendedLimit {
        basic: BasicLimit,
        io: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process: usize,
        peak_job: usize,
    }
    // Finite script runs release tokens explicitly with defer. Provider hot-unload
    // is not a supported lifecycle; process exit also closes remaining OS handles.
    static JOBS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<i64, usize>>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    static NEXT_JOB: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
    struct KernelHandle(Handle);
    impl Drop for KernelHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != -1isize as Handle {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }
    fn child_of_executor(pid: u32) -> Result<(), String> {
        let snapshot = KernelHandle(unsafe { CreateToolhelp32Snapshot(2, 0) });
        if snapshot.0 == -1isize as Handle {
            return Err(last_error("Owned child ancestry snapshot"));
        }
        let mut entry: ProcessEntry = unsafe { zeroed() };
        entry.size = size_of::<ProcessEntry>() as u32;
        let mut present = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
        while present {
            if entry.pid == pid {
                return if entry.parent == std::process::id() {
                    Ok(())
                } else {
                    Err("Job target is not a direct child of the executor".into())
                };
            }
            present = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
        }
        Err("Owned child exited before job assignment".into())
    }
    // Admission occurs immediately after spawn; it does not claim atomic suspended-spawn assignment.
    pub(super) fn process_job(pid: i64) -> Result<i64, String> {
        if pid <= 0 || pid > u32::MAX as i64 {
            return Err("Invalid owned child PID".into());
        }
        let process = KernelHandle(unsafe { OpenProcess(0x100 | 0x001 | 0x1000, 0, pid as u32) });
        if process.0.is_null() {
            return Err(last_error("Open owned child"));
        }
        child_of_executor(pid as u32)?;
        let job = KernelHandle(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) });
        if job.0.is_null() {
            return Err(last_error("Create owned process job"));
        }
        let mut limits: ExtendedLimit = unsafe { zeroed() };
        limits.basic.flags = 0x2000;
        if unsafe {
            SetInformationJobObject(
                job.0,
                9,
                &limits as *const _ as *const c_void,
                size_of::<ExtendedLimit>() as u32,
            )
        } == 0
        {
            return Err(last_error("Configure kill-on-close owned process job"));
        }
        if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
            return Err(last_error("Assign owned child to process job"));
        }
        let token = NEXT_JOB.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        JOBS.lock()
            .map_err(|_| "Owned process job registry poisoned")?
            .insert(token, job.0 as usize);
        std::mem::forget(job);
        Ok(token)
    }
    pub(super) fn release_job(token: i64) -> Result<(), String> {
        if let Some(value) = JOBS
            .lock()
            .map_err(|_| "Owned process job registry poisoned")?
            .remove(&token)
        {
            if unsafe { CloseHandle(value as Handle) } == 0 {
                return Err(last_error("Close owned process job"));
            }
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn descendant_job_cleanup() {
        use std::io::{BufRead, BufReader, Write};
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "tests::job_child_helper",
                "--nocapture",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert!(output.read_line(&mut line).unwrap() > 0);
            if line.contains("JOB_CHILD_READY") {
                break;
            }
        }
        let token = process_job(child.id() as i64).unwrap();
        writeln!(child.stdin.as_mut().unwrap(), "admitted").unwrap();
        let descendant_pid = loop {
            line.clear();
            assert!(output.read_line(&mut line).unwrap() > 0);
            if let Some(pid) = line.trim().strip_prefix("JOB_DESCENDANT:") {
                break pid.parse::<u32>().unwrap();
            }
        };
        let descendant = KernelHandle(unsafe { OpenProcess(0x100000 | 0x1000, 0, descendant_pid) });
        assert!(!descendant.0.is_null());
        release_job(token).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(descendant.0, 5000) },
            0,
            "admitted descendant survived closing its owning Job"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "admitted root child survived Job close"
            );
            sleep(Duration::from_millis(10));
        }
        release_job(token).unwrap();
    }
    fn hwnd(window: i64) -> Handle {
        window as usize as Handle
    }
    fn last_error(label: &str) -> String {
        format!("{label}: {}", std::io::Error::last_os_error())
    }
    fn owner(window: i64, pid: i64) -> Result<Handle, String> {
        let window = hwnd(window);
        let mut actual = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut actual);
        }
        if pid <= 0
            || pid > u32::MAX as i64
            || window.is_null()
            || unsafe { IsWindow(window) } == 0
            || actual != pid as u32
        {
            return Err("Target window is not owned by the child process".into());
        }
        Ok(window)
    }
    fn input_target(window: i64, pid: i64) -> Result<Handle, String> {
        let window = owner(window, pid)?;
        if unsafe { GetForegroundWindow() } != window {
            return Err("Owned window lost foreground focus; no input was sent".into());
        }
        Ok(window)
    }
    struct Dpi(Handle);
    impl Dpi {
        fn enter() -> Self {
            Self(unsafe { SetThreadDpiAwarenessContext(-4isize as Handle) })
        }
    }
    impl Drop for Dpi {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    SetThreadDpiAwarenessContext(self.0);
                }
            }
        }
    }
    fn foreground(window: Handle) {
        unsafe {
            let previous = GetForegroundWindow();
            let mut ignored = 0;
            let prev_thread = GetWindowThreadProcessId(previous, &mut ignored);
            let owned_thread = GetWindowThreadProcessId(window, &mut ignored);
            let calling = GetCurrentThreadId();
            let attach_previous = prev_thread != 0
                && prev_thread != calling
                && AttachThreadInput(calling, prev_thread, 1) != 0;
            let attach_owned = owned_thread != 0
                && owned_thread != calling
                && owned_thread != prev_thread
                && AttachThreadInput(calling, owned_thread, 1) != 0;
            if IsIconic(window) != 0 {
                ShowWindow(window, 9);
            }
            BringWindowToTop(window);
            SetForegroundWindow(window);
            if attach_owned {
                AttachThreadInput(calling, owned_thread, 0);
            }
            if attach_previous {
                AttachThreadInput(calling, prev_thread, 0);
            }
        }
    }
    pub(super) fn activate(window: i64, pid: i64) -> Result<(), String> {
        let owned = owner(window, pid)?;
        foreground(owned);
        sleep(Duration::from_millis(60));
        if unsafe { GetForegroundWindow() } != owned {
            return Err("Windows did not grant focus to the owned child; no input was sent".into());
        }
        Ok(())
    }
    pub(super) fn snapshot() -> DesktopSnapshot {
        let mut point = Point::default();
        let error = if unsafe { GetPhysicalCursorPos(&mut point) } == 0 {
            last_error("GetPhysicalCursorPos")
        } else {
            String::new()
        };
        DesktopSnapshot {
            window: unsafe { GetForegroundWindow() } as usize as i64,
            x: point.x as i64,
            y: point.y as i64,
            error,
        }
    }
    pub(super) fn restore(window: i64, x: i64, y: i64) -> Result<(), String> {
        let moved = unsafe { SetPhysicalCursorPos(x as i32, y as i32) } != 0;
        let window = hwnd(window);
        if !window.is_null() && unsafe { IsWindow(window) } != 0 {
            foreground(window);
        }
        if moved {
            Ok(())
        } else {
            Err(last_error("Restore pointer"))
        }
    }
    pub(super) fn windows(pid: i64, prefix: &str) -> Vec<WindowInfo> {
        struct Query<'a> {
            pid: i64,
            prefix: &'a str,
            result: Vec<WindowInfo>,
        }
        unsafe extern "system" fn enumerate(window: Handle, pointer: isize) -> i32 {
            let query = unsafe { &mut *(pointer as *mut Query<'_>) };
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(window, &mut pid);
            }
            if query.pid > 0
                && query.pid <= u32::MAX as i64
                && pid == query.pid as u32
                && unsafe { IsWindowVisible(window) } != 0
            {
                let mut chars = [0u16; 1024];
                let count =
                    unsafe { GetWindowTextW(window, chars.as_mut_ptr(), chars.len() as i32) };
                let title = String::from_utf16_lossy(&chars[..count.max(0) as usize]);
                if title.starts_with(query.prefix) {
                    query.result.push(WindowInfo {
                        hwnd: window as usize as i64,
                        title,
                    });
                }
            }
            1
        }
        let mut query = Query {
            pid,
            prefix,
            result: Vec::new(),
        };
        unsafe {
            EnumWindows(enumerate, &mut query as *mut _ as isize);
        }
        query.result
    }
    fn client_point(window: Handle, x: f64, y: f64, screen: bool) -> Result<Point, String> {
        let _dpi = Dpi::enter();
        let dpi = unsafe { GetDpiForWindow(window) };
        let scale = if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 };
        if !x.is_finite() || !y.is_finite() {
            return Err("Fixture coordinates must be finite".into());
        }
        let mut point = Point {
            x: (x * scale).round() as i32,
            y: (y * scale).round() as i32,
        };
        let mut rect = Rect::default();
        if unsafe { GetClientRect(window, &mut rect) } == 0
            || point.x < 0
            || point.y < 0
            || point.x >= rect.right
            || point.y >= rect.bottom
        {
            return Err("Fixture control is outside the owned client area".into());
        }
        if screen && unsafe { ClientToScreen(window, &mut point) } == 0 {
            return Err(last_error("ClientToScreen"));
        }
        Ok(point)
    }
    fn inject(window: i64, pid: i64, inputs: &[Input]) -> Result<(), String> {
        input_target(window, pid)?;
        if inputs.iter().any(|input| input.kind == 0) {
            verify_pointer(window, pid, None)?;
        }
        if unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                size_of::<Input>() as i32,
            )
        } != inputs.len() as u32
        {
            return Err(last_error(
                "SendInput did not inject all owned-window input",
            ));
        }
        Ok(())
    }
    fn key(key: u16, up: bool) -> Input {
        let mut result: Input = unsafe { zeroed() };
        result.kind = 1;
        result.data.keyboard = KeyboardInput {
            key,
            scan: 0,
            flags: if up { 2 } else { 0 },
            time: 0,
            extra: 0,
        };
        result
    }
    fn mouse(up: bool) -> Input {
        let mut result: Input = unsafe { zeroed() };
        result.data.mouse = MouseInput {
            x: 0,
            y: 0,
            data: 0,
            flags: if up { 4 } else { 2 },
            time: 0,
            extra: 0,
        };
        result
    }
    fn release_mouse() -> Result<(), String> {
        // This is cleanup of our accepted DOWN, never a new DOWN or movement.
        // GPUI captures the button on DOWN; refusing UP after an obstruction
        // would leave global injected state pressed after the gate aborts.
        let up = mouse(true);
        if unsafe { SendInput(1, &up, size_of::<Input>() as i32) } != 1 {
            Err(last_error(
                "Could not release the mouse pressed by this gate",
            ))
        } else {
            Ok(())
        }
    }
    fn pointer(window: i64, pid: i64, point: Point) -> Result<(), String> {
        input_target(window, pid)?;
        let _dpi = Dpi::enter();
        if unsafe { SetPhysicalCursorPos(point.x, point.y) } == 0 {
            return Err(last_error("SetPhysicalCursorPos"));
        }
        verify_pointer(window, pid, Some(point))
    }
    fn verify_pointer(window: i64, pid: i64, expected: Option<Point>) -> Result<(), String> {
        input_target(window, pid)?;
        let _dpi = Dpi::enter();
        let mut physical = Point::default();
        if unsafe { GetPhysicalCursorPos(&mut physical) } == 0 {
            return Err(last_error("GetPhysicalCursorPos"));
        }
        if expected.is_some_and(|point| physical.x != point.x || physical.y != point.y) {
            return Err(
                "The pointer moved away from the owned control; no mouse input was sent".into(),
            );
        }
        let under_pointer = unsafe { WindowFromPoint(physical) };
        let top_level = unsafe { GetAncestor(under_pointer, 2) };
        if top_level != owner(window, pid)? {
            let mut rect = Rect::default();
            let mut hit_pid = 0;
            let mut class_name = [0u16; 256];
            let class_size =
                unsafe { GetClassNameW(top_level, class_name.as_mut_ptr(), 256) }.max(0) as usize;
            let class_name = String::from_utf16_lossy(&class_name[..class_size]);
            unsafe {
                GetWindowRect(hwnd(window), &mut rect);
                GetWindowThreadProcessId(top_level, &mut hit_pid);
            }
            return Err(format!("Owned control is obscured; no mouse input sent: owned={window} owned_pid={pid} root={} root_pid={hit_pid} class={class_name} physical={},{} owned_rect={},{},{},{} dpi_context={}", top_level as usize, physical.x, physical.y, rect.left, rect.top, rect.right, rect.bottom, _dpi.0 as usize));
        }
        Ok(())
    }
    pub(super) fn click(window: i64, pid: i64, x: f64, y: f64) -> Result<(), String> {
        activate(window, pid)?;
        let target = client_point(owner(window, pid)?, x, y, true)?;
        pointer(window, pid, target)?;
        sleep(Duration::from_millis(30));
        verify_pointer(window, pid, Some(target))?;
        inject(window, pid, &[mouse(false)])?;
        let mut pressed = ReleaseOnDrop::new(release_mouse);
        pressed.release()
    }
    pub(super) fn dev_click(window: i64, pid: i64, x: f64, y: f64) -> Result<(), String> {
        let owned = owner(window, pid)?;
        let point = client_point(owned, x, y, false)?;
        let position = ((point.y as u32) << 16 | (point.x as u32 & 0xffff)) as isize;
        for (message, w) in [(0x200, 0), (0x201, 1), (0x202, 0)] {
            owner(window, pid)?;
            if unsafe { PostMessageW(owned, message, w, position) } == 0 {
                return Err(last_error("Owned dev click"));
            }
        }
        Ok(())
    }
    pub(super) fn wheel(window: i64, pid: i64, x: f64, y: f64, delta: i64) -> Result<(), String> {
        activate(window, pid)?;
        let target = client_point(owner(window, pid)?, x, y, true)?;
        pointer(window, pid, target)?;
        sleep(Duration::from_millis(30));
        verify_pointer(window, pid, Some(target))?;
        let mut input = mouse(false);
        input.data.mouse.flags = 0x800;
        input.data.mouse.data = delta as u32;
        inject(window, pid, &[input])
    }
    pub(super) fn drag(
        window: i64,
        pid: i64,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    ) -> Result<(), String> {
        activate(window, pid)?;
        let owned = owner(window, pid)?;
        let first = client_point(owned, x1, y1, true)?;
        let last = client_point(owned, x2, y2, true)?;
        pointer(window, pid, first)?;
        sleep(Duration::from_millis(40));
        verify_pointer(window, pid, Some(first))?;
        inject(window, pid, &[mouse(false)])?;
        let mut pressed = ReleaseOnDrop::new(release_mouse);
        let moved = (|| {
            for step in 1..=8 {
                input_target(window, pid)?;
                pointer(
                    window,
                    pid,
                    Point {
                        x: first.x + (last.x - first.x) * step / 8,
                        y: first.y + (last.y - first.y) * step / 8,
                    },
                )?;
                sleep(Duration::from_millis(30));
            }
            Ok(())
        })();
        let released = pressed.release();
        moved.and(released)
    }
    pub(super) fn chord(
        window: i64,
        pid: i64,
        code: i64,
        control: bool,
        shift: bool,
    ) -> Result<(), String> {
        input_target(window, pid)?;
        if !(0..=65535).contains(&code) {
            return Err("Key is outside UTF16/virtual-key range".into());
        }
        let mut inputs = Vec::new();
        if control {
            inputs.push(key(0x11, false));
        }
        if shift {
            inputs.push(key(0x10, false));
        }
        inputs.push(key(code as u16, false));
        inputs.push(key(code as u16, true));
        if shift {
            inputs.push(key(0x10, true));
        }
        if control {
            inputs.push(key(0x11, true));
        }
        inject(window, pid, &inputs)
    }
    pub(super) fn text(window: i64, pid: i64, value: &str) -> Result<(), String> {
        input_target(window, pid)?;
        for unit in value.encode_utf16() {
            let mut down = key(0, false);
            down.data.keyboard.scan = unit;
            down.data.keyboard.flags = 4;
            let mut up = down;
            up.data.keyboard.flags = 6;
            inject(window, pid, &[down, up])?;
            sleep(Duration::from_millis(25));
        }
        Ok(())
    }
    pub(super) fn close(window: i64, pid: i64) -> Result<(), String> {
        let owned = owner(window, pid)?;
        if unsafe { PostMessageW(owned, 0x10, 0, 0) } == 0 {
            Err(last_error("Close owned window"))
        } else {
            Ok(())
        }
    }
    struct CaptureObjects {
        window: Handle,
        source: Handle,
        destination: Handle,
        bitmap: Handle,
        old: Handle,
    }
    impl Drop for CaptureObjects {
        fn drop(&mut self) {
            unsafe {
                if !self.old.is_null() && !self.destination.is_null() {
                    SelectObject(self.destination, self.old);
                }
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap);
                }
                if !self.destination.is_null() {
                    DeleteDC(self.destination);
                }
                if !self.source.is_null() {
                    ReleaseDC(self.window, self.source);
                }
            }
        }
    }
    pub(super) fn capture(
        window: i64,
        pid: i64,
        path: &str,
        print_window: bool,
    ) -> Result<(usize, usize, usize), String> {
        activate(window, pid)?;
        let _dpi = Dpi::enter();
        let owned = input_target(window, pid)?;
        let mut rect = Rect::default();
        if unsafe { GetClientRect(owned, &mut rect) } == 0 || rect.right <= 0 || rect.bottom <= 0 {
            return Err(last_error("Owned client rectangle"));
        }
        let (width, height) = (rect.right as usize, rect.bottom as usize);
        let size = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .ok_or("Owned window bitmap overflow")?;
        if size > 128 * 1024 * 1024 {
            return Err("Owned window capture exceeded 128 MiB".into());
        }
        let mut objects = CaptureObjects {
            window: owned,
            source: null_mut(),
            destination: null_mut(),
            bitmap: null_mut(),
            old: null_mut(),
        };
        objects.source = unsafe { GetDC(owned) };
        objects.destination = unsafe { CreateCompatibleDC(objects.source) };
        objects.bitmap = unsafe { CreateCompatibleBitmap(objects.source, rect.right, rect.bottom) };
        if objects.source.is_null() || objects.destination.is_null() || objects.bitmap.is_null() {
            return Err(last_error("Owned bitmap allocation"));
        }
        objects.old = unsafe { SelectObject(objects.destination, objects.bitmap) };
        input_target(window, pid)?;
        let captured = unsafe {
            if print_window {
                PrintWindow(owned, objects.destination, 3)
            } else {
                BitBlt(
                    objects.destination,
                    0,
                    0,
                    rect.right,
                    rect.bottom,
                    objects.source,
                    0,
                    0,
                    0x00cc0020,
                )
            }
        };
        if captured == 0 {
            return Err(last_error("Could not capture the owned child window"));
        }
        unsafe {
            SelectObject(objects.destination, objects.old);
        }
        objects.old = null_mut();
        let mut bytes = vec![0u8; size];
        let mut info: BitmapInfo = unsafe { zeroed() };
        info.header.size = size_of::<BitmapHeader>() as u32;
        info.header.width = rect.right;
        info.header.height = -rect.bottom;
        info.header.planes = 1;
        info.header.bits = 32;
        if unsafe {
            GetDIBits(
                objects.destination,
                objects.bitmap,
                0,
                rect.bottom as u32,
                bytes.as_mut_ptr().cast(),
                &mut info,
                0,
            )
        } != rect.bottom
        {
            return Err(last_error("Owned bitmap pixels"));
        }
        let mut colors = HashSet::new();
        for y in (12..height).step_by(19) {
            for x in (12..width).step_by(19) {
                let at = (y * width + x) * 4;
                colors.insert([bytes[at], bytes[at + 1], bytes[at + 2]]);
            }
        }
        std::fs::write(path, encode_png(width, height, &bytes)?)
            .map_err(|error| error.to_string())?;
        Ok((width, height, colors.len()))
    }
}

#[cfg(not(windows))]
mod os {
    use super::*;
    fn unavailable() -> Result<(), String> {
        Err("This gate requires an interactive Windows desktop".into())
    }
    pub(super) fn process_job(_: i64) -> Result<i64, String> {
        Err(unavailable().unwrap_err())
    }
    pub(super) fn release_job(_: i64) -> Result<(), String> {
        unavailable()
    }
    pub(super) fn snapshot() -> DesktopSnapshot {
        DesktopSnapshot {
            window: 0,
            x: 0,
            y: 0,
            error: unavailable().unwrap_err(),
        }
    }
    pub(super) fn restore(_: i64, _: i64, _: i64) -> Result<(), String> {
        unavailable()
    }
    pub(super) fn windows(_: i64, _: &str) -> Vec<WindowInfo> {
        Vec::new()
    }
    pub(super) fn activate(_: i64, _: i64) -> Result<(), String> {
        unavailable()
    }
    pub(super) fn click(_: i64, _: i64, _: f64, _: f64) -> Result<(), String> {
        unavailable()
    }
    pub(super) fn dev_click(_: i64, _: i64, _: f64, _: f64) -> Result<(), String> {
        unavailable()
    }
    pub(super) fn wheel(_: i64, _: i64, _: f64, _: f64, _: i64) -> Result<(), String> {
        unavailable()
    }
    pub(super) fn drag(_: i64, _: i64, _: f64, _: f64, _: f64, _: f64) -> Result<(), String> {
        unavailable()
    }
    pub(super) fn chord(_: i64, _: i64, _: i64, _: bool, _: bool) -> Result<(), String> {
        unavailable()
    }
    pub(super) fn text(_: i64, _: i64, _: &str) -> Result<(), String> {
        unavailable()
    }
    pub(super) fn close(_: i64, _: i64) -> Result<(), String> {
        unavailable()
    }
    pub(super) fn capture(
        _: i64,
        _: i64,
        _: &str,
        _: bool,
    ) -> Result<(usize, usize, usize), String> {
        Err(unavailable().unwrap_err())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepted_mouse_down_releases_on_error_unwind_and_only_once_on_success() {
        use std::cell::Cell;
        let releases = Cell::new(0);
        let release = || {
            releases.set(releases.get() + 1);
            Ok(())
        };
        {
            let _pressed = ReleaseOnDrop::new(release);
            let failed: Result<(), String> = Err("guard lost ownership".into());
            assert!(failed.is_err());
        }
        assert_eq!(releases.get(), 1);
        {
            let mut pressed = ReleaseOnDrop::new(release);
            pressed.release().unwrap();
        }
        assert_eq!(
            releases.get(),
            2,
            "successful explicit UP must not be repeated by Drop"
        );
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _pressed = ReleaseOnDrop::new(release);
            panic!("simulated drag unwind");
        }));
        assert!(outcome.is_err());
        assert_eq!(
            releases.get(),
            3,
            "unwind must release our accepted DOWN without another ownership gate"
        );
    }

    #[test]
    fn invalid_window_rejects_every_input_before_native_injection() {
        let pid = std::process::id() as i64;
        assert!(!click(0, pid, 1.0, 1.0).error.is_empty());
        assert!(!dev_click(0, pid, 1.0, 1.0).error.is_empty());
        assert!(!chord(0, pid, 13, true, false).error.is_empty());
        assert!(!text(0, pid, "a🦀".into()).error.is_empty());
        assert!(!close(0, pid).error.is_empty());
        assert!(!capture(0, pid, "never-created.png".into(), true)
            .error
            .is_empty());
    }
    #[test]
    fn zero_pid_never_enumerates_desktop_windows() {
        assert!(windows(0, "".into()).is_empty());
    }
    #[cfg(windows)]
    #[test]
    fn owned_job_closes_admitted_root_and_descendant() {
        os::descendant_job_cleanup();
    }
    #[test]
    #[ignore]
    fn job_child_helper() {
        use std::io::BufRead;
        println!("JOB_CHILD_READY");
        let mut admitted = String::new();
        std::io::stdin().lock().read_line(&mut admitted).unwrap();
        assert_eq!(admitted.trim(), "admitted");
        let mut descendant = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "tests::job_grandchild_helper",
                "--nocapture",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        println!("JOB_DESCENDANT:{}", descendant.id());
        descendant.wait().unwrap();
    }
    #[test]
    #[ignore]
    fn job_grandchild_helper() {
        std::thread::sleep(Duration::from_secs(60));
    }
    #[test]
    fn png_encoder_rejects_inconsistent_dimensions() {
        assert!(encode_png(0, 1, &[]).is_err());
        assert!(encode_png(2, 2, &[0; 4]).is_err());
        let png = encode_png(1, 1, &[1, 2, 3, 255]).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(png.windows(4).any(|part| part == b"IDAT"));
    }
}
