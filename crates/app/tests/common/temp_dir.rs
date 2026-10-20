//! A temp directory that outlives the engine it belongs to. The core crate's helper of the same
//! name is not importable from app tests, and a real engine keeps SQLite open on a background
//! thread after the view that owns it is gone, so removal has to be retried rather than tried
//! once.
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("kite-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("test directory");
        Self(path)
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // The engine's runtime thread can still hold the database files open for a moment
        // after the last handle drops, and Windows refuses to remove an open file.
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::remove_dir_all(&self.0).is_err() && self.0.exists() {
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
