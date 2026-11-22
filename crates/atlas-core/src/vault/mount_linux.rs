//! Linux FUSE (fuser) virtual folder mirroring mount.rs's WinFsp callbacks.
//! Only ciphertext is persisted by filesystem callbacks.
#![cfg(target_os = "linux")]
// reason: FUSE callbacks and mount setup run on their own threads, never on the UI thread.
#![allow(clippy::disallowed_methods)]

use super::{live, Entry, Session};
use anyhow::Result;
use fuser::{
    FileAttr, FileType, Filesystem, KernelConfig, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request,
};
use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TTL: Duration = Duration::from_secs(1);
const BLOCK_SIZE: u32 = 4096;
const ROOT_INO: u64 = 1;

struct Shared {
    session: Mutex<Option<Session>>,
    files: AtomicUsize,
    closing: AtomicBool,
    by_inode: Mutex<HashMap<u64, Option<String>>>,
    by_id: Mutex<HashMap<String, u64>>,
    next_inode: AtomicU64,
    activity: Mutex<Instant>,
    uid: u32,
    gid: u32,
}

struct PrivateFs(Arc<Shared>);

pub struct Mounted {
    session: Option<fuser::BackgroundSession>,
    shared: Arc<Shared>,
    path: PathBuf,
    root: PathBuf,
    name: String,
}

fn process_uid_gid() -> (u32, u32) {
    // Safe alternative to libc::getuid/getgid; avoids the workspace's
    // `-D unsafe-code` lint and works on any Linux.
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let mut uid = 0u32;
    let mut gid = 0u32;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:\t") {
            if let Some(v) = rest.split_whitespace().next() {
                uid = v.parse().unwrap_or(0);
            }
        } else if let Some(rest) = line.strip_prefix("Gid:\t") {
            if let Some(v) = rest.split_whitespace().next() {
                gid = v.parse().unwrap_or(0);
            }
        }
    }
    (uid, gid)
}

fn unix_to_systemtime(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn errno(error: &anyhow::Error) -> i32 {
    let msg = format!("{error:#}");
    if msg.contains("vault_missing_entry") {
        libc::ENOENT
    } else if msg.contains("vault_name_exists") {
        libc::EEXIST
    } else if msg.contains("vault_directory_not_empty") {
        libc::ENOTEMPTY
    } else if msg.contains("vault_damaged") {
        libc::EIO
    } else if msg.contains("vault_invalid") || msg.contains("vault_not_file") {
        libc::EINVAL
    } else if msg.contains("vault_in_use") {
        libc::EBUSY
    } else {
        libc::EACCES
    }
}

fn attr_from(shared: &Shared, ino: u64, entry: Option<&Entry>) -> FileAttr {
    let (kind, size, mtime) = match entry {
        Some(e) if e.directory => (FileType::Directory, 0, e.modified()),
        Some(e) => (FileType::RegularFile, e.size, e.modified()),
        None => (FileType::Directory, 0, 0),
    };
    let perm: u16 = match kind {
        FileType::Directory => 0o755,
        _ => 0o644,
    };
    let time = unix_to_systemtime(mtime);
    FileAttr {
        ino,
        size,
        blocks: size.div_ceil(512),
        atime: time,
        mtime: time,
        ctime: time,
        crtime: time,
        kind,
        perm,
        nlink: if kind == FileType::Directory { 2 } else { 1 },
        uid: shared.uid,
        gid: shared.gid,
        rdev: 0,
        flags: 0,
        blksize: BLOCK_SIZE,
    }
}

impl Shared {
    fn session(&self) -> anyhow::Result<MutexGuard<'_, Option<Session>>> {
        let guard = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("vault_internal"))?;
        if guard.is_none() || self.closing.load(Ordering::Acquire) {
            anyhow::bail!("vault_device_not_ready");
        }
        Ok(guard)
    }

    fn session_mut(&self) -> anyhow::Result<MutexGuard<'_, Option<Session>>> {
        self.session()
    }

    fn inode_for(&self, id: &str) -> u64 {
        let mut by_id = self.by_id.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&ino) = by_id.get(id) {
            return ino;
        }
        let ino = self.next_inode.fetch_add(1, Ordering::Relaxed);
        by_id.insert(id.to_owned(), ino);
        drop(by_id);
        self.by_inode
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(ino, Some(id.to_owned()));
        ino
    }

    fn id_for(&self, ino: u64) -> Option<Option<String>> {
        self.by_inode
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ino)
            .cloned()
    }

    fn child_id(&self, parent: u64, name: &OsStr) -> anyhow::Result<Option<String>> {
        let guard = self.session()?;
        let s = guard.as_ref().unwrap();
        let parent_id = match self.id_for(parent) {
            Some(p) => p,
            None => anyhow::bail!("vault_missing_entry"),
        };
        let name_str = name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("vault_invalid_name"))?;
        let child = s
            .children(parent_id.as_deref())
            .into_iter()
            .find(|e| e.name.eq_ignore_ascii_case(name_str));
        Ok(child.map(|e| e.id.clone()))
    }

    fn touch(&self) {
        *self
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    }
}

impl Mounted {
    pub fn start(session: Session) -> std::result::Result<Self, (Session, String)> {
        let root = session.root().to_path_buf();
        let name = session.name().to_owned();
        let path = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(live::NAMESPACE)
            .join(uuid::Uuid::new_v4().simple().to_string());
        if let Err(e) = session.collect_orphans() {
            return Err((session, format!("vault_mount_failed: {e:#}")));
        }
        let (uid, gid) = process_uid_gid();
        let shared = Arc::new(Shared {
            session: Mutex::new(Some(session)),
            files: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
            by_inode: Mutex::new(HashMap::from([(ROOT_INO, None)])),
            by_id: Mutex::new(HashMap::new()),
            next_inode: AtomicU64::new(ROOT_INO + 1),
            activity: Mutex::new(Instant::now()),
            uid,
            gid,
        });
        let result = (|| -> anyhow::Result<fuser::BackgroundSession> {
            let parent = path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("mount path has no parent"))?;
            std::fs::create_dir_all(parent)?;
            remove_stale_mounts(parent);
            std::fs::create_dir_all(&path)?;
            // No AutoUnmount: it needs `allow_other`, which would expose plaintext to
            // other users and fails unless /etc/fuse.conf sets user_allow_other.
            // Drop unmounts; mounts orphaned by a crash are removed on the next start.
            let options = vec![
                fuser::MountOption::FSName("atlas-private".into()),
                fuser::MountOption::Subtype("atlas".into()),
                fuser::MountOption::DefaultPermissions,
                fuser::MountOption::NoDev,
                fuser::MountOption::NoSuid,
            ];
            let fs = PrivateFs(shared.clone());
            Ok(fuser::spawn_mount2(fs, &path, &options)?)
        })();
        match result {
            Ok(bg) => {
                live::register(path.clone(), name.clone());
                Ok(Self {
                    session: Some(bg),
                    shared,
                    path,
                    root,
                    name,
                })
            }
            Err(e) => Err((
                shared
                    .session
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                    .expect("session present on failure"),
                format!("vault_mount_failed: {e:#}"),
            )),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn has_open_files(&self) -> bool {
        self.shared.files.load(Ordering::Acquire) != 0
    }
    pub fn prepare_lock(&self) -> bool {
        let _guard = self
            .shared
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.shared.closing.store(true, Ordering::Release);
        if self.has_open_files() {
            self.shared.closing.store(false, Ordering::Release);
            false
        } else {
            true
        }
    }
    pub fn idle_for(&self) -> Duration {
        self.shared
            .activity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
    }
    pub fn touch(&self) {
        self.shared.touch();
    }
    pub fn into_session(mut self) -> Session {
        self.detach();
        self.shared
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .expect("session present until into_session")
    }
    fn detach(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        if let Some(bg) = self.session.take() {
            // Unmount synchronously so the mount point is gone before we return.
            let _ = bg.join();
        }
        live::unregister(&self.path);
        let _ = std::fs::remove_dir(&self.path);
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        self.detach();
        self.shared
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
}

/// Lazily unmount mount points whose FUSE process is gone (they report ENOTCONN)
/// and remove empty leftovers. Live mounts of other Atlas processes are untouched.
fn remove_stale_mounts(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let stale =
            matches!(std::fs::metadata(&path), Err(e) if e.raw_os_error() == Some(libc::ENOTCONN));
        if stale {
            let _ = std::process::Command::new("fusermount3")
                .args(["-u", "-z"])
                .arg(&path)
                .status();
        }
        // Succeeds only for an empty, unmounted directory.
        let _ = std::fs::remove_dir(&path);
    }
}

pub async fn install_driver() -> Result<()> {
    anyhow::bail!(
        "vault_driver_already_installed: FUSE is provided by the system (pacman -S fuse3)"
    )
}

impl Filesystem for PrivateFs {
    fn init(&mut self, _req: &Request, _config: &mut KernelConfig) -> Result<(), libc::c_int> {
        Ok(())
    }

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        self.0.touch();
        match self.0.child_id(parent, name) {
            Ok(Some(id)) => {
                let ino = self.0.inode_for(&id);
                let attr = {
                    let Ok(guard) = self.0.session() else {
                        reply.error(libc::EIO);
                        return;
                    };
                    let s = guard.as_ref().unwrap();
                    attr_from(&self.0, ino, s.entry(&id))
                };
                reply.entry(&TTL, &attr, 0);
            }
            Ok(None) if parent == ROOT_INO => {
                // No child by that name — unless `name` is a stale inode the
                // kernel still remembers. That case is handled by the kernel.
                reply.error(libc::ENOENT);
            }
            Ok(None) => reply.error(libc::ENOENT),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        self.0.touch();
        let Some(id) = self.0.id_for(ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(guard) = self.0.session() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_ref().unwrap();
        let entry = id.as_deref().and_then(|id| s.entry(id));
        reply.attr(&TTL, &attr_from(&self.0, ino, entry));
    }

    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        self.0.touch();
        let Some(id) = self.0.id_for(ino).flatten() else {
            reply.error(libc::ENOENT);
            return;
        };
        if let Some(size) = size {
            let Ok(mut guard) = self.0.session_mut() else {
                reply.error(libc::EIO);
                return;
            };
            let s = guard.as_mut().unwrap();
            if let Err(e) = s.resize(&id, size) {
                reply.error(errno(&e));
                return;
            }
        }
        let Ok(guard) = self.0.session() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_ref().unwrap();
        let entry = s.entry(&id);
        reply.attr(&TTL, &attr_from(&self.0, ino, entry));
    }

    fn open(&mut self, _req: &Request, ino: u64, _flags: i32, reply: ReplyOpen) {
        self.0.touch();
        if self.0.id_for(ino).is_none() {
            reply.error(libc::ENOENT);
            return;
        }
        self.0.files.fetch_add(1, Ordering::AcqRel);
        reply.opened(ino, 0);
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        self.0.touch();
        let Some(id) = self.0.id_for(ino).flatten() else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(guard) = self.0.session() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_ref().unwrap();
        let mut buffer = vec![0u8; size as usize];
        match s.read_range(&id, offset as u64, &mut buffer) {
            Ok(n) => reply.data(&buffer[..n]),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        self.0.touch();
        let Some(id) = self.0.id_for(ino).flatten() else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(mut guard) = self.0.session_mut() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_mut().unwrap();
        match s.write_at(&id, offset as u64, data) {
            Ok(n) => reply.written(n as u32),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn flush(&mut self, _req: &Request, _ino: u64, _fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        // Every write acknowledges only after encrypted objects and manifest are
        // synced. Nothing to do here — the ciphertext is already durable.
        reply.ok();
    }

    fn release(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.0.touch();
        if let Some(Some(id)) = self.0.id_for(ino) {
            if self.0.files.fetch_sub(1, Ordering::AcqRel) == 1 {
                // Last handle on this inode: drop any retired block set.
                if let Ok(mut guard) = self.0.session_mut() {
                    if let Some(s) = guard.as_mut() {
                        s.release_entry(&id);
                    }
                }
            }
        }
        reply.ok();
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        self.0.touch();
        let Some(parent_id) = self.0.id_for(ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(guard) = self.0.session() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_ref().unwrap();

        let mut entries: Vec<(u64, FileType, String)> = Vec::new();

        // ".." — parent inode (or self for root).
        let dotdot_ino = match &parent_id {
            Some(id) => {
                let parent_of = s
                    .entry(id)
                    .and_then(|e| e.parent.clone())
                    .map(|pid| self.0.inode_for(&pid))
                    .unwrap_or(ROOT_INO);
                parent_of
            }
            None => ROOT_INO,
        };
        // "." — current inode.
        entries.push((ino, FileType::Directory, ".".into()));
        entries.push((dotdot_ino, FileType::Directory, "..".into()));

        for child in s.children(parent_id.as_deref()) {
            let child_ino = self.0.inode_for(&child.id);
            let kind = if child.directory {
                FileType::Directory
            } else {
                FileType::RegularFile
            };
            entries.push((child_ino, kind, child.name.clone()));
        }

        for (i, (child_ino, kind, name)) in
            entries.into_iter().enumerate().skip(offset.max(0) as usize)
        {
            if reply.add(child_ino, (i + 1) as i64, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        self.0.touch();
        let Some(parent_id) = self.0.id_for(parent) else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(name_str) = name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("vault_invalid_name"))
        else {
            reply.error(libc::EINVAL);
            return;
        };
        let Ok(mut guard) = self.0.session_mut() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_mut().unwrap();
        match s.create_entry(parent_id.as_deref(), name_str, false) {
            Ok(id) => {
                let ino = self.0.inode_for(&id);
                self.0.files.fetch_add(1, Ordering::AcqRel);
                let entry = s.entry(&id);
                reply.created(&TTL, &attr_from(&self.0, ino, entry), 0, ino, 0);
            }
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        self.0.touch();
        let Some(parent_id) = self.0.id_for(parent) else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(name_str) = name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("vault_invalid_name"))
        else {
            reply.error(libc::EINVAL);
            return;
        };
        let Ok(mut guard) = self.0.session_mut() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_mut().unwrap();
        match s.create_entry(parent_id.as_deref(), name_str, true) {
            Ok(id) => {
                let ino = self.0.inode_for(&id);
                let entry = s.entry(&id);
                reply.entry(&TTL, &attr_from(&self.0, ino, entry), 0);
            }
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        self.0.touch();
        let child = self.0.child_id(parent, name);
        let child_id = match child {
            Ok(Some(id)) => id,
            Ok(None) => {
                reply.error(libc::ENOENT);
                return;
            }
            Err(e) => {
                reply.error(errno(&e));
                return;
            }
        };
        let Ok(mut guard) = self.0.session_mut() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_mut().unwrap();
        match s.delete_entry(&child_id) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        // Same code path as unlink: `delete_entry` enforces "directory empty".
        self.unlink(_req, parent, name, reply);
    }

    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        self.0.touch();
        let child = self.0.child_id(parent, name);
        let child_id = match child {
            Ok(Some(id)) => id,
            Ok(None) => {
                reply.error(libc::ENOENT);
                return;
            }
            Err(e) => {
                reply.error(errno(&e));
                return;
            }
        };
        let Some(newparent_id) = self.0.id_for(newparent) else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(new_name_str) = newname
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("vault_invalid_name"))
        else {
            reply.error(libc::EINVAL);
            return;
        };
        // RENAME_NOREPLACE (Linux 3.15+) forbids overwriting an existing name.
        let replace = flags & libc::RENAME_NOREPLACE as u32 == 0;
        let Ok(mut guard) = self.0.session_mut() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_mut().unwrap();
        match s.rename_entry(&child_id, newparent_id.as_deref(), new_name_str, replace) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn statfs(&mut self, _req: &Request, _ino: u64, reply: ReplyStatfs) {
        let Ok(guard) = self.0.session() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_ref().unwrap();
        let (blocks, bfree, bavail) =
            match (fs2::total_space(s.root()), fs2::available_space(s.root())) {
                (Ok(total), Ok(free)) => (
                    total.div_ceil(BLOCK_SIZE as u64),
                    free.div_ceil(BLOCK_SIZE as u64),
                    free.div_ceil(BLOCK_SIZE as u64),
                ),
                _ => (0, 0, 0),
            };
        let files_used = s.children(None).len() as u64;
        // Conservative hard cap; matches MAX_ENTRIES in vault.rs.
        let files_total = 100_000u64;
        reply.statfs(
            blocks,
            bfree,
            bavail,
            files_used,
            files_total.saturating_sub(files_used),
            BLOCK_SIZE,
            255, // max filename length
            BLOCK_SIZE,
        );
    }
}
