//! FreeDesktop icon-theme resolution for Linux: parse `index.theme` INI files,
//! walk the theme inheritance chain, and resolve a requested icon *name*
//! (e.g. `folder`, `drive-harddisk`, `text-x-generic`, `image-x-generic`) to an
//! absolute SVG/PNG path. No external crates; parses the INI directly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
struct ThemeHeader {
    inherits: Vec<String>,
    /// `size/scale/context` → `directory-name`, order preserved for lookup order.
    dirs: Vec<(u16, u16, String, String)>, // (size, scale, context, dir_path)
}

#[derive(Debug, Clone)]
pub struct FreedesktopTheme {
    /// Name of the base theme (e.g. `hicolor`).
    pub name: String,
    /// `icon-name` → absolute path, resolved once at load time.
    icons: HashMap<String, PathBuf>,
    /// Fallback base name resolved from inheritance chain.
    pub default_fallback: Option<String>,
}

/// Search roots, in priority order (per XDG Base Directory spec).
fn icon_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        roots.push(PathBuf::from(data_home).join("icons"));
    } else if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".local/share/icons"));
    }
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".icons"));
    }
    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for dir in std::env::split_paths(&data_dirs) {
            roots.push(dir.join("icons"));
        }
    } else {
        roots.push(PathBuf::from("/usr/share/icons"));
        roots.push(PathBuf::from("/usr/local/share/icons"));
    }
    roots
}

/// Directory names the FreeDesktop spec guarantees will be scanned for any size.
const ICON_EXTS: &[&str] = &["svg", "png"];

/// Parse one `index.theme` file. Returns `None` if the file is missing/malformed.
fn parse_index_theme(path: &Path) -> Option<ThemeHeader> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut inherits = Vec::new();
    let mut dirs = Vec::new();
    let mut in_icon_theme = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_icon_theme = line == "[Icon Theme]";
            if !in_icon_theme && line.starts_with('[') && line.ends_with(']') {
                // Directory section: `[size/scale/context]`.
                let inner = &line[1..line.len() - 1];
                let parts: Vec<_> = inner.split('/').collect();
                if parts.len() == 3 {
                    if let (Ok(size), Ok(scale)) =
                        (parts[0].parse::<u16>(), parts[1].parse::<u16>())
                    {
                        // Path will arrive on the `Directories=` line of `[Icon Theme]`.
                        dirs.push((size, scale, parts[2].to_string(), String::new()));
                    }
                }
            }
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let val = val.trim();
        if in_icon_theme {
            match key.trim() {
                "Inherits" => {
                    inherits = val.split(',').map(|s| s.trim().to_string()).collect();
                }
                "Directories" => {
                    // `Directories=16x16/apps,22x22/apps,...` — we don't need the
                    // sizes (path lookup walks the tree), only the dir names.
                    // We *do* honor the order for lookup preference.
                    for dir in val.split(',') {
                        let dir = dir.trim();
                        if let Some((_size, _scale, _context, slot)) = dirs
                            .iter_mut()
                            .find(|(_, _, _, path)| path.is_empty() || path.ends_with(dir))
                        {
                            let _ = slot;
                        }
                    }
                    // Rebuild `dirs` preserving the order from `Directories=`.
                    dirs = val
                        .split(',')
                        .filter_map(|d| {
                            let d = d.trim();
                            let parts: Vec<_> = d.split('/').collect();
                            if parts.len() == 2 {
                                let (size_str, ctx) = (parts[0], parts[1].to_string());
                                let size = size_str
                                    .split('x')
                                    .next()
                                    .and_then(|s| s.parse::<u16>().ok())
                                    .unwrap_or(0);
                                Some((size, 1, ctx, d.to_string()))
                            } else if parts.len() == 3 {
                                let scale = parts[1].parse::<u16>().unwrap_or(1);
                                Some((0, scale, parts[2].to_string(), d.to_string()))
                            } else {
                                None
                            }
                        })
                        .collect();
                }
                _ => {}
            }
        }
    }
    Some(ThemeHeader { inherits, dirs })
}

/// Walk every candidate icon-theme directory and index every icon we find.
fn index_theme_dir(root: &Path, name: &str) -> Option<(HashMap<String, PathBuf>, Vec<String>)> {
    let theme_root = root.join(name);
    let header = parse_index_theme(&theme_root.join("index.theme"))?;
    let mut icons: HashMap<String, PathBuf> = HashMap::new();
    for (_, _, _, dir) in &header.dirs {
        let dir_path = theme_root.join(dir);
        let Ok(entries) = std::fs::read_dir(&dir_path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !ICON_EXTS.contains(&ext) {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // Prefer SVG over PNG.
            let replace = match icons.get(stem) {
                Some(existing) => {
                    existing.extension().and_then(|e| e.to_str()) != Some("svg") && ext == "svg"
                }
                None => true,
            };
            if replace {
                icons.insert(stem.to_string(), path);
            }
        }
    }
    // Follow `Inherits=` transitively so we still resolve `folder` from `hicolor`
    // when the primary theme doesn't define it.
    let mut inherited: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    seen.insert(name.to_string());
    for parent in &header.inherits {
        if !seen.insert(parent.clone()) {
            continue;
        }
        if let Some((parent_icons, _)) = index_theme_dir(root, parent) {
            for (k, v) in parent_icons {
                icons.entry(k).or_insert(v);
            }
            inherited.push(parent.clone());
        }
    }
    if name != "hicolor" && !seen.contains("hicolor") {
        if let Some((hicolor_icons, _)) = index_theme_dir(root, "hicolor") {
            for (k, v) in hicolor_icons {
                icons.entry(k).or_insert(v);
            }
            inherited.push("hicolor".to_string());
        }
    }
    Some((icons, inherited))
}

/// Try to load a FreeDesktop theme by name. Returns `None` if not installed.
pub fn load_theme(name: &str) -> Option<FreedesktopTheme> {
    for root in icon_search_roots() {
        if !root.join(name).is_dir() {
            continue;
        }
        if let Some((icons, _inherits)) = index_theme_dir(&root, name) {
            if icons.is_empty() {
                continue;
            }
            return Some(FreedesktopTheme {
                name: name.to_string(),
                icons,
                default_fallback: Some("hicolor".to_string()),
            });
        }
    }
    None
}

/// Best-effort default: the first of (Papirus, Adwaita, hicolor) that resolves.
pub fn load_default() -> Option<FreedesktopTheme> {
    for name in ["Papirus", "Papirus-Dark", "Adwaita", "breeze", "hicolor"] {
        if let Some(t) = load_theme(name) {
            return Some(t);
        }
    }
    None
}

impl FreedesktopTheme {
    /// Resolve a FreeDesktop icon *name* (e.g. `folder`, `text-x-generic`,
    /// `drive-harddisk`) to an absolute SVG/PNG path.
    pub fn resolve(&self, name: &str) -> Option<&Path> {
        self.icons.get(name).map(PathBuf::as_path)
    }

    /// Map a file extension to the conventional FreeDesktop icon name. Kept
    /// deliberately conservative: known types get specific icons; unknown types
    /// get `text-x-generic` (the FreeDesktop standard fallback for regular files).
    pub fn for_extension(ext: &str) -> &'static str {
        match ext.to_ascii_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "svg" => {
                "image-x-generic"
            }
            "mp4" | "mkv" | "mov" | "avi" | "webm" | "flv" | "m4v" => "video-x-generic",
            "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "opus" => "audio-x-generic",
            "pdf" => "application-pdf",
            "doc" | "docx" | "odt" | "rtf" => "x-office-document",
            "xls" | "xlsx" | "ods" => "x-office-spreadsheet",
            "ppt" | "pptx" | "odp" => "x-office-presentation",
            "zip" | "tar" | "gz" | "7z" | "rar" | "xz" | "bz2" => "package-x-generic",
            "txt" | "md" | "rst" | "log" => "text-x-generic",
            "rs" | "py" | "js" | "ts" | "c" | "cpp" | "h" | "java" | "go" => "text-x-script",
            "html" | "htm" => "text-html",
            "json" | "toml" | "yaml" | "yml" | "xml" => "text-x-generic",
            "exe" | "msi" | "deb" | "rpm" | "appimage" => "application-x-executable",
            "ttf" | "otf" | "woff" | "woff2" => "font-x-generic",
            _ => "text-x-generic",
        }
    }

    /// FreeDesktop icon name for a directory.
    pub fn for_directory() -> &'static str {
        "folder"
    }

    /// FreeDesktop icon name for a removable drive.
    pub fn for_removable() -> &'static str {
        "drive-removable-media"
    }

    /// FreeDesktop icon name for a fixed disk.
    pub fn for_disk() -> &'static str {
        "drive-harddisk"
    }
}
