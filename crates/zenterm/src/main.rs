use std::sync::Arc;

use zenterm_config::Config;

fn main() -> eframe::Result<()> {
    // Initialise logging.
    env_logger::init();

    // Load configuration from TOML file.
    // If the file is missing or invalid we log a warning and use defaults.
    let config = match Config::load() {
        Ok(cfg) => {
            log::info!("config: loaded from {:?}", Config::path());
            cfg
        }
        Err(e) => {
            log::error!("config: error loading {:?} — using defaults: {e}", Config::path(),);
            Config::default()
        }
    };

    // Estimate a window size that accommodates the desired terminal grid.
    let initial_size = estimate_window_size(&config);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(initial_size)
            .with_title(&config.window.title)
            .with_transparent(config.window.opacity < 1.0)
            .with_resizable(true),
        // Use smaller GPU memory blocks — we're a terminal, not a game.
        // Performance (default) pre-allocates 128–256 MB blocks from the
        // driver; MemoryUsage starts at 8 MB and grows as needed.
        // See https://github.com/gfx-rs/wgpu/pull/5875
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(
                eframe::egui_wgpu::WgpuSetupCreateNew {
                    instance_descriptor: wgpu::InstanceDescriptor::new_without_display_handle(),
                    display_handle: None,
                    power_preference: wgpu::PowerPreference::default(),
                    native_adapter_selector: None,
                    device_descriptor: Arc::new(|_adapter: &wgpu::Adapter| {
                        wgpu::DeviceDescriptor {
                            label: None,
                            required_features: wgpu::Features::empty(),
                            required_limits: wgpu::Limits::default(),
                            memory_hints: wgpu::MemoryHints::MemoryUsage,
                            trace: wgpu::Trace::Off,
                            experimental_features: unsafe {
                                wgpu::ExperimentalFeatures::enabled()
                            },
                        }
                    }),
                },
            ),
            ..Default::default()
        },
        ..Default::default()
    };

    let window_title = config.window.title.clone();

    eframe::run_native(
        &window_title,
        native_options,
        Box::new(|cc| {
            let render_state = cc
                .wgpu_render_state
                .clone()
                .expect("zenterm requires wgpu rendering");

            let pixels_per_point = cc.egui_ctx.pixels_per_point();

            Ok(Box::new(zenterm_ui::ZentermApp::new_with_wgpu(
                render_state.device,
                render_state.queue,
                render_state.target_format,
                pixels_per_point,
                config,
            )))
        }),
    )
}

/// Roughly estimate the pixel size of a window that can hold the
/// configured grid at the configured font size.
///
/// This is a best-effort starting size since the real cell dimensions
/// aren't known until the font atlas is initialised inside the app.
fn estimate_window_size(config: &Config) -> egui::Vec2 {
    let cols = config.window.dimensions.columns.max(40) as f32;
    let rows = config.window.dimensions.lines.max(10) as f32;
    // At 1× DPI, `config.font.size` is the logical pixel size.
    // Use a generous aspect ratio estimate (character ~0.6× cell height).
    let cell_w = config.font.size * 0.6;
    let cell_h = config.font.size * 1.2;
    egui::Vec2::new(
        (cols * cell_w).round().max(400.0),
        (rows * cell_h).round().max(200.0),
    )
}
