use std::sync::Arc;

use zenterm_config::Config;

fn main() -> eframe::Result<()> {
    // Initialise logging.
    env_logger::init();

    // On macOS, bind the notification system to our bundle identifier early.
    // Without this, mac-notification-sys falls back to a hardcoded "use_default"
    // app name, which triggers a "Choose Application" dialog on Sequoia 15+.
    #[cfg(target_os = "macos")]
    if let Err(e) = notify_rust::set_application("org.eu.eslzzyl.zenterm") {
        log::warn!("failed to set macOS notification bundle: {e}");
    }

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

/// Pick a pixel size for the initial window.
///
/// If we have a last-known window size from a previous session, use it
/// directly — this gives pixel-perfect restoration regardless of font
/// metrics.  Otherwise, fall back to estimating from the configured
/// terminal grid and font size.
fn estimate_window_size(config: &Config) -> egui::Vec2 {
    // Exact restoration from a previous session.
    if let Some([w, h]) = config.window.last_window_size {
        return egui::Vec2::new(w.max(400.0), h.max(200.0));
    }

    // Rough estimate: convert grid cells → pixels using guessed
    // font metrics.  This is only used on the very first launch
    // (before any window resize has been persisted).
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
