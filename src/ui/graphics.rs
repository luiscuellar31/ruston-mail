//! Which graphics API draws the windows. macOS and Linux draw with OpenGL.
//! Windows draws with DirectX 12, which wgpu also runs on WARP, Windows'
//! software renderer, where there is no GPU; OpenGL remains for a machine
//! whose DirectX 12 cannot draw the window.

/// Set to `opengl` to draw with OpenGL on Windows too.
#[cfg(target_os = "windows")]
const RENDERER_ENV: &str = "RUSTON_RENDERER";

/// DirectX 12, unless `RUSTON_RENDERER=opengl` asks for OpenGL.
#[cfg(target_os = "windows")]
pub(super) fn preferred_renderer() -> eframe::Renderer {
    if std::env::var_os(RENDERER_ENV).is_some_and(|value| value == "opengl") {
        eframe::Renderer::Glow
    } else {
        eframe::Renderer::Wgpu
    }
}

/// OpenGL, the one renderer built here.
#[cfg(not(target_os = "windows"))]
pub(super) fn preferred_renderer() -> eframe::Renderer {
    eframe::Renderer::Glow
}

/// The renderer to try when `result` says the first could not draw the
/// window, and why, to print: OpenGL, after DirectX 12 failed.
#[cfg(target_os = "windows")]
pub(super) fn next_renderer(result: &eframe::Result) -> Option<(eframe::Renderer, String)> {
    match result {
        Err(eframe::Error::Wgpu(error)) => Some((
            eframe::Renderer::Glow,
            format!("DirectX 12 cannot draw the window ({error}); using OpenGL"),
        )),
        _ => None,
    }
}

/// OpenGL is the only renderer here, with none after it.
#[cfg(not(target_os = "windows"))]
pub(super) fn next_renderer(_result: &eframe::Result) -> Option<(eframe::Renderer, String)> {
    None
}

/// The main window's options, drawn by `renderer`: with DirectX 12's set-up.
#[cfg(target_os = "windows")]
pub(super) fn native_options(
    viewport: eframe::egui::ViewportBuilder,
    renderer: eframe::Renderer,
) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport,
        renderer,
        wgpu_options: wgpu_options(),
        ..Default::default()
    }
}

/// The main window's options, drawn by `renderer`.
#[cfg(not(target_os = "windows"))]
pub(super) fn native_options(
    viewport: eframe::egui::ViewportBuilder,
    renderer: eframe::Renderer,
) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport,
        renderer,
        ..Default::default()
    }
}

/// DirectX 12 on the GPU that uses the least power: the integrated one on a
/// laptop that also has a discrete GPU, which mail does not need. With no GPU
/// at all, wgpu takes WARP.
#[cfg(target_os = "windows")]
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = wgpu::Backends::DX12;
    setup.power_preference = wgpu::PowerPreference::LowPower;
    eframe::egui_wgpu::WgpuConfiguration {
        wgpu_setup: setup.into(),
        ..Default::default()
    }
}
