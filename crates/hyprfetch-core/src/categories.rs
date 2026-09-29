//! Category-based save directories.
//!
//! Downloads are automatically sorted into per-type folders under the base
//! download directory (`~/Downloads` by default):
//!
//! ```text
//! ~/Downloads/video/  ~/Downloads/pictures/  ~/Downloads/music/
//! ~/Downloads/compress/  ~/Downloads/documents/  ~/Downloads/apps/
//! ~/Downloads/other/
//! ```
//!
//! The folders are created automatically on server startup and whenever the
//! directory settings change — the user never has to mkdir anything. Each
//! category folder can be overridden individually via the
//! `category_dir_<name>` setting, and a task can opt out with an explicit
//! `save_dir` (direct save) or an explicit `category`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Canonical category names (also used as folder names).
pub const CATEGORIES: &[&str] = &[
    "video",
    "pictures",
    "music",
    "compress",
    "documents",
    "apps",
    "other",
];

/// Settings key holding the base download directory.
pub const SET_DOWNLOAD_DIR: &str = "download_dir";
/// Settings key toggling auto-categorization (`"false"` disables it).
pub const SET_CATEGORIZE: &str = "categorize";
/// Settings key prefix for per-category overrides: `category_dir_<name>`.
pub const SET_CATEGORY_DIR_PREFIX: &str = "category_dir_";

/// Per-category override settings key, e.g. `category_dir_video`.
pub fn override_key(category: &str) -> String {
    format!("{SET_CATEGORY_DIR_PREFIX}{category}")
}

/// Map a filename to its category by extension. Unknown extensions land in
/// `other`; a filename with no extension does too.
pub fn category_for_filename(filename: &str) -> &'static str {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    category_for_ext(&ext)
}

/// Extension → category table (lowercase, no leading dot).
pub fn category_for_ext(ext: &str) -> &'static str {
    match ext {
        // ---- video ----
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "m4v" | "mpg" | "mpeg" | "ts"
        | "m2ts" | "3gp" | "ogv" | "vob" | "rmvb" => "video",
        // ---- pictures ----
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "heic" | "heif" | "avif"
        | "tiff" | "tif" | "ico" | "psd" | "raw" | "cr2" | "nef" => "pictures",
        // ---- music ----
        "mp3" | "flac" | "wav" | "ogg" | "oga" | "m4a" | "aac" | "opus" | "wma" | "aiff"
        | "alac" | "mid" | "midi" => "music",
        // ---- compress / archives ----
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "tbz2" | "xz" | "txz" | "zst" | "zstd" | "7z"
        | "rar" | "lz4" | "lzma" | "cab" | "arj" | "jar" => "compress",
        // ---- documents ----
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp"
        | "txt" | "md" | "rtf" | "epub" | "mobi" | "azw3" | "csv" | "tsv" | "tex" | "djvu" => {
            "documents"
        }
        // ---- apps / disk images / packages ----
        "exe" | "msi" | "msix" | "dmg" | "pkg" | "deb" | "rpm" | "appimage" | "apk" | "iso"
        | "img" | "bin" | "flatpak" | "snap" | "app" => "apps",
        _ => "other",
    }
}

/// True when the name is one of the canonical categories.
pub fn is_valid_category(name: &str) -> bool {
    CATEGORIES.contains(&name)
}

/// Expand a leading `~` or `~user`-less tilde to `$HOME` (`$HOME` defaults
/// to `/tmp` when unset, matching the rest of the codebase).
pub fn expand_tilde(path: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    if path == "~" {
        return PathBuf::from(home);
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

/// Resolve the effective save directory for one category.
///
/// `settings` maps raw settings keys to raw (unexpanded) values. Precedence:
/// `category_dir_<name>` override → `<base>/<name>`. The returned path is
/// tilde-expanded and absolute.
pub fn dir_for_category(
    category: &str,
    base: &str,
    settings: &BTreeMap<String, String>,
) -> PathBuf {
    let override_dir = settings
        .get(&override_key(category))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    match override_dir {
        Some(dir) => expand_tilde(dir),
        None => expand_tilde(base).join(category),
    }
}

/// Create the base + every category directory (idempotent). Returns the
/// paths that did not exist before (i.e. were just created) so callers can
/// log them. Errors are per-directory: a failing directory is skipped so one
/// read-only path can't stop startup.
pub fn ensure_all_dirs(base: &str, settings: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let mut created = Vec::new();
    // Base first (best-effort — one read-only path must not stop startup);
    // we only report the leaf folders, since we can't tell whether the base
    // pre-existed.
    let base_path = expand_tilde(base);
    let _ = std::fs::create_dir_all(&base_path);
    for cat in CATEGORIES {
        let dir = dir_for_category(cat, base, settings);
        if !dir.exists() && std::fs::create_dir_all(&dir).is_ok() {
            created.push(dir);
        }
    }
    created
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_mapping() {
        assert_eq!(category_for_filename("movie.mkv"), "video");
        assert_eq!(category_for_filename("MOVIE.MP4"), "video");
        assert_eq!(category_for_filename("photo.jpeg"), "pictures");
        assert_eq!(category_for_filename("song.flac"), "music");
        assert_eq!(category_for_filename("backup.tar.gz"), "compress");
        assert_eq!(category_for_filename("archive.7z"), "compress");
        assert_eq!(category_for_filename("report.pdf"), "documents");
        assert_eq!(category_for_filename("setup.exe"), "apps");
        assert_eq!(category_for_filename("ubuntu.iso"), "apps");
        assert_eq!(category_for_filename("unknown.xyz"), "other");
        assert_eq!(category_for_filename("noext"), "other");
    }

    #[test]
    fn category_validity() {
        for c in CATEGORIES {
            assert!(is_valid_category(c));
        }
        assert!(!is_valid_category("movies"));
        assert!(!is_valid_category(""));
    }

    #[test]
    fn tilde_expansion_uses_home() {
        // SAFETY: tests run in one process; use a unique env probe instead of
        // mutating HOME (racy). We only assert the mechanics with /tmp.
        std::env::set_var("HOME", "/tmp/hf-test-home");
        assert_eq!(expand_tilde("~"), PathBuf::from("/tmp/hf-test-home"));
        assert_eq!(
            expand_tilde("~/Downloads/x"),
            PathBuf::from("/tmp/hf-test-home/Downloads/x")
        );
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
    }

    #[test]
    fn dir_for_category_prefers_override() {
        // Both HOME-touching tests pin the same value, so running in
        // parallel is race-free.
        std::env::set_var("HOME", "/tmp/hf-test-home");
        let mut s = BTreeMap::new();
        s.insert(override_key("music"), "/data/music-collection".into());
        assert_eq!(
            dir_for_category("music", "~/Downloads", &s),
            PathBuf::from("/data/music-collection")
        );
        assert_eq!(
            dir_for_category("video", "~/Downloads", &s),
            PathBuf::from("/tmp/hf-test-home/Downloads/video"),
            "non-overridden categories follow the base dir"
        );
    }

    #[test]
    fn ensure_all_dirs_creates_leaves() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("dl").to_string_lossy().into_owned();
        let s = BTreeMap::new();
        let created = ensure_all_dirs(&base, &s);
        assert_eq!(created.len(), CATEGORIES.len(), "all leaves created");
        for cat in CATEGORIES {
            assert!(tmp.path().join("dl").join(cat).is_dir());
        }
        // Idempotent: second run creates nothing new.
        assert!(ensure_all_dirs(&base, &s).is_empty());
    }
}
