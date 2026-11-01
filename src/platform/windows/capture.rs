use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, PW_RENDERFULLCONTENT};

use super::WinResultExt as _;
use super::apps::{size_u32, window_rect};
use crate::error::{Error, Result};
use crate::screenshot::RawImage;

pub(super) fn screenshot(hwnd: HWND, frame: RECT) -> Result<RawImage> {
    if frame.right <= frame.left || frame.bottom <= frame.top {
        return Err(Error::StateUnavailable("the window has an empty frame".into()));
    }
    match print_window(hwnd, frame) {
        Some(image) if image.rgba.as_chunks::<4>().0.iter().any(|px| px[..3] != [0, 0, 0]) => Ok(image),
        // SAFETY: plain FFI call; any HWND value is accepted.
        Some(image) if unsafe { IsIconic(hwnd) }.as_bool() => Ok(image),
        _ => copy_screen(frame),
    }
}

fn print_window(hwnd: HWND, frame: RECT) -> Option<RawImage> {
    let outer = window_rect(hwnd)?;
    let dib = Dib::new(outer.right - outer.left, outer.bottom - outer.top).ok()?;
    // SAFETY: `dib.dc` is a live memory DC with a bitmap the size of the window selected into it.
    let ok = unsafe { PrintWindow(hwnd, dib.dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)) };
    if !ok.as_bool() {
        return None;
    }
    dib.crop(frame.left - outer.left, frame.top - outer.top, frame.right - frame.left, frame.bottom - frame.top)
}

fn copy_screen(frame: RECT) -> Result<RawImage> {
    let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
    let dib = Dib::new(width, height)?;
    let screen = ScreenDc::new()?;
    // SAFETY: both DCs are live; the destination bitmap is `width` x `height` pixels.
    unsafe { BitBlt(dib.dc, 0, 0, width, height, Some(screen.0), frame.left, frame.top, SRCCOPY | CAPTUREBLT) }
        .or_platform("copying the screen failed")?;
    dib.crop(0, 0, width, height).ok_or_else(|| Error::platform("screen copy produced no pixels"))
}

struct ScreenDc(HDC);

impl ScreenDc {
    fn new() -> Result<Self> {
        // SAFETY: plain FFI call; `None` requests the DC of the whole virtual screen.
        let dc = unsafe { GetDC(None) };
        if dc.is_invalid() {
            return Err(Error::platform("GetDC failed for the screen"));
        }
        Ok(Self(dc))
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: the DC was obtained with `GetDC(None)` and is released exactly once.
        unsafe { ReleaseDC(None, self.0) };
    }
}

struct Dib {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *const u8,
    width: usize,
    height: usize,
}

impl Dib {
    fn new(width: i32, height: i32) -> Result<Self> {
        let (Ok(w), Ok(h)) = (usize::try_from(width), usize::try_from(height)) else {
            return Err(Error::platform("invalid bitmap size"));
        };
        if w == 0 || h == 0 {
            return Err(Error::platform("invalid bitmap size"));
        }
        // SAFETY: plain FFI call; `None` creates a DC compatible with the screen.
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return Err(Error::platform("CreateCompatibleDC failed"));
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_u32::<BITMAPINFOHEADER>(),
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        // SAFETY: `info` describes a valid 32-bit top-down bitmap and `bits` is a writable pointer.
        let bitmap = unsafe { CreateDIBSection(Some(dc), &raw const info, DIB_RGB_COLORS, &raw mut bits, None, 0) };
        let bitmap = match bitmap {
            Ok(bitmap) if !bits.is_null() => bitmap,
            _ => {
                // SAFETY: `dc` was created above and is not used again.
                let _ = unsafe { DeleteDC(dc) };
                return Err(Error::platform("CreateDIBSection failed"));
            }
        };
        // SAFETY: both objects are live; the previous selection is restored on drop.
        let previous = unsafe { SelectObject(dc, bitmap.into()) };
        Ok(Self { dc, bitmap, previous, bits: bits.cast_const().cast(), width: w, height: h })
    }

    fn crop(&self, x: i32, y: i32, width: i32, height: i32) -> Option<RawImage> {
        // SAFETY: the DIB section owns `width * height * 4` bytes at `bits` while `self` lives,
        // and GDI has finished drawing (the calls above are synchronous).
        let pixels = unsafe { std::slice::from_raw_parts(self.bits, self.width * self.height * 4) };
        bgra_to_rgba(pixels, (self.width, self.height), x, y, width, height)
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: restores the DC's original bitmap so ours can be deleted.
        unsafe { SelectObject(self.dc, self.previous) };
        // SAFETY: the bitmap is owned by this struct and no longer selected.
        let _ = unsafe { DeleteObject(self.bitmap.into()) };
        // SAFETY: the DC is owned by this struct and deleted exactly once.
        let _ = unsafe { DeleteDC(self.dc) };
    }
}

fn bgra_to_rgba(pixels: &[u8], size: (usize, usize), x: i32, y: i32, width: i32, height: i32) -> Option<RawImage> {
    let (src_width, src_height) = size;
    let (x, y) = (usize::try_from(x).ok()?, usize::try_from(y).ok()?);
    let (w, h) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
    if w == 0 || h == 0 || x + w > src_width || y + h > src_height || pixels.len() < src_width * src_height * 4 {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        let start = (row * src_width + x) * 4;
        for px in pixels[start..start + w * 4].as_chunks::<4>().0 {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    Some(RawImage { width: u32::try_from(w).ok()?, height: u32::try_from(h).ok()?, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_and_swizzles() {
        let pixels = [255, 0, 0, 0, 0, 255, 0, 0, 0, 0, 255, 0, 255, 255, 255, 0];
        let full = bgra_to_rgba(&pixels, (2, 2), 0, 0, 2, 2).map(|i| i.rgba);
        assert_eq!(full, Some(vec![0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255]));
        let corner = bgra_to_rgba(&pixels, (2, 2), 1, 1, 1, 1).map(|i| (i.width, i.height, i.rgba));
        assert_eq!(corner, Some((1, 1, vec![255, 255, 255, 255])));
        assert!(bgra_to_rgba(&pixels, (2, 2), 1, 1, 2, 1).is_none());
        assert!(bgra_to_rgba(&pixels, (2, 2), -1, 0, 1, 1).is_none());
    }
}
