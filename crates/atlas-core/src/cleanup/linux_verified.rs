//! Linux counterpart of `atlas_win_space::cleanup`. A candidate pins the
//! (device, inode) of the file and of every ancestor directory. Deletion re-walks
//! the path with `openat(O_NOFOLLOW)` from `/`, requires every identity to match,
//! and unlinks the name relative to the verified parent descriptor, so a swapped
//! directory or symlink can never redirect the delete.
use atlas_model::cleanup::{AncestorIdentity, CleanupCandidate, FileIdentity};
use rustix::fs::{self as rfs, AtFlags, FileType, Mode, OFlags, Stat};
use std::{
    ffi::OsString,
    io,
    os::fd::OwnedFd,
    path::{Component, Path, PathBuf},
};

/// Seconds between 1601-01-01 (FILETIME epoch used by the cleanup policy) and 1970.
const FILETIME_UNIX_OFFSET: u64 = 11_644_473_600;

fn scope_changed() -> io::Error {
    io::Error::other("cleanup_scope_changed")
}

fn inode_id(ino: u64) -> [u8; 16] {
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&ino.to_le_bytes());
    id
}

fn filetime(stat: &Stat) -> u64 {
    let secs = u64::try_from(stat.st_mtime).unwrap_or(0);
    #[allow(clippy::useless_conversion)] // reason: st_mtime_nsec is signed on some targets
    let nanos = u64::try_from(stat.st_mtime_nsec).unwrap_or(0);
    (secs + FILETIME_UNIX_OFFSET) * 10_000_000 + nanos / 100
}

fn open_dir(parent: Option<&OwnedFd>, name: &Path) -> io::Result<OwnedFd> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let fd = match parent {
        Some(parent) => rfs::openat(parent, name, flags, Mode::empty()),
        None => rfs::open(name, flags, Mode::empty()),
    };
    fd.map_err(io::Error::from)
}

/// Opens every ancestor of `path` without following symlinks and returns the
/// parent descriptor, the ancestor identities and the leaf name.
fn walk(path: &Path) -> io::Result<(OwnedFd, Vec<AncestorIdentity>, OsString)> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(io::Error::other("cleanup_unsafe_path"));
    }
    let names: Vec<_> = components
        .map(|c| match c {
            Component::Normal(name) => Ok(name.to_owned()),
            _ => Err(io::Error::other("cleanup_unsafe_path")),
        })
        .collect::<io::Result<_>>()?;
    let Some((leaf, parents)) = names.split_last() else {
        return Err(io::Error::other("cleanup_unsafe_path"));
    };
    let mut dir = open_dir(None, Path::new("/"))?;
    let mut current = PathBuf::from("/");
    let mut ancestors = Vec::with_capacity(parents.len() + 1);
    let root = rfs::fstat(&dir)?;
    ancestors.push(AncestorIdentity {
        path: current.clone(),
        volume: root.st_dev,
        id: inode_id(root.st_ino),
    });
    for name in parents {
        dir = open_dir(Some(&dir), Path::new(name))?;
        current.push(name);
        let stat = rfs::fstat(&dir)?;
        ancestors.push(AncestorIdentity {
            path: current.clone(),
            volume: stat.st_dev,
            id: inode_id(stat.st_ino),
        });
    }
    Ok((dir, ancestors, leaf.clone()))
}

fn leaf_identity(
    parent: &OwnedFd,
    name: &OsString,
    ancestors: &[AncestorIdentity],
) -> io::Result<FileIdentity> {
    let stat = rfs::statat(parent, Path::new(name), AtFlags::SYMLINK_NOFOLLOW)?;
    // Regular files only, never symlinks, directories or other hardlinked names,
    // and never across a mount boundary at the last level.
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_nlink != 1
        || ancestors.last().is_none_or(|a| a.volume != stat.st_dev)
    {
        return Err(io::Error::other("cleanup_unsafe_link"));
    }
    Ok(FileIdentity {
        volume: stat.st_dev,
        id: inode_id(stat.st_ino),
        bytes: u64::try_from(stat.st_size).unwrap_or(0),
        modified: filetime(&stat),
        links: u32::try_from(stat.st_nlink).unwrap_or(u32::MAX),
    })
}

pub fn inspect(path: &Path) -> io::Result<CleanupCandidate> {
    let (parent, ancestors, name) = walk(path)?;
    let identity = leaf_identity(&parent, &name, &ancestors)?;
    Ok(CleanupCandidate {
        path: path.to_owned(),
        identity,
        ancestors,
    })
}

/// Deletes only the reviewed file: fails if any ancestor or the file itself was
/// replaced, modified or gained another hardlink since `inspect`.
pub fn delete_verified(expected: &CleanupCandidate) -> io::Result<u64> {
    let (parent, ancestors, name) = walk(&expected.path)?;
    if ancestors != expected.ancestors {
        return Err(scope_changed());
    }
    if leaf_identity(&parent, &name, &ancestors)? != expected.identity {
        return Err(scope_changed());
    }
    rfs::unlinkat(&parent, Path::new(&name), AtFlags::empty())?;
    Ok(expected.identity.bytes)
}

/// (total, available to this user) bytes of the filesystem holding `path`.
pub fn capacity(path: &Path) -> Option<(u64, u64)> {
    let stat = rfs::statvfs(path).ok()?;
    Some((
        stat.f_blocks.saturating_mul(stat.f_frsize),
        stat.f_bavail.saturating_mul(stat.f_frsize),
    ))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // reason: test fixtures
mod tests {
    use super::*;

    #[test]
    fn changed_leaf_and_ancestor_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("old");
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("file");
        std::fs::write(&file, b"original").unwrap();
        let expected = inspect(&file).unwrap();
        std::fs::rename(&dir, root.path().join("saved")).unwrap();
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(&file, b"sentinel").unwrap();
        assert!(delete_verified(&expected).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"sentinel");
        assert!(root.path().join("saved/file").exists());
    }

    #[test]
    fn hardlinks_and_symlinks_are_refused_and_only_reviewed_leaf_is_deleted() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        let other = root.path().join("other");
        std::fs::write(&file, b"temporary").unwrap();
        let expected = inspect(&file).unwrap();
        std::fs::hard_link(&file, &other).unwrap();
        assert!(delete_verified(&expected).is_err());
        std::fs::remove_file(&other).unwrap();
        assert_eq!(delete_verified(&expected).unwrap(), 9);
        assert!(!file.exists());

        let target = root.path().join("target");
        std::fs::write(&target, b"keep").unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(inspect(&link).is_err());
    }

    #[test]
    fn modified_file_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, b"data").unwrap();
        let expected = inspect(&file).unwrap();
        std::fs::write(&file, b"new data").unwrap();
        assert!(delete_verified(&expected).is_err());
        assert!(file.exists());
    }

    #[test]
    fn symlinked_ancestor_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("file"), b"x").unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        assert!(inspect(&alias.join("file")).is_err());
    }

    #[test]
    fn relative_paths_are_refused_and_capacity_is_reported() {
        assert!(inspect(Path::new("relative/file")).is_err());
        assert!(inspect(Path::new("/tmp/../etc/passwd")).is_err());
        let (total, free) = capacity(Path::new("/")).unwrap();
        assert!(total > 0 && free <= total);
    }
}
