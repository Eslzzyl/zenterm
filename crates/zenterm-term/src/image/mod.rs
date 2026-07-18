//! Image cache and cell-placement logic.
//!
//! The cache stores decoded image data keyed by Kitty protocol
//! `image_id` / `image_number`.  Placement functions compute how
//! to distribute an image across the terminal grid.

pub mod kitty;
pub mod placement;
pub mod sixel;
pub use placement::assign_image_to_cells;
pub use placement::{PlacementParams, PlacementStyle};

use std::collections::HashMap;
use std::sync::Arc;

use zenterm_core::image::ImageData;

/// Cache of decoded images referenced by Kitty / Sixel / iTerm protocols.
///
/// Images are identified by a `u32` image-id (Kitty's `i=` parameter).
/// The optional number-to-id mapping (Kitty's `I=` parameter) allows
/// referring to images by a persistent number.
pub struct ImageCache {
    max_image_id: u32,
    number_to_id: HashMap<u32, u32>,
    id_to_data: HashMap<u32, Arc<ImageData>>,
    used_memory: usize,
    /// Maximum allowed memory for cached images.
    /// When exceeded, unreferenced images are pruned.
    max_memory: usize,
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            max_image_id: 0,
            number_to_id: HashMap::new(),
            id_to_data: HashMap::new(),
            used_memory: 0,
            max_memory: 320 * 1024 * 1024, // 320 MB
        }
    }

    /// Assign a new or reuse an existing image-id.
    /// Returns the resolved image-id.
    pub fn assign_id(&mut self, image_id: Option<u32>, image_number: Option<u32>) -> u32 {
        match (image_id, image_number) {
            (Some(id), _) => id,
            (None, Some(no)) => {
                if let Some(&id) = self.number_to_id.get(&no) {
                    id
                } else {
                    let id = self.max_image_id + 1;
                    self.max_image_id = id;
                    self.number_to_id.insert(no, id);
                    id
                }
            }
            (None, None) => 0,
        }
    }

    /// Store an image under the given id.
    pub fn insert(&mut self, image_id: u32, data: Arc<ImageData>) {
        if image_id != 0 {
            self.remove(image_id);
        }
        self.used_memory += data.len();
        self.id_to_data.insert(image_id, data);
        self.prune();
    }

    /// Look up an image by id.
    pub fn get(&self, image_id: u32) -> Option<&Arc<ImageData>> {
        self.id_to_data.get(&image_id)
    }

    /// Remove an image by id.
    /// Returns the content hash if the image existed, for atlas cleanup.
    pub fn remove(&mut self, image_id: u32) -> Option<[u8; 32]> {
        // Clean up number_to_id entries pointing to this id.
        self.number_to_id.retain(|_, v| *v != image_id);
        if let Some(data) = self.id_to_data.remove(&image_id) {
            self.used_memory = self.used_memory.saturating_sub(data.len());
            Some(data.hash)
        } else {
            None
        }
    }

    /// Return all content hashes currently in the cache.
    pub fn all_hashes(&self) -> Vec<[u8; 32]> {
        self.id_to_data.values().map(|d| d.hash).collect()
    }

    /// Return all image IDs currently in the cache.
    pub fn all_image_ids(&self) -> Vec<u32> {
        self.id_to_data.keys().copied().collect()
    }

    /// Remove all images and placements.
    pub fn clear(&mut self) {
        self.id_to_data.clear();
        self.number_to_id.clear();
        self.used_memory = 0;
    }

    /// Prune unreferenced images when memory budget is exceeded.
    fn prune(&mut self) {
        if self.used_memory <= self.max_memory {
            return;
        }
        let referenced: std::collections::HashSet<u32> =
            self.id_to_data.keys().copied().collect();
        let target = self.used_memory - self.max_memory;
        let mut freed = 0;
        self.id_to_data.retain(|id, data| {
            if referenced.contains(id) || freed >= target {
                true
            } else {
                freed += data.len();
                false
            }
        });
        self.used_memory = self.used_memory.saturating_sub(freed);
    }
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}
