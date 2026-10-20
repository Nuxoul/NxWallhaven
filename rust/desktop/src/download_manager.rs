//! Native sequential queue. UI-independent; original transfers belong to ImageLoader's cache.
use crate::image_loader::{validate_download_url, ImageLoader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use wh_core::library::{DownloadTask, Library};

type Result<T> = std::result::Result<T, String>;
static MANAGERS: OnceLock<Mutex<HashMap<PathBuf, Weak<DownloadManager>>>> = OnceLock::new();
#[derive(Clone, Serialize, Deserialize)]
struct Destination {
    directory: PathBuf,
    filename: String,
    // Durable publication intent: written before no-replace export, verified on recovery.
    sha256: Option<String>,
    length: Option<u64>,
}
struct State {
    allow_nsfw: bool,
    stopped: bool,
    active: Option<String>,
}
struct Inner {
    db: Library,
    loader: Arc<ImageLoader>,
    destinations: PathBuf,
    state: Mutex<State>,
    changed: Condvar,
}
pub struct DownloadManager {
    inner: Arc<Inner>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl DownloadManager {
    /// One manager/worker per canonical data directory; recovery occurs before its worker starts.
    pub fn new(data_dir: PathBuf, loader: Arc<ImageLoader>) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let data_dir = std::fs::canonicalize(data_dir).map_err(|e| e.to_string())?;
        let mut managers = MANAGERS.get_or_init(Default::default).lock().map_err(err)?;
        if let Some(manager) = managers.get(&data_dir).and_then(Weak::upgrade) {
            if manager.inner.state.lock().map_err(err)?.stopped {
                return Err("下载管理器已关闭".into());
            }
            return Ok(manager);
        }
        let db = Library::open(&data_dir).map_err(err)?;
        let destinations = data_dir.join("download-destinations-v1");
        std::fs::create_dir_all(&destinations).map_err(err)?;
        ordinary_directory(&destinations)?;
        db.recover().map_err(err)?;
        // Unknown purity is restricted too. A persisted grant is deliberately never trusted.
        for task in db.tasks().map_err(err)? {
            if restricted(&task) && matches!(task.state.as_str(), "queued" | "running") {
                db.set_state(&task.id, "paused").map_err(err)?;
            }
        }
        let inner = Arc::new(Inner {
            db,
            loader,
            destinations,
            state: Mutex::new(State {
                allow_nsfw: false,
                stopped: false,
                active: None,
            }),
            changed: Condvar::new(),
        });
        let running = inner.clone();
        let worker = std::thread::Builder::new()
            .name("wallhaven-downloads".into())
            .spawn(move || run(running))
            .map_err(err)?;
        let manager = Arc::new(Self {
            inner,
            worker: Mutex::new(Some(worker)),
        });
        managers.insert(data_dir, Arc::downgrade(&manager));
        Ok(manager)
    }
    /// destination is a directory; the final name is always wallhaven-<id>.<original extension>.
    pub fn enqueue(&self, wall: &wh_core::Wallpaper, destination: &Path) -> Result<()> {
        if !wh_core::library::valid_id(&wall.id) {
            return Err("壁纸 ID 无效".into());
        }
        let url = wall.path.as_deref().ok_or("缺少原图地址")?;
        validate_download_url(url)?;
        let state = self.inner.state.lock().map_err(err)?;
        if state.stopped {
            return Err("下载管理器已关闭".into());
        }
        self.inner.refresh_missing()?;
        if state.active.as_deref() == Some(&wall.id) {
            let current = self.inner.db.get(&wall.id).map_err(err)?;
            return if matches!(current.state.as_str(), "running" | "queued") {
                Ok(())
            } else {
                Err("任务尚未停止，请稍后重试".into())
            };
        }
        // Preserve completed/library dedup and never change a live task's destination.
        let previous = self
            .inner
            .db
            .tasks()
            .map_err(err)?
            .into_iter()
            .find(|t| t.id == wall.id)
            .or_else(|| {
                self.inner
                    .db
                    .library()
                    .ok()?
                    .into_iter()
                    .find(|t| t.id == wall.id)
            });
        if previous
            .as_ref()
            .is_some_and(|t| !matches!(t.state.as_str(), "missing" | "failed" | "cancelled"))
        {
            if previous.as_ref().is_some_and(|t| t.state == "completed") {
                self.inner
                    .db
                    .enqueue(serde_json::to_value(wall).map_err(err)?)
                    .map_err(err)?;
            }
            return Ok(());
        }
        std::fs::create_dir_all(destination).map_err(err)?;
        let directory = std::fs::canonicalize(destination).map_err(err)?;
        ordinary_directory(&directory)?;
        // Reject UNC/network destinations: core's local file URI contract has no host.
        let probe =
            reqwest::Url::from_file_path(directory.join("probe")).map_err(|_| "下载目录无效")?;
        if probe.host_str().is_some() {
            return Err("下载目录必须是本地目录".into());
        }
        let filename = filename(&wall.id, url)?;
        let record = Destination {
            directory,
            filename,
            sha256: None,
            length: None,
        };
        self.inner.write_destination(&wall.id, &record)?;
        // Only public wallpaper metadata goes into SQLite. Library rejects nested credentials.
        let task = self
            .inner
            .db
            .enqueue(serde_json::to_value(wall).map_err(err)?)
            .map_err(err)?;
        if restricted(&task) && !state.allow_nsfw {
            self.inner.db.set_state(&task.id, "paused").map_err(err)?;
        }
        self.inner.changed.notify_all();
        Ok(())
    }
    pub fn tasks(&self) -> Result<Vec<DownloadTask>> {
        let _state = self.inner.state.lock().map_err(err)?;
        self.inner.refresh_missing()?;
        self.inner.db.tasks().map_err(err)
    }
    pub fn library(&self) -> Result<Vec<DownloadTask>> {
        let _state = self.inner.state.lock().map_err(err)?;
        self.inner.refresh_missing()?;
        self.inner.db.library().map_err(err)
    }
    pub fn set_state(&self, id: &str, requested: &str) -> Result<()> {
        let mut state = self.inner.state.lock().map_err(err)?;
        if state.stopped {
            return Err("下载管理器已关闭".into());
        }
        let task = self.inner.db.get(id).map_err(err)?;
        if requested == "queued" && restricted(&task) && !state.allow_nsfw {
            return Err("未授权下载受限壁纸".into());
        }
        if requested == "queued" {
            if task.state == "running" {
                return Ok(());
            }
            while state.active.as_deref() == Some(id) {
                state = self.inner.changed.wait(state).map_err(err)?;
            }
        }
        self.inner.db.set_state(id, requested).map_err(err)?;
        self.inner.changed.notify_all();
        if requested == "cancelled" {
            // Wait without holding the control lock, allowing the live transfer to checkpoint.
            while state.active.as_deref() == Some(id) {
                state = self.inner.changed.wait(state).map_err(err)?;
            }
            if let Some(url) = task.wall.get("path").and_then(|v| v.as_str()) {
                self.inner.loader.discard_download_partial(url)?;
            }
        }
        Ok(())
    }
    /// Permission revocation and worker/export checks share this lock: no restricted commit
    /// can cross a completed revoke. Granting never silently resumes a user's paused tasks.
    pub fn permissions_changed(&self, allow_nsfw: bool) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        state.allow_nsfw = allow_nsfw;
        if !allow_nsfw {
            if let Ok(tasks) = self.inner.db.tasks() {
                for task in tasks {
                    if restricted(&task) && matches!(task.state.as_str(), "queued" | "running") {
                        let _ = self.inner.db.set_state(&task.id, "paused");
                    }
                }
            }
        }
        self.inner.changed.notify_all();
    }
    /// Queue-only removal retains the library item and its ownership receipt for dedup.
    /// Deletion is permitted only for the exact generated output with its committed receipt.
    pub fn remove(&self, id: &str, delete_file: bool) -> Result<()> {
        self.remove_inner(id, delete_file, delete_file)
    }
    /// Remove queue and library records atomically, retaining the exported source file.
    pub fn remove_records(&self, id: &str) -> Result<()> {
        self.remove_inner(id, false, true)
    }
    fn remove_inner(&self, id: &str, delete_file: bool, remove_library: bool) -> Result<()> {
        if !wh_core::library::valid_id(id) {
            return Err("壁纸 ID 无效".into());
        }
        let mut state = self.inner.state.lock().map_err(err)?;
        let task = self
            .inner
            .db
            .tasks()
            .map_err(err)?
            .into_iter()
            .find(|t| t.id == id)
            .or_else(|| {
                self.inner
                    .db
                    .library()
                    .ok()?
                    .into_iter()
                    .find(|t| t.id == id)
            });
        let Some(task) = task else {
            return Err("下载记录不存在".into());
        };
        if task.state != "completed" {
            self.inner.db.set_state(id, "cancelled").map_err(err)?;
        }
        while state.active.as_deref() == Some(id) {
            state = self.inner.changed.wait(state).map_err(err)?;
        }
        if let Some(url) = task.wall.get("path").and_then(|v| v.as_str()) {
            self.inner.loader.discard_download_partial(url)?;
        }
        if delete_file && matches!(task.state.as_str(), "completed" | "missing") {
            let record = self.inner.read_destination(&task)?;
            let path = checked_output(&task, &record)?;
            match std::fs::symlink_metadata(&path) {
                Ok(meta) => {
                    if !meta.is_file() || is_link(&meta) {
                        return Err("下载文件已被替换，拒绝删除".into());
                    }
                    let (length, hash) = file_digest(&path)?;
                    if record.length != Some(length) || record.sha256.as_deref() != Some(&hash) {
                        return Err("文件不再是此任务保存的原图，拒绝删除".into());
                    }
                    std::fs::remove_file(path).map_err(err)?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(err(e)),
            }
        }
        self.inner.db.remove(id, remove_library).map_err(err)?;
        if remove_library
            || !self
                .inner
                .db
                .library()
                .map_err(err)?
                .iter()
                .any(|t| t.id == id)
        {
            remove_receipt(&self.inner.destinations.join(format!("{id}.json")))?;
        }
        self.inner.changed.notify_all();
        Ok(())
    }
    pub fn shutdown(&self) {
        if let Ok(mut state) = self.inner.state.lock() {
            state.stopped = true;
            if let Ok(tasks) = self.inner.db.tasks() {
                for task in tasks {
                    if matches!(task.state.as_str(), "running" | "queued") {
                        let _ = self.inner.db.set_state(&task.id, "paused");
                    }
                }
            }
            self.inner.changed.notify_all();
        }
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(worker) = worker.take() {
                // A preview may keep the shared GET alive. Never join that transport on UI.
                let _ = std::thread::Builder::new()
                    .name("wallhaven-download-shutdown".into())
                    .spawn(move || {
                        let _ = worker.join();
                    });
            }
        }
    }
}
impl Drop for DownloadManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}
impl Inner {
    fn read_destination(&self, task: &DownloadTask) -> Result<Destination> {
        if !wh_core::library::valid_id(&task.id) {
            return Err("下载 ID 无效".into());
        }
        ordinary_directory(&self.destinations)?;
        let path = self.destinations.join(format!("{}.json", task.id));
        let meta = std::fs::symlink_metadata(&path).map_err(err)?;
        if !meta.is_file() || is_link(&meta) || meta.len() > 32768 {
            return Err("下载目录记录无效".into());
        }
        let record: Destination =
            serde_json::from_slice(&std::fs::read(path).map_err(err)?).map_err(err)?;
        let url = task
            .wall
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("缺少原图地址")?;
        validate_download_url(url)?;
        if record.filename != filename(&task.id, url)? || !record.directory.is_absolute() {
            return Err("下载目录记录不属于此任务".into());
        }
        Ok(record)
    }
    fn write_destination(&self, id: &str, record: &Destination) -> Result<()> {
        ordinary_directory(&self.destinations)?;
        let path = self.destinations.join(format!("{id}.json"));
        let temp = self.destinations.join(format!("{id}.tmp"));
        remove_receipt(&temp)?;
        let result = (|| {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(err)?;
            f.write_all(&serde_json::to_vec(record).map_err(err)?)
                .map_err(err)?;
            f.sync_all().map_err(err)?;
            drop(f);
            if path.exists() {
                let meta = std::fs::symlink_metadata(&path).map_err(err)?;
                if !meta.is_file() || is_link(&meta) {
                    return Err("下载目录记录无效".into());
                }
            }
            atomic_replace(&temp, &path)
        })();
        let _ = std::fs::remove_file(temp);
        result
    }
    fn refresh_missing(&self) -> Result<()> {
        for task in self.db.library().map_err(err)? {
            if task.state != "completed" {
                continue;
            }
            // A deleted destination directory is missing too; access-denied and
            // changed junctions must never be reported as missing/retryable.
            let record = self.read_destination(&task)?;
            let path = owned_output(&task, &record)?;
            match std::fs::symlink_metadata(&record.directory) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    self.db.mark_missing(&task.id).map_err(err)?;
                    continue;
                }
                Err(e) => return Err(err(e)),
                Ok(_) => validate_directory(&record.directory)?,
            }
            match std::fs::symlink_metadata(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    self.db.mark_missing(&task.id).map_err(err)?
                }
                Err(e) => return Err(err(e)),
                Ok(meta) if !meta.is_file() || is_link(&meta) => {
                    return Err("下载文件已被替换".into())
                }
                Ok(_) => (),
            }
        }
        Ok(())
    }
    fn transfer(&self, task: &DownloadTask) -> Result<()> {
        let mut record = self.read_destination(task)?;
        let path = record.directory.join(&record.filename);
        validate_directory(&record.directory)?;
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => {
                if !meta.is_file()
                    || is_link(&meta)
                    || record.length.is_none()
                    || record.sha256.is_none()
                {
                    return Err("文件已存在，未覆盖".into());
                }
                let (length, hash) = file_digest(&path)?;
                if record.length != Some(length) || record.sha256.as_deref() != Some(&hash) {
                    return Err("文件已存在且不匹配下载收据，未覆盖".into());
                }
                let state = self.state.lock().map_err(err)?;
                let current = self.db.get(&task.id).map_err(err)?;
                if state.stopped
                    || current.state != "running"
                    || (restricted(&current) && !state.allow_nsfw)
                {
                    return Ok(());
                }
                let uri =
                    reqwest::Url::from_file_path(&path).map_err(|_| "无法生成本地文件 URI")?;
                self.db
                    .progress(&task.id, length, Some(length))
                    .map_err(err)?;
                return self.db.complete(&task.id, uri.as_str()).map_err(err);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(err(e)),
        }
        let url = task
            .wall
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("缺少原图地址")?;
        let mut last = None::<Instant>;
        let mut latest = (task.received, task.total);
        let acquired = self.loader.acquire_for_download(url, |received, total| {
            latest = (received, total);
            let Ok(state) = self.state.lock() else {
                return false;
            };
            let Ok(current) = self.db.get(&task.id) else {
                return false;
            };
            if state.stopped
                || current.state != "running"
                || (restricted(&current) && !state.allow_nsfw)
            {
                return false;
            }
            if last.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) {
                last = Some(Instant::now());
                if self.db.progress(&task.id, received, total).is_err() {
                    return false;
                }
            }
            true
        });
        let _state = self.state.lock().map_err(err)?;
        let current = self.db.get(&task.id).map_err(err)?;
        // A terminal checkpoint includes bytes since the last throttled DB update.
        self.db
            .progress(&task.id, latest.0, latest.1)
            .map_err(err)?;
        let data = acquired?;
        if _state.stopped
            || current.state != "running"
            || (restricted(&current) && !_state.allow_nsfw)
        {
            return Ok(());
        }
        let Some(data) = data else {
            self.db.pause(&task.id).map_err(err)?;
            return Ok(());
        };
        // Keep the control lock through no-replace commit and receipt/DB completion. Revoke,
        // pause, cancel and remove cannot observe half an export or delete arbitrary URIs.
        validate_directory(&record.directory)?;
        record.length = Some(data.bytes.len() as u64);
        record.sha256 = Some(format!("{:x}", Sha256::digest(data.bytes.as_slice())));
        // Persist intent first: a crash after publication but before SQLite completion
        // is reconciled by exact length/digest, without another GET or replacement.
        self.write_destination(&task.id, &record)?;
        crate::image_disk_cache::save_copy(&data.bytes, &path, |_, _| {})?;
        let uri = reqwest::Url::from_file_path(&path).map_err(|_| "无法生成本地文件 URI")?;
        self.db
            .progress(
                &task.id,
                data.bytes.len() as u64,
                Some(data.bytes.len() as u64),
            )
            .map_err(err)?;
        self.db.complete(&task.id, uri.as_str()).map_err(err)
    }
}
fn run(inner: Arc<Inner>) {
    loop {
        let mut state = match inner.state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        let task = loop {
            if state.stopped {
                return;
            }
            let tasks = inner.db.tasks().unwrap_or_default();
            let mut next = None;
            for task in tasks {
                if task.state == "queued" {
                    if restricted(&task) && !state.allow_nsfw {
                        let _ = inner.db.set_state(&task.id, "paused");
                    } else if inner.db.claim(&task.id).unwrap_or(false) {
                        next = Some(task);
                        break;
                    }
                }
            }
            if let Some(task) = next {
                break task;
            }
            state = match inner.changed.wait(state) {
                Ok(s) => s,
                Err(_) => return,
            };
        };
        state.active = Some(task.id.clone());
        drop(state);
        let result = inner.transfer(&task);
        let Ok(mut state) = inner.state.lock() else {
            return;
        };
        if result.is_err() {
            let _ = inner.db.fail(&task.id);
        }
        if let Ok(current) = inner.db.get(&task.id) {
            if current.state == "cancelled" {
                if let Some(url) = current.wall.get("path").and_then(|v| v.as_str()) {
                    let _ = inner.loader.discard_download_partial(url);
                }
            }
        }
        state.active = None;
        inner.changed.notify_all();
    }
}
pub(super) fn restricted(task: &DownloadTask) -> bool {
    !task
        .wall
        .get("purity")
        .and_then(|v| v.as_str())
        .is_some_and(|p| p.eq_ignore_ascii_case("sfw"))
}
fn filename(id: &str, url: &str) -> Result<String> {
    let url = reqwest::Url::parse(url).map_err(err)?;
    let ext = url
        .path()
        .rsplit('.')
        .next()
        .filter(|s| matches!(*s, "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif"))
        .ok_or("原图扩展名无效")?;
    Ok(format!("wallhaven-{id}.{ext}"))
}
fn checked_output(task: &DownloadTask, record: &Destination) -> Result<PathBuf> {
    validate_directory(&record.directory)?;
    owned_output(task, record)
}
fn validate_directory(path: &Path) -> Result<()> {
    ordinary_directory(path)?;
    if std::fs::canonicalize(path).map_err(err)? != path {
        return Err("下载目录已改变".into());
    }
    Ok(())
}
fn owned_output(task: &DownloadTask, record: &Destination) -> Result<PathBuf> {
    let path = record.directory.join(&record.filename);
    let expected = reqwest::Url::from_file_path(&path).map_err(|_| "下载路径无效")?;
    if expected.host_str().is_some()
        || task.uri.as_deref() != Some(expected.as_str())
        || record.sha256.is_none()
        || record.length.is_none()
    {
        return Err("文件不属于此下载任务".into());
    }
    Ok(path)
}
fn ordinary_directory(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path).map_err(err)?;
    if !meta.is_dir() || is_link(&meta) {
        return Err("目录不是普通本地目录".into());
    }
    Ok(())
}
fn is_link(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
fn file_digest(path: &Path) -> Result<(u64, String)> {
    let mut f = File::open(path).map_err(err)?;
    let mut sha = Sha256::new();
    let mut length = 0;
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).map_err(err)?;
        if n == 0 {
            break;
        }
        length += n as u64;
        sha.update(&buf[..n]);
    }
    Ok((length, format!("{:x}", sha.finalize())))
}
fn remove_receipt(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !is_link(&meta) => std::fs::remove_file(path).map_err(err),
        Ok(_) => Err("下载目录记录不是普通文件".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(err(e)),
    }
}
fn atomic_replace(from: &Path, to: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<_> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<_> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8)
        } == 0
        {
            return Err(err(std::io::Error::last_os_error()));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to).map_err(err)
    }
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[cfg(test)]
#[path = "download_manager_tests.rs"]
mod tests;
