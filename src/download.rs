//! Parallel, verifying downloader.
//!
//! Files that already exist with the expected size (and, if requested, SHA-1)
//! are skipped, so a warm start touches nothing but file metadata and is fast.

use crate::paths::{ensure_parent, write_atomic};
use crate::{report, Progress, ProgressSink};
use anyhow::{anyhow, Context, Result};
use sha1::{Digest, Sha1};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct Task {
    pub url: String,
    pub dest: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    /// Set the executable bit after download (Unix).
    pub executable: bool,
}

impl Task {
    pub fn new(url: impl Into<String>, dest: impl Into<PathBuf>) -> Self {
        Self {
            url: url.into(),
            dest: dest.into(),
            sha1: None,
            size: None,
            executable: false,
        }
    }
    pub fn sha1(mut self, s: Option<String>) -> Self {
        self.sha1 = s.filter(|s| !s.is_empty());
        self
    }
    pub fn size(mut self, s: Option<u64>) -> Self {
        self.size = s.filter(|s| *s > 0);
        self
    }
}

pub fn sha1_hex(data: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(data);
    hex(&h.finalize())
}

pub fn sha1_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha1::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Is the file on disk already what we want?
///
/// With `verify_hash == false` only the size is compared, which is what makes
/// a warm launch nearly free. The size check still catches truncated files.
pub fn is_satisfied(task: &Task, verify_hash: bool) -> bool {
    let Ok(meta) = std::fs::metadata(&task.dest) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    if let Some(size) = task.size {
        if meta.len() != size {
            return false;
        }
    }
    if verify_hash {
        if let Some(sha) = &task.sha1 {
            return sha1_file(&task.dest)
                .map(|h| h.eq_ignore_ascii_case(sha))
                .unwrap_or(false);
        }
    } else if task.size.is_none() && task.sha1.is_some() {
        // No size to compare: fall back to hashing, otherwise trust existence.
        return sha1_file(&task.dest)
            .map(|h| h.eq_ignore_ascii_case(task.sha1.as_ref().unwrap()))
            .unwrap_or(false);
    }
    true
}

/// Download a single file with up to 3 attempts and verification.
pub fn fetch_one(task: &Task) -> Result<()> {
    let mut last_err = None;
    for attempt in 1..=3 {
        match fetch_once(task) {
            Ok(()) => return Ok(()),
            Err(e) => {
                log::warn!("download {} attempt {attempt} failed: {e}", task.url);
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(300 * attempt));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("download failed")))
}

fn fetch_once(task: &Task) -> Result<()> {
    let data = crate::http::get_bytes(&task.url)?;
    if let Some(size) = task.size {
        if data.len() as u64 != size {
            return Err(anyhow!(
                "{}: size mismatch (expected {size}, got {})",
                task.url,
                data.len()
            ));
        }
    }
    if let Some(sha) = &task.sha1 {
        let got = sha1_hex(&data);
        if !got.eq_ignore_ascii_case(sha) {
            return Err(anyhow!(
                "{}: sha1 mismatch (expected {sha}, got {got})",
                task.url
            ));
        }
    }
    ensure_parent(&task.dest)?;
    write_atomic(&task.dest, &data).with_context(|| format!("writing {}", task.dest.display()))?;
    #[cfg(unix)]
    if task.executable {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&task.dest, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// Download everything that is missing, in parallel. Returns the number of
/// files actually fetched.
pub fn fetch_all(
    tasks: &[Task],
    verify_hash: bool,
    parallel: usize,
    sink: &ProgressSink,
) -> Result<u64> {
    let todo: Vec<&Task> = tasks
        .iter()
        .filter(|t| !is_satisfied(t, verify_hash))
        .collect();
    let total = todo.len() as u64;
    report(sink, Progress::Step { done: 0, total });
    if todo.is_empty() {
        return Ok(0);
    }
    let parallel = parallel.clamp(1, 64).min(todo.len());
    let next = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    let errors: Mutex<Vec<anyhow::Error>> = Mutex::new(Vec::new());
    let todo = Arc::new(todo);

    std::thread::scope(|s| {
        for _ in 0..parallel {
            let todo = Arc::clone(&todo);
            let next = &next;
            let done = &done;
            let errors = &errors;
            s.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed) as usize;
                if i >= todo.len() {
                    break;
                }
                if errors.lock().map(|e| e.len() > 5).unwrap_or(true) {
                    break; // give up early after many failures
                }
                let t = todo[i];
                if let Err(e) = fetch_one(t) {
                    if let Ok(mut g) = errors.lock() {
                        g.push(e);
                    }
                }
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                report(sink, Progress::Step { done: d, total });
            });
        }
    });

    let errors = errors.into_inner().unwrap_or_default();
    if errors.is_empty() {
        Ok(total)
    } else {
        let mut msg = format!("{} download(s) failed:", errors.len());
        for e in errors.iter().take(5) {
            msg.push_str("\n  - ");
            msg.push_str(&e.to_string());
        }
        Err(anyhow!(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_known() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn satisfied_by_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.bin");
        std::fs::write(&p, b"hello").unwrap();
        let t = Task::new("http://x/", &p).size(Some(5));
        assert!(is_satisfied(&t, false));
        let t = Task::new("http://x/", &p).size(Some(6));
        assert!(!is_satisfied(&t, false));
        let t = Task::new("http://x/", &p)
            .size(Some(5))
            .sha1(Some("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d".into()));
        assert!(is_satisfied(&t, true));
        let t = Task::new("http://x/", &p)
            .size(Some(5))
            .sha1(Some("00".into()));
        assert!(!is_satisfied(&t, true));
        assert!(is_satisfied(&t, false), "size-only check ignores hash");
    }
}
