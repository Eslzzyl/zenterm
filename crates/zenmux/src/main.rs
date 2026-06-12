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
        Box::new(|_cc| Ok(Box::new(zenmux_ui::ZenmuxApp::new()))),
    )
}
