//! Font metrics and cell geometry derived from the configured font.

use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping};

use zenterm_core::{Error, Result};

use crate::GlyphAtlas;

impl GlyphAtlas {
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// Font size in pixels.
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Returns the cell size (width, height) in pixels.
    ///
    /// Width is the advance of a representative monospace glyph ('W'),
    /// rounded **up** to an integer pixel boundary so column-to-column
    /// background quads tile perfectly with no sub-pixel gaps or overlap.
    /// Height is the font's line height rounded **up** to an integer pixel
    /// boundary, matching the strategy used by `cosmic-term`
    /// (`(font_size * 1.4).ceil()`) and the spirit of `alacritty`'s
    /// `(line_height + offset_y).floor()`.  Integer cell dimensions are
    /// critical for the cell-background rasterizer: fractional sizes cause
    /// adjacent cells' SOLID quads to overlap by a sub-pixel in the 1-px
    /// grid, which shows up as a 1-px "fringe" between rows/columns of
    /// coloured cells.
    ///
    /// Side-effect: this also measures the cell's baseline offset by shaping
    /// a full-height reference character ('M') and reading `max_ascent` from
    /// the cosmic-text layout.  Callers must use this value to position
    /// glyphs, *not* the raw `line_height`.
    ///
    /// This method will rasterize 'W' and 'M' if they aren't cached yet.
    /// Once computed, the values are cached for subsequent calls.
    pub fn cell_size(&mut self) -> Result<(f32, f32)> {
        if self.cell_width > 0.0 && self.cell_height > 0.0 {
            return Ok((self.cell_width, self.cell_height));
        }
        // ── Order matters ─────────────────────────────────────────────
        // Measure baseline FIRST so `cell_ascent` and `cell_descent` are
        // valid by the time we rasterise 'W' below.
        self.measure_baseline()?;
        // Update line_height to the font's actual ascent + descent.
        // The initial Metrics used font_size * 1.0 which was too tight —
        // the font's real body height (e.g. 41px for Menlo at 36px) exceeds
        // font_size, causing glyphs to overflow the cell.  We now use the
        // measured values so the cell is tall enough to contain all glyphs.
        self.metrics.line_height = self.cell_ascent + self.cell_descent;
        // Cap height is measured from a separate 'M' rasterisation, but it
        // doesn't depend on cell_ascent / cell_descent so it could in
        // principle be called in parallel.  We keep it sequential for
        // simplicity.
        self.measure_cap_height()?;
        let (entry, _is_new) = self.ensure_glyph('W')?;
        // Integer cell width/height: avoid sub-pixel drift between
        // adjacent columns (width) and rows (height).
        self.cell_width = entry.advance.ceil();
        self.cell_height = self.metrics.line_height.ceil();
        // Measure the font's underline thickness for box-drawing stroke
        // width (matching WezTerm's approach).
        self.measure_underline_thickness();
        // Authoritative baseline: ask cosmic-text where it would put the
        // baseline for a full-height glyph.  This is exactly the value
        // alacritty calls `ascent` and wezterm calls `ascender` — the
        // y-down distance from the cell top to the baseline.
        // (The actual `measure_baseline` call is now at the top of this
        // function so the 'W' rasterisation above sees valid metrics.)
        log::info!(
            "GlyphAtlas::cell_size: cw={:.2} ch={:.2} ascent={:.2} descent={:.2} \
             cap_height={:.2} (line_height={:.2} font_size={:.2})",
            self.cell_width,
            self.cell_height,
            self.cell_ascent,
            self.cell_descent,
            self.cap_height,
            self.metrics.line_height,
            self.font_size,
        );
        Ok((self.cell_width, self.cell_height))
    }

    /// Measure the cell's baseline offset (ascent) and descent via
    /// `cosmic-text`'s own layout pass.
    ///
    /// We use the string `"Mg"` (M for the full font ascent, g for the full
    /// font descent) so that `max_ascent` and `max_descent` both reflect the
    /// font's design metrics.  Using a single character like `'M'` would
    /// yield `max_descent = 0` (M has no descender).
    ///
    /// alacritty and wezterm take the same dual-character approach (or
    /// pull the metrics directly from the font's OS/2 table).
    fn measure_baseline(&mut self) -> Result<()> {
        // Use a temporary buffer.  `line_height` here only affects inter-line
        // spacing inside cosmic-text; per-glyph metrics like `max_ascent` are
        // independent of it (they come from the shaped glyph's own font size).
        // We use a generous line height so cosmic-text doesn't truncate.
        let mut buf = Buffer::new(
            &mut self.font_system,
            Metrics::new(self.font_size, self.font_size * 2.0),
        );
        let attrs = Attrs::new().family(Family::Name(&self.font_family));
        // "Mg" — M contributes the full font ascent, g contributes the full
        // font descent (e.g. a typical 14/4 em-square).  cosmic-text's
        // layout pass exposes the *line-wide* max, so a single buffer line
        // of "Mg" gives us both numbers.
        buf.set_text("Mg", &attrs, Shaping::Basic, None);
        buf.shape_until_scroll(&mut self.font_system, true);

        let line = buf.lines[0]
            .layout_opt()
            .and_then(|l| l.first())
            .ok_or_else(|| Error::Glyph("measure_baseline: empty layout".into()))?;

        let max_ascent = line.max_ascent;
        let max_descent = line.max_descent;
        if max_ascent <= 0.0 {
            return Err(Error::Glyph(format!(
                "measure_baseline: got non-positive max_ascent (font_size={}, family={:?})",
                self.font_size, self.font_family,
            )));
        }
        self.cell_ascent = max_ascent;
        self.cell_descent = max_descent;
        Ok(())
    }

    /// Measure the typographic cap height by rasterising a single capital
    /// letter (`'M'`) and reading `placement.top` from swash.
    ///
    /// `placement.top` is the y-up distance from the baseline to the top
    /// edge of the glyph bitmap.  For a capital letter with no ascender
    /// above the cap line, this is exactly the cap height — the height of
    /// capital letters, distinct from the font ascent (which includes
    /// extra room for diacritics).
    ///
    /// This is what Alacritty implicitly uses to size its block cursor:
    /// the cursor stops at `cap_height` above the baseline (no
    /// "above-cap buffer") and includes the full descent below.
    fn measure_cap_height(&mut self) -> Result<()> {
        // Shape a single 'M' in its own buffer.  We need a layout run so
        // cosmic-text hands us a physical glyph we can feed to swash.
        let mut buf = Buffer::new(&mut self.font_system, self.metrics);
        let attrs = Attrs::new().family(Family::Name(&self.font_family));
        buf.set_text("M", &attrs, Shaping::Basic, None);
        buf.shape_until_scroll(&mut self.font_system, true);

        let gl = match buf
            .lines
            .first()
            .and_then(|l| l.layout_opt())
            .and_then(|l| l.first())
            .and_then(|l| l.glyphs.first())
        {
            Some(g) => g,
            None => {
                // Fallback: use cell_ascent (slightly too tall but never
                // smaller than the cap height) so the cursor still works.
                self.cap_height = self.cell_ascent;
                return Ok(());
            }
        };

        let physical_glyph = gl.physical((0.0, 0.0), 1.0);
        match self.rasterize_swash(&physical_glyph.cache_key) {
            Some(img) => {
                // `placement.top` is the y-up distance from the baseline to
                // the topmost row of the bitmap.  For a capital letter
                // with no ascender above the cap line, that's the cap
                // height.  It's an integer in swash's scaled units, so
                // cast to f32 without losing precision at our sizes.
                self.cap_height = img.placement.top as f32;
                Ok(())
            }
            None => {
                self.cap_height = self.cell_ascent;
                Ok(())
            }
        }
    }

    /// Return the cached cap height in pixels.  Must call
    /// [`cell_size()`](Self::cell_size) first.
    pub fn cap_height(&self) -> f32 {
        self.cap_height
    }

    /// Return the cached cell dimensions (must call `cell_size()` first).
    pub fn cell_dimensions(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    /// Measure the font's design underline thickness in pixels.
    ///
    /// Queries the primary font matching the configured family name and
    /// extracts the underline thickness from the font's OS/2 or `post`
    /// table (via skrifa metrics).  This value is used as the stroke width
    /// for box-drawing characters so that rendered lines match the font's
    /// own stroke weight (matching WezTerm's approach).
    ///
    /// If the font cannot be found or the metric is unavailable, the field
    /// stays at its default (0.0), and `builtin.rs` falls back to the
    /// Alacritty heuristic (`cell_width / 8`).
    fn measure_underline_thickness(&mut self) {
        use cosmic_text::fontdb::{Family, Query, Stretch, Style, Weight};

        let families = [Family::Name(&self.font_family), Family::Monospace];
        let query = Query {
            families: &families,
            weight: Weight::NORMAL,
            stretch: Stretch::Normal,
            style: Style::Normal,
        };

        let Some(id) = self.font_system.db().query(&query) else {
            return;
        };
        let Some(font) = self.font_system.get_font(id, Weight::NORMAL) else {
            return;
        };

        let metrics = font.metrics();
        let Some(ref underline) = metrics.underline else {
            return;
        };

        // Scale from font design units to physical pixels.
        let ppem = self.font_size * self.pixels_per_point;
        self.underline_thickness_px = underline.thickness * ppem / metrics.units_per_em as f32;

        log::info!(
            "GlyphAtlas::underline_thickness: design={:.2} units/em={} ppem={:.2} => {:.2}px",
            underline.thickness,
            metrics.units_per_em,
            ppem,
            self.underline_thickness_px,
        );
    }

    /// Return the cell's baseline offset: the y-down distance from the cell
    /// TOP to the baseline, in pixels.
    ///
    /// This is what callers should use to position glyphs vertically.  The
    /// standard formula is
    ///
    /// ```text
    /// glyph_top_y = row * cell_height + cell_baseline_offset() - glyph_bearing_y
    /// ```
    ///
    /// which places the baseline at `row * cell_height + cell_baseline_offset()`
    /// and the glyph top at `baseline - bearing_y`, exactly as in alacritty
    /// and wezterm.
    pub fn cell_baseline_offset(&self) -> f32 {
        self.cell_ascent
    }

    /// Return the cell's descent: the y-down distance from the baseline to
    /// the cell BOTTOM, in pixels.  Useful for placing decorations
    /// (underline, strikethrough) just below the baseline.
    pub fn cell_descent(&self) -> f32 {
        self.cell_descent
    }
}
