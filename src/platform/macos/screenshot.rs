use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
    CGPreflightScreenCaptureAccess,
};
use objc2_foundation::NSError;
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
};

use crate::error::{Error, Permission, Result};
use crate::screenshot::RawImage;

const TIMEOUT: Duration = Duration::from_secs(5);

type Reply = mpsc::Sender<Result<RawImage, String>>;

pub(super) fn capture_window(window_id: u32) -> Result<RawImage> {
    if !CGPreflightScreenCaptureAccess() {
        return Err(Error::PermissionDenied(Permission::ScreenRecording));
    }
    // Unlike ScreenCaptureKit, this handles covered windows and other Spaces.
    if let Some(image) = super::skylight::get().capture(window_id).and_then(|image| to_rgba(&image)) {
        return Ok(image);
    }
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        // SAFETY: a non-null pointer is a valid object for the duration of the callback.
        let result = unsafe { content.as_ref() }
            .map_or_else(|| Err(describe(error)), |content| start_capture(content, window_id, tx.clone()));
        if let Err(e) = result {
            let _ = tx.send(Err(e));
        }
    });
    // SAFETY: the handler is a valid 'static block.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, false, &handler,
        );
    }
    rx.recv_timeout(TIMEOUT)
        .map_err(|_| Error::platform("timed out waiting for ScreenCaptureKit"))?
        .map_err(|e| Error::platform(format!("screenshot failed: {e}")))
}

fn start_capture(content: &SCShareableContent, window_id: u32, reply: Reply) -> Result<(), String> {
    // SAFETY: `windows` is a plain getter on a valid object.
    let windows = unsafe { content.windows() };
    let window: Retained<SCWindow> = windows
        .iter()
        // SAFETY: `windowID` is a plain getter.
        .find(|w| unsafe { w.windowID() } == window_id)
        .ok_or_else(|| format!("window {window_id} is not capturable"))?;

    // SAFETY: `window` is a valid SCWindow from the current shareable content.
    let filter = unsafe { SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window) };
    // SAFETY: `filter` is fully initialized.
    let info = unsafe { SCShareableContent::infoForFilter(&filter) };
    // SAFETY: plain getter.
    let scale = f64::from(unsafe { info.pointPixelScale() });
    // SAFETY: plain getter.
    let rect = unsafe { info.contentRect() };

    // SAFETY: `new` has no preconditions.
    let config = unsafe { SCStreamConfiguration::new() };
    // SAFETY: plain setter on a fresh configuration.
    unsafe { config.setWidth(pixels(rect.size.width * scale)) };
    // SAFETY: plain setter on a fresh configuration.
    unsafe { config.setHeight(pixels(rect.size.height * scale)) };
    // SAFETY: plain setter on a fresh configuration.
    unsafe { config.setShowsCursor(false) };
    // SAFETY: plain setter on a fresh configuration (macOS 14+).
    unsafe { config.setIgnoreShadowsSingleWindow(true) };

    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        // SAFETY: a non-null pointer is a valid image for the duration of the callback.
        let result = unsafe { image.as_ref() }.map_or_else(
            || Err(describe(error)),
            |image| to_rgba(image).ok_or_else(|| "could not read pixels".to_owned()),
        );
        let _ = reply.send(result);
    });
    // SAFETY: filter, configuration, and handler are valid; the handler is 'static.
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(&filter, &config, Some(&handler));
    }
    Ok(())
}

fn describe(error: *mut NSError) -> String {
    // SAFETY: a non-null pointer is a valid NSError for the duration of the callback.
    unsafe { error.as_ref() }.map_or_else(|| "unknown error".to_owned(), |e| e.localizedDescription().to_string())
}

fn to_rgba(image: &CGImage) -> Option<RawImage> {
    let width = CGImage::width(Some(image));
    let height = CGImage::height(Some(image));
    if width == 0 || height == 0 {
        return None;
    }
    let mut rgba = vec![0u8; width.checked_mul(height)?.checked_mul(4)?];
    let space = CGColorSpace::new_device_rgb()?;
    let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    // SAFETY: `rgba` is large enough for `height` rows of `width * 4` bytes and outlives `ctx`.
    let ctx =
        unsafe { CGBitmapContextCreate(rgba.as_mut_ptr().cast(), width, height, 8, width * 4, Some(&space), info) }?;
    #[allow(clippy::cast_precision_loss)] // image dimensions are far below 2^52
    let bounds = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width as f64, height as f64));
    CGContext::draw_image(Some(&ctx), bounds, Some(image));
    drop(ctx);
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = u8::try_from((u16::from(*c) * 255 + a / 2) / a).unwrap_or(u8::MAX);
            }
        }
    }
    Some(RawImage { width: u32::try_from(width).ok()?, height: u32::try_from(height).ok()?, rgba })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pixels(value: f64) -> usize {
    value.round().max(1.0) as usize
}
