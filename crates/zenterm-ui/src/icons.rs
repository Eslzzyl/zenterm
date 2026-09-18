//! Phosphor vector icon integration for Zenterm.
//!
//! Embeds the Phosphor Regular icon font into egui, providing
//! crisp, resolution-independent, battle-tested vector glyphs
//! without OS emoji divergence, missing glyph tofu, or crude line approximations.

use egui::{Color32, FontDefinitions, FontId, Rect, Response, Sense, Ui, Vec2, WidgetText};

// Re-export common Phosphor regular icon glyphs
pub use egui_phosphor::regular::{
    APP_WINDOW, ARROW_COUNTER_CLOCKWISE, CURSOR, DOT, GEAR, IMAGE, INFO, MAGNIFYING_GLASS, PALETTE,
    PENCIL_SIMPLE, PLUS, SELECTION, SLIDERS_HORIZONTAL, TERMINAL, TERMINAL_WINDOW, TEXT_T, TRASH,
    WARNING_CIRCLE, X,
};

/// Install Phosphor icon font definitions into `fonts`.
pub fn init_fonts(fonts: &mut FontDefinitions) {
    egui_phosphor::add_to_fonts(fonts, egui_phosphor::Variant::Regular);
}

/// Helper for rendering a clickable icon button using Phosphor glyphs.
pub fn icon_button(
    ui: &mut Ui,
    icon: &str,
    icon_size: f32,
    btn_size: Vec2,
    tooltip: impl Into<WidgetText>,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(btn_size, Sense::click());
    let is_hovered = response.hovered();
    let is_active = response.is_pointer_button_down_on();

    if is_active {
        ui.painter()
            .rect_filled(rect, 6.0, ui.visuals().widgets.active.bg_fill);
    } else if is_hovered {
        ui.painter()
            .rect_filled(rect, 6.0, ui.visuals().widgets.hovered.bg_fill);
    }

    let text_color = if is_active || is_hovered {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };

    let shape = ui.painter().layout_no_wrap(
        icon.to_string(),
        FontId::proportional(icon_size),
        text_color,
    );
    let pos = rect.center() - shape.size() * 0.5;
    ui.painter().galley(pos, shape, Color32::WHITE);

    let tip = tooltip.into();
    if !tip.is_empty() {
        response.on_hover_text(tip)
    } else {
        response
    }
}

/// Draw a Phosphor icon glyph centered inside a given rectangle.
pub fn draw_icon(painter: &egui::Painter, rect: Rect, icon: &str, icon_size: f32, color: Color32) {
    let shape = painter.layout_no_wrap(icon.to_string(), FontId::proportional(icon_size), color);
    let pos = rect.center() - shape.size() * 0.5;
    painter.galley(pos, shape, Color32::WHITE);
}
