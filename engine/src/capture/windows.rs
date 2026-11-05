//! The Windows capture backend: Windows.Graphics.Capture on the game window only.
//!
//! It sees the game window even when other windows cover it. Frames are taken only when the
//! engine asks (up to 4 Hz), Windows is asked not to deliver them faster than that where it
//! supports it, and only the requested rows of the game area are copied off the GPU: a
//! HUD strip is about 0.2 MB, where a whole 1920x1080 frame is 8 MB. Windows 11 can hide the
//! yellow capture border, and Windows 10 always draws it.
//!
//! Window discovery reads only window-manager facts and process names, never the game's memory
//! or files (see Fair play in README.md). Proven first in the capture tool (tools/capture/windows).

use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::Foundation::Metadata::ApiInformation;
use windows::Foundation::TimeSpan;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess,
    GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{CloseHandle, HMODULE, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetForegroundWindow, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible,
};
use windows::core::{BOOL, Interface, factory, h};

use super::{Backend, CaptureError, GameWindow, Grab, Placement, ScreenRect};

const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
/// The fastest Windows is asked to deliver frames: a little faster than the engine reads (up to
/// 4 Hz), so a frame is usually waiting when it asks.
const MIN_UPDATE_INTERVAL: Duration = Duration::from_millis(200);
/// How long a grab with no frame queued waits for one before reusing the last picture.
const FRESH_WAIT: Duration = Duration::from_millis(100);
/// How long a new session waits for its first frame before calling capture broken.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(2);

/// Windows.Graphics.Capture of the MapleStory Classic window.
#[derive(Default)]
pub struct WindowsBackend {
    session: Option<Session>,
}

impl WindowsBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether this Windows has Windows.Graphics.Capture (Windows 10 1903 and later).
    pub fn supported() -> bool {
        GraphicsCaptureSession::IsSupported().unwrap_or(false)
    }
}

impl Backend for WindowsBackend {
    fn find(&mut self) -> Option<GameWindow> {
        let window = physical_pixels(find_game_window)?;
        Some(GameWindow {
            id: window.hwnd.0 as usize as u64,
            width: (window.client.right - window.client.left).max(0) as u32,
            height: (window.client.bottom - window.client.top).max(0) as u32,
            // Windows hands over the game's pixels at their own size. (Display scaling above 100%
            // is untested.)
            scale: 1,
            minimized: window.minimized,
            placement: (!window.minimized).then(|| Placement {
                frame: screen_rect(&window.frame),
                area: screen_rect(&window.client),
                // SAFETY: a read-only window-manager query.
                front: unsafe { GetForegroundWindow() } == window.hwnd,
            }),
        })
    }

    fn grab(&mut self, window: &GameWindow, rows: u32) -> Result<Grab, CaptureError> {
        let hwnd = HWND(window.id as usize as *mut c_void);
        if self.session.as_ref().is_none_or(|s| s.hwnd != hwnd) {
            self.session = None;
            let session = Session::start(hwnd).map_err(|e| {
                CaptureError(format!(
                    "Windows couldn't start capturing the game window: {e}"
                ))
            })?;
            self.session = Some(session);
        }
        // Where the game area sits in the captured image, which covers the whole window.
        let area = physical_pixels(|| game_area(hwnd));
        let session = self.session.as_mut().expect("started above");
        session.grab(area, rows)
    }

    fn stop(&mut self) {
        self.session = None;
    }
}

// MARK: finding the game window

struct Found {
    hwnd: HWND,
    /// The game's drawing area on screen, in physical pixels.
    client: RECT,
    /// The whole window as drawn, in physical pixels.
    frame: RECT,
    minimized: bool,
}

fn screen_rect(rect: &RECT) -> ScreenRect {
    ScreenRect {
        x: rect.left,
        y: rect.top,
        width: (rect.right - rect.left).max(0) as u32,
        height: (rect.bottom - rect.top).max(0) as u32,
    }
}

/// Runs `f` with this thread reporting window geometry in physical pixels, as captures are, and
/// puts the thread's previous setting back.
fn physical_pixels<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: a thread-local setting, restored before returning.
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let result = f();
    if !previous.is_invalid() {
        // SAFETY: as above.
        unsafe { SetThreadDpiAwarenessContext(previous) };
    }
    result
}

/// Executable names by process ID, from the system process list (no handle to any process).
fn process_names() -> HashMap<u32, String> {
    let mut names = HashMap::new();
    // SAFETY: a process snapshot read with a correctly sized entry, then closed.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return names;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                names.insert(
                    entry.th32ProcessID,
                    String::from_utf16_lossy(&entry.szExeFile[..len]),
                );
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    names
}

unsafe extern "system" fn collect_window(hwnd: HWND, list: LPARAM) -> BOOL {
    // SAFETY: `list` is the &mut Vec<HWND> passed to EnumWindows below, alive for the call.
    unsafe { (*(list.0 as *mut Vec<HWND>)).push(hwnd) };
    BOOL(1)
}

fn top_level_windows() -> Vec<HWND> {
    let mut list: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into `list`, which outlives the call.
    let _ = unsafe {
        EnumWindows(
            Some(collect_window),
            LPARAM(&mut list as *mut Vec<HWND> as isize),
        )
    };
    list
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: a u32 out-value, as DWMWA_CLOAKED expects.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            4,
        )
    }
    .is_ok()
        && cloaked != 0
}

/// The window's outer bounds as drawn (DWM extended frame bounds), which is what a window
/// capture covers.
fn frame_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: RECT out-values of the sizes these calls expect.
    unsafe {
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
        .is_err()
        {
            let _ = GetWindowRect(hwnd, &mut rect);
        }
    }
    rect
}

/// The game's drawing area on screen.
fn client_rect(hwnd: HWND) -> RECT {
    let mut client = RECT::default();
    let mut origin = POINT::default();
    // SAFETY: window-manager queries with out-values owned here.
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
        let _ = ClientToScreen(hwnd, &mut origin);
    }
    RECT {
        left: origin.x,
        top: origin.y,
        right: origin.x + client.right,
        bottom: origin.y + client.bottom,
    }
}

fn title(hwnd: HWND) -> String {
    let mut text = [0u16; 256];
    // SAFETY: reads the title from the window manager without sending the window a message.
    let len = unsafe { GetWindowTextW(hwnd, &mut text) };
    String::from_utf16_lossy(&text[..len.max(0) as usize])
}

/// The game: `MapleStory.exe`, or a window titled exactly "MapleStory" (an unverified Steam
/// build may use another process name). Never MesoVibe itself, nor the launcher. The largest
/// such window that's on screen wins.
fn find_game_window() -> Option<Found> {
    let names = process_names();
    let own = std::process::id();
    top_level_windows()
        .into_iter()
        // SAFETY: a read-only visibility query.
        .filter(|&hwnd| unsafe { IsWindowVisible(hwnd) }.as_bool() && !is_cloaked(hwnd))
        .filter_map(|hwnd| {
            let mut pid = 0u32;
            // SAFETY: an out-value owned here.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            let process = names
                .get(&pid)
                .map(|n| n.to_lowercase())
                .unwrap_or_default();
            let is_game = process == "maplestory.exe" || title(hwnd) == "MapleStory";
            (is_game && pid != own && !process.contains("launcher")).then(|| Found {
                hwnd,
                client: client_rect(hwnd),
                frame: frame_rect(hwnd),
                // SAFETY: a read-only query.
                minimized: unsafe { IsIconic(hwnd) }.as_bool(),
            })
        })
        .max_by_key(|w| {
            let area = i64::from(w.client.right - w.client.left)
                * i64::from(w.client.bottom - w.client.top);
            (!w.minimized, area)
        })
}

/// The game area inside a capture of the whole window: (left, top, width, height).
fn game_area(hwnd: HWND) -> (u32, u32, u32, u32) {
    let (frame, client) = (frame_rect(hwnd), client_rect(hwnd));
    (
        (client.left - frame.left).max(0) as u32,
        (client.top - frame.top).max(0) as u32,
        (client.right - client.left).max(0) as u32,
        (client.bottom - client.top).max(0) as u32,
    )
}

// MARK: capture

struct Session {
    hwnd: HWND,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    rt_device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    size: SizeInt32,
    /// A CPU-readable texture the size of the last region read back, reused while it fits.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    /// The last frame's timestamp and grab, for "no new frame" ticks.
    last: Option<(i64, Grab)>,
}

impl Session {
    fn start(hwnd: HWND) -> windows::core::Result<Session> {
        // WinRT on this thread; already initialized is fine.
        // SAFETY: initializes the thread's apartment; the result only says if it already was.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        let mut device = None;
        let mut context = None;
        // SAFETY: out-pointers to locals; D3D11CreateDevice fills them on success.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?
        };
        let device: ID3D11Device = device.ok_or_else(windows::core::Error::empty)?;
        let context: ID3D11DeviceContext = context.ok_or_else(windows::core::Error::empty)?;
        let dxgi: IDXGIDevice = device.cast()?;
        // SAFETY: wraps a valid DXGI device for the WinRT capture API.
        let rt_device: IDirect3DDevice =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        // SAFETY: hwnd is a top-level window found by EnumWindows.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&rt_device, FORMAT, 2, size)?;
        let session = pool.CreateCaptureSession(&item)?;
        // Not every Windows 10 build has these; capture works without them.
        let _ = session.SetIsCursorCaptureEnabled(false);
        hide_border(&session);
        if property_present(h!("MinUpdateInterval")) {
            let interval = TimeSpan {
                Duration: (MIN_UPDATE_INTERVAL.as_nanos() / 100) as i64,
            };
            let _ = session.SetMinUpdateInterval(interval);
        }
        session.StartCapture()?;
        Ok(Session {
            hwnd,
            device,
            context,
            rt_device,
            pool,
            session,
            size,
            staging: None,
            last: None,
        })
    }

    /// The bottom `rows` rows of the game area `area` (left, top, width, height in the captured
    /// image) from the newest frame.
    fn grab(&mut self, area: (u32, u32, u32, u32), rows: u32) -> Result<Grab, CaptureError> {
        // Keep only the newest queued frame. Windows delivers at most one per update interval,
        // so it's recent enough; wait only when none has arrived since the last grab.
        let mut frame = None;
        while let Ok(queued) = self.pool.TryGetNextFrame() {
            frame = Some(queued);
        }
        if frame.is_none() {
            let wait = match self.last {
                Some(_) => FRESH_WAIT,
                None => FIRST_FRAME_WAIT,
            };
            let deadline = Instant::now() + wait;
            while Instant::now() < deadline {
                if let Ok(newer) = self.pool.TryGetNextFrame() {
                    frame = Some(newer);
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let read = match frame {
            Some(frame) => {
                let time = frame.SystemRelativeTime().map(|t| t.Duration).unwrap_or(0);
                self.read(&frame, area, rows)
                    .map_err(|e| CaptureError(format!("reading the game's picture failed: {e}")))?
                    .map(|grab| (time, grab))
            }
            None => None,
        };
        let Some((time, grab)) = read else {
            // No new frame, or one from mid-resize: the last picture, marked as not new.
            return match &self.last {
                Some((_, grab)) => Ok(Grab {
                    fresh: false,
                    ..grab.clone()
                }),
                None => Err(CaptureError(
                    "the game window sent no usable picture. Is it minimized, mid-resize or \
                     blocked from capture?"
                        .to_string(),
                )),
            };
        };
        let fresh = self.last.as_ref().is_none_or(|(last, _)| *last != time);
        let grab = Grab { fresh, ..grab };
        self.last = Some((time, grab.clone()));
        Ok(grab)
    }

    /// Copies the bottom `rows` rows of the game area, and only those, off the GPU. `None` when
    /// the frame doesn't hold the whole game area (the window is mid-resize).
    fn read(
        &mut self,
        frame: &Direct3D11CaptureFrame,
        (left, top, width, height): (u32, u32, u32, u32),
        rows: u32,
    ) -> windows::core::Result<Option<Grab>> {
        let content = frame.ContentSize()?;
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        // SAFETY: the frame's surface is a D3D11 texture on our device.
        let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: desc is a valid out-pointer.
        unsafe { texture.GetDesc(&mut desc) };
        // The window resized: resize the pool for the next frames.
        if content.Width > 0
            && content.Height > 0
            && (content.Width != self.size.Width || content.Height != self.size.Height)
        {
            self.pool.Recreate(&self.rt_device, FORMAT, 2, content)?;
            self.size = content;
        }
        // A frame from mid-resize may not hold the whole game area: skip it.
        let picture_width = (content.Width.max(0) as u32).min(desc.Width);
        let picture_height = (content.Height.max(0) as u32).min(desc.Height);
        let (right, bottom) = (left + width, top + height);
        let rows = rows.min(height);
        if rows == 0 || right > picture_width || bottom > picture_height {
            return Ok(None);
        }
        let staging = self.staging_for(&desc, width, rows)?;
        let region = D3D11_BOX {
            left,
            top: bottom - rows,
            front: 0,
            right,
            bottom,
            back: 1,
        };
        let row_bytes = width as usize * 4;
        let mut bgra = Vec::with_capacity(row_bytes * rows as usize);
        // SAFETY: copies the region into our staging texture, maps it for the row copy, unmaps.
        unsafe {
            self.context
                .CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture, 0, Some(&region));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            for y in 0..rows as usize {
                let start = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                bgra.extend_from_slice(std::slice::from_raw_parts(start, row_bytes));
            }
            self.context.Unmap(&staging, 0);
        }
        Ok(Some(Grab {
            width,
            height,
            first_row: height - rows,
            bgra,
            scale: 1,
            fresh: true,
        }))
    }

    /// A staging texture of exactly `width` x `rows` in the frame's format.
    fn staging_for(
        &mut self,
        frame: &D3D11_TEXTURE2D_DESC,
        width: u32,
        rows: u32,
    ) -> windows::core::Result<ID3D11Texture2D> {
        if let Some((texture, w, h)) = &self.staging
            && (*w, *h) == (width, rows)
        {
            return Ok(texture.clone());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: rows,
            MipLevels: 1,
            ArraySize: 1,
            Format: frame.Format,
            SampleDesc: frame.SampleDesc,
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut texture = None;
        // SAFETY: creates a texture from a filled-in description into a local.
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))?
        };
        let texture = texture.ok_or_else(windows::core::Error::empty)?;
        self.staging = Some((texture.clone(), width, rows));
        Ok(texture)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

fn property_present(name: &windows::core::HSTRING) -> bool {
    ApiInformation::IsPropertyPresent(h!("Windows.Graphics.Capture.GraphicsCaptureSession"), name)
        .unwrap_or(false)
}

/// Asks Windows 11 not to draw the yellow capture border. Windows 10 always draws it.
fn hide_border(session: &GraphicsCaptureSession) {
    if !property_present(h!("IsBorderRequired")) {
        return;
    }
    let _ = GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
        .and_then(|operation| operation.join());
    let _ = session.SetIsBorderRequired(false);
}
