//! Window capture through Windows.Graphics.Capture, the API the engine's Windows backend uses.
//! It sees the game window even when other windows cover it. While a session runs, Windows may
//! draw a yellow border around the captured window.

use std::time::{Duration, Instant};

use windows::Foundation::Metadata::ApiInformation;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess,
    GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Security::Authorization::AppCapabilityAccess::AppCapabilityAccessStatus;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{Interface, factory, h};

const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;

pub fn supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

/// Windows 11 can turn the yellow border off; Windows 10 always draws it.
pub fn border_setting_present() -> bool {
    ApiInformation::IsPropertyPresent(
        h!("Windows.Graphics.Capture.GraphicsCaptureSession"),
        h!("IsBorderRequired"),
    )
    .unwrap_or(false)
}

pub struct Frame {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// False when no frame newer than the last one arrived, so this is the previous image.
    pub fresh: bool,
    /// How long ago Windows rendered this frame.
    pub age_ms: f64,
}

pub struct Capture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    rt_device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    size: SizeInt32,
    last: Option<(i64, Vec<u8>, u32, u32)>,
    /// What happened to the yellow border request, for meta.json.
    pub border: String,
}

impl Capture {
    pub fn start(hwnd: HWND, hide_border: bool) -> windows::core::Result<Capture> {
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
        // Not every Windows 10 build has this; the cursor just shows up where it's missing.
        let _ = session.SetIsCursorCaptureEnabled(false);
        let border = if hide_border {
            request_no_border(&session)
        } else {
            "shown (default)".to_string()
        };
        session.StartCapture()?;
        Ok(Capture {
            device,
            context,
            rt_device,
            pool,
            session,
            size,
            last: None,
            border,
        })
    }

    /// The newest frame. Waits up to `wait` for a frame rendered after this call; if none comes
    /// (minimized, or nothing redrew), returns the last image with `fresh: false`.
    pub fn grab(&mut self, wait: Duration) -> Result<Frame, String> {
        // Frames already queued are from the previous tick or earlier: drop all but the newest.
        let mut queued = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            queued = Some(frame);
        }
        let deadline = Instant::now() + wait;
        let frame = loop {
            if let Ok(frame) = self.pool.TryGetNextFrame() {
                break Some(frame);
            }
            if Instant::now() >= deadline {
                break queued;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        match frame {
            Some(frame) => {
                let time = frame.SystemRelativeTime().map(|t| t.Duration).unwrap_or(0);
                let (bgra, width, height) = self
                    .read(&frame)
                    .map_err(|e| format!("reading the frame failed: {e}"))?;
                let fresh = self.last.as_ref().is_none_or(|(last, ..)| *last != time);
                self.last = Some((time, bgra.clone(), width, height));
                Ok(Frame {
                    bgra,
                    width,
                    height,
                    fresh,
                    age_ms: age_ms(time),
                })
            }
            None => match &self.last {
                Some((time, bgra, width, height)) => Ok(Frame {
                    bgra: bgra.clone(),
                    width: *width,
                    height: *height,
                    fresh: false,
                    age_ms: age_ms(*time),
                }),
                None => Err("no frame arrived (is the window minimized?)".to_string()),
            },
        }
    }

    fn read(
        &mut self,
        frame: &Direct3D11CaptureFrame,
    ) -> windows::core::Result<(Vec<u8>, u32, u32)> {
        let content = frame.ContentSize()?;
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        // SAFETY: the frame's surface is a D3D11 texture on our device.
        let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: desc is a valid out-pointer.
        unsafe { texture.GetDesc(&mut desc) };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        // SAFETY: a CPU-readable copy of the frame texture, mapped only for the row copy below.
        let (bgra, width, height) = unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging: ID3D11Texture2D = staging.ok_or_else(windows::core::Error::empty)?;
            self.context.CopyResource(&staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let width = (content.Width.max(0) as u32).min(desc.Width);
            let height = (content.Height.max(0) as u32).min(desc.Height);
            let row = width as usize * 4;
            let mut bgra = Vec::with_capacity(row * height as usize);
            for y in 0..height as usize {
                let start = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                bgra.extend_from_slice(std::slice::from_raw_parts(start, row));
            }
            self.context.Unmap(&staging, 0);
            (bgra, width, height)
        };
        // The window changed size (a resolution change): resize the pool for the next frames.
        if content.Width > 0
            && content.Height > 0
            && (content.Width != self.size.Width || content.Height != self.size.Height)
        {
            self.pool.Recreate(&self.rt_device, FORMAT, 2, content)?;
            self.size = content;
        }
        if width == 0 || height == 0 {
            return Err(windows::core::Error::empty());
        }
        Ok((bgra, width, height))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// Asks Windows to drop the yellow border. Records the outcome instead of failing.
fn request_no_border(session: &GraphicsCaptureSession) -> String {
    if !border_setting_present() {
        return "shown: this Windows has no setting to hide it (Windows 10 always draws it)"
            .to_string();
    }
    let access = GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
        .and_then(|operation| operation.join());
    let access = match access {
        Ok(status) if status == AppCapabilityAccessStatus::Allowed => "allowed".to_string(),
        Ok(status) if status == AppCapabilityAccessStatus::DeniedByUser => {
            "denied by user".to_string()
        }
        Ok(status) if status == AppCapabilityAccessStatus::DeniedBySystem => {
            "denied by system".to_string()
        }
        Ok(status) if status == AppCapabilityAccessStatus::NotDeclaredByApp => {
            "not declared by app".to_string()
        }
        Ok(status) if status == AppCapabilityAccessStatus::UserPromptRequired => {
            "user prompt required".to_string()
        }
        Ok(status) => format!("status {}", status.0),
        Err(e) => format!("request failed: {e}"),
    };
    match session.SetIsBorderRequired(false) {
        Ok(()) => format!("hide requested, accepted (borderless access: {access})"),
        Err(e) => format!("hide requested, refused: {e} (borderless access: {access})"),
    }
}

/// Milliseconds since Windows rendered a frame stamped `time` (QueryPerformanceCounter-based,
/// in 100 ns units).
fn age_ms(time: i64) -> f64 {
    let (mut now, mut frequency) = (0i64, 0i64);
    // SAFETY: out-pointers to locals.
    let ok = unsafe {
        QueryPerformanceCounter(&mut now).is_ok()
            && QueryPerformanceFrequency(&mut frequency).is_ok()
    };
    if !ok || frequency == 0 || time == 0 {
        return -1.0;
    }
    let now_100ns = now as f64 * 10_000_000.0 / frequency as f64;
    (now_100ns - time as f64) / 10_000.0
}
