//! Manual, deterministic image-pipeline measurements.
//!
//! Run with:
//! `cargo test -p zenterm-term --test image_pipeline_baseline -- --ignored --nocapture`
//!
//! The test intentionally reports allocation deltas instead of asserting a
//! machine-dependent threshold.  The same file can be run at a historical
//! revision to compare the implementation before and after an optimization.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use image::GenericImageView;
use zenterm_core::image::{ImageData, ImageDataType};
use zenterm_term::image::ImageCache;
use zenterm_term::image::kitty::{
    KittyAccumulator, KittyFrameCompositionMode, KittyImage, KittyImageCompression, KittyImageData,
    KittyImageFormat, KittyImageFrame, KittyImageTransmit, KittyImageVerbosity, decode_image_data,
    decode_image_frame, decode_image_to_rgba,
};

struct CountingAllocator;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static DEALLOCATED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK_LIVE: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        record_dealloc(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size >= layout.size() {
                record_alloc(new_size - layout.size());
            } else {
                record_dealloc(layout.size() - new_size);
            }
        }
        new_ptr
    }
}

fn record_alloc(bytes: usize) {
    ALLOCATED.fetch_add(bytes, Ordering::Relaxed);
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    let mut peak = PEAK_LIVE.load(Ordering::Relaxed);
    while live > peak {
        match PEAK_LIVE.compare_exchange_weak(peak, live, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => peak = observed,
        }
    }
}

fn record_dealloc(bytes: usize) {
    DEALLOCATED.fetch_add(bytes, Ordering::Relaxed);
    LIVE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        Some(live.saturating_sub(bytes))
    })
    .ok();
}

#[derive(Debug, Clone, Copy)]
struct Snapshot {
    elapsed_us: u128,
    allocated: usize,
    deallocated: usize,
    peak_live: usize,
}

fn reset_counters() {
    ALLOCATED.store(0, Ordering::Relaxed);
    DEALLOCATED.store(0, Ordering::Relaxed);
    LIVE.store(0, Ordering::Relaxed);
    PEAK_LIVE.store(0, Ordering::Relaxed);
}

fn snapshot(start: Instant) -> Snapshot {
    Snapshot {
        elapsed_us: start.elapsed().as_micros(),
        allocated: ALLOCATED.load(Ordering::Relaxed),
        deallocated: DEALLOCATED.load(Ordering::Relaxed),
        peak_live: PEAK_LIVE.load(Ordering::Relaxed),
    }
}

fn bytes(len: usize, seed: u8) -> Vec<u8> {
    let mut data = vec![0; len];
    for (index, byte) in data.iter_mut().enumerate() {
        *byte = seed
            .wrapping_add(index as u8)
            .rotate_left((index % 8) as u32);
    }
    data
}

fn rgb_decode_snapshot() -> Snapshot {
    const WIDTH: u32 = 2048;
    const HEIGHT: u32 = 2048;
    let raw = bytes(WIDTH as usize * HEIGHT as usize * 3, 17);
    let transmit = KittyImageTransmit {
        format: Some(KittyImageFormat::Rgb),
        data: KittyImageData::DirectBin(raw),
        width: Some(WIDTH),
        height: Some(HEIGHT),
        image_id: Some(1),
        image_number: None,
        compression: KittyImageCompression::None,
        more_data_follows: false,
    };
    let mut cache = ImageCache::new();

    reset_counters();
    let start = Instant::now();
    let image_id = decode_image_data(transmit, &mut cache).expect("RGB decode");
    let result = snapshot(start);

    let data = cache.get(image_id).expect("decoded image");
    assert_eq!(data.len(), WIDTH as usize * HEIGHT as usize * 4);
    result
}

fn frame_edit_snapshot() -> Snapshot {
    const WIDTH: u32 = 2048;
    const HEIGHT: u32 = 2048;
    const FRAME_WIDTH: u32 = 512;
    const FRAME_HEIGHT: u32 = 512;

    let mut cache = ImageCache::new();
    cache.insert(
        1,
        Arc::new(ImageData::new(ImageDataType::new_rgba8(
            bytes(WIDTH as usize * HEIGHT as usize * 4, 23),
            WIDTH,
            HEIGHT,
        ))),
    );
    let transmit = KittyImageTransmit {
        format: Some(KittyImageFormat::Rgba),
        data: KittyImageData::DirectBin(bytes(
            FRAME_WIDTH as usize * FRAME_HEIGHT as usize * 4,
            71,
        )),
        width: Some(FRAME_WIDTH),
        height: Some(FRAME_HEIGHT),
        image_id: Some(1),
        image_number: None,
        compression: KittyImageCompression::None,
        more_data_follows: false,
    };
    let frame = KittyImageFrame {
        x: Some(256),
        y: Some(256),
        duration_ms: None,
        frame_number: Some(1),
        base_frame: None,
        composition_mode: KittyFrameCompositionMode::Overwrite,
        background_pixel: None,
    };

    reset_counters();
    let start = Instant::now();
    decode_image_frame(transmit, frame, &mut cache).expect("frame edit");
    snapshot(start)
}

fn real_png_decode_snapshot(path: &Path) -> Snapshot {
    let encoded = fs::read(path).expect("read PNG fixture");
    let transmit = KittyImageTransmit {
        format: Some(KittyImageFormat::Png),
        data: KittyImageData::DirectBin(encoded),
        width: None,
        height: None,
        image_id: Some(2),
        image_number: None,
        compression: KittyImageCompression::None,
        more_data_follows: false,
    };
    let mut cache = ImageCache::new();

    reset_counters();
    let start = Instant::now();
    let image_id = decode_image_data(transmit, &mut cache).expect("PNG decode");
    let result = snapshot(start);

    let data = cache.get(image_id).expect("decoded PNG image");
    assert!(!data.is_empty());
    result
}

fn real_jpeg_decode_snapshot(path: &Path, optimized: bool) -> Snapshot {
    let encoded = fs::read(path).expect("read JPEG fixture");

    reset_counters();
    let start = Instant::now();
    let (rgba, width, height) = if optimized {
        decode_image_to_rgba(&encoded).expect("optimized JPEG decode")
    } else {
        let decoded = image::load_from_memory(&encoded).expect("baseline JPEG decode");
        let (width, height) = decoded.dimensions();
        (decoded.into_rgba8().into_vec(), width, height)
    };
    assert_eq!(rgba.len(), width as usize * height as usize * 4);
    snapshot(start)
}

fn real_png_chunked_decode_snapshot(path: &Path) -> Snapshot {
    let encoded = fs::read(path).expect("read chunked PNG fixture");
    let split = encoded.len() / 2;
    let first = encoded[..split].to_vec();
    let second = encoded[split..].to_vec();
    let transmit = |data, more_data_follows| KittyImage::TransmitData {
        transmit: KittyImageTransmit {
            format: Some(KittyImageFormat::Png),
            data: KittyImageData::DirectBin(data),
            width: None,
            height: None,
            image_id: Some(3),
            image_number: None,
            compression: KittyImageCompression::None,
            more_data_follows,
        },
        verbosity: KittyImageVerbosity::Quiet,
    };
    let mut accumulator = KittyAccumulator::default();
    let mut cache = ImageCache::new();

    reset_counters();
    let start = Instant::now();
    assert!(accumulator.feed(transmit(first, true)).unwrap().is_none());
    let assembled = accumulator
        .feed(transmit(second, false))
        .unwrap()
        .expect("assembled PNG");
    let KittyImage::TransmitData { transmit, .. } = assembled else {
        panic!("expected transmit data");
    };
    let image_id = decode_image_data(transmit, &mut cache).expect("chunked PNG decode");
    assert!(!cache.get(image_id).expect("decoded chunked PNG").is_empty());
    snapshot(start)
}

#[test]
#[ignore = "manual performance measurement"]
fn report_image_pipeline_baseline() {
    let rgb = rgb_decode_snapshot();
    let frame = frame_edit_snapshot();
    let png_path = std::env::var_os("ZENTERM_IMAGE_FIXTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets/runtime/zenterm.png"));
    let png = real_png_decode_snapshot(&png_path);
    let png_chunked = real_png_chunked_decode_snapshot(&png_path);
    let jpeg_path = std::env::var_os("ZENTERM_JPEG_FIXTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets/runtime/zenterm.png"));
    let jpeg_baseline = real_jpeg_decode_snapshot(&jpeg_path, false);
    let jpeg_optimized = real_jpeg_decode_snapshot(&jpeg_path, true);
    println!(
        "image_pipeline_baseline rgb_decode elapsed_us={} allocated={} deallocated={} peak_live={}",
        rgb.elapsed_us, rgb.allocated, rgb.deallocated, rgb.peak_live
    );
    println!(
        "image_pipeline_baseline frame_edit elapsed_us={} allocated={} deallocated={} peak_live={}",
        frame.elapsed_us, frame.allocated, frame.deallocated, frame.peak_live
    );
    println!(
        "image_pipeline_baseline real_png_decode path={} elapsed_us={} allocated={} deallocated={} peak_live={}",
        png_path.display(),
        png.elapsed_us,
        png.allocated,
        png.deallocated,
        png.peak_live
    );
    println!(
        "image_pipeline_baseline real_png_chunked_decode path={} elapsed_us={} allocated={} deallocated={} peak_live={}",
        png_path.display(),
        png_chunked.elapsed_us,
        png_chunked.allocated,
        png_chunked.deallocated,
        png_chunked.peak_live
    );
    println!(
        "image_pipeline_baseline real_jpeg_decode_baseline path={} elapsed_us={} allocated={} deallocated={} peak_live={}",
        jpeg_path.display(),
        jpeg_baseline.elapsed_us,
        jpeg_baseline.allocated,
        jpeg_baseline.deallocated,
        jpeg_baseline.peak_live
    );
    println!(
        "image_pipeline_baseline real_jpeg_decode_optimized path={} elapsed_us={} allocated={} deallocated={} peak_live={}",
        jpeg_path.display(),
        jpeg_optimized.elapsed_us,
        jpeg_optimized.allocated,
        jpeg_optimized.deallocated,
        jpeg_optimized.peak_live
    );
}
