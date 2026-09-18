//! egui UI font setup, including a system CJK fallback.

use std::borrow::Cow;
use std::sync::Arc;

const CJK_FALLBACK_NAME: &str = "zenterm-ui-cjk-fallback";

/// Install the default egui UI fonts, Phosphor icons, and a system CJK
/// fallback.  The fallback is appended after egui's primary faces so Latin
/// metrics and the icon font remain unchanged.
pub(crate) fn install_ui_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    crate::icons::init_fonts(&mut fonts);
    append_cjk_fallback(&mut fonts);
    ctx.set_fonts(fonts);
}

/// Append a validated CJK-capable system font to egui's proportional and
/// monospace families.  Returns whether a fallback was installed.
pub(crate) fn append_cjk_fallback(fonts: &mut egui::FontDefinitions) -> bool {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let Some(source) = zenterm_glyph::font_list::find_cjk_font_source(&db) else {
        log::warn!("egui UI CJK fallback: no CJK-capable system font found");
        return false;
    };
    let Ok(data) = std::fs::read(&source.path) else {
        log::warn!(
            "egui UI CJK fallback: failed to read {}",
            source.path.display()
        );
        return false;
    };
    if !zenterm_glyph::font_list::validate_font_data(&data, source.index) {
        log::warn!(
            "egui UI CJK fallback: rejected {}:{}",
            source.path.display(),
            source.index
        );
        return false;
    }

    fonts.font_data.insert(
        CJK_FALLBACK_NAME.to_owned(),
        Arc::new(egui::FontData {
            font: Cow::Owned(data),
            index: source.index,
            tweak: Default::default(),
        }),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let entries = fonts.families.entry(family).or_default();
        if !entries.iter().any(|name| name == CJK_FALLBACK_NAME) {
            entries.push(CJK_FALLBACK_NAME.to_owned());
        }
    }
    log::info!(
        "egui UI CJK fallback: installed {}:{}",
        source.path.display(),
        source.index
    );
    true
}
