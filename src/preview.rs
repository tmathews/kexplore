//! Preview decoding on a persistent worker thread with generation-counter
//! cancellation: selecting a file bumps the generation; the worker coalesces
//! queued jobs to the latest and drops results whose generation is stale, so
//! rapid clicking can never show the wrong image (the C version's
//! fire-and-forget race).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use crate::tasks::{TaskResult, Tasks};

/// Don't inherit the C app's decompression-bomb click: cap input size.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PIXELS: u64 = 64_000_000;

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8, premultiplied alpha.
    pub rgba: Vec<u8>,
}

struct Job {
    gen: u64,
    path: PathBuf,
}

pub struct Preview {
    pub gen: Arc<AtomicU64>,
    job_tx: mpsc::Sender<Job>,
}

pub fn previewable(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "jfif" | "webp"))
}

impl Preview {
    pub fn new(tasks: &Tasks) -> Preview {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let gen = Arc::new(AtomicU64::new(0));
        let worker_gen = gen.clone();
        let (result_tx, wake_fd) = tasks.result_sender();
        std::thread::spawn(move || {
            while let Ok(mut job) = job_rx.recv() {
                // Coalesce to the newest queued job.
                while let Ok(newer) = job_rx.try_recv() {
                    job = newer;
                }
                if job.gen != worker_gen.load(Ordering::Acquire) {
                    continue; // stale before we even started
                }
                let image = decode(&job.path);
                if job.gen != worker_gen.load(Ordering::Acquire) {
                    continue; // superseded while decoding
                }
                if result_tx.send(TaskResult::PreviewDone { gen: job.gen, image }).is_ok() {
                    Tasks::wake_fd(wake_fd);
                }
            }
        });
        Preview { gen, job_tx }
    }

    /// Request a preview for `path`; any in-flight decode becomes stale.
    pub fn request(&self, path: PathBuf) {
        let gen = self.gen.fetch_add(1, Ordering::AcqRel) + 1;
        self.job_tx.send(Job { gen, path }).ok();
    }

    /// Invalidate without requesting (selection moved to a non-previewable
    /// file or a directory).
    pub fn cancel(&self) {
        self.gen.fetch_add(1, Ordering::AcqRel);
    }

    pub fn current_gen(&self) -> u64 {
        self.gen.load(Ordering::Acquire)
    }
}

fn decode(path: &Path) -> Option<DecodedImage> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let reader = image::ImageReader::open(path).ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    if w as u64 * h as u64 > MAX_PIXELS {
        return None;
    }
    let img = image::open(path).ok()?;
    // The preview draws at 400 logical px wide; keep the texture (and the
    // staging traffic) bounded for huge photos.
    let img = if img.width() > 2048 || img.height() > 2048 {
        img.thumbnail(2048, 2048)
    } else {
        img
    };
    let mut rgba = img.into_rgba8();
    for px in rgba.pixels_mut() {
        let a = px.0[3] as u16;
        px.0[0] = ((px.0[0] as u16 * a) / 255) as u8;
        px.0[1] = ((px.0[1] as u16 * a) / 255) as u8;
        px.0[2] = ((px.0[2] as u16 * a) / 255) as u8;
    }
    let (width, height) = rgba.dimensions();
    Some(DecodedImage { width, height, rgba: rgba.into_raw() })
}
