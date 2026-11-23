//! One frame from the computer's camera (AVFoundation on the Mac, Media Foundation on Windows, through nokhwa), as grey pixels, for the camera overlay.
//! Runs on its own thread: macOS asks the user for camera access the first time.

use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use nokhwa::Camera;

/// (width, height, grey pixels row by row from the top); the camera is switched off again afterwards
pub fn frame() -> Result<(u32, u32, Vec<u8>), String> {
    #[cfg(target_os = "macos")]
    if !nokhwa::nokhwa_check() {
        let (tx, rx) = std::sync::mpsc::channel();
        nokhwa::nokhwa_initialize(move |granted| {
            let _ = tx.send(granted);
        });
        if !rx.recv_timeout(std::time::Duration::from_secs(60)).unwrap_or(false) {
            return Err("no camera access (System Settings › Privacy & Security › Camera)".into());
        }
    }
    let mut cam = Camera::new(CameraIndex::Index(0), RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestResolution)).map_err(|e| format!("no camera: {e}"))?;
    cam.open_stream().map_err(|e| format!("the camera didn't start: {e}"))?;
    // a few frames first: the exposure settles
    for _ in 0..8 {
        let _ = cam.frame();
    }
    let got = cam.frame().map_err(|e| e.to_string()).and_then(|f| f.decode_image::<RgbFormat>().map_err(|e| e.to_string()));
    let _ = cam.stop_stream();
    let img = got.map_err(|e| format!("couldn't read the camera: {e}"))?;
    let (w, h) = img.dimensions();
    let px: Vec<u8> = img.pixels().map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) as u8).collect();
    Ok((w, h, px))
}
