//! Bounded, checkout-relative text edits with optimistic conflict detection.
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_BYTES: usize = 1024 * 1024;
static NEXT_SAVE: AtomicU64 = AtomicU64::new(0);

fn resolve(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty() || relative.contains('\0') {
        return Err("Choose a file inside the checkout".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut path = root.clone();
    for part in Path::new(relative).components() {
        let Component::Normal(name) = part else {
            return Err("The file path must stay inside the checkout".into());
        };
        if name.to_string_lossy().eq_ignore_ascii_case(".git") {
            return Err("Git metadata cannot be edited here".into());
        }
        path.push(name);
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("Symbolic links cannot be edited here; open the real file instead".into());
        }
    }
    if !path.is_file() || !path.starts_with(&root) {
        return Err("Choose a regular file inside the checkout".into());
    }
    Ok(path)
}

pub fn read(root: &Path, relative: &str) -> Result<String, String> {
    let path = resolve(root, relative)?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("This editor supports files up to 1 MiB".into());
    }
    if bytes.contains(&0) {
        return Err("Binary files cannot be edited here".into());
    }
    String::from_utf8(bytes).map_err(|_| "This editor supports UTF-8 text files".into())
}

pub fn save(root: &Path, relative: &str, original: &str, content: &str) -> Result<(), String> {
    if content.len() > MAX_BYTES || content.contains('\0') {
        return Err("Save requires UTF-8 text up to 1 MiB without null bytes".into());
    }
    let path = resolve(root, relative)?;
    if read(root, relative)? != original {
        return Err("This file changed on disk. Your draft is preserved. Copy it or reload the latest version before saving.".into());
    }
    let temporary = path.with_file_name(format!(
        ".arachne-edit-{}-{}",
        std::process::id(),
        NEXT_SAVE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut temporary_created = false;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        temporary_created = true;
        file.set_permissions(
            std::fs::metadata(&path)
                .map_err(|e| e.to_string())?
                .permissions(),
        )
        .map_err(|e| e.to_string())?;
        file.write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        // Recheck immediately before replacement: an agent may be editing too.
        if resolve(root, relative)? != path || read(root, relative)? != original {
            return Err("This file changed during save; your draft is preserved".into());
        }
        std::fs::rename(&temporary, &path).map_err(|e| e.to_string())
    })();
    if result.is_err() && temporary_created {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn checkout() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "arachne-editor-{}-{}",
            std::process::id(),
            NEXT_SAVE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("readme.md"), "before\r\n").unwrap();
        root
    }
    #[test]
    fn round_trip_and_stale_save_preserve_agent_changes() {
        let root = checkout();
        let original = read(&root, "readme.md").unwrap();
        save(&root, "readme.md", &original, "after\r\n").unwrap();
        assert_eq!(read(&root, "readme.md").unwrap(), "after\r\n");
        assert!(save(&root, "readme.md", &original, "stale draft").is_err());
        assert_eq!(read(&root, "readme.md").unwrap(), "after\r\n");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rejects_escaping_metadata_binary_and_oversized_files() {
        let root = checkout();
        for path in ["../readme.md", "/etc/passwd", ".git/config", ""] {
            assert!(read(&root, path).is_err());
        }
        std::fs::write(root.join("binary"), [0, 1, 2]).unwrap();
        std::fs::write(root.join("large"), vec![b'a'; MAX_BYTES + 1]).unwrap();
        assert!(read(&root, "binary").is_err());
        assert!(read(&root, "large").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("readme.md", root.join("link")).unwrap();
            assert!(save(&root, "link", "before\r\n", "bad").is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
