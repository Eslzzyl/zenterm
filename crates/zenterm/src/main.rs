use std::sync::Arc;

fn main() -> eframe::Result<()> {
    // Initialise logging.
    env_logger::init();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size((960.0, 540.0))
            .with_title("Zenterm"),
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

    eframe::run_native(
        "Zenterm",
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
            )))
        }),
    )
}
