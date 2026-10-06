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
use std::time::{Duration, Instant};

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

    pub fn assign_id(&mut self, image_id: Option<u32>, image_number: Option<u32>) -> u32 {
        match (image_id, image_number) {
            (Some(id), Some(number)) => {
                self.max_image_id = self.max_image_id.max(id);
                self.number_to_id.insert(number, id);
                id
            }
            (Some(id), None) => {
                self.max_image_id = self.max_image_id.max(id);
                id
            }
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
    /// Resolve an existing image reference without allocating a new id.
    pub fn resolve_id(&self, image_id: Option<u32>, image_number: Option<u32>) -> Option<u32> {
        match (image_id, image_number) {
            (Some(id), None) if id != 0 => self.id_to_data.contains_key(&id).then_some(id),
            (None, Some(number)) if number != 0 => self
                .number_to_id
                .get(&number)
                .copied()
                .filter(|id| self.id_to_data.contains_key(id)),
            _ => None,
        }
    }

    /// Advance every terminal-driven animation and return the nearest future
    /// wake-up interval.
    pub fn advance_animations(&mut self, now: Instant) -> (bool, Option<Duration>) {
        let mut changed = false;
        let mut next = None;
        for data in self.id_to_data.values() {
            changed |= data.advance_animation(now);
            if let Some(deadline) = data.animation_deadline() {
                let delay = if deadline <= now {
                    Duration::ZERO
                } else {
                    deadline.duration_since(now)
                };
                next = Some(next.map_or(delay, |current: Duration| current.min(delay)));
            }
        }
        (changed, next)
    }

    /// Store an image under the given id.
    pub fn insert(&mut self, image_id: u32, data: Arc<ImageData>) {
        // Keep image-number aliases intact when an image ID is retransmitted.
        if let Some(old) = self.id_to_data.remove(&image_id) {
            self.used_memory = self.used_memory.saturating_sub(old.len());
            self.evicted_hashes.extend(old.frame_hashes());
        }
        self.used_memory += data.len();
        self.id_to_data.insert(image_id, data);
        self.prune();
    }

    /// Look up an image by id.
    pub fn get(&self, image_id: u32) -> Option<&Arc<ImageData>> {
        self.id_to_data.get(&image_id)
    }

    /// Remove an image and return the hashes of all frame buffers it owned.
    pub fn remove_with_hashes(&mut self, image_id: u32) -> Option<Vec<[u8; 32]>> {
        self.number_to_id.retain(|_, v| *v != image_id);
        self.id_to_data.remove(&image_id).map(|data| {
            self.used_memory = self.used_memory.saturating_sub(data.len());
            data.frame_hashes()
        })
    }

    /// Remove an image by id, returning one aggregate hash for legacy callers.
    pub fn remove(&mut self, image_id: u32) -> Option<[u8; 32]> {
        self.remove_with_hashes(image_id)
            .and_then(|hashes| hashes.into_iter().next())
    }

    /// Return the hashes of all frame buffers currently in the cache.
    pub fn all_frame_hashes(&self) -> Vec<[u8; 32]> {
        self.id_to_data
            .values()
            .flat_map(|data| data.frame_hashes())
            .collect()
    }

    /// Return all aggregate content hashes currently in the cache.
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
        self.evicted_hashes.extend(self.all_frame_hashes());
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
                let hashes = data.frame_hashes();
                freed += data.len();
                if !self.id_to_data.values().any(|other| {
                    other
                        .frame_hashes()
                        .into_iter()
                        .any(|hash| hashes.contains(&hash))
                }) {
                    self.evicted_hashes.extend(hashes);
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

    #[test]
    fn image_number_alias_survives_retransmit() {
        let mut cache = ImageCache::new();
        let id = cache.assign_id(None, Some(9));
        cache.insert(id, image(8));
        cache.insert(id, image(16));

        assert_eq!(cache.resolve_id(None, Some(9)), Some(id));
    }
}
