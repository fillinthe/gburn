//! One frame from the browser's camera (getUserMedia), as grey pixels, for the camera overlay.

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

async fn sleep(ms: i32) {
    let p = js_sys::Promise::new(&mut |res, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&res, ms);
        }
    });
    let _ = JsFuture::from(p).await;
}

/// (width, height, grey pixels row by row from the top); the camera is switched off again afterwards
pub async fn frame() -> Result<(u32, u32, Vec<u8>), String> {
    let win = web_sys::window().ok_or("no window")?;
    let md = win.navigator().media_devices().map_err(|_| "this browser has no camera access")?;
    let c = web_sys::MediaStreamConstraints::new();
    c.set_video(&wasm_bindgen::JsValue::TRUE);
    let p = md.get_user_media_with_constraints(&c).map_err(|_| "the camera was refused")?;
    let stream: web_sys::MediaStream = JsFuture::from(p).await.map_err(|_| "the camera was refused or isn't there")?.dyn_into().map_err(|_| "no camera stream")?;
    let doc = win.document().ok_or("no document")?;
    let video: web_sys::HtmlVideoElement = doc.create_element("video").map_err(|_| "no video")?.dyn_into().map_err(|_| "no video")?;
    video.set_muted(true);
    let _ = video.set_attribute("playsinline", "");
    video.set_src_object(Some(&stream));
    if let Ok(p) = video.play() {
        let _ = JsFuture::from(p).await;
    }
    for _ in 0..40 {
        if video.video_width() > 0 {
            break;
        }
        sleep(100).await;
    }
    sleep(500).await; // let the exposure settle
    let (w, h) = (video.video_width(), video.video_height());
    let result = (|| {
        if w == 0 || h == 0 {
            return Err("the camera sent no picture".to_string());
        }
        let canvas: web_sys::HtmlCanvasElement = doc.create_element("canvas").map_err(|_| "no canvas")?.dyn_into().map_err(|_| "no canvas")?;
        canvas.set_width(w);
        canvas.set_height(h);
        let g: web_sys::CanvasRenderingContext2d = canvas.get_context("2d").ok().flatten().ok_or("no 2d")?.dyn_into().map_err(|_| "no 2d")?;
        g.draw_image_with_html_video_element(&video, 0.0, 0.0).map_err(|_| "couldn't grab the frame")?;
        let rgba = g.get_image_data(0.0, 0.0, w as f64, h as f64).map_err(|_| "couldn't read the frame")?.data().0;
        Ok(rgba.as_chunks::<4>().0.iter().map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) as u8).collect())
    })();
    for t in stream.get_tracks().iter() {
        if let Ok(t) = t.dyn_into::<web_sys::MediaStreamTrack>() {
            t.stop();
        }
    }
    result.map(|px| (w, h, px))
}
