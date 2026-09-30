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

// ---------------------------------------------------------------------------
// Content-Type sniffing — filename/extension auto-detection
// ---------------------------------------------------------------------------

/// Common `Content-Type` → extension mappings for types whose extension
/// cannot be reliably guessed from a URL (or where the URL carries none at
/// all — think `https://images.example/tbn?id=ANd9Gc…`). Deliberately small:
/// `mime_guess` already covers the long tail from extensions; this table is
/// the reverse direction, which needs hand-picked canonical extensions.
pub fn ext_for_content_type(ct: &str) -> Option<&'static str> {
    // Strip parameters: `image/jpeg; charset=binary` → `image/jpeg`.
    let mime = ct.split(';').next()?.trim().to_ascii_lowercase();
    Some(match mime.as_str() {
        // images
        "image/jpeg" | "image/jpg" | "image/pjpeg" => "jpg",
        "image/png" | "image/apng" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/svg+xml" => "svg",
        "image/bmp" | "image/x-ms-bmp" => "bmp",
        "image/tiff" => "tif",
        "image/heic" | "image/heif" => "heic",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        // video
        "video/mp4" | "video/x-m4v" => "mp4",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        "video/mpeg" => "mpeg",
        "video/x-msvideo" => "avi",
        // audio
        "audio/mpeg" => "mp3",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
        "audio/aac" => "aac",
        "audio/opus" => "opus",
        // archives
        "application/zip" => "zip",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-7z-compressed" => "7z",
        "application/x-rar-compressed" | "application/vnd.rar" => "rar",
        "application/x-tar" => "tar",
        "application/x-xz" => "xz",
        "application/zstd" => "zst",
        "application/x-bzip2" => "bz2",
        // documents
        "application/pdf" => "pdf",
        "application/msword" => "doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.ms-excel" => "xls",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.ms-powerpoint" => "ppt",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/epub+zip" => "epub",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "text/markdown" => "md",
        "text/html" => "html",
        "application/json" => "json",
        "application/xml" | "text/xml" => "xml",
        // apps / disk images
        "application/x-iso9660-image" => "iso",
        "application/vnd.android.package-archive" => "apk",
        "application/x-deb" | "application/vnd.debian.binary-package" => "deb",
        "application/x-rpm" => "rpm",
        "application/x-apple-diskimage" => "dmg",
        "application/x-msdownload" | "application/x-msi" => "exe",
        // `application/octet-stream` and everything unknown → None: the
        // server is not telling us anything we can act on.
        _ => return None,
    })
}

/// Sanitize a URL-derived filename: keep the last path segment only, drop
/// any query/fragment leftovers, strip control characters, and never return
/// an empty string.
pub fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        match ch {
            // A separator starts a NEW segment — last one wins (mirrors how
            // the caller picks the tail of a URL path).
            '/' | '\\' => out.clear(),
            '\0' => {}                // never allow NUL
            '?' | '#' | '&' => break, // query/fragment junk pasted into the name
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    let trimmed = out.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "download.bin".into()
    } else {
        trimmed.to_string()
    }
}

/// Does this filename carry an extension the category table knows?
fn has_known_ext(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| category_for_ext(&e.to_ascii_lowercase()) != "other")
        .unwrap_or(false)
}

/// Filename with the extension corrected from the server's `Content-Type`.
///
/// Rules (IDM-style):
/// - name already has a KNOWN extension (`.jpg`, `.pdf`, …) → keep as-is;
/// - name has no extension, or an unknown one (`.php`, `.aspx`, query junk)
///   and the server sent a usable Content-Type → use/replace with the
///   sniffed extension (`images` + `image/jpeg` → `images.jpg`);
/// - nothing usable → return the sanitized name unchanged.
///
/// The stem is always sanitized first, so `images?q=tbn:ANd9…` becomes
/// `images.jpg` rather than a 200-character query string.
pub fn sniff_filename(raw_name: &str, content_type: Option<&str>) -> String {
    let name = sanitize_filename(raw_name);
    if has_known_ext(&name) {
        return name;
    }
    let Some(ext) = content_type.and_then(ext_for_content_type) else {
        return name;
    };
    let stem = match name.rsplit_once('.') {
        // Only trim the extension part off when there IS one; a plain
        // `images` (no dot) must not lose characters.
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => name.clone(),
    };
    let stem = if stem.is_empty() {
        "download".into()
    } else {
        stem
    };
    format!("{stem}.{ext}")
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
    fn content_type_to_extension() {
        assert_eq!(ext_for_content_type("image/jpeg"), Some("jpg"));
        assert_eq!(
            ext_for_content_type("image/jpeg; charset=binary"),
            Some("jpg")
        );
        assert_eq!(ext_for_content_type("IMAGE/PNG"), Some("png"));
        assert_eq!(ext_for_content_type("video/mp4"), Some("mp4"));
        assert_eq!(ext_for_content_type("application/pdf"), Some("pdf"));
        assert_eq!(ext_for_content_type("application/octet-stream"), None);
        assert_eq!(ext_for_content_type("text/x-made-up"), None);
        assert_eq!(ext_for_content_type(""), None);
    }

    #[test]
    fn sanitize_strips_query_junk() {
        assert_eq!(sanitize_filename("images?q=tbn:ANd9GcQ"), "images");
        assert_eq!(sanitize_filename("photo.png#anchor"), "photo.png");
        assert_eq!(sanitize_filename("a/b/c?x=1"), "c");
        assert_eq!(sanitize_filename("file\\name?.mp4"), "name");
        assert_eq!(sanitize_filename("?q=only-query"), "download.bin");
        assert_eq!(sanitize_filename(""), "download.bin");
        assert_eq!(sanitize_filename("..."), "download.bin");
        assert_eq!(sanitize_filename("résumé.pdf"), "résumé.pdf");
    }

    #[test]
    fn sniff_filename_from_content_type() {
        // The exact report that motivated v0.4.6: Google image-thumbnail
        // URLs carry the query in the last path segment and no extension.
        assert_eq!(
            sniff_filename("images?q=tbn:ANd9GcQ", Some("image/jpeg")),
            "images.jpg"
        );
        assert_eq!(
            sniff_filename("images?q=tbn:ANd9GcQ", Some("image/png")),
            "images.png"
        );
        // No extension at all, server tells the type.
        assert_eq!(
            sniff_filename("download", Some("application/pdf")),
            "download.pdf"
        );
        // Unknown extension is REPLACED by the sniffed one.
        assert_eq!(sniff_filename("file.php", Some("image/jpeg")), "file.jpg");
        // Known extension is kept even when the type disagrees.
        assert_eq!(sniff_filename("photo.png", Some("image/jpeg")), "photo.png");
        // No usable type → sanitized name unchanged.
        assert_eq!(
            sniff_filename("images?q=tbn:x", Some("application/octet-stream")),
            "images"
        );
        assert_eq!(sniff_filename("images?q=tbn:x", None), "images");
        // Parameters in the content type are ignored.
        assert_eq!(
            sniff_filename("song", Some("audio/mpeg; bitrate=320")),
            "song.mp3"
        );
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
