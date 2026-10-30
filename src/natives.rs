//! Extraction of native libraries (LWJGL etc.) into the per-version natives dir.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

/// Extract `jar` into `dest`, skipping entries that start with any of `exclude`.
/// Already present files with the same size are left untouched.
pub fn extract_natives(jar: &Path, dest: &Path, exclude: &[String]) -> Result<usize> {
    let f = std::fs::File::open(jar).with_context(|| format!("opening {}", jar.display()))?;
    let mut zip = zip::ZipArchive::new(f).with_context(|| format!("reading {}", jar.display()))?;
    std::fs::create_dir_all(dest)?;
    let mut n = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        if name.ends_with('/')
            || exclude.iter().any(|e| name.starts_with(e))
            || name.starts_with("META-INF/")
        {
            continue;
        }
        // Natives are expected flat (or nested); only keep the file name component
        // for nested entries so the JVM finds them via java.library.path.
        let Some(file_name) = Path::new(&name).file_name() else {
            continue;
        };
        let out = dest.join(file_name);
        if out.exists()
            && std::fs::metadata(&out)
                .map(|m| m.len() == entry.size())
                .unwrap_or(false)
        {
            continue;
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        crate::paths::write_atomic(&out, &buf)?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn extracts_and_skips_meta_inf() {
        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("n.jar");
        {
            let f = std::fs::File::create(&jar).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("liblwjgl.so", o).unwrap();
            z.write_all(b"ELF").unwrap();
            z.start_file("META-INF/MANIFEST.MF", o).unwrap();
            z.write_all(b"x").unwrap();
            z.start_file("linux/x64/org/lwjgl/libglfw.so", o).unwrap();
            z.write_all(b"ELF2").unwrap();
            z.finish().unwrap();
        }
        let out = dir.path().join("natives");
        let n = extract_natives(&jar, &out, &["META-INF/".into()]).unwrap();
        assert_eq!(n, 2);
        assert!(out.join("liblwjgl.so").exists());
        assert!(out.join("libglfw.so").exists());
        assert!(!out.join("MANIFEST.MF").exists());
        // second run is a no-op
        assert_eq!(extract_natives(&jar, &out, &[]).unwrap(), 0);
    }
}
