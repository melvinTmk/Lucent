//! Downloads folder stack reader: reads recent files on demand (zero background polling).
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StackFile {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub modified: u64,
    pub is_dir: bool,
}

/// Resolves a symbolic folder alias (e.g. "downloads") or an absolute folder path.
pub fn resolve_stack_dir(folder: &str) -> Option<PathBuf> {
    if folder.eq_ignore_ascii_case("downloads") {
        dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
    } else {
        let p = PathBuf::from(folder);
        if p.is_dir() {
            Some(p)
        } else {
            None
        }
    }
}

/// Reads the most recent files from the resolved folder up to `limit` entries.
pub fn read_folder_stack(folder: &str, limit: usize) -> Result<Vec<StackFile>, String> {
    let dir = resolve_stack_dir(folder)
        .ok_or_else(|| format!("Could not resolve folder: {folder}"))?;
    read_stack_from_path(&dir, limit)
}

/// Reads entries from a given path, sorted by modified timestamp descending.
pub fn read_stack_from_path(dir: &Path, limit: usize) -> Result<Vec<StackFile>, String> {
    let read_dir = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut files = Vec::new();

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden files and common OS clutter
        if name.starts_with('.')
            || name.eq_ignore_ascii_case("desktop.ini")
            || name.eq_ignore_ascii_case("thumbs.db")
        {
            continue;
        }

        if let Ok(meta) = entry.metadata() {
            let is_dir = meta.is_dir();
            let size = meta.len();
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let path = entry.path().to_string_lossy().to_string();

            files.push(StackFile {
                name,
                path,
                size,
                modified,
                is_dir,
            });
        }
    }

    // Sort by modified descending (most recent first), tie-break alphabetically
    files.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.name.cmp(&b.name)));
    if limit > 0 && files.len() > limit {
        files.truncate(limit);
    }

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn stack_reads_and_orders_by_modified() {
        let tmp = std::env::temp_dir().join(format!("lucent_test_stack_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let f1 = tmp.join("file1.txt");
        let f2 = tmp.join("file2.txt");
        let f3 = tmp.join("file3.txt");
        let hidden = tmp.join(".hidden.txt");
        let ini = tmp.join("desktop.ini");

        File::create(&f1).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(60));
        File::create(&f2).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(60));
        File::create(&f3).unwrap();
        File::create(&hidden).unwrap();
        File::create(&ini).unwrap();

        let files = read_stack_from_path(&tmp, 2).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "file3.txt");
        assert_eq!(files[1].name, "file2.txt");

        let all = read_stack_from_path(&tmp, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].name, "file3.txt");
        assert_eq!(all[1].name, "file2.txt");
        assert_eq!(all[2].name, "file1.txt");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_downloads_folder() {
        let res = resolve_stack_dir("downloads");
        assert!(res.is_some());
    }
}
