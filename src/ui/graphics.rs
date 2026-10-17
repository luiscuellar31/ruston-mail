//! Which graphics API draws the windows. macOS and Linux draw with OpenGL.
//! Windows draws with DirectX 12, which wgpu also runs on WARP, Windows'
//! software renderer, where there is no GPU; OpenGL remains for a machine
//! whose DirectX 12 cannot draw the window.

/// Set to `opengl` to draw with OpenGL on Windows too.
#[cfg(target_os = "windows")]
const RENDERER_ENV: &str = "RUSTON_RENDERER";

pub(super) fn preferred_renderer() -> eframe::Renderer {
    #[cfg(target_os = "windows")]
    if std::env::var_os(RENDERER_ENV).is_some_and(|value| value == "opengl") {
        eframe::Renderer::Glow
    } else {
        eframe::Renderer::Wgpu
    }
    #[cfg(not(target_os = "windows"))]
    eframe::Renderer::Glow
}

/// DirectX 12 on the GPU that uses the least power: the integrated one on a
/// laptop that also has a discrete GPU, which mail does not need. With no GPU
/// at all, wgpu takes WARP.
#[cfg(target_os = "windows")]
pub(super) fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = wgpu::Backends::DX12;
    setup.power_preference = wgpu::PowerPreference::LowPower;
    eframe::egui_wgpu::WgpuConfiguration {
        wgpu_setup: setup.into(),
        ..Default::default()
    }
}
