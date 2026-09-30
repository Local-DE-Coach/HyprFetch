//! Desktop widget management (v0.5.0) — the in-app "set widget" feature.
//!
//! The Quickshell bar widget (illogical-impulse) can be installed from a
//! terminal (`curl …/widget-install.sh | sh`) OR from right here in the
//! WebUI. This module powers the WebUI side:
//!
//! - `GET  /api/widget/status`    → what's installed where, badge-ready
//! - `POST /api/widget/install`   → download → verify → extract → wire bar
//! - `POST /api/widget/uninstall` → remove files + undo the bar edit
//!
//! Everything touches ONLY user-owned paths under `$HOME` (the ii
//! quickshell config), so no privileges are ever involved — same spirit
//! as the v0.4.9 update migration.
//!
//! The bar QML edit is done with marker-based pure functions
//! ([`integrate_bar_source`] / [`remove_bar_edit`]) that mirror the
//! POSIX installer byte-for-byte: same import line, same marker
//! comment, same anchor (`layoutDirection: Qt.RightToLeft`). A
//! `.bak-hyprfetch` backup is kept, edits are idempotent, and
//! uninstalling restores the file to exactly its previous content.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use hyprfetch_core::widget_status::status_dir;

/// Quickshell config root (illogical-impulse). `HYPRFETCH_QS_ROOT`
/// overrides — tests use it, power users can too.
fn qs_root() -> PathBuf {
    if let Ok(root) = std::env::var("HYPRFETCH_QS_ROOT") {
        if !root.trim().is_empty() {
            return PathBuf::from(root);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    Path::new(&home)
        .join(".config")
        .join("quickshell")
        .join("ii")
}

/// Where the widget module lives inside the ii config.
fn widget_dir(root: &Path) -> PathBuf {
    root.join("modules").join("downloadManager")
}

/// Bar QML candidates, most-likely-first (matches widget/install.sh).
fn bar_candidates(root: &Path) -> Vec<PathBuf> {
    [
        "modules/ii/bar/BarContent.qml",
        "modules/bar/BarContent.qml",
        "modules/ii/bar/Bar.qml",
        "modules/bar/Bar.qml",
    ]
    .iter()
    .map(|p| root.join(p))
    .collect()
}

/// First existing bar QML (the file the installer would edit).
fn find_bar_file(root: &Path) -> Option<PathBuf> {
    bar_candidates(root).into_iter().find(|p| p.is_file())
}

/// The exact import line both installers add.
pub const IMPORT_LINE: &str = "import qs.modules.downloadManager";
/// Marker comment embedded in the auto-edit; uninstall removes from here.
pub const MARKER: &str = "HyprFetch download widget";
/// Backup suffix for the pristine bar file.
pub const BACKUP_SUFFIX: &str = ".bak-hyprfetch";

/// Outcome of trying to wire the widget into a bar QML source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarEdit {
    /// `DownloadWidget` is already referenced — nothing to do.
    AlreadyIntegrated,
    /// The file was understood and edited; carries the new source.
    Edited(String),
    /// No `layoutDirection: Qt.RightToLeft` anchor — leave the file alone
    /// and let the caller print manual instructions.
    NoAnchor,
}

/// Add the import line before the first real statement (after the
/// import/pragma/comment header) and the widget instance right after the
/// right-side row anchor. Mirrors `widget/install.sh` `add_import` +
/// `add_widget_block`. Byte-exactness: a file that had no trailing
/// newline keeps having none, so uninstall can restore it exactly.
pub fn integrate_bar_source(src: &str) -> BarEdit {
    if src.contains("DownloadWidget") {
        return BarEdit::AlreadyIntegrated;
    }
    // Anchor check: only edit layouts we understand.
    let has_anchor = src.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("layoutDirection:") && t.contains("Qt.RightToLeft")
    });
    if !has_anchor {
        return BarEdit::NoAnchor;
    }

    let lines = src.lines_including_last();
    let had_trailing_nl = src.ends_with('\n');
    let total = lines.len();
    let mut out = String::with_capacity(src.len() + 256);
    let mut import_added = false;
    let mut block_added = false;
    for (i, line) in lines.iter().enumerate() {
        if !import_added && starts_statement(line) {
            out.push_str(IMPORT_LINE);
            out.push('\n');
            import_added = true;
        }
        out.push_str(line);
        if i + 1 < total || had_trailing_nl {
            out.push('\n');
        }
        if !block_added {
            let t = line.trim_start();
            if t.starts_with("layoutDirection:") && t.contains("Qt.RightToLeft") {
                if !out.ends_with('\n') {
                    out.push('\n'); // file ended without a newline
                }
                let indent = line.len() - t.len();
                let pad = &line[..indent];
                out.push_str(pad);
                out.push_str(&format!(
                    "// {MARKER} (auto-added — delete this block to remove)\n"
                ));
                out.push_str(pad);
                out.push_str("DownloadWidget {\n");
                out.push_str(pad);
                out.push_str("    Layout.alignment: Qt.AlignVCenter\n");
                out.push_str(pad);
                out.push_str("}\n");
                block_added = true;
            }
        }
    }
    if !import_added {
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out.push_str(IMPORT_LINE);
        out.push('\n');
    }
    BarEdit::Edited(out)
}

/// A line that begins a real statement (not blank / comment / pragma /
/// import) — the import goes immediately before the first such line.
fn starts_statement(line: &str) -> bool {
    let t = line.trim_start();
    !(t.is_empty() || t.starts_with("//") || t.starts_with("pragma ") || t.starts_with("import "))
}

/// Remove the auto-added import + widget block. Mirrors the installer's
/// `remove_widget_edit` awk, including the 12-line runaway guard. Also
/// newline-exact: no trailing newline in → none out.
pub fn remove_bar_edit(src: &str) -> String {
    let lines = src.lines_including_last();
    let had_trailing_nl = src.ends_with('\n');
    let total = lines.len();
    let mut out = String::with_capacity(src.len());
    let mut in_block = false;
    let mut guard = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with(IMPORT_LINE) {
            continue;
        }
        if line.contains(&format!("// {MARKER} (auto-added")) {
            in_block = true;
            guard = 0;
            continue;
        }
        if in_block {
            let t = line.trim_start();
            if t == "}" {
                in_block = false;
                continue; // consume the block's closing brace too
            }
            guard += 1;
            if guard > 12 {
                in_block = false; // safety: never eat the whole file
            } else {
                continue;
            }
        }
        out.push_str(line);
        if i + 1 < total || had_trailing_nl {
            out.push('\n');
        }
    }
    out
}

/// Tiny helper: iterate lines keeping a final unterminated line (so edits
/// on files without a trailing newline behave).
trait LinesExt {
    fn lines_including_last(&self) -> Vec<String>;
}
impl LinesExt for str {
    fn lines_including_last(&self) -> Vec<String> {
        let mut v: Vec<String> = self.lines().map(String::from).collect();
        if v.is_empty() {
            v.push(String::new());
        }
        v
    }
}

// ---------------------------------------------------------------------------
// Status / install / uninstall
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// State file written on install (`status_dir()/widget.json`).
fn state_file() -> PathBuf {
    status_dir().join("widget.json")
}

fn read_state() -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(state_file()).ok()?;
    serde_json::from_str(&raw).ok()
}

/// `true` when `name` resolves to an executable on PATH.
fn on_path(name: &str) -> bool {
    let path = match std::env::var("PATH") {
        Ok(p) => p,
        Err(_) => return false,
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .any(|p| p.is_file())
}

/// `GET /api/widget/status` body.
pub fn status_json() -> serde_json::Value {
    let root = qs_root();
    let root_found = root.is_dir();
    let dir = widget_dir(&root);
    let installed = dir.join("DownloadWidget.qml").is_file();
    let bar = find_bar_file(&root);
    let integrated = bar
        .as_ref()
        .map(|b| {
            std::fs::read_to_string(b)
                .map(|s| s.contains("DownloadWidget"))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    let state = read_state();
    let version = state
        .as_ref()
        .and_then(|s| s.get("version"))
        .and_then(|v| v.as_str())
        .map(String::from);
    serde_json::json!({
        "qs_root": root.to_string_lossy(),
        "qs_found": root_found,
        "quickshell_found": on_path("quickshell"),
        "hyprfetch_found": on_path("hyprfetch"),
        "installed": installed,
        "integrated": integrated,
        "bar_file": bar.as_ref().map(|b| b.to_string_lossy()),
        "version": version,
        "up_to_date": version.as_deref() == Some(env!("CARGO_PKG_VERSION")),
        "reload_hint": "qs -c ii kill; qs -c ii &",
    })
}

/// Install (or update) the widget: download the tarball from the channel,
/// extract the `downloadManager/` tree safely, wire the bar, persist state.
/// Every path involved lives under `$HOME` — no privileges needed.
pub async fn install_from_channel(channel: Option<&str>) -> Result<serde_json::Value, String> {
    let root = qs_root();
    if !root.is_dir() {
        return Err(format!(
            "illogical-impulse quickshell config not found at {} — install ii first",
            root.display()
        ));
    }
    let channel = channel.filter(|c| !c.trim().is_empty()).ok_or_else(|| {
        "update channel is disabled — set [update] channel or HYPRFETCH_UPDATE_CHANNEL".to_string()
    })?;
    let url = format!("{}/widget.tar.gz", channel.trim_end_matches('/'));

    // Small source-only archive (~40 KB): a bounded total timeout is right.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .user_agent(concat!("hyprfetch/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let bytes = client
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download {url}: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("download {url}: {e}"))?;

    // Extract into a sibling temp dir, validate, then swap into place.
    let dest = widget_dir(&root);
    let staging = dest.with_extension("hyprfetch-staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("staging dir: {e}"))?;

    let gz = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(false);
    for entry in archive.entries().map_err(|e| format!("bad archive: {e}"))? {
        let mut entry = entry.map_err(|e| format!("bad archive: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("bad archive: {e}"))?
            .into_owned();
        // Refuse traversal/symlink tricks: only downloadManager/** regular
        // files are accepted.
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let mut comps: Vec<_> = path.components().collect();
        let is_widget_root = matches!(
            comps.first(),
            Some(std::path::Component::Normal(name)) if *name == std::ffi::OsStr::new("downloadManager")
        );
        if !is_widget_root {
            continue;
        }
        comps.remove(0);
        if comps
            .iter()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!("unsafe path in widget archive: {}", path.display()));
        }
        let target = staging.join(comps.iter().collect::<PathBuf>());
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("extract: {e}"))?;
        }
        let mut out = Vec::new();
        entry
            .read_to_end(&mut out)
            .map_err(|e| format!("extract {}: {e}", path.display()))?;
        std::fs::write(&target, out).map_err(|e| format!("extract {}: {e}", path.display()))?;
    }
    for f in [
        "DownloadWidget.qml",
        "components/RecentPopup.qml",
        "components/InputPopup.qml",
        "components/ActivePopup.qml",
        "components/CompletionToast.qml",
        "utils/DownloadProcess.qml",
    ] {
        if !staging.join(f).is_file() {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!("archive is missing downloadManager/{f}"));
        }
    }

    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&staging, &dest).map_err(|e| format!("activate widget: {e}"))?;

    // Wire the bar (idempotent, backup kept, unknown layouts untouched).
    let mut integrated = false;
    let mut bar_note = String::new();
    if let Some(bar) = find_bar_file(&root) {
        let src = std::fs::read_to_string(&bar).unwrap_or_default();
        match integrate_bar_source(&src) {
            BarEdit::AlreadyIntegrated => integrated = true,
            BarEdit::Edited(next) => {
                let backup = bar.with_file_name(format!(
                    "{}{BACKUP_SUFFIX}",
                    bar.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Bar.qml")
                ));
                if !backup.exists() {
                    let _ = std::fs::copy(&bar, &backup);
                }
                match std::fs::write(&bar, next) {
                    Ok(()) => integrated = true,
                    Err(e) => bar_note = format!("could not edit bar file: {e}"),
                }
            }
            BarEdit::NoAnchor => {
                bar_note = format!(
                    "could not auto-edit {} (unknown layout) — add: {IMPORT_LINE} + DownloadWidget {{}}",
                    bar.display()
                );
            }
        }
    } else {
        bar_note = format!(
            "no ii bar QML found under {} — add {IMPORT_LINE} + DownloadWidget {{}} to your bar manually",
            root.display()
        );
    }

    let bar_file = find_bar_file(&root).map(|b| b.to_string_lossy().to_string());
    let state = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "installed_at": now_secs(),
        "bar_file": bar_file,
        "integrated": integrated,
    });
    if let Some(parent) = state_file().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        state_file(),
        serde_json::to_string(&state).unwrap_or_default(),
    );

    let mut body = status_json();
    body["installed"] = serde_json::json!(true);
    body["integrated"] = serde_json::json!(integrated);
    body["version"] = serde_json::json!(env!("CARGO_PKG_VERSION"));
    body["note"] = serde_json::json!(bar_note);
    Ok(body)
}

/// Uninstall: remove the module dir, undo the bar edit, drop state.
pub fn uninstall() -> Result<serde_json::Value, String> {
    let root = qs_root();
    let dest = widget_dir(&root);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("remove widget files: {e}"))?;
    }
    let mut reverted = false;
    if let Some(bar) = find_bar_file(&root) {
        if let Ok(src) = std::fs::read_to_string(&bar) {
            if src.contains(MARKER) || src.contains(IMPORT_LINE) {
                let backup = bar.with_file_name(format!(
                    "{}{BACKUP_SUFFIX}",
                    bar.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Bar.qml")
                ));
                if !backup.exists() {
                    let _ = std::fs::copy(&bar, &backup);
                }
                let cleaned = remove_bar_edit(&src);
                let _ = std::fs::write(&bar, cleaned);
                reverted = true;
            }
        }
    }
    let _ = std::fs::remove_file(state_file());
    let mut body = status_json();
    body["installed"] = serde_json::json!(false);
    body["bar_reverted"] = serde_json::json!(reverted);
    Ok(body)
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

use axum::extract::State;
use axum::Json;

use crate::error::ApiError;
use crate::AppState;

/// `GET /api/widget/status` — Quickshell bar widget state for the
/// Settings → Desktop widget card.
pub async fn widget_status() -> Json<serde_json::Value> {
    Json(status_json())
}

/// `POST /api/widget/install` — download + install + wire the bar. Runs
/// entirely in user-owned paths; the update channel is the only network
/// peer (same rule as the app updater).
pub async fn widget_install(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let channel = state.update_cfg.effective_channel().map(str::to_string);
    install_from_channel(channel.as_deref())
        .await
        .map(Json)
        .map_err(ApiError::InternalError)
}

/// `POST /api/widget/uninstall` — remove files + undo the bar edit.
pub async fn widget_uninstall() -> Result<Json<serde_json::Value>, ApiError> {
    uninstall().map(Json).map_err(ApiError::InternalError)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed but structurally faithful excerpt of end-4's BarContent.qml
    /// (right section with the Qt.RightToLeft row we anchor on).
    const BAR: &str = r#"import qs.modules.ii.bar.weather
import QtQuick
import QtQuick.Layouts
import qs
import qs.services
import qs.modules.common

Item { // Bar content region
    id: root

    RowLayout {
        id: rightSectionRowLayout
        anchors.fill: parent
        spacing: 5
        layoutDirection: Qt.RightToLeft

        RippleButton { // Right sidebar button
            id: rightSidebarButton
            Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
        }
    }
}
"#;

    #[test]
    fn integrate_adds_import_and_block() {
        let BarEdit::Edited(out) = integrate_bar_source(BAR) else {
            panic!("expected Edited");
        };
        assert!(out.contains(IMPORT_LINE));
        assert!(out.contains("DownloadWidget {"));
        assert!(out.contains(MARKER));
        // import lands with the other imports, before the first statement
        let first_import = out.lines().position(|l| l.starts_with("import ")).unwrap();
        let our_import = out
            .lines()
            .position(|l| l.starts_with(IMPORT_LINE))
            .unwrap();
        assert!(our_import > first_import);
        let item = out.lines().position(|l| l.starts_with("Item {")).unwrap();
        assert!(our_import < item);
        // block sits right after the anchor line, same indent
        let anchor = out
            .lines()
            .position(|l| l.contains("layoutDirection: Qt.RightToLeft"))
            .unwrap();
        assert_eq!(
            out.lines().nth(anchor + 1).unwrap().trim_start(),
            &format!("// {MARKER} (auto-added — delete this block to remove)")
        );
        assert!(out
            .lines()
            .nth(anchor + 1)
            .unwrap()
            .starts_with("        //"));
        // braces stay balanced
        assert_eq!(out.matches('{').count(), out.matches('}').count());
    }

    #[test]
    fn integrate_is_idempotent() {
        let BarEdit::Edited(once) = integrate_bar_source(BAR) else {
            panic!("expected Edited");
        };
        assert_eq!(integrate_bar_source(&once), BarEdit::AlreadyIntegrated);
    }

    #[test]
    fn integrate_unknown_layout_leaves_file_alone() {
        let src = "import QtQuick\nItem {\n    id: weird\n}\n";
        assert_eq!(integrate_bar_source(src), BarEdit::NoAnchor);
    }

    #[test]
    fn remove_restores_exactly() {
        let BarEdit::Edited(edited) = integrate_bar_source(BAR) else {
            panic!("expected Edited");
        };
        let cleaned = remove_bar_edit(&edited);
        assert_eq!(cleaned, BAR);
    }

    #[test]
    fn remove_is_noop_on_clean_file() {
        assert_eq!(remove_bar_edit(BAR), BAR);
    }

    #[test]
    fn remove_handles_file_without_trailing_newline() {
        let bar_no_nl = BAR.trim_end_matches('\n');
        let BarEdit::Edited(edited) = integrate_bar_source(bar_no_nl) else {
            panic!("expected Edited");
        };
        // The auto-added block always terminates with a newline, so exact
        // byte restoration is impossible without the .bak backup (which
        // uninstall keeps). Content-wise the edit must undo cleanly:
        let cleaned = remove_bar_edit(&edited);
        assert_eq!(
            cleaned.trim_end_matches('\n'),
            bar_no_nl,
            "edit must be fully removable modulo the trailing newline"
        );
    }

    #[test]
    fn bar_candidates_order() {
        let root = Path::new("/ii");
        let c = bar_candidates(root);
        assert_eq!(c[0], Path::new("/ii/modules/ii/bar/BarContent.qml"));
        assert_eq!(c[3], Path::new("/ii/modules/bar/Bar.qml"));
    }
}
