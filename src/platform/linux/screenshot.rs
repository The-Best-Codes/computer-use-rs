use std::path::Path;

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder, Screen, Visualtype};
use x11rb::reexports::x11rb_protocol::parse_display::parse_display;
use x11rb::reexports::x11rb_protocol::xauth::Family;
use x11rb::rust_connection::{DefaultStream, RustConnection};

use super::session::SessionEnv;
use crate::error::{Error, Result};
use crate::geometry::Rect;
use crate::screenshot::RawImage;

#[derive(Debug)]
pub(crate) enum Shot {
    Captured(Rect, RawImage),
    Omitted(String),
}

pub(crate) fn capture(session: &SessionEnv, frame: Rect) -> Result<Shot> {
    let omitted = |reason: &str| Ok(Shot::Omitted(format!("Screenshot omitted: {reason}")));
    if frame.is_empty() {
        return omitted("the window has no on-screen frame");
    }
    let Some(display) = session.display() else { return omitted("no X11 display was found") };
    let Some((conn, screen_num)) = connect(display, session.xauthority().as_deref()) else {
        return omitted(&format!("cannot connect to X11 display {display}"));
    };
    let setup = conn.setup();
    let Some(screen) = setup.roots.get(screen_num) else { return omitted("the X11 screen does not exist") };
    let bounds = Rect::new(0.0, 0.0, f64::from(screen.width_in_pixels), f64::from(screen.height_in_pixels));
    let Some(area) = frame.intersection(&bounds) else { return omitted("the window is off screen") };
    let (x, y) = (pixel_i16(area.min_x()), pixel_i16(area.min_y()));
    let (width, height) = (pixel_u16(area.size.width), pixel_u16(area.size.height));
    if width == 0 || height == 0 {
        return omitted("the window is off screen");
    }
    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, screen.root, x, y, width, height, u32::MAX)
        .map_err(|e| Error::platform(format!("X11 GetImage failed: {e}")))?
        .reply()
        .map_err(|e| Error::platform(format!("X11 GetImage failed: {e}")))?;
    let visual_id = if reply.visual == 0 { screen.root_visual } else { reply.visual };
    let visual = find_visual(screen, visual_id)
        .ok_or_else(|| Error::platform(format!("X11 visual {visual_id:#x} not found")))?;
    let format = setup
        .pixmap_formats
        .iter()
        .find(|f| f.depth == reply.depth)
        .ok_or_else(|| Error::platform(format!("no X11 pixmap format for depth {}", reply.depth)))?;
    let layout = PixelLayout {
        bits_per_pixel: format.bits_per_pixel,
        scanline_pad: format.scanline_pad,
        msb_first: setup.image_byte_order == ImageOrder::MSB_FIRST,
        masks: [visual.red_mask, visual.green_mask, visual.blue_mask],
    };
    let image = RawImage {
        width: u32::from(width),
        height: u32::from(height),
        rgba: to_rgba(&reply.data, u32::from(width), u32::from(height), &layout)?,
    };
    // Under Wayland the X11 root window shows native windows as black.
    if looks_black(&image) {
        return omitted("the captured frame was black");
    }
    let captured = Rect::new(f64::from(x), f64::from(y), f64::from(width), f64::from(height));
    Ok(Shot::Captured(captured, image))
}

fn connect(display: &str, xauthority: Option<&Path>) -> Option<(RustConnection, usize)> {
    let parsed = parse_display(Some(display)).ok()?;
    let screen = usize::from(parsed.screen);
    let cookies = xauthority.and_then(|path| std::fs::read(path).ok()).unwrap_or_default();
    parsed.connect_instruction().find_map(|address| {
        let (stream, (family, peer)) = DefaultStream::connect(&address).ok()?;
        let (name, data) = find_cookie(&cookies, family, &peer, parsed.display).unwrap_or_default();
        RustConnection::connect_to_stream_with_auth_info(stream, screen, name, data).ok().map(|conn| (conn, screen))
    })
}

fn find_cookie(data: &[u8], family: Family, address: &[u8], display: u16) -> Option<(Vec<u8>, Vec<u8>)> {
    const COOKIE: &[u8] = b"MIT-MAGIC-COOKIE-1";
    let display = display.to_string();
    let mut rest = data;
    let take_u16 = |rest: &mut &[u8]| -> Option<u16> {
        let (head, tail) = rest.split_first_chunk::<2>()?;
        *rest = tail;
        Some(u16::from_be_bytes(*head))
    };
    let take_field = |rest: &mut &[u8], len: u16| -> Option<Vec<u8>> {
        let len = usize::from(len);
        let field = rest.get(..len)?.to_vec();
        *rest = &rest[len..];
        Some(field)
    };
    while !rest.is_empty() {
        let entry_family = Family::from(take_u16(&mut rest)?);
        let len = take_u16(&mut rest)?;
        let entry_address = take_field(&mut rest, len)?;
        let len = take_u16(&mut rest)?;
        let number = take_field(&mut rest, len)?;
        let len = take_u16(&mut rest)?;
        let name = take_field(&mut rest, len)?;
        let len = take_u16(&mut rest)?;
        let cookie = take_field(&mut rest, len)?;
        let address_matches = entry_family == Family::WILD
            || family == Family::WILD
            || (entry_family == family && entry_address == address);
        let number_matches = number.is_empty() || number == display.as_bytes();
        if address_matches && number_matches && name == COOKIE {
            return Some((name, cookie));
        }
    }
    None
}

fn looks_black(image: &RawImage) -> bool {
    let (width, height) = (image.width as usize, image.height as usize);
    if width == 0 || height == 0 {
        return true;
    }
    let (step_x, step_y) = ((width / 16).max(1), (height / 16).max(1));
    (0..height).step_by(step_y).all(|y| {
        (0..width).step_by(step_x).all(|x| {
            let offset = (y * width + x) * 4;
            image.rgba.get(offset..offset + 3).is_some_and(|rgb| rgb.iter().all(|&c| c <= 3))
        })
    })
}

fn find_visual(screen: &Screen, id: u32) -> Option<&Visualtype> {
    screen.allowed_depths.iter().flat_map(|d| &d.visuals).find(|v| v.visual_id == id)
}

#[allow(clippy::cast_possible_truncation)] // Clamped to the i16 range first.
fn pixel_i16(value: f64) -> i16 {
    value.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Clamped to the u16 range first.
fn pixel_u16(value: f64) -> u16 {
    value.round().clamp(0.0, f64::from(u16::MAX)) as u16
}

#[derive(Debug, Clone, Copy)]
struct PixelLayout {
    bits_per_pixel: u8,
    scanline_pad: u8,
    msb_first: bool,
    masks: [u32; 3],
}

fn to_rgba(data: &[u8], width: u32, height: u32, layout: &PixelLayout) -> Result<Vec<u8>> {
    let bytes_per_pixel = match layout.bits_per_pixel {
        8 | 16 | 24 | 32 => usize::from(layout.bits_per_pixel / 8),
        bpp => return Err(Error::Unsupported(format!("{bpp}-bit X11 pixmaps"))),
    };
    let (width, height) = (width as usize, height as usize);
    let pad = usize::from(layout.scanline_pad.max(8));
    let stride = (width * bytes_per_pixel * 8).div_ceil(pad) * pad / 8;
    if data.len() < stride * height {
        return Err(Error::platform("X11 image data is shorter than expected"));
    }
    let channels = layout.masks.map(Channel::new);
    let mut rgba = Vec::with_capacity(width * height * 4);
    for row in data.chunks(stride).take(height) {
        for px in row[..width * bytes_per_pixel].chunks_exact(bytes_per_pixel) {
            let value = if layout.msb_first {
                px.iter().fold(0u32, |acc, &b| (acc << 8) | u32::from(b))
            } else {
                px.iter().rev().fold(0u32, |acc, &b| (acc << 8) | u32::from(b))
            };
            rgba.extend(channels.iter().map(|c| c.extract(value)));
            rgba.push(u8::MAX);
        }
    }
    Ok(rgba)
}

#[derive(Debug, Clone, Copy)]
struct Channel {
    mask: u32,
    shift: u32,
    max: u32,
}

impl Channel {
    const fn new(mask: u32) -> Self {
        let shift = if mask == 0 { 0 } else { mask.trailing_zeros() };
        Self { mask, shift, max: mask >> shift }
    }

    fn extract(self, pixel: u32) -> u8 {
        if self.max == 0 {
            return 0;
        }
        let raw = u64::from((pixel & self.mask) >> self.shift);
        u8::try_from(raw * 255 / u64::from(self.max)).unwrap_or(u8::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BGRX: PixelLayout =
        PixelLayout { bits_per_pixel: 32, scanline_pad: 32, msb_first: false, masks: [0xff_0000, 0xff00, 0xff] };

    #[test]
    fn converts_bgrx() -> Result<()> {
        let data = [0x30, 0x20, 0x10, 0x00, 0xff, 0x00, 0x00, 0x00];
        assert_eq!(to_rgba(&data, 2, 1, &BGRX)?, vec![0x10, 0x20, 0x30, 0xff, 0x00, 0x00, 0xff, 0xff]);
        Ok(())
    }

    #[test]
    fn converts_rgb565_with_padding() -> Result<()> {
        let layout =
            PixelLayout { bits_per_pixel: 16, scanline_pad: 32, msb_first: false, masks: [0xf800, 0x07e0, 0x001f] };
        let data = [0x00, 0xf8, 0, 0, 0x1f, 0x00, 0, 0];
        assert_eq!(to_rgba(&data, 1, 2, &layout)?, vec![255, 0, 0, 255, 0, 0, 255, 255]);
        Ok(())
    }

    #[test]
    fn detects_black_frames() {
        let black = RawImage { width: 40, height: 20, rgba: [2, 3, 1, 255].repeat(800) };
        assert!(looks_black(&black));
        let mut lit = black;
        lit.rgba[0] = 200;
        assert!(!looks_black(&lit));
    }

    #[test]
    fn finds_xauthority_cookie() {
        let mut file = Vec::new();
        for (family, address, number, cookie) in
            [(256u16, &b"box"[..], &b"1"[..], [0xde, 0xad, 0xbe, 0xef]), (65535, b"", b"2", [1, 2, 3, 4])]
        {
            file.extend(family.to_be_bytes());
            for field in [address, number, b"MIT-MAGIC-COOKIE-1", &cookie[..]] {
                file.extend(u16::try_from(field.len()).unwrap_or_default().to_be_bytes());
                file.extend(field);
            }
        }
        let cookie = |family, address: &[u8], display| find_cookie(&file, family, address, display).map(|c| c.1);
        assert_eq!(cookie(Family::LOCAL, b"box", 1), Some(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(cookie(Family::LOCAL, b"other", 2), Some(vec![1, 2, 3, 4]));
        assert_eq!(cookie(Family::LOCAL, b"other", 1), None);
        assert_eq!(find_cookie(&file[..10], Family::LOCAL, b"box", 1), None);
    }

    #[test]
    fn rejects_short_data() {
        assert!(to_rgba(&[0; 4], 2, 1, &BGRX).is_err());
    }
}
