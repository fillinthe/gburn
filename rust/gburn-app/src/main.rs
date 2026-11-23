//! GBurn entry: native window (Windows, macOS, Linux) or the browser canvas (wasm32). Same App either way.

mod app;
#[cfg(target_os = "macos")]
mod cam_native;
mod fonts;
mod galvo_link;
mod i18n;
mod k40_link;
mod laser;
mod look;
#[cfg(not(target_arch = "wasm32"))]
mod m1_link;
mod machines;
mod menu;
#[cfg(target_os = "macos")]
mod native;
mod ruida_link;
#[cfg(target_arch = "wasm32")]
mod webcam;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    gcarve_ui::store::set_app("GBurn");
    gcarve_ui::help::init("fillinthe/gburn", env!("CARGO_PKG_VERSION")); // before anything can report, so errors carry GBurn's version
    #[cfg(target_os = "macos")]
    native::catch_launch_files();
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        gcarve_ui::store::save("crash.txt", &format!("v{} {}: {info}", env!("CARGO_PKG_VERSION"), std::env::consts::OS));
    }));
    let size = std::env::var("GBURN_SIZE").ok().and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some([w.parse().ok()?, h.parse().ok()?])
    });
    let viewport = eframe::egui::ViewportBuilder::default().with_title("GBurn").with_inner_size(size.unwrap_or([1440.0, 900.0])).with_min_inner_size(if std::env::var_os("GBURN_SHOT").is_some() {
        [200.0, 200.0]
    } else {
        [720.0, 480.0]
    });
    // the rounded icon with clear corners (the web's square picture showed a white square in the Dock)
    let viewport = viewport.with_icon(window_icon());
    let options = eframe::NativeOptions {
        viewport,
        // ponytail: no multisampling on Windows (software GL has no 4× config and eframe then opens no window)
        multisampling: if cfg!(target_os = "windows") { 0 } else { 4 },
        ..Default::default()
    };
    let r = eframe::run_native("GBurn", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))));
    #[cfg(windows)]
    if let Err(e) = &r {
        return software_gl(e);
    }
    r
}

/// No usable OpenGL (a PC or VM without a graphics driver, Remote Desktop: Windows' own GL is 1.1): start again on
/// the bundled Mesa software renderer. glutin links wgl* to opengl32.dll at load, so Mesa's opengl32.dll must sit
/// beside the exe: copy gburn.exe and software\*.dll into %LOCALAPPDATA%\GBurn\software-gl and run that copy on Mesa
/// (the unzipped folder may be read-only, and a GPU driver must win everywhere else; Mesa's own D3D12 path hung in a
/// VM like Microsoft's GL-on-D3D12 pack). If that fails too, say so in a box
/// instead of closing silently (a GUI exe has no console for the error).
#[cfg(windows)]
fn software_gl(e: &eframe::Error) -> eframe::Result {
    const ONCE: &str = "GBURN_SOFTWARE_GL";
    let relaunch = || -> Option<()> {
        if std::env::var_os(ONCE).is_some() {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        let soft = exe.parent()?.join("software");
        let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("GBurn").join("software-gl");
        std::fs::create_dir_all(&dir).ok()?;
        for (src, name) in [(exe.clone(), "gburn.exe"), (soft.join("opengl32.dll"), "opengl32.dll"), (soft.join("libgallium_wgl.dll"), "libgallium_wgl.dll")] {
            let (a, to) = (std::fs::metadata(&src).ok()?, dir.join(name));
            let same = std::fs::metadata(&to).is_ok_and(|b| b.len() == a.len() && b.modified().ok() >= a.modified().ok());
            if !same {
                std::fs::copy(&src, &to).ok()?;
            }
        }
        // llvmpipe (fast, makes CPU code as it goes); if that dies at once (x64 emulation on ARM PCs refuses its
        // code: illegal instruction), softpipe (slow, plain code). This process waits, windowless, for the copy.
        let run = |driver: &str| {
            let t = std::time::Instant::now();
            let ok = std::process::Command::new(dir.join("gburn.exe")).args(std::env::args_os().skip(1)).env(ONCE, "1").env("GALLIUM_DRIVER", driver).status().ok()?.success();
            Some(ok || t.elapsed().as_secs() > 10)
        };
        if run("llvmpipe")? {
            return Some(());
        }
        run("softpipe").map(|_| ())
    };
    if relaunch().is_some() {
        return Ok(());
    }
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let text = format!(
        "GBurn couldn't open its window: this PC's graphics don't offer OpenGL 2.\n\n\
         Update the graphics driver (Intel, AMD or NVIDIA). On a Windows ARM PC, install Microsoft's \
         \"OpenCL, OpenGL, and Vulkan Compatibility Pack\" from the Microsoft Store.\n\n({e})"
    );
    unsafe { MessageBoxW(0, wide(&text).as_ptr(), wide("GBurn").as_ptr(), 0x10) };
    Err(eframe::Error::AppCreation(e.to_string().into()))
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast;
    gcarve_ui::store::set_app("GBurn");
    gcarve_ui::help::init("fillinthe/gburn", env!("CARGO_PKG_VERSION")); // before anything can report, so errors carry GBurn's version
    std::panic::set_hook(Box::new(|info| {
        let msg = info.to_string();
        web_sys::console::error_1(&msg.clone().into());
        gcarve_ui::cloud::report_panic(&msg); // the site's error log (admin overview): the panic text, app, version
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            if doc.get_element_by_id("gburn-crash").is_none() {
                if let Ok(el) = doc.create_element("div") {
                    el.set_id("gburn-crash");
                    let _ = el.set_attribute("style", "position:fixed;left:50%;top:16px;transform:translateX(-50%);z-index:9999;background:#3a1f1a;color:#fff;border:1px solid #e0603c;border-radius:8px;padding:12px 16px;font:14px system-ui,sans-serif;max-width:90vw");
                    el.set_inner_html("GBurn hit a problem and stopped. <a href=\"\" style=\"color:#8ec5ff\">Reload</a> to continue: your design from a few seconds ago is offered back.");
                    if let Some(body) = doc.body() {
                        let _ = body.append_child(&el);
                    }
                }
            }
        }
    }));
    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window().unwrap().document().unwrap();
        let canvas = document.get_element_by_id("gburn").unwrap().dyn_into::<web_sys::HtmlCanvasElement>().unwrap();
        let result = eframe::WebRunner::new().start(canvas, eframe::WebOptions::default(), Box::new(|cc| Ok(Box::new(app::App::new(cc))))).await;
        if let Some(loading) = document.get_element_by_id("loading") {
            match result {
                Ok(_) => loading.remove(),
                Err(e) => {
                    gcarve_ui::cloud::report_panic(&format!("GBurn couldn't start: {e:?}"));
                    let msg = format!("{e:?}");
                    loading.set_inner_html(&if msg.contains("WebGL") {
                        // the browser's 3D drawing is off (hardware acceleration, blocked graphics driver, some VMs)
                        "GBurn needs WebGL, and this browser has it turned off. Turn on <b>hardware acceleration</b> in the browser's settings \
                         (Chrome / Edge / Brave: Settings › System), restart the browser and reload, or try another browser. \
                         Or use the <a href=\"/download/\">desktop app</a>, which doesn't need it.".to_string()
                    } else {
                        format!("GBurn couldn't start in this browser: {msg}")
                    })
                }
            }
        }
    });
}

/// The window / taskbar / Dock icon (the same artwork as the Mac bundle's GBurn.icns).
#[cfg(not(target_arch = "wasm32"))]
fn window_icon() -> std::sync::Arc<eframe::egui::IconData> {
    let (width, height, rgba) = gcarve_ui::shot::png_rgba(include_bytes!("../icon-512.png")).unwrap_or((1, 1, vec![0; 4]));
    std::sync::Arc::new(eframe::egui::IconData { rgba, width, height })
}
