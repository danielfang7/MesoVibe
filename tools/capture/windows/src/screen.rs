//! Copies a rectangle of the screen, the way Print Screen does. Unlike Windows.Graphics.Capture it
//! sees whatever is on top, so the game must be in front and uncovered.

use std::ffi::c_void;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SRCCOPY, SelectObject,
};

/// Top-down BGRA pixels of `rect`, in physical screen coordinates.
pub fn grab(rect: RECT) -> Result<(Vec<u8>, u32, u32), String> {
    let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
    if w <= 0 || h <= 0 {
        return Err(format!("empty window rectangle {w}x{h}"));
    }
    let mut pixels = vec![0u8; w as usize * h as usize * 4];
    // SAFETY: plain GDI calls on handles created and released here. The DIB buffer holds exactly
    // w * h 32-bit pixels, which is what the header below asks GetDIBits to write.
    let copied = unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let previous = SelectObject(memory, bitmap.into());
        let blit = BitBlt(
            memory,
            0,
            0,
            w,
            h,
            Some(screen),
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        );
        // GetDIBits needs the bitmap out of the DC first.
        SelectObject(memory, previous);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative: rows top to bottom
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = GetDIBits(
            memory,
            bitmap,
            0,
            h as u32,
            Some(pixels.as_mut_ptr() as *mut c_void),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        blit.map_err(|e| format!("BitBlt failed: {e}"))
            .and(if lines == h {
                Ok(())
            } else {
                Err(format!("GetDIBits copied {lines} of {h} rows"))
            })
    };
    copied.map(|()| (pixels, w as u32, h as u32))
}
