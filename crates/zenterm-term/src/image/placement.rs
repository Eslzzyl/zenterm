//! Image placement — computing how an image is distributed across cells.
//!
//! This is the zenterm equivalent of wezterm's
//! `TerminalState::assign_image_to_cells()`.

use std::sync::Arc;

use zenterm_core::image::{ImageCell, ImageData, TextureCoordinate};

/// Parameters for placing an image on the terminal grid.
#[derive(Debug, Clone)]
pub struct PlacementParams {
    /// Desired number of columns to span (Kitty `c=`).
    /// `None` = compute from image width ÷ cell width.
    pub columns: Option<usize>,
    /// Desired number of rows to span (Kitty `r=`).
    /// `None` = compute from image height ÷ cell height.
    pub rows: Option<usize>,

    /// Source rectangle within the image (Kitty `x=`, `y=`, `w=`, `h=`).
    pub source_x: Option<u32>,
    pub source_y: Option<u32>,
    pub source_w: Option<u32>,
    pub source_h: Option<u32>,

    /// Pixel offset within the cell (Kitty `X=`, `Y=`).
    pub cell_padding_left: u16,
    pub cell_padding_top: u16,

    /// Compositing layer.
    pub z_index: i32,

    /// Whether to avoid moving the cursor after placement (Sixel DECSDM).
    pub do_not_move_cursor: bool,

    /// Kitty protocol identifiers.
    pub image_id: Option<u32>,
    pub placement_id: Option<u32>,

    /// Placement style — affects cursor movement semantics.
    pub style: PlacementStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementStyle {
    Sixel,
    Iterm,
    Kitty,
}

/// The result of placing an image on the grid.
#[derive(Debug, Clone)]
pub struct PlacementResult {
    /// Per-cell image assignments: `(col, row) → ImageCell`.
    pub cells: Vec<(usize, usize, ImageCell)>,
    /// How many cells the image spans.
    pub width_in_cells: usize,
    pub height_in_cells: usize,
    /// Whether cursor should be moved (Kitty/iTerm put cursor after bottom-right)
    pub move_cursor: bool,
}

/// Distribute an image across terminal cells.
///
/// `image_data` — shared image reference.
/// `image_width`, `image_height` — source image pixel dimensions.
/// `params` — placement parameters from the protocol.
/// `cell_pixel_w`, `cell_pixel_h` — size of one cell in pixels.
/// `cursor_col`, `cursor_row` — starting grid position.
/// `max_cols`, `max_rows` — grid bounds.
pub fn assign_image_to_cells(
    data: Arc<ImageData>,
    image_width: u32,
    image_height: u32,
    params: &PlacementParams,
    cell_pixel_w: u32,
    cell_pixel_h: u32,
    cursor_col: usize,
    cursor_row: usize,
    max_cols: usize,
    max_rows: usize,
) -> PlacementResult {
    let padding_left = params.cell_padding_left.min(cell_pixel_w.saturating_sub(1) as u16);
    let padding_top = params.cell_padding_top.min(cell_pixel_h.saturating_sub(1) as u16);

    let src_x = params.source_x.unwrap_or(0);
    let src_y = params.source_y.unwrap_or(0);
    let draw_w = params.source_w.unwrap_or(image_width.saturating_sub(src_x)).min(image_width.saturating_sub(src_x));
    let draw_h = params.source_h.unwrap_or(image_height.saturating_sub(src_y)).min(image_height.saturating_sub(src_y));

    // Compute cell span.
    let (full_cells_w, _rem_w) = params.columns.map(|c| (c, 0)).unwrap_or_else(|| {
        let fw = draw_w as usize / cell_pixel_w as usize;
        let rw = draw_w as usize % cell_pixel_w as usize;
        (fw, rw)
    });

    let (full_cells_h, _rem_h) = params.rows.map(|r| (r, 0)).unwrap_or_else(|| {
        let fh = draw_h as usize / cell_pixel_h as usize;
        let rh = draw_h as usize % cell_pixel_h as usize;
        (fh, rh)
    });

    // Ceiling division for partial cells.
    let width_in_cells = if draw_w == 0 { 1 } else {
        ((draw_w + cell_pixel_w - 1) / cell_pixel_w) as usize
    };
    let height_in_cells = if draw_h == 0 { 1 } else {
        ((draw_h + cell_pixel_h - 1) / cell_pixel_h) as usize
    };

    let width_in_cells = params.columns.unwrap_or(width_in_cells);
    let height_in_cells = params.rows.unwrap_or(height_in_cells);

    // Clamp to grid bounds.
    let height_in_cells = if params.do_not_move_cursor {
        height_in_cells.min(max_rows.saturating_sub(cursor_row))
    } else {
        height_in_cells
    };

    let target_pixel_w = full_cells_w * cell_pixel_w as usize + _rem_w;
    let target_pixel_h = full_cells_h * cell_pixel_h as usize + _rem_h;

    // Normalised source origin.
    let start_xpos = src_x as f32 / image_width as f32;
    let start_ypos = src_y as f32 / image_height as f32;

    let x_delta_divisor = params.columns.map(|cols| {
        (cols * cell_pixel_w as usize) as u32 * image_width / draw_w
    }).unwrap_or(image_width);
    let y_delta_divisor = params.rows.map(|rows| {
        (rows * cell_pixel_h as usize) as u32 * image_height / draw_h
    }).unwrap_or(image_height);

    let mut cells = Vec::with_capacity(width_in_cells * height_in_cells);
    let mut remain_y = if params.rows.is_some() { draw_h } else { target_pixel_h as u32 };

    for row_offset in 0..height_in_cells {
        let padding_bottom = cell_pixel_h.saturating_sub(remain_y) as u16;
        let y_delta = (remain_y.min(cell_pixel_h) as f32) / y_delta_divisor as f32;
        remain_y = remain_y.saturating_sub(cell_pixel_h);

        let mut xpos = start_xpos;
        let mut remain_x = if params.columns.is_some() { draw_w } else { target_pixel_w as u32 };
        let grid_y = cursor_row + row_offset;

        if grid_y >= max_rows { break; }

        for col_offset in 0..width_in_cells {
            let padding_right = cell_pixel_w.saturating_sub(remain_x) as u16;
            let x_delta = (remain_x.min(cell_pixel_w) as f32) / x_delta_divisor as f32;
            remain_x = remain_x.saturating_sub(cell_pixel_w);

            let grid_x = cursor_col + col_offset;
            if grid_x >= max_cols { break; }

            let top_left = TextureCoordinate::new(xpos, start_ypos + row_offset as f32 * y_delta);
            let bottom_right = TextureCoordinate::new(
                xpos + x_delta,
                start_ypos + (row_offset + 1) as f32 * y_delta,
            );

            let cell = ImageCell {
                top_left,
                bottom_right,
                data: data.clone(),
                z_index: params.z_index,
                padding_left,
                padding_top,
                padding_right,
                padding_bottom,
                image_id: params.image_id,
                placement_id: params.placement_id,
            };

            cells.push((grid_x, grid_y, cell));
            xpos += x_delta;
        }
    }

    let move_cursor = !params.do_not_move_cursor;

    PlacementResult {
        cells,
        width_in_cells,
        height_in_cells,
        move_cursor,
    }
}
