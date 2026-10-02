//! Screenshot D-Bus interface implementation.
//!
//! Implements `org.freedesktop.impl.portal.Screenshot` version 2.
//!
//! Provides two methods:
//! - `Screenshot` — captures a screen frame, encodes as PNG, returns file URI
//! - `PickColor` — captures a frame and returns the color at a point (stub)
//!
//! Uses the existing screencopy infrastructure for one-shot frame capture.

use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
};

use tokio::sync::Mutex;
use zbus::{
    interface,
    zvariant::{ObjectPath, OwnedValue, Value},
};

use super::{Response, empty_results, get_option_bool};
use crate::{
    services::capture::CaptureBackend,
    types::SourceType,
    wayland::{CaptureCommand, ScreenshotData},
};

/// Screenshot portal interface implementation.
pub struct ScreenshotInterface {
    /// Capture backend for getting available sources.
    capture_backend: Arc<Mutex<Box<dyn CaptureBackend>>>,
    /// Capture command sender for one-shot screenshots via the Wayland event loop.
    capture_tx: mpsc::Sender<CaptureCommand>,
}

impl ScreenshotInterface {
    /// Create a new Screenshot interface.
    pub fn new(
        capture_backend: Arc<Mutex<Box<dyn CaptureBackend>>>,
        capture_tx: mpsc::Sender<CaptureCommand>,
    ) -> Self {
        Self {
            capture_backend,
            capture_tx,
        }
    }
}

#[interface(
    name = "org.freedesktop.impl.portal.Screenshot",
    introspection_docs = false
)]
impl ScreenshotInterface {
    /// Capture a screenshot of the screen.
    ///
    /// Captures a single frame via screencopy, encodes it as PNG, saves to
    /// a temporary file, and returns the file URI.
    #[zbus(name = "Screenshot")]
    async fn screenshot(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let _ = parent_window;
        let _interactive = get_option_bool(&options, "interactive").unwrap_or(false);

        tracing::debug!(app_id = app_id, "Screenshot.Screenshot called");

        let (request, mut cancelled) = super::RequestInterface::cancellable();
        server.at(&handle, request).await?;
        let result = async {
        let sources = self.capture_backend.lock().await.get_sources(&[SourceType::Monitor])
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let selected = crate::picker::select(app_id, handle.as_str(), knave_portal_api::Operation::Screenshot,
            false, &sources, &self.capture_tx, cancelled.clone()).await;
        let output_id = match selected {
            Ok(selected) if !selected.is_empty() => selected[0].id,
            Ok(_) => return Ok((Response::Cancelled.to_u32(), empty_results())),
            Err(error) => { tracing::warn!(%error, "screenshot selection failed"); return Ok((Response::Other.to_u32(), empty_results())); }
        };

        // Request a one-shot frame capture via the Wayland event loop
        let (reply_tx, reply_rx) =
            tokio::sync::oneshot::channel::<std::result::Result<ScreenshotData, String>>();

        self.capture_tx
            .send(CaptureCommand::CaptureScreenshot {
                output_global_name: output_id,
                reply: reply_tx,
            })
            .map_err(|e| {
                zbus::fdo::Error::Failed(format!("Failed to send screenshot command: {e}"))
            })?;

        // Wait for the frame data from the event loop
        let screenshot_data = tokio::select! {
            result = tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx) => result.map_err(|_| zbus::fdo::Error::Failed("screenshot timed out".into()))?,
            _ = cancelled.changed() => return Ok((Response::Cancelled.to_u32(), empty_results())),
        }
            .map_err(|_| zbus::fdo::Error::Failed("Screenshot capture channel closed".to_string()))?
            .map_err(|e| zbus::fdo::Error::Failed(format!("Screenshot capture failed: {e}")))?;

        // Encode as PNG and save to temp file
        let uri = encode_and_save_png(&screenshot_data)
            .map_err(|e| zbus::fdo::Error::Failed(format!("PNG encoding failed: {e}")))?;

        tracing::info!(
            uri = %uri,
            width = screenshot_data.width,
            height = screenshot_data.height,
            "Screenshot captured"
        );

        let mut results = HashMap::new();
        if let Ok(val) = OwnedValue::try_from(Value::from(uri.as_str())) {
            results.insert("uri".to_string(), val);
        }

        Ok((Response::Success.to_u32(), results))
        }.await;
        let _ = server.remove::<super::RequestInterface, _>(&handle).await;
        result
    }

    /// Pick a color from the screen.
    ///
    /// Returns NotSupported until a native pixel-selection UI is implemented.
    #[zbus(name = "PickColor")]
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "zbus interface requires async"
    )]
    async fn pick_color(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let _ = (handle, app_id, parent_window, options, server);
        // A center-pixel fallback does not implement user-selected color picking.
        Err(zbus::fdo::Error::NotSupported(
            "Native color picking is not implemented".into(),
        ))
    }

    // === Properties ===

    /// Interface version.
    #[zbus(property, name = "version")]
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "zbus interface requires async"
    )]
    async fn version(&self) -> u32 {
        2
    }
}

/// Convert `BGRx`/ARGB pixel data to RGBA and encode as PNG, saving to a temp file.
///
/// Returns the file URI (e.g., `file:///tmp/xdp-screenshot-XXXX.png`).
fn encode_and_save_png(data: &ScreenshotData) -> Result<String, String> {
    // Convert BGRx to RGBA
    let rgba = convert_bgrx_to_rgba(
        &data.data,
        data.width,
        data.height,
        data.stride,
        data.format_raw,
    );

    // Unpredictable, owner-only file; retained for the frontend to export.
    let temporary = tempfile::Builder::new()
        .prefix("knave-screenshot-")
        .suffix(".png")
        .tempfile()
        .map_err(|e| format!("Failed to create screenshot: {e}"))?;
    let (file, path) = temporary.keep().map_err(|e| e.to_string())?;

    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), data.width, data.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);

    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("PNG header error: {e}"))?;

    writer
        .write_image_data(&rgba)
        .map_err(|e| format!("PNG write error: {e}"))?;

    writer
        .finish()
        .map_err(|e| format!("PNG finish error: {e}"))?;

    let uri = format!("file://{}", path.display());
    Ok(uri)
}

/// Convert a captured 32-bit-per-pixel buffer to RGBA for PNG encoding.
///
/// The compositor's `wl_shm` buffer format determines the actual in-memory
/// channel order (`format_raw`; see [`crate::types::wl_shm_format_needs_rb_swap`]
/// for the full explanation) -- `argb8888`/`xrgb8888` land as `[B,G,R,X/A]`,
/// `xbgr8888`/`abgr8888` (e.g. wlroots + virtio-gpu) land as `[R,G,B,X/A]`.
/// Reading the wrong order here transposes red and blue in the output PNG.
pub(crate) fn convert_bgrx_to_rgba(
    data: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    format_raw: u32,
) -> Vec<u8> {
    let swap_rb = crate::types::wl_shm_format_needs_rb_swap(format_raw);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);

    for y in 0..height {
        let row_start = (y * stride) as usize;
        for x in 0..width {
            let pixel_offset = row_start + (x * 4) as usize;
            if pixel_offset + 3 < data.len() {
                let first = data[pixel_offset];
                let g = data[pixel_offset + 1];
                let third = data[pixel_offset + 2];
                let a = data[pixel_offset + 3];
                // Already-BGR-ordered input: first=B, third=R -> emit (third, g, first).
                // RGB-ordered input (needs swap): first=R, third=B -> emit (first, g, third).
                let (r, b) = if swap_rb {
                    (first, third)
                } else {
                    (third, first)
                };
                rgba.push(r);
                rgba.push(g);
                rgba.push(b);
                // Use alpha if available (ARGB8888/ABGR8888), otherwise opaque.
                rgba.push(if a == 0 { 255 } else { a });
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 255]);
            }
        }
    }

    rgba
}

/// Pick the color at specific coordinates in the captured frame.
///
/// Returns (r, g, b) as f64 values in the range 0.0 to 1.0.
#[cfg(test)]
fn pick_color_at(data: &ScreenshotData, px: u32, py: u32) -> (f64, f64, f64) {
    let px = px.min(data.width.saturating_sub(1));
    let py = py.min(data.height.saturating_sub(1));
    let offset = (py * data.stride + px * 4) as usize;

    if offset + 3 <= data.data.len() {
        // BGRx layout in memory
        let blue = f64::from(data.data[offset]) / 255.0;
        let green = f64::from(data.data[offset + 1]) / 255.0;
        let red = f64::from(data.data[offset + 2]) / 255.0;
        (red, green, blue)
    } else {
        (0.0, 0.0, 0.0)
    }
}

/// Pick the color at the center of the captured frame.
///
/// Returns (r, g, b) as f64 values in the range 0.0 to 1.0.
#[cfg(test)]
fn pick_center_color(data: &ScreenshotData) -> (f64, f64, f64) {
    let cx = data.width / 2;
    let cy = data.height / 2;
    pick_color_at(data, cx, cy)
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "tests use expect for clearer failure messages"
)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_bgrx_to_rgba() {
        // BGRx (xrgb8888, format_raw=1): B=0x10, G=0x20, R=0x30, X=0xFF
        let bgrx = vec![0x10, 0x20, 0x30, 0xFF];
        let rgba = convert_bgrx_to_rgba(&bgrx, 1, 1, 4, 1);
        assert_eq!(rgba, vec![0x30, 0x20, 0x10, 0xFF]); // R, G, B, A
    }

    #[test]
    fn test_convert_bgrx_to_rgba_with_stride() {
        // 2x1 image with stride=12 (8 bytes of pixels + 4 bytes padding), xrgb8888
        let mut bgrx = vec![0u8; 12];
        // Pixel (0,0): B=0xFF, G=0x00, R=0x00, X=0xFF (blue)
        bgrx[0] = 0xFF;
        bgrx[1] = 0x00;
        bgrx[2] = 0x00;
        bgrx[3] = 0xFF;
        // Pixel (1,0): B=0x00, G=0xFF, R=0x00, X=0xFF (green)
        bgrx[4] = 0x00;
        bgrx[5] = 0xFF;
        bgrx[6] = 0x00;
        bgrx[7] = 0xFF;

        let rgba = convert_bgrx_to_rgba(&bgrx, 2, 1, 12, 1);
        assert_eq!(rgba.len(), 8);
        // Pixel 0: R=0, G=0, B=255, A=255
        assert_eq!(rgba[0..4], [0x00, 0x00, 0xFF, 0xFF]);
        // Pixel 1: R=0, G=255, B=0, A=255
        assert_eq!(rgba[4..8], [0x00, 0xFF, 0x00, 0xFF]);
    }

    #[test]
    fn test_convert_rgbx_to_rgba_swaps_channels() {
        // Regression test for the color-format bug: xbgr8888 (format_raw
        // 0x34324258, e.g. wlroots + virtio-gpu) is RGBx in memory, not
        // BGRx. Without consulting format_raw this pixel's red and blue
        // were transposed in the output PNG.
        // xbgr8888 in-memory: R=0x30, G=0x20, B=0x10, X=0xFF
        let rgbx = vec![0x30, 0x20, 0x10, 0xFF];
        let rgba = convert_bgrx_to_rgba(&rgbx, 1, 1, 4, 0x3432_4258);
        assert_eq!(rgba, vec![0x30, 0x20, 0x10, 0xFF]); // R, G, B, A -- unswapped from source
    }

    #[test]
    fn test_pick_color_at_specific_pixel() {
        // 2x2 image: pixel (0,0)=red, pixel (1,0)=green
        let mut data = vec![0u8; 2 * 2 * 4];
        // (0,0) BGRx: B=0, G=0, R=255, X=255
        data[0] = 0;
        data[1] = 0;
        data[2] = 255;
        data[3] = 255;
        // (1,0) BGRx: B=0, G=255, R=0, X=255
        data[4] = 0;
        data[5] = 255;
        data[6] = 0;
        data[7] = 255;

        let screenshot = ScreenshotData {
            data,
            width: 2,
            height: 2,
            stride: 8,
            format_raw: 0,
        };

        let (r, g, b) = pick_color_at(&screenshot, 0, 0);
        assert!((r - 1.0).abs() < 0.01);
        assert!(g.abs() < 0.01);
        assert!(b.abs() < 0.01);

        let (r, g, b) = pick_color_at(&screenshot, 1, 0);
        assert!(r.abs() < 0.01);
        assert!((g - 1.0).abs() < 0.01);
        assert!(b.abs() < 0.01);
    }

    #[test]
    fn test_pick_color_at_clamped() {
        // 1x1 image, should clamp out-of-bounds coordinates
        let data = vec![0, 0, 255, 255]; // BGRx: red
        let screenshot = ScreenshotData {
            data,
            width: 1,
            height: 1,
            stride: 4,
            format_raw: 0,
        };

        let (r, _g, _b) = pick_color_at(&screenshot, 100, 100);
        assert!((r - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_pick_center_color() {
        // 2x2 image, all pixels red (BGRx: B=0, G=0, R=255, X=255)
        let mut data = vec![0u8; 2 * 2 * 4];
        for i in (0..data.len()).step_by(4) {
            data[i] = 0; // B
            data[i + 1] = 0; // G
            data[i + 2] = 255; // R
            data[i + 3] = 255; // X
        }

        let screenshot = ScreenshotData {
            data,
            width: 2,
            height: 2,
            stride: 8,
            format_raw: 0,
        };

        let (r, g, b) = pick_center_color(&screenshot);
        assert!((r - 1.0).abs() < 0.01);
        assert!(g.abs() < 0.01);
        assert!(b.abs() < 0.01);
    }

    #[test]
    fn test_pick_center_color_empty() {
        let screenshot = ScreenshotData {
            data: vec![],
            width: 0,
            height: 0,
            stride: 0,
            format_raw: 0,
        };

        let (red, green, blue) = pick_center_color(&screenshot);
        assert!(red.abs() < f64::EPSILON);
        assert!(green.abs() < f64::EPSILON);
        assert!(blue.abs() < f64::EPSILON);
    }

    #[test]
    fn test_encode_and_save_png() {
        // Create a small 4x4 test image (BGRx format)
        let mut data = vec![0u8; 4 * 4 * 4];
        for i in (0..data.len()).step_by(4) {
            data[i] = 128; // B
            data[i + 1] = 64; // G
            data[i + 2] = 32; // R
            data[i + 3] = 255; // X
        }

        let screenshot = ScreenshotData {
            data,
            width: 4,
            height: 4,
            stride: 16,
            format_raw: 0,
        };

        let uri = encode_and_save_png(&screenshot).expect("PNG encoding should succeed");
        assert!(uri.starts_with("file:///"));
        assert!(
            std::path::Path::new(&uri)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
        );

        // Verify the file exists
        let path = uri
            .strip_prefix("file://")
            .expect("should have file:// prefix");
        assert!(std::path::Path::new(path).exists());

        // Clean up
        let _ = std::fs::remove_file(path);
    }
}
