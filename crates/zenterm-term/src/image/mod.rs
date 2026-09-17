//! Image cache and cell-placement logic.
//!
//! The cache stores decoded image data keyed by Kitty protocol
//! `image_id` / `image_number`.  Placement functions compute how
//! to distribute an image across the terminal grid.

pub mod kitty;
pub mod placement;
pub mod sixel;
pub use placement::assign_image_to_cells;
pub use placement::{PlacementParams, PlacementRequest, PlacementStyle};

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
    /// Content hashes whose CPU cache entries were evicted and whose GPU
    /// atlas allocations can be released by the renderer.
    evicted_hashes: Vec<[u8; 32]>,
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
            evicted_hashes: Vec::new(),
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
        if self.id_to_data.contains_key(&image_id) {
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
            Some(data.hash())
        } else {
            None
        }
    }

    /// Return all content hashes currently in the cache.
    pub fn all_hashes(&self) -> Vec<[u8; 32]> {
        self.id_to_data.values().map(|d| d.hash()).collect()
    }

    /// Return all image IDs currently in the cache.
    pub fn all_image_ids(&self) -> Vec<u32> {
        self.id_to_data.keys().copied().collect()
    }

    /// Take hashes evicted by the memory budget since the last render pass.
    pub fn take_evicted_hashes(&mut self) -> Vec<[u8; 32]> {
        std::mem::take(&mut self.evicted_hashes)
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
        let target = self.used_memory - self.max_memory;
        let mut freed = 0;

        // `ImageCell` keeps an Arc to every image that is currently placed
        // on the terminal grid.  Use that ownership as the reference signal
        // instead of treating every cached image as referenced.  Images that
        // are still displayed therefore remain available, while stale cache
        // entries can be reclaimed when the budget is exceeded.
        let candidates: Vec<u32> = self
            .id_to_data
            .iter()
            .filter_map(|(&id, data)| (Arc::strong_count(data) == 1).then_some(id))
            .collect();

        for id in candidates {
            if freed >= target {
                break;
            }
            if let Some(data) = self.id_to_data.remove(&id) {
                self.number_to_id.retain(|_, mapped_id| *mapped_id != id);
                let hash = data.hash();
                freed += data.len();
                if !self.id_to_data.values().any(|other| other.hash() == hash) {
                    self.evicted_hashes.push(hash);
                }
            }
        }

        self.used_memory = self.used_memory.saturating_sub(freed);
    }
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenterm_core::image::ImageDataType;

    fn image(size: usize) -> Arc<ImageData> {
        Arc::new(ImageData::new(ImageDataType::new_rgba8(
            vec![0; size],
            size as u32,
            1,
        )))
    }

    #[test]
    fn prune_removes_unreferenced_images() {
        let mut cache = ImageCache::new();
        cache.max_memory = 8;

        cache.insert(1, image(8));
        cache.insert(2, image(8));

        assert_eq!(cache.used_memory, 8);
        assert_eq!(cache.id_to_data.len(), 1);
    }

    #[test]
    fn prune_keeps_images_referenced_by_image_cells() {
        let mut cache = ImageCache::new();
        cache.max_memory = 8;

        cache.insert(1, image(8));
        let referenced = Arc::clone(cache.get(1).expect("inserted image"));
        cache.insert(2, image(8));

        assert!(cache.get(1).is_some());
        assert!(cache.used_memory <= 16);

        drop(referenced);
        cache.insert(3, image(8));
        assert!(cache.used_memory <= 8);
        assert!(cache.id_to_data.len() <= 1);
    }

    #[test]
    fn replacing_anonymous_image_keeps_memory_accounting_consistent() {
        let mut cache = ImageCache::new();

        cache.insert(0, image(8));
        cache.insert(0, image(16));

        assert_eq!(cache.used_memory, 16);
        assert_eq!(cache.get(0).expect("anonymous image").len(), 16);
    }
}
