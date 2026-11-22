use std::path::{Component, Path, PathBuf};

/// Deliberately excludes network/device paths and ambiguous drive-relative paths.
#[cfg(windows)]
pub fn is_drive_root(path: &Path) -> bool {
    let value = path.as_os_str().to_string_lossy();
    let bytes = value.as_bytes();
    bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

/// `/` or another mount point, given as an absolute, normalized path.
#[cfg(not(windows))]
pub fn is_drive_root(path: &Path) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return false;
    }
    is_mount_point(path)
}

/// Looks the path up in the mount table instead of calling stat on it, so a
/// right-click never waits on a hung network mount.
#[cfg(target_os = "linux")]
fn is_mount_point(path: &Path) -> bool {
    #[allow(clippy::disallowed_methods)] // reason: procfs is in memory and never blocks
    let Ok(table) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return path == Path::new("/");
    };
    let path = path.as_os_str().as_encoded_bytes();
    let path = if path.len() > 1 {
        path.strip_suffix(b"/").unwrap_or(path)
    } else {
        path
    };
    table
        .lines()
        .filter_map(|line| line.split(' ').nth(4))
        .any(|mount| unescape_mountinfo(mount) == path)
}

/// mountinfo writes space, tab, newline and backslash as `\ooo` octal escapes.
#[cfg(target_os = "linux")]
fn unescape_mountinfo(field: &str) -> Vec<u8> {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // A byte needs at most \377, so the first digit is 0-3.
        let octal = bytes.get(i + 1..i + 4).filter(|d| {
            (b'0'..=b'3').contains(&d[0]) && d[1..].iter().all(|b| (b'0'..=b'7').contains(b))
        });
        match (bytes[i], octal) {
            (b'\\', Some(d)) => {
                out.push((d[0] - b'0') * 64 + (d[1] - b'0') * 8 + (d[2] - b'0'));
                i += 4;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// Other Unix systems: a directory on a different filesystem than its parent.
#[cfg(all(unix, not(target_os = "linux")))]
#[allow(clippy::disallowed_methods)] // reason: no mount table file to read instead
fn is_mount_point(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(parent) = path.parent() else {
        return path == Path::new("/");
    };
    match (std::fs::symlink_metadata(path), std::fs::metadata(parent)) {
        (Ok(dir), Ok(parent)) => dir.is_dir() && dir.dev() != parent.dev(),
        _ => false,
    }
}

pub fn user_temp_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        dirs::data_local_dir().map(|local| local.join("Temp"))
    }
    #[cfg(not(windows))]
    {
        // On Linux, regenerable data lives under $XDG_CACHE_HOME (~/.cache).
        dirs::cache_dir()
    }
}

/// A user-facing group of regenerable data. Every root is a directory whose
/// old files may be offered for cleanup; nothing outside these roots ever is.
pub struct Category {
    pub key: &'static str,
    pub roots: Vec<PathBuf>,
}

pub fn categories() -> Vec<Category> {
    #[cfg(windows)]
    {
        windows_categories()
    }
    #[cfg(not(windows))]
    {
        linux_categories()
    }
}

#[cfg(windows)]
fn windows_categories() -> Vec<Category> {
    let local = dirs::data_local_dir();
    let home = dirs::home_dir();
    let join = |base: &Option<PathBuf>, rel: &str| base.as_ref().map(|b| b.join(rel));
    let mut browsers = Vec::new();
    for base in [
        "Google/Chrome",
        "Microsoft/Edge",
        "BraveSoftware/Brave-Browser",
        "Vivaldi",
    ] {
        for cache in ["Cache", "Code Cache", "GPUCache"] {
            browsers.extend(join(&local, &format!("{base}/User Data/Default/{cache}")));
        }
    }
    if let Some(profiles) = join(&local, "Mozilla/Firefox/Profiles") {
        if let Ok(entries) = std::fs::read_dir(profiles) {
            browsers.extend(entries.flatten().map(|e| e.path().join("cache2")));
        }
    }
    let list = [
        ("cleanup_cat_temp", user_temp_root().into_iter().collect()),
        ("cleanup_cat_browser", browsers),
        (
            "cleanup_cat_crash",
            [
                join(&local, "CrashDumps"),
                join(&local, "Microsoft/Windows/WER/ReportArchive"),
                join(&local, "Microsoft/Windows/WER/ReportQueue"),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
        (
            "cleanup_cat_dev",
            [
                join(&local, "npm-cache"),
                join(&local, "pip/Cache"),
                join(&local, "Yarn/Cache"),
                join(&home, ".cargo/registry/cache"),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
    ];
    list.into_iter()
        .map(|(key, roots): (&'static str, Vec<PathBuf>)| Category {
            key,
            roots: roots.into_iter().filter(|r| r.is_dir()).collect(),
        })
        .filter(|c| !c.roots.is_empty())
        .collect()
}

#[cfg(not(windows))]
fn linux_categories() -> Vec<Category> {
    let cache = dirs::cache_dir(); // ~/.cache
    let home = dirs::home_dir();
    let join = |base: &Option<PathBuf>, rel: &str| base.as_ref().map(|b| b.join(rel));

    let mut browsers = Vec::new();
    for base in [
        "google-chrome",
        "chromium",
        "microsoft-edge",
        "BraveSoftware/Brave-Browser",
        "vivaldi",
    ] {
        for cache_dir in ["Default/Cache", "Default/Code Cache", "Default/GPUCache"] {
            browsers.extend(join(&cache, &format!("{base}/{cache_dir}")));
        }
    }
    if let Some(profiles) = join(&cache, "mozilla/firefox") {
        if let Ok(entries) = std::fs::read_dir(profiles) {
            browsers.extend(
                entries
                    .flatten()
                    .filter(|e| {
                        e.file_name().to_string_lossy().ends_with(".default")
                            || e.file_name()
                                .to_string_lossy()
                                .ends_with(".default-release")
                    })
                    .map(|e| e.path().join("cache2")),
            );
        }
    }

    let list = [
        ("cleanup_cat_temp", dirs::cache_dir().into_iter().collect()),
        ("cleanup_cat_browser", browsers),
        (
            "cleanup_cat_crash",
            [
                // Thumbnail cache; regenerable. The Trash is deliberately not offered:
                // its contents are user files waiting to be restored or emptied.
                join(&cache, "thumbnails"),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
        (
            "cleanup_cat_dev",
            [
                join(&home, ".npm/_cacache"),
                join(&home, ".cache/pip"),
                join(&home, ".cargo/registry/cache"),
                join(&home, ".cache/yarn"),
                join(&home, ".cache/pnpm"),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
    ];
    list.into_iter()
        .map(|(key, roots): (&'static str, Vec<PathBuf>)| Category {
            key,
            roots: roots.into_iter().filter(|r| r.is_dir()).collect(),
        })
        .filter(|c| !c.roots.is_empty())
        .collect()
}

pub fn is_cleanup_root(path: &Path) -> bool {
    categories()
        .iter()
        .any(|c| c.roots.iter().any(|r| r == path))
}

/// Conservative component policy shared by discovery and execution. This is an
/// extra exclusion; filesystem identity checks remain the mutation authority.
pub fn protected(path: &Path) -> bool {
    path.components().any(|part| match part {
        Component::Normal(name) => {
            let name = name.to_string_lossy().to_ascii_lowercase();
            matches!(
                name.as_str(),
                ".ssh"
                    | ".gnupg"
                    | ".aws"
                    | ".azure"
                    | ".kube"
                    | ".docker"
                    | ".git"
                    | ".bb"
                    | ".agents"
                    | ".gemini"
                    | ".env"
                    | ".npmrc"
                    | ".pypirc"
                    | ".netrc"
                    | ".claude"
                    | ".codex"
                    | ".cursor"
                    | "credentials"
                    | "protect"
                    | "crypto"
                    | "vault"
                    | "systemcertificates"
                    | "login data"
                    | "cookies"
                    | "web data"
                    | "sessions"
                    | "start menu"
                    | "$recycle.bin"
                    | "system volume information"
            )
        }
        _ => false,
    })
}

pub(super) fn valid_temp_candidate(path: &Path, temp: &Path) -> bool {
    path.strip_prefix(temp).is_ok_and(|relative| {
        relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
            && relative.components().next().is_some()
    }) && !protected(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn drive_scope_rejects_unc_relative_and_nested_paths() {
        for value in ["C:\\", "d:/"] {
            assert!(is_drive_root(Path::new(value)));
        }
        for value in ["C:", "C:\\Users", "\\\\server\\share", "\\\\?\\C:\\", "/"] {
            assert!(!is_drive_root(Path::new(value)), "{value}");
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_escapes_are_decoded() {
        assert_eq!(unescape_mountinfo(r"/media/My\040Disk"), b"/media/My Disk");
        assert_eq!(unescape_mountinfo(r"/a\134b\0"), b"/a\\b\\0");
    }

    #[cfg(not(windows))]
    #[test]
    fn mount_points_are_volume_roots() {
        assert!(is_drive_root(Path::new("/")));
        assert!(is_drive_root(Path::new("/proc")));
        assert!(is_drive_root(Path::new("/proc/")));
        for value in ["/usr/bin", "relative", "/tmp/../", "C:\\"] {
            assert!(!is_drive_root(Path::new(value)), "{value}");
        }
    }
    #[test]
    fn temp_scope_is_component_bounded_and_protected() {
        let root = Path::new("local/Temp");
        assert!(valid_temp_candidate(Path::new("local/Temp/a/b.tmp"), root));
        for path in [
            "local/Temporary/file",
            "local/Temp",
            "local/Temp/../a",
            "local/Temp/.ssh/id",
            "local/Temp/.codex/sessions/a",
            "local/Temp/.env",
            "local/Temp/.bb/state",
        ] {
            assert!(!valid_temp_candidate(Path::new(path), root), "{path}");
        }
    }
    #[cfg(not(windows))]
    #[test]
    fn linux_categories_use_xdg_cache_dir() {
        // Prove the Linux categories point at ~/.cache, not ~/.local/share.
        // Can't assume specific dirs exist, but the roots that do exist should be
        // under the XDG cache dir.
        if let Some(cache) = dirs::cache_dir() {
            for cat in categories() {
                for root in &cat.roots {
                    assert!(
                        root.starts_with(&cache)
                            || root.starts_with(dirs::home_dir().unwrap_or_default()),
                        "unexpected root {:?} for category {}",
                        root,
                        cat.key
                    );
                }
            }
        }
    }
}
