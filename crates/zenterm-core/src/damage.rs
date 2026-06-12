//! Damage tracking for incremental GPU updates.
//!
//! Instead of re-uploading the entire grid every frame, we track which rows
//! have changed and only upload those instances to the GPU.

use std::ops::Range;

/// A set of dirty rows that need re-uploading.
#[derive(Debug, Clone)]
pub struct DamageSet {
    /// Bitmap of dirty row indices.
    dirty: Vec<bool>,
    /// Number of tracked rows (== terminal height).
    capacity: usize,
}

impl DamageSet {
    /// Create a new damage set for the given number of rows.
    pub fn new(row_count: usize) -> Self {
        Self {
            dirty: vec![false; row_count],
            capacity: row_count,
        }
    }

    /// Mark a single row as dirty.
    pub fn mark(&mut self, row: usize) {
        if row < self.capacity {
            self.dirty[row] = true;
        }
    }

    /// Mark a range of rows as dirty.
    pub fn mark_range(&mut self, range: Range<usize>) {
        for row in range {
            self.mark(row);
        }
    }

    /// Mark all rows as dirty.
    pub fn mark_all(&mut self) {
        self.dirty.fill(true);
    }

    /// Check whether a row is dirty.
    pub fn is_dirty(&self, row: usize) -> bool {
        row < self.capacity && self.dirty[row]
    }

    /// Iterate over all dirty row indices.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.dirty
            .iter()
            .enumerate()
            .filter(|&(_, &dirty)| dirty)
            .map(|(i, _)| i)
    }

    /// Return `true` if no rows are dirty.
    pub fn is_empty(&self) -> bool {
        !self.dirty.iter().any(|&d| d)
    }

    /// Clear all dirty marks.
    pub fn clear(&mut self) {
        self.dirty.fill(false);
    }

    /// Resize the damage tracker to a new row count.
    /// If the new count is larger, new rows are clean.
    /// If smaller, old rows beyond the new count are discarded.
    pub fn resize(&mut self, new_row_count: usize) {
        self.dirty.resize(new_row_count, false);
        self.capacity = new_row_count;
    }
}

impl Default for DamageSet {
    fn default() -> Self {
        Self::new(24)
    }
}
