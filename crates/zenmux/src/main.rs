fn main() -> eframe::Result<()> {
    // Initialise logging.
    env_logger::init();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size((960.0, 540.0))
            .with_title("Zenmux"),
        ..Default::default()
    };

    eframe::run_native(
        "Zenmux",
        native_options,
        Box::new(|cc| {
            let render_state = cc
                .wgpu_render_state
                .clone()
                .expect("zenmux requires wgpu rendering");

            let pixels_per_point = cc.egui_ctx.pixels_per_point();

            Ok(Box::new(zenmux_ui::ZenmuxApp::new_with_wgpu(
                render_state.device,
                render_state.queue,
                render_state.target_format,
                pixels_per_point,
            )))
        }),
    )
}
