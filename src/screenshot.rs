use std::io::Cursor;

use image::imageops::FilterType;
use image::{ImageFormat, RgbaImage};

use crate::error::{Error, Result};
use crate::geometry::{Point, Rect};

/// Uncompressed RGBA8 pixels returned by a backend.
#[derive(Clone, PartialEq, Eq)]
pub struct RawImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major RGBA bytes, `width * height * 4` long, no row padding.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for RawImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

/// A PNG screenshot of a window together with the screen area it shows.
///
/// Coordinates handed to [`Computer::click`](crate::Computer::click) and
/// [`Computer::drag`](crate::Computer::drag) are pixels in this image; the crate converts
/// them back to screen points with [`Screenshot::to_screen_point`].
#[derive(Clone, PartialEq)]
pub struct Screenshot {
    png: Vec<u8>,
    width: u32,
    height: u32,
    frame: Rect,
}

impl std::fmt::Debug for Screenshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screenshot")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frame", &self.frame)
            .field("png_bytes", &self.png.len())
            .finish()
    }
}

impl Screenshot {
    /// Downscales `image` so neither side exceeds `max_dimension`, then encodes it as PNG.
    ///
    /// `frame` is the screen area, in points, the image covers.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer size does not match the dimensions or encoding fails.
    pub fn encode(image: RawImage, frame: Rect, max_dimension: Option<u32>) -> Result<Self> {
        let RawImage { width, height, rgba } = image;
        let mut buffer = RgbaImage::from_raw(width, height, rgba)
            .ok_or_else(|| Error::platform("screenshot buffer does not match its dimensions"))?;
        if let Some(max) = max_dimension.filter(|&m| m > 0)
            && (width > max || height > max)
        {
            let scale = f64::from(max) / f64::from(width.max(height));
            let w = scaled(width, scale);
            let h = scaled(height, scale);
            buffer = image::imageops::resize(&buffer, w, h, FilterType::Triangle);
        }
        let (width, height) = buffer.dimensions();
        let mut png = Vec::new();
        buffer.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
        Ok(Self { png, width, height, frame })
    }

    /// PNG-encoded bytes.
    #[must_use]
    pub fn png(&self) -> &[u8] {
        &self.png
    }

    /// Consumes the screenshot and returns the PNG bytes.
    #[must_use]
    pub fn into_png(self) -> Vec<u8> {
        self.png
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Screen area covered, in points.
    #[must_use]
    pub const fn frame(&self) -> Rect {
        self.frame
    }

    /// Converts a pixel position in this image to a screen point.
    #[must_use]
    pub fn to_screen_point(&self, x: f64, y: f64) -> Point {
        Point::new(
            (x * self.frame.size.width).mul_add(1.0 / f64::from(self.width.max(1)), self.frame.origin.x),
            (y * self.frame.size.height).mul_add(1.0 / f64::from(self.height.max(1)), self.frame.origin.y),
        )
    }

    /// Converts a screen point to a pixel position in this image.
    #[must_use]
    pub fn from_screen_point(&self, point: Point) -> (f64, f64) {
        let sx = f64::from(self.width) / self.frame.size.width.max(f64::EPSILON);
        let sy = f64::from(self.height) / self.frame.size.height.max(f64::EPSILON);
        ((point.x - self.frame.origin.x) * sx, (point.y - self.frame.origin.y) * sy)
    }

    /// True when `(x, y)` lies inside the image.
    #[must_use]
    pub fn contains_pixel(&self, x: f64, y: f64) -> bool {
        x.is_finite()
            && y.is_finite()
            && x >= 0.0
            && y >= 0.0
            && x <= f64::from(self.width)
            && y <= f64::from(self.height)
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scaled(value: u32, scale: f64) -> u32 {
    ((f64::from(value) * scale).round() as u32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32) -> RawImage {
        RawImage { width, height, rgba: vec![255; (width * height * 4) as usize] }
    }

    #[test]
    fn downscales_and_maps_coordinates() -> Result<()> {
        let frame = Rect::new(100.0, 50.0, 400.0, 200.0);
        let shot = Screenshot::encode(solid(800, 400), frame, Some(200))?;
        assert_eq!((shot.width(), shot.height()), (200, 100));
        assert!(shot.png().starts_with(b"\x89PNG"));
        assert_eq!(shot.to_screen_point(100.0, 50.0), Point::new(300.0, 150.0));
        assert_eq!(shot.from_screen_point(Point::new(300.0, 150.0)), (100.0, 50.0));
        Ok(())
    }

    #[test]
    fn rejects_mismatched_buffer() {
        let bad = RawImage { width: 10, height: 10, rgba: vec![0; 4] };
        assert!(Screenshot::encode(bad, Rect::default(), None).is_err());
    }
}
