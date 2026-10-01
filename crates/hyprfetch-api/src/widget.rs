//! Desktop widget management (v0.5.1) — the in-app "set widget" feature.
//!
//! The Quickshell sidebar widget (illogical-impulse) can be installed from
//! a terminal (`curl …/widget-install.sh | sh`) OR from right here in the
//! WebUI. This module powers the WebUI side:
//!
//! - `GET  /api/widget/status`    → what's installed where, badge-ready
//! - `POST /api/widget/install`   → download → verify → extract → wire tab
//! - `POST /api/widget/uninstall` → remove files + undo the sidebar edit
//!
//! Everything touches ONLY user-owned paths under `$HOME` (the ii
//! quickshell config), so no privileges are ever involved — same spirit
//! as the v0.4.9 update migration.
//!
//! v0.5.1 — the widget moved from the BAR to the SIDEBAR: it is now the
//! "Downloads" tab of ii's left sidebar
//! (`modules/ii/sidebarLeft/downloadManager/`). Installing wires it into
//! `SidebarLeftContent.qml` (surgical, marker-based, idempotent edits with
//! a `.bak-hyprfetch` backup) and sets `policies.downloadManager: 1` in
//! `~/.config/illogical-impulse/config.json`. Installing also REMOVES the
//! old v0.5.0 bar widget (module dir + its auto-added bar edit), so one
//! click upgrades cleanly.

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

/// Where the sidebar widget module lives inside the ii config (v0.5.1+).
fn widget_dir(root: &Path) -> PathBuf {
    root.join("modules")
        .join("ii")
        .join("sidebarLeft")
        .join("downloadManager")
}

/// The sidebar's tab-content QML the widget integrates into.
fn sidebar_content_file(root: &Path) -> PathBuf {
    root.join("modules")
        .join("ii")
        .join("sidebarLeft")
        .join("SidebarLeftContent.qml")
}

/// The OLD v0.5.0 bar-widget module (removed on install/uninstall).
fn legacy_widget_dir(root: &Path) -> PathBuf {
    root.join("modules").join("downloadManager")
}

/// Bar QML candidates, most-likely-first (matches the legacy bar edit).
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

/// First existing bar QML (only used to UNDO the legacy v0.5.0 bar edit).
fn find_bar_file(root: &Path) -> Option<PathBuf> {
    bar_candidates(root).into_iter().find(|p| p.is_file())
}

/// ii's own config file (the shell's `Config.options`).
fn ii_config_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    Path::new(&home)
        .join(".config")
        .join("illogical-impulse")
        .join("config.json")
}

// ---------------------------------------------------------------------------
// Sidebar integration (v0.5.1) — surgical, marker-based, idempotent edits
// ---------------------------------------------------------------------------

/// The exact import line both installers add to SidebarLeftContent.qml.
pub const SIDEBAR_IMPORT_LINE: &str = r#"import "./downloadManager""#;
/// The enabled-flag property added next to ii's other policy properties.
pub const POLICY_PROP_LINE: &str =
    "property bool downloadManagerEnabled: Config.options.policies.downloadManager !== 0";
/// Backup suffix for the pristine SidebarLeftContent.qml.
pub const BACKUP_SUFFIX: &str = ".bak-hyprfetch";

/// The tab entry added to `tabButtonList` (line content after indent).
const TAB_ENTRY: &str = r#"...(root.downloadManagerEnabled ? [{"icon": "download", "name": Translation.tr("Downloads")}] : [])"#;
/// The page instance added to `contentChildren`.
const CHILDREN_ENTRY: &str =
    "...(root.downloadManagerEnabled ? [downloadManager.createObject()] : [])";
/// The one-line Component that makes `DownloadManager {}` resolvable.
const COMPONENT_LINE: &str = "Component { id: downloadManager; DownloadManager {} }";

/// Files every widget archive must contain (mirrors widget/downloadManager/).
pub const WIDGET_FILES: [&str; 5] = [
    "DownloadManager.qml",
    "components/DownloadHeader.qml",
    "components/DownloadList.qml",
    "components/DownloadItem.qml",
    "components/DownloadInputBar.qml",
];

/// Outcome of wiring the widget into a SidebarLeftContent.qml source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarEdit {
    /// The file already references the widget — nothing to do.
    AlreadyIntegrated,
    /// The file was understood and edited; carries the new source.
    Edited(String),
    /// The file doesn't look like SidebarLeftContent.qml — leave it alone
    /// and let the caller print manual instructions.
    UnknownFile,
}

/// Wire the "Downloads" tab into `SidebarLeftContent.qml`:
///
/// 1. `import "./downloadManager"` after the last import
/// 2. the `downloadManagerEnabled` policy property after `animeCloset`
/// 3. the tab entry at the end of `tabButtonList`
/// 4. the page instance at the end of `contentChildren`
/// 5. the one-line `Component { id: downloadManager; … }` after the anime
///    component
///
/// Idempotent (a second run is a no-op), and every added line contains
/// `downloadManager` so [`remove_sidebar_edit`] can undo it exactly.
pub fn integrate_sidebar_source(src: &str) -> SidebarEdit {
    if src.contains(COMPONENT_LINE) || src.contains(POLICY_PROP_LINE) {
        return SidebarEdit::AlreadyIntegrated;
    }
    // Structural sanity: only edit files we understand.
    if !src.contains("tabButtonList") || !src.contains("contentChildren") {
        return SidebarEdit::UnknownFile;
    }

    let lines = src.lines_including_last();
    let had_trailing_nl = src.ends_with('\n');
    let total = lines.len();
    let mut out = String::with_capacity(src.len() + 512);
    let mut import_added = false;
    let mut prop_added = false;
    let mut tabs_added = false;
    let mut children_added = false;
    let mut component_added = false;
    let mut skip_next = false;

    for (i, line) in lines.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        let emit = |out: &mut String, l: &str| {
            out.push_str(l);
            if i + 1 < total || had_trailing_nl {
                out.push('\n');
            }
        };

        // 1. Import — right after the LAST import line.
        if !import_added && trimmed.starts_with("import ") {
            let is_last = !lines[i + 1..]
                .iter()
                .any(|l| l.trim_start().starts_with("import "));
            emit(&mut out, line);
            if is_last {
                out.push_str(SIDEBAR_IMPORT_LINE);
                out.push('\n');
                import_added = true;
            }
            continue;
        }

        // 2. The enabled-flag property — right after ii's `animeCloset`.
        if !prop_added && trimmed.starts_with("property bool animeCloset:") {
            emit(&mut out, line);
            out.push_str(indent);
            out.push_str(POLICY_PROP_LINE);
            out.push('\n');
            prop_added = true;
            continue;
        }

        // 3. Tab entry — just before the `]` that closes `tabButtonList`.
        //    The last existing entry may lack a trailing comma (upstream ii
        //    ends the list without one) — add it, or the spread syntax
        //    becomes a QML parse error.
        if !tabs_added && trimmed == "]" && out.contains("property var tabButtonList: [") {
            let prev = &lines[i - 1];
            if !prev.trim_end().ends_with(',') {
                let new_len = out.len() - prev.len() - 1; // drop prev + its newline
                out.truncate(new_len);
                out.push_str(prev);
                out.push_str(",\n");
            }
            let prev_indent = last_entry_indent(&out);
            out.push_str(&prev_indent);
            out.push_str(TAB_ENTRY);
            out.push('\n');
            emit(&mut out, line);
            tabs_added = true;
            continue;
        }

        // 4. Page instance — just before the `]` that closes contentChildren.
        //    Same trailing-comma safety as the tab list above.
        if !children_added && trimmed == "]" && out.contains("contentChildren: [") {
            let prev = &lines[i - 1];
            if !prev.trim_end().ends_with(',') {
                let new_len = out.len() - prev.len() - 1;
                out.truncate(new_len);
                out.push_str(prev);
                out.push_str(",\n");
            }
            let prev_indent = last_entry_indent(&out);
            out.push_str(&prev_indent);
            out.push_str(CHILDREN_ENTRY);
            out.push('\n');
            emit(&mut out, line);
            children_added = true;
            continue;
        }

        // 5. The Component — after the anime component's closing brace.
        if !component_added && trimmed == "Anime {}" {
            emit(&mut out, line);
            if let Some(next) = lines.get(i + 1) {
                if next.trim_start() == "}" {
                    out.push_str(next);
                    out.push('\n');
                    let next_indent = &next[..next.len() - next.trim_start().len()];
                    out.push_str(next_indent);
                    out.push_str(COMPONENT_LINE);
                    out.push('\n');
                    skip_next = true;
                }
            }
            component_added = true;
            continue;
        }

        emit(&mut out, line);
    }

    SidebarEdit::Edited(out)
}

/// Indent of the closest preceding `...(root.` spread line already emitted
/// (the last entry of the list we're appending to).
fn last_entry_indent(emitted: &str) -> String {
    emitted
        .lines()
        .rev()
        .find(|l| l.contains("...(root."))
        .map(|l| l[..l.len() - l.trim_start().len()].to_string())
        .unwrap_or_else(|| "        ".to_string())
}

/// Remove every line we added (matched by their exact prefixes — the same
/// lines [`integrate_sidebar_source`] inserts). Mirrors the POSIX
/// installer's uninstall. Newline-exact: no trailing newline in → none out.
pub fn remove_sidebar_edit(src: &str) -> String {
    let lines = src.lines_including_last();
    let had_trailing_nl = src.ends_with('\n');
    let total = lines.len();
    let mut out = String::with_capacity(src.len());
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let ours = t.starts_with(SIDEBAR_IMPORT_LINE)
            || t.starts_with("property bool downloadManagerEnabled:")
            || t.starts_with(TAB_ENTRY)
            || t.starts_with(CHILDREN_ENTRY)
            || t.starts_with(COMPONENT_LINE);
        if ours {
            continue;
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
// Legacy v0.5.0 bar-widget cleanup
// ---------------------------------------------------------------------------

/// The exact import line the v0.5.0 installer added to bar QML.
pub const LEGACY_IMPORT_LINE: &str = "import qs.modules.downloadManager";
/// Marker comment embedded in the v0.5.0 auto-edit.
pub const LEGACY_MARKER: &str = "HyprFetch download widget";

/// Remove the v0.5.0 auto-added import + widget block from a bar source.
/// Mirrors the POSIX installer's awk, including the 12-line runaway guard.
pub fn remove_legacy_bar_edit(src: &str) -> String {
    let lines = src.lines_including_last();
    let had_trailing_nl = src.ends_with('\n');
    let total = lines.len();
    let mut out = String::with_capacity(src.len());
    let mut in_block = false;
    let mut guard = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with(LEGACY_IMPORT_LINE) {
            continue;
        }
        if line.contains(&format!("// {LEGACY_MARKER} (auto-added")) {
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

/// True when the legacy v0.5.0 bar widget is present anywhere.
fn legacy_present(root: &Path) -> bool {
    if legacy_widget_dir(root).is_dir() {
        return true;
    }
    find_bar_file(root).is_some_and(|bar| {
        std::fs::read_to_string(bar)
            .map(|s| s.contains(LEGACY_MARKER) || s.contains(LEGACY_IMPORT_LINE))
            .unwrap_or(false)
    })
}

/// Remove the legacy bar widget: module dir + bar edit. Best-effort;
/// returns what was cleaned (for the install note).
fn cleanup_legacy(root: &Path) -> Vec<String> {
    let mut cleaned = Vec::new();
    let legacy = legacy_widget_dir(root);
    if legacy.is_dir() {
        match std::fs::remove_dir_all(&legacy) {
            Ok(()) => cleaned.push(format!("removed old bar widget ({})", legacy.display())),
            Err(e) => cleaned.push(format!("could not remove old bar widget: {e}")),
        }
    }
    if let Some(bar) = find_bar_file(root) {
        if let Ok(src) = std::fs::read_to_string(&bar) {
            if src.contains(LEGACY_MARKER) || src.contains(LEGACY_IMPORT_LINE) {
                let backup = bar.with_file_name(format!(
                    "{}{BACKUP_SUFFIX}",
                    bar.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Bar.qml")
                ));
                if !backup.exists() {
                    let _ = std::fs::copy(&bar, &backup);
                }
                let cleaned_src = remove_legacy_bar_edit(&src);
                match std::fs::write(&bar, cleaned_src) {
                    Ok(()) => cleaned.push("removed the old widget from your bar".into()),
                    Err(e) => cleaned.push(format!("could not clean the bar file: {e}")),
                }
            }
        }
    }
    cleaned
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
    let installed = dir.join("DownloadManager.qml").is_file();
    let sidebar = sidebar_content_file(&root);
    let integrated = sidebar
        .is_file()
        .then(|| std::fs::read_to_string(&sidebar).unwrap_or_default())
        .map(|s| s.contains(COMPONENT_LINE) || s.contains(CHILDREN_ENTRY))
        .unwrap_or(false);
    let policy_set = ii_policy_set();
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
        "policy_set": policy_set,
        "sidebar_file": sidebar.to_string_lossy(),
        "widget_dir": dir.to_string_lossy(),
        "legacy_bar_widget_found": legacy_present(&root),
        "version": version,
        "up_to_date": version.as_deref() == Some(env!("CARGO_PKG_VERSION")),
        "reload_hint": "qs -c ii kill; qs -c ii &",
    })
}

/// Is `policies.downloadManager` present in ii's config.json?
fn ii_policy_set() -> bool {
    std::fs::read_to_string(ii_config_file())
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|cfg| {
            cfg.get("policies")
                .and_then(|p| p.get("downloadManager"))
                .and_then(|v| v.as_i64())
        })
        .map(|v| v != 0)
        .unwrap_or(false)
}

/// Set `policies.downloadManager: 1` in ii's config.json (idempotent —
/// only writes when the key is missing or 0). Returns the resulting value.
fn set_ii_policy() -> Result<bool, String> {
    let path = ii_config_file();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(false); // no ii config — the sidebar property defaults on
    };
    let mut cfg: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
    let current = cfg
        .get("policies")
        .and_then(|p| p.get("downloadManager"))
        .and_then(|v| v.as_i64());
    if current == Some(1) {
        return Ok(true);
    }
    let policies = cfg
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", path.display()))?
        .entry("policies")
        .or_insert_with(|| serde_json::json!({}));
    policies
        .as_object_mut()
        .ok_or_else(|| "policies is not a JSON object".to_string())?
        .insert("downloadManager".into(), serde_json::json!(1));
    let pretty =
        serde_json::to_string_pretty(&cfg).map_err(|e| format!("serialize config: {e}"))?;
    std::fs::write(&path, pretty + "\n").map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(true)
}

/// Install (or update) the widget: download the tarball from the channel,
/// extract the `downloadManager/` tree safely, wire the sidebar tab, set
/// the ii policy, remove any legacy bar widget. Every path involved lives
/// under `$HOME` — no privileges needed.
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
    for f in WIDGET_FILES {
        if !staging.join(f).is_file() {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!("archive is missing downloadManager/{f}"));
        }
    }

    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&staging, &dest).map_err(|e| format!("activate widget: {e}"))?;

    let mut notes: Vec<String> = cleanup_legacy(&root);

    // Wire the "Downloads" tab (idempotent, backup kept, unknown files
    // untouched with manual instructions returned in `note`).
    let mut integrated = false;
    let sidebar = sidebar_content_file(&root);
    if sidebar.is_file() {
        let src = std::fs::read_to_string(&sidebar).unwrap_or_default();
        match integrate_sidebar_source(&src) {
            SidebarEdit::AlreadyIntegrated => integrated = true,
            SidebarEdit::Edited(next) => {
                let backup = sidebar.with_file_name(format!(
                    "{}{BACKUP_SUFFIX}",
                    sidebar
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("SidebarLeftContent.qml")
                ));
                if !backup.exists() {
                    let _ = std::fs::copy(&sidebar, &backup);
                }
                match std::fs::write(&sidebar, next) {
                    Ok(()) => integrated = true,
                    Err(e) => notes.push(format!("could not edit sidebar file: {e}")),
                }
            }
            SidebarEdit::UnknownFile => {
                notes.push(format!(
                    "could not auto-edit {} (unknown layout) — add: {SIDEBAR_IMPORT_LINE} + Component {{ id: downloadManager; DownloadManager {{}} }} + a tab entry",
                    sidebar.display()
                ));
            }
        }
    } else {
        notes.push(format!(
            "no SidebarLeftContent.qml under {} — the widget files are installed; add the Downloads tab manually (see the README)",
            root.display()
        ));
    }

    let mut policy_set = false;
    match set_ii_policy() {
        Ok(v) => policy_set = v,
        Err(e) => notes.push(format!("could not set the ii policy: {e}")),
    }

    let sidebar_file = if sidebar.is_file() {
        Some(sidebar.to_string_lossy().to_string())
    } else {
        None
    };
    let state = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "installed_at": now_secs(),
        "sidebar_file": sidebar_file,
        "integrated": integrated,
        "policy_set": policy_set,
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
    body["policy_set"] = serde_json::json!(policy_set);
    body["version"] = serde_json::json!(env!("CARGO_PKG_VERSION"));
    body["legacy_bar_widget_found"] = serde_json::json!(legacy_present(&root));
    body["note"] = serde_json::json!(notes.join(" · "));
    Ok(body)
}

/// Uninstall: remove the module dir, undo the sidebar edit, clean any
/// legacy bar widget, drop state.
pub fn uninstall() -> Result<serde_json::Value, String> {
    let root = qs_root();
    let dest = widget_dir(&root);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("remove widget files: {e}"))?;
    }
    let mut reverted = false;
    let sidebar = sidebar_content_file(&root);
    let sidebar_touched = std::fs::read_to_string(&sidebar)
        .map(|src| {
            src.contains(COMPONENT_LINE)
                || src.contains(POLICY_PROP_LINE)
                || src.contains(SIDEBAR_IMPORT_LINE)
        })
        .unwrap_or(false);
    if sidebar_touched {
        let backup = sidebar.with_file_name(format!(
            "{}{BACKUP_SUFFIX}",
            sidebar
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("SidebarLeftContent.qml")
        ));
        // Prefer the pristine backup: byte-exact restore (also undoes the
        // trailing comma the installer added to the last existing entry).
        if backup.exists() {
            std::fs::copy(&backup, &sidebar).map_err(|e| format!("restore sidebar backup: {e}"))?;
            reverted = true;
        } else if let Ok(src) = std::fs::read_to_string(&sidebar) {
            // Fallback: line-removal (leaves a legal trailing comma at worst).
            let cleaned = remove_sidebar_edit(&src);
            std::fs::write(&sidebar, cleaned).map_err(|e| format!("restore sidebar file: {e}"))?;
            reverted = true;
        }
    }
    let _ = cleanup_legacy(&root);
    let _ = std::fs::remove_file(state_file());
    let mut body = status_json();
    body["installed"] = serde_json::json!(false);
    body["sidebar_reverted"] = serde_json::json!(reverted);
    Ok(body)
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

use axum::extract::State;
use axum::Json;

use crate::error::ApiError;
use crate::AppState;

/// `GET /api/widget/status` — Quickshell sidebar widget state for the
/// Settings → Desktop widget card.
pub async fn widget_status() -> Json<serde_json::Value> {
    Json(status_json())
}

/// `POST /api/widget/install` — download + install + wire the tab. Runs
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

/// `POST /api/widget/uninstall` — remove files + undo the sidebar edit.
pub async fn widget_uninstall() -> Result<Json<serde_json::Value>, ApiError> {
    uninstall().map(Json).map_err(ApiError::InternalError)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Structurally faithful copy of end-4's SidebarLeftContent.qml
    /// (upstream main, 2025) — the file our installer patches.
    const SIDEBAR: &str = r#"import qs.services
import qs.modules.common
import qs.modules.common.widgets
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Qt5Compat.GraphicalEffects
import Qt.labs.synchronizer

Item {
    id: root
    required property var scopeRoot
    property int sidebarPadding: 10
    anchors.fill: parent
    property bool aiChatEnabled: Config.options.policies.ai !== 0
    property bool translatorEnabled: Config.options.sidebar.translator.enable
    property bool animeEnabled: Config.options.policies.weeb !== 0
    property bool animeCloset: Config.options.policies.weeb === 2
    property var tabButtonList: [
        ...(root.aiChatEnabled ? [{"icon": "neurology", "name": Translation.tr("Intelligence")}] : []),
        ...(root.translatorEnabled ? [{"icon": "translate", "name": Translation.tr("Translator")}] : []),
        ...((root.animeEnabled && !root.animeCloset) ? [{"icon": "bookmark_heart", "name": Translation.tr("Anime")}] : [])
    ]
    property int tabCount: swipeView.count

    function focusActiveItem() {
        swipeView.currentItem.forceActiveFocus()
    }

    Keys.onPressed: (event) => {
        if (event.modifiers === Qt.ControlModifier) {
            if (event.key === Qt.Key_PageDown) {
                swipeView.incrementCurrentIndex()
                event.accepted = true;
            }
            else if (event.key === Qt.Key_PageUp) {
                swipeView.decrementCurrentIndex()
                event.accepted = true;
            }
        }
    }

    ColumnLayout {
        anchors {
            fill: parent
            margins: sidebarPadding
        }
        spacing: sidebarPadding

        Toolbar {
            visible: tabButtonList.length > 0
            Layout.alignment: Qt.AlignHCenter
            enableShadow: false
            ToolbarTabBar {
                id: tabBar
                Layout.alignment: Qt.AlignHCenter
                tabButtonList: root.tabButtonList
                currentIndex: swipeView.currentIndex
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            implicitWidth: swipeView.implicitWidth
            implicitHeight: swipeView.implicitHeight
            radius: Appearance.rounding.normal
            color: Appearance.colors.colLayer1

            SwipeView { // Content pages
                id: swipeView
                anchors.fill: parent
                spacing: 10
                currentIndex: tabBar.currentIndex

                clip: true
                layer.enabled: true
                layer.effect: OpacityMask {
                    maskSource: Rectangle {
                        width: swipeView.width
                        height: swipeView.height
                        radius: Appearance.rounding.small
                    }
                }

                contentChildren: [
                    ...(root.aiChatEnabled ? [aiChat.createObject()] : []),
                    ...(root.translatorEnabled ? [translator.createObject()] : []),
                    ...((root.tabButtonList.length === 0 || (!root.aiChatEnabled && !root.translatorEnabled && root.animeCloset)) ? [placeholder.createObject()] : []),
                    ...(root.animeEnabled ? [anime.createObject()] : []),
                ]
            }
        }

        Component {
            id: aiChat
            AiChat {}
        }
        Component {
            id: translator
            Translator {}
        }
        Component {
            id: anime
            Anime {}
        }
        Component {
            id: placeholder
            Item {
                StyledText {
                    anchors.centerIn: parent
                    text: root.animeCloset ? Translation.tr("Nothing") : Translation.tr("Enjoy your empty sidebar...")
                    color: Appearance.colors.colSubtext
                }
            }
        }
    }
}
"#;

    #[test]
    fn integrate_adds_all_five_pieces() {
        let SidebarEdit::Edited(out) = integrate_sidebar_source(SIDEBAR) else {
            panic!("expected Edited");
        };
        assert!(out.contains(SIDEBAR_IMPORT_LINE));
        assert!(out.contains(POLICY_PROP_LINE));
        assert!(out.contains(TAB_ENTRY));
        assert!(out.contains(CHILDREN_ENTRY));
        assert!(out.contains(COMPONENT_LINE));
        // The import lands after the last real import.
        let last_import = out
            .lines()
            .enumerate()
            .filter(|(_, l)| l.starts_with("import Qt.labs.synchronizer"))
            .map(|(i, _)| i)
            .last()
            .unwrap();
        let our_import = out
            .lines()
            .position(|l| l.trim() == SIDEBAR_IMPORT_LINE)
            .unwrap();
        assert_eq!(our_import, last_import + 1);
        // The property lands right after animeCloset.
        let closet = out
            .lines()
            .position(|l| l.contains("property bool animeCloset:"))
            .unwrap();
        assert_eq!(
            out.lines().nth(closet + 1).unwrap().trim_start(),
            POLICY_PROP_LINE
        );
        // The tab entry sits just before the tabButtonList closing bracket.
        let tabs_open = out
            .lines()
            .position(|l| l.contains("property var tabButtonList: ["))
            .unwrap();
        let tab_entry = out.lines().position(|l| l.contains(TAB_ENTRY)).unwrap();
        // The previously-last entry gained its trailing comma (upstream ii
        // omits it — without the comma the spread syntax is a parse error).
        assert!(
            out.lines()
                .nth(tab_entry - 1)
                .unwrap()
                .trim_end()
                .ends_with(','),
            "entry before the inserted one must end with a comma"
        );
        let tabs_close = tabs_open
            + 1
            + out
                .lines()
                .skip(tabs_open + 1)
                .position(|l| l.trim() == "]")
                .unwrap();
        assert_eq!(tab_entry, tabs_close - 1);
        assert_eq!(out.lines().nth(tab_entry + 1).unwrap().trim(), "]");
        // The page instance sits just before the contentChildren closing bracket.
        let children_open = out
            .lines()
            .position(|l| l.contains("contentChildren: ["))
            .unwrap();
        let children_entry = out
            .lines()
            .position(|l| l.contains(CHILDREN_ENTRY))
            .unwrap();
        let children_close = children_open
            + 1
            + out
                .lines()
                .skip(children_open + 1)
                .position(|l| l.trim() == "]")
                .unwrap();
        assert_eq!(children_entry, children_close - 1);
        assert_eq!(out.lines().nth(children_entry + 1).unwrap().trim(), "]");
        // The Component sits right after the anime component's closing brace.
        let anime = out.lines().position(|l| l.trim() == "Anime {}").unwrap();
        assert_eq!(out.lines().nth(anime + 1).unwrap().trim(), "}");
        assert_eq!(out.lines().nth(anime + 2).unwrap().trim(), COMPONENT_LINE);
        // Braces stay balanced and the placeholder component is untouched.
        assert_eq!(out.matches('{').count(), out.matches('}').count());
        assert!(out.contains("id: placeholder"));
    }

    #[test]
    fn integrate_is_idempotent() {
        let SidebarEdit::Edited(once) = integrate_sidebar_source(SIDEBAR) else {
            panic!("expected Edited");
        };
        assert_eq!(
            integrate_sidebar_source(&once),
            SidebarEdit::AlreadyIntegrated
        );
    }

    #[test]
    fn integrate_unknown_file_leaves_it_alone() {
        let src = "import QtQuick\nItem {\n    id: weird\n}\n";
        assert_eq!(integrate_sidebar_source(src), SidebarEdit::UnknownFile);
    }

    #[test]
    fn remove_restores_semantically() {
        let SidebarEdit::Edited(edited) = integrate_sidebar_source(SIDEBAR) else {
            panic!("expected Edited");
        };
        let cleaned = remove_sidebar_edit(&edited);
        // Line-removal cannot undo the trailing comma the installer added to
        // the previously-last entry — that is why uninstall prefers the
        // .bak backup (which IS byte-exact). QML-wise a trailing comma in a
        // JS array literal is legal, so this is still a valid restore.
        let expected = SIDEBAR.replacen(": [])\n", ": []),\n", 1);
        assert_eq!(cleaned, expected);
    }

    #[test]
    fn remove_is_noop_on_clean_file() {
        assert_eq!(remove_sidebar_edit(SIDEBAR), SIDEBAR);
    }

    #[test]
    fn remove_handles_file_without_trailing_newline() {
        let no_nl = SIDEBAR.trim_end_matches('\n');
        let SidebarEdit::Edited(edited) = integrate_sidebar_source(no_nl) else {
            panic!("expected Edited");
        };
        let cleaned = remove_sidebar_edit(&edited);
        let expected = no_nl.replacen(": [])\n", ": []),\n", 1);
        assert_eq!(
            cleaned.trim_end_matches('\n'),
            expected,
            "edit must be fully removable modulo the trailing newline + our comma"
        );
    }

    /// Trimmed but structurally faithful excerpt of end-4's BarContent.qml
    /// with the v0.5.0 legacy auto-edit applied.
    fn legacy_bar() -> String {
        format!(
            r#"import qs.modules.ii.bar.weather
import QtQuick
import QtQuick.Layouts
import qs
import qs.services
import qs.modules.common

Item {{ // Bar content region
    id: root

    RowLayout {{
        id: rightSectionRowLayout
        anchors.fill: parent
        spacing: 5
        layoutDirection: Qt.RightToLeft

        RippleButton {{ // Right sidebar button
            id: rightSidebarButton
            Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
        }}
{LEGACY_IMPORT_LINE}
        // {LEGACY_MARKER} (auto-added — delete this block to remove)
        DownloadWidget {{
            Layout.alignment: Qt.AlignVCenter
        }}
    }}
}}
"#
        )
    }

    #[test]
    fn legacy_bar_edit_is_removed_cleanly() {
        let bar = legacy_bar();
        let cleaned = remove_legacy_bar_edit(&bar);
        assert!(!cleaned.contains(LEGACY_IMPORT_LINE));
        assert!(!cleaned.contains(LEGACY_MARKER));
        assert!(!cleaned.contains("DownloadWidget"));
        assert!(cleaned.contains("rightSidebarButton"));
        // Braces stay balanced after the removal.
        assert_eq!(cleaned.matches('{').count(), cleaned.matches('}').count());
    }

    #[test]
    fn widget_files_list_is_complete() {
        // Every file the repo ships must be listed for archive validation.
        let repo_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../widget/downloadManager");
        for f in WIDGET_FILES {
            assert!(
                Path::new(repo_dir).join(f).is_file(),
                "widget/{f} missing from the repo"
            );
        }
    }

    #[test]
    fn bar_candidates_order() {
        let root = Path::new("/ii");
        let c = bar_candidates(root);
        assert_eq!(c[0], Path::new("/ii/modules/ii/bar/BarContent.qml"));
        assert_eq!(c[3], Path::new("/ii/modules/bar/Bar.qml"));
    }
}
