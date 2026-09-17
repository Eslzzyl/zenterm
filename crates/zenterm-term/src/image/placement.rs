//! Image placement — computing how an image is distributed across cells.
//!
//! This is the zenterm equivalent of wezterm's
//! `TerminalState::assign_image_to_cells()`.

use std::sync::Arc;

use zenterm_core::image::{ImageCell, ImageData, TextureCoordinate};

const MAX_PLACEMENT_CELLS: usize = 1_000_000;

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

/// Inputs required to distribute one image across the terminal grid.
pub struct PlacementRequest<'a> {
    pub data: Arc<ImageData>,
    pub image_width: u32,
    pub image_height: u32,
    pub params: &'a PlacementParams,
    pub cell_pixel_w: u32,
    pub cell_pixel_h: u32,
    pub cursor_col: usize,
    pub cursor_row: usize,
    pub max_cols: usize,
    pub max_rows: usize,
}

/// Distribute an image across terminal cells.
///
/// `image_data` — shared image reference.
/// `image_width`, `image_height` — source image pixel dimensions.
/// `params` — placement parameters from the protocol.
/// `cell_pixel_w`, `cell_pixel_h` — size of one cell in pixels.
/// `cursor_col`, `cursor_row` — starting grid position.
/// `max_cols`, `max_rows` — grid bounds.
pub fn assign_image_to_cells(request: PlacementRequest<'_>) -> PlacementResult {
    let PlacementRequest {
        data,
        image_width,
        image_height,
        params,
        cell_pixel_w,
        cell_pixel_h,
        cursor_col,
        cursor_row,
        max_cols,
        max_rows,
    } = request;

    // Protocol input can describe an empty image or an unavailable grid
    // while the terminal is being initialised. Treat those as a no-op rather
    // than allowing the span calculations below to divide by zero.
    if image_width == 0
        || image_height == 0
        || cell_pixel_w == 0
        || cell_pixel_h == 0
        || max_cols == 0
        || max_rows == 0
        || params.columns == Some(0)
        || params.rows == Some(0)
        || cursor_col >= max_cols
        || cursor_row >= max_rows
    {
        return PlacementResult {
            cells: Vec::new(),
            width_in_cells: 0,
            height_in_cells: 0,
            move_cursor: false,
        };
    }

    let padding_left = params
        .cell_padding_left
        .min(cell_pixel_w.saturating_sub(1) as u16);
    let padding_top = params
        .cell_padding_top
        .min(cell_pixel_h.saturating_sub(1) as u16);

    let src_x = params.source_x.unwrap_or(0);
    let src_y = params.source_y.unwrap_or(0);
    let draw_w = params
        .source_w
        .unwrap_or(image_width.saturating_sub(src_x))
        .min(image_width.saturating_sub(src_x));
    let draw_h = params
        .source_h
        .unwrap_or(image_height.saturating_sub(src_y))
        .min(image_height.saturating_sub(src_y));

    if draw_w == 0 || draw_h == 0 {
        return PlacementResult {
            cells: Vec::new(),
            width_in_cells: 0,
            height_in_cells: 0,
            move_cursor: false,
        };
    }

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
    let width_in_cells = if draw_w == 0 {
        1
    } else {
        draw_w.div_ceil(cell_pixel_w) as usize
    };
    let height_in_cells = if draw_h == 0 {
        1
    } else {
        draw_h.div_ceil(cell_pixel_h) as usize
    };

    let width_in_cells = params.columns.unwrap_or(width_in_cells);
    let height_in_cells = params.rows.unwrap_or(height_in_cells);

    // Clamp to grid bounds.
    let height_in_cells = if params.do_not_move_cursor {
        height_in_cells.min(max_rows.saturating_sub(cursor_row))
    } else {
        height_in_cells
    };

    let target_pixel_w = (full_cells_w as u64)
        .checked_mul(cell_pixel_w as u64)
        .and_then(|pixels| pixels.checked_add(_rem_w as u64))
        .and_then(|pixels| usize::try_from(pixels).ok())
        .unwrap_or(usize::MAX);
    let target_pixel_h = (full_cells_h as u64)
        .checked_mul(cell_pixel_h as u64)
        .and_then(|pixels| pixels.checked_add(_rem_h as u64))
        .and_then(|pixels| usize::try_from(pixels).ok())
        .unwrap_or(usize::MAX);

    // Normalised source origin.
    let start_xpos = src_x as f32 / image_width as f32;
    let start_ypos = src_y as f32 / image_height as f32;

    let x_delta_divisor = params
        .columns
        .map(|columns| {
            (columns as f64 * cell_pixel_w as f64 * image_width as f64 / draw_w as f64).max(1.0)
                as f32
        })
        .unwrap_or(image_width as f32);
    let y_delta_divisor = params
        .rows
        .map(|rows| {
            (rows as f64 * cell_pixel_h as f64 * image_height as f64 / draw_h as f64).max(1.0)
                as f32
        })
        .unwrap_or(image_height as f32);

    let visible_width = width_in_cells.min(max_cols.saturating_sub(cursor_col));
    let visible_height = height_in_cells
        .min(max_rows.saturating_sub(cursor_row))
        .min(MAX_PLACEMENT_CELLS / visible_width.max(1));
    let capacity = visible_width.saturating_mul(visible_height);
    let mut cells = Vec::with_capacity(capacity);
    let mut remain_y = if params.rows.is_some() {
        draw_h
    } else {
        target_pixel_h as u32
    };

    for row_offset in 0..visible_height {
        let padding_bottom = cell_pixel_h.saturating_sub(remain_y) as u16;
        let y_delta = (remain_y.min(cell_pixel_h) as f32) / y_delta_divisor;
        remain_y = remain_y.saturating_sub(cell_pixel_h);

        let mut xpos = start_xpos;
        let mut remain_x = if params.columns.is_some() {
            draw_w
        } else {
            target_pixel_w as u32
        };
        let grid_y = cursor_row + row_offset;

        if grid_y >= max_rows {
            break;
        }

        for col_offset in 0..visible_width {
            let padding_right = cell_pixel_w.saturating_sub(remain_x) as u16;
            let x_delta = (remain_x.min(cell_pixel_w) as f32) / x_delta_divisor;
            remain_x = remain_x.saturating_sub(cell_pixel_w);

            let grid_x = cursor_col + col_offset;
            if grid_x >= max_cols {
                break;
            }

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

#[cfg(test)]
mod tests {
    use zenterm_core::image::ImageDataType;

    use super::*;

    fn request<'a>(
        data: Arc<ImageData>,
        params: &'a PlacementParams,
        image_size: (u32, u32),
        cell_size: (u32, u32),
        cursor: (usize, usize),
        grid_size: (usize, usize),
    ) -> PlacementRequest<'a> {
        PlacementRequest {
            data,
            image_width: image_size.0,
            image_height: image_size.1,
            params,
            cell_pixel_w: cell_size.0,
            cell_pixel_h: cell_size.1,
            cursor_col: cursor.0,
            cursor_row: cursor.1,
            max_cols: grid_size.0,
            max_rows: grid_size.1,
        }
    }

    fn params() -> PlacementParams {
        PlacementParams {
            columns: None,
            rows: None,
            source_x: None,
            source_y: None,
            source_w: None,
            source_h: None,
            cell_padding_left: 0,
            cell_padding_top: 0,
            z_index: 0,
            do_not_move_cursor: false,
            image_id: None,
            placement_id: None,
            style: PlacementStyle::Kitty,
        }
    }

    fn image() -> Arc<ImageData> {
        Arc::new(ImageData::new(ImageDataType::new_rgba8(
            vec![0; 10 * 10 * 4],
            10,
            10,
        )))
    }

    #[test]
    fn empty_dimensions_are_a_noop() {
        let mut params = params();
        params.source_w = Some(0);

        let result = assign_image_to_cells(request(
            image(),
            &params,
            (10, 10),
            (5, 5),
            (0, 0),
            (10, 10),
        ));

        assert!(result.cells.is_empty());
        assert_eq!((result.width_in_cells, result.height_in_cells), (0, 0));
        assert!(!result.move_cursor);
    }

    #[test]
    fn unavailable_cell_or_grid_is_a_noop() {
        let params = params();
        for (cell_pixel_w, cell_pixel_h, max_cols, max_rows) in
            [(0, 5, 10, 10), (5, 0, 10, 10), (5, 5, 0, 10), (5, 5, 10, 0)]
        {
            let result = assign_image_to_cells(request(
                image(),
                &params,
                (10, 10),
                (cell_pixel_w, cell_pixel_h),
                (0, 0),
                (max_cols, max_rows),
            ));
            assert!(result.cells.is_empty());
            assert!(!result.move_cursor);
        }
    }

    #[test]
    fn placement_is_clipped_to_grid() {
        let params = params();
        let result =
            assign_image_to_cells(request(image(), &params, (10, 10), (4, 4), (1, 1), (3, 3)));

        assert_eq!((result.width_in_cells, result.height_in_cells), (3, 3));
        assert_eq!(result.cells.len(), 4);
        assert!(
            result
                .cells
                .iter()
                .all(|(col, row, _)| *col < 3 && *row < 3)
        );
        assert!(result.move_cursor);
    }

    #[test]
    fn huge_requested_span_has_bounded_cell_allocation() {
        let mut params = params();
        params.columns = Some(usize::MAX);
        params.rows = Some(usize::MAX);
        let result = assign_image_to_cells(request(
            image(),
            &params,
            (10, 10),
            (1, 1),
            (0, 0),
            (10, 10),
        ));

        assert_eq!(result.cells.len(), 100);
    }
}
