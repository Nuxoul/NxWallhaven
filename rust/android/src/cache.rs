//! Marker-owned compressed-original disk cache. No image bytes cross UniFFI.
use crate::{image_format, urls, MobileError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const OWNER: &str = "nxwallhaven-images-v1";
const PREFIX: &str = "nxwh-v1-";
const LIMIT: u64 = 512 * 1024 * 1024;
const MARKER: &str = ".nxwh-images-v1";
type Result<T> = std::result::Result<T, MobileError>;

#[derive(Serialize, Deserialize)]
struct Entry {
    owner: String,
    url_hash: String,
    length: u64,
    sha256: String,
    last_access: u64,
    complete: bool,
    #[serde(default)]
    validator: Option<String>,
    #[serde(default)]
    validator_header: Option<String>,
    #[serde(default)]
    total: Option<u64>,
}
#[derive(Serialize)]
pub(crate) struct Usage {
    bytes: u64,
    files: u64,
    path: String,
}
struct Flight {
    result: Mutex<Option<Result<String>>>,
    ready: Condvar,
    progress: Mutex<(u64, Option<u64>)>,
    readers: Mutex<Vec<Weak<AtomicBool>>>,
}
struct Shared {
    // Shared readers permit independent URLs to download concurrently. Clear is exclusive.
    gate: RwLock<()>,
    disk: Mutex<()>,
    flights: Mutex<HashMap<String, Weak<Flight>>>,
}
static ROOTS: OnceLock<Mutex<HashMap<PathBuf, Weak<Shared>>>> = OnceLock::new();
pub(crate) struct ImageCache {
    root: PathBuf,
    shared: Arc<Shared>,
    http: reqwest::blocking::Client,
    limit: u64,
    #[cfg(test)]
    before_hit_hash: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Continue,
    Pause,
    Cancel,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn digest(s: &str) -> String {
    hex(&Sha256::digest(s.as_bytes()))
}
fn valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        } // junctions/reparse points too
    }
    meta.file_type().is_symlink()
}
fn regular(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) if is_link(&m) || !m.is_file() => Err(MobileError::Cache),
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(MobileError::Cache),
    }
}
fn directory(path: &Path, create: bool) -> Result<()> {
    let mut current = PathBuf::new();
    for c in path.components() {
        if matches!(c, Component::ParentDir) {
            return Err(MobileError::Cache);
        }
        current.push(c);
        // A Windows drive/verbatim prefix is not itself a filesystem path.
        if matches!(c, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(m) if is_link(&m) || !m.is_dir() => return Err(MobileError::Cache),
            Ok(_) => (),
            Err(e) if create && e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|_| MobileError::Cache)?
            }
            Err(_) => return Err(MobileError::Cache),
        }
    }
    Ok(())
}
fn remove(path: &Path) -> Result<()> {
    if regular(path)? {
        fs::remove_file(path).map_err(|_| MobileError::Cache)?;
    }
    Ok(())
}
fn create(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| MobileError::Cache)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| MobileError::Cache)
}

impl ImageCache {
    pub(crate) fn new(cache_dir: &str) -> Result<Self> {
        if cache_dir.trim().is_empty() {
            return Err(crate::invalid("cache directory is empty"));
        }
        let path = PathBuf::from(cache_dir);
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map_err(|_| MobileError::Cache)?
                .join(path)
        };
        directory(&path, true)?;
        let root = path.join("images");
        directory(&root, true)?;
        let root = fs::canonicalize(root).map_err(|_| MobileError::Cache)?;
        if root.to_str().is_none() {
            return Err(MobileError::Cache);
        }
        let mut roots = ROOTS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| MobileError::Cache)?;
        roots.retain(|_, v| v.strong_count() > 0);
        let shared = if let Some(shared) = roots.get(&root).and_then(Weak::upgrade) {
            shared
        } else {
            let marker = root.join(MARKER);
            if regular(&marker)? {
                if fs::metadata(&marker).map_err(|_| MobileError::Cache)?.len()
                    != OWNER.len() as u64
                    || fs::read(&marker).map_err(|_| MobileError::Cache)? != OWNER.as_bytes()
                {
                    return Err(MobileError::Cache);
                }
            } else {
                create(&marker, OWNER.as_bytes())?;
            }
            let shared = Arc::new(Shared {
                gate: RwLock::new(()),
                disk: Mutex::new(()),
                flights: Mutex::new(HashMap::new()),
            });
            roots.insert(root.clone(), Arc::downgrade(&shared));
            shared
        };
        drop(roots);
        let http = reqwest::blocking::Client::builder()
            .user_agent("NxWallhaven/0.1 (image cache)")
            .connect_timeout(Duration::from_secs(10))
            // Blocking reqwest applies this independently to each read operation.
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()
            .map_err(|_| MobileError::Network)?;
        let cache = Self {
            root,
            shared,
            http,
            limit: LIMIT,
            #[cfg(test)]
            before_hit_hash: Mutex::new(None),
        };
        {
            let _gate = cache.shared.gate.read().map_err(|_| MobileError::Cache)?;
            let _disk = cache.shared.disk.lock().map_err(|_| MobileError::Cache)?;
            cache.check_root()?;
            // Crash/paused partials survive construction; validated on use, never cache hits.
            cache.evict(None)?;
        }
        Ok(cache)
    }
    fn check_root(&self) -> Result<()> {
        directory(&self.root, false)?;
        let marker = self.root.join(MARKER);
        if !regular(&marker)?
            || fs::metadata(&marker).map_err(|_| MobileError::Cache)?.len() != OWNER.len() as u64
            || fs::read(marker).map_err(|_| MobileError::Cache)? != OWNER.as_bytes()
        {
            return Err(MobileError::Cache);
        }
        Ok(())
    }
    fn path(&self, hash: &str, suffix: &str) -> PathBuf {
        self.root.join(format!("{PREFIX}{hash}.{suffix}"))
    }
    fn entry(&self, hash: &str) -> Result<Option<Entry>> {
        let path = self.path(hash, "json");
        if !regular(&path)? {
            return Ok(None);
        }
        if fs::metadata(&path).map_err(|_| MobileError::Cache)?.len() > 4096 {
            return Ok(None);
        }
        let entry: Entry =
            match serde_json::from_reader(File::open(path).map_err(|_| MobileError::Cache)?) {
                Ok(e) => e,
                Err(_) => return Ok(None),
            };
        if entry.owner != OWNER || entry.url_hash != hash || !valid_hash(&entry.url_hash) {
            return Ok(None);
        }
        Ok(Some(entry))
    }
    fn entries(&self) -> Result<Vec<(String, Entry)>> {
        let mut entries = Vec::new();
        for file in fs::read_dir(&self.root).map_err(|_| MobileError::Cache)? {
            let file = file.map_err(|_| MobileError::Cache)?;
            let name = file.file_name();
            let name = name.to_string_lossy();
            let Some(hash) = name
                .strip_prefix(PREFIX)
                .and_then(|s| s.strip_suffix(".json"))
            else {
                continue;
            };
            if !valid_hash(hash) {
                continue;
            }
            if let Some(entry) = self.entry(hash)? {
                entries.push((hash.into(), entry));
            }
        }
        Ok(entries)
    }
    fn delete(&self, hash: &str) -> Result<()> {
        // Only a valid per-entry ownership marker permits deletion of these exact siblings.
        if self.entry(hash)?.is_none() {
            return Ok(());
        }
        for suffix in ["img", "part", "json.part", "json"] {
            remove(&self.path(hash, suffix))?;
        }
        Ok(())
    }
    fn update(&self, hash: &str, entry: &Entry) -> Result<()> {
        let part = self.path(hash, "json.part");
        remove(&part)?;
        create(
            &part,
            &serde_json::to_vec(entry).map_err(|_| MobileError::Cache)?,
        )?;
        regular(&self.path(hash, "json"))?;
        // Android/Unix rename replaces atomically, so a crash cannot orphan a valid partial.
        #[cfg(windows)]
        remove(&self.path(hash, "json"))?;
        fs::rename(part, self.path(hash, "json")).map_err(|_| MobileError::Cache)
    }
    fn hit(&self, hash: &str) -> Result<Option<String>> {
        let disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
        self.check_root()?;
        let Some(mut entry) = self.entry(hash)? else {
            return Ok(None);
        };
        if !entry.complete {
            return Ok(None);
        }
        let path = self.path(hash, "img");
        if !regular(&path)? {
            self.delete(hash)?;
            return Ok(None);
        }
        let mut file = File::open(&path).map_err(|_| MobileError::Cache)?;
        // The caller holds the clear gate and owns this URL's flight. Eviction and
        // discard_partial skip active flights, so hashing needs no root-wide disk
        // mutex: slow original verification must not stall other hits or GETs.
        drop(disk);
        #[cfg(test)]
        {
            let probe = self.before_hit_hash.lock().unwrap().take();
            if let Some(probe) = probe {
                probe();
            }
        }
        let mut sha = Sha256::new();
        let mut length = 0u64;
        let mut buf = [0; 64 * 1024];
        loop {
            let n = file.read(&mut buf).map_err(|_| MobileError::Cache)?;
            if n == 0 {
                break;
            }
            sha.update(&buf[..n]);
            length += n as u64;
        }
        drop(file);
        let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
        self.check_root()?;
        if !regular(&path)? {
            self.delete(hash)?;
            return Ok(None);
        }
        if length != entry.length || hex(&sha.finalize()) != entry.sha256 {
            self.delete(hash)?;
            return Ok(None);
        }
        entry.last_access = now();
        self.update(hash, &entry)?;
        Ok(Some(path.to_str().ok_or(MobileError::Cache)?.into()))
    }
    pub(crate) fn acquire(&self, input: &str) -> Result<String> {
        self.acquire_inner(input, None)
    }
    pub(crate) fn acquire_controlled(
        &self,
        input: &str,
        control: &mut dyn FnMut(u64, Option<u64>) -> Control,
    ) -> Result<String> {
        self.acquire_inner(input, Some(control))
    }
    fn acquire_inner(
        &self,
        input: &str,
        mut control: Option<&mut dyn FnMut(u64, Option<u64>) -> Control>,
    ) -> Result<String> {
        let url = urls::image_url(input)?;
        let hash = digest(url.as_str());
        let _gate = self.shared.gate.read().map_err(|_| MobileError::Cache)?;
        let reader = Arc::new(AtomicBool::new(true));
        let (flight, owner) = {
            let mut flights = self.shared.flights.lock().map_err(|_| MobileError::Cache)?;
            flights.retain(|_, v| v.strong_count() > 0);
            if let Some(f) = flights.get(&hash).and_then(Weak::upgrade) {
                f.readers
                    .lock()
                    .map_err(|_| MobileError::Cache)?
                    .push(Arc::downgrade(&reader));
                (f, false)
            } else {
                let f = Arc::new(Flight {
                    result: Mutex::new(None),
                    ready: Condvar::new(),
                    progress: Mutex::new((0, None)),
                    readers: Mutex::new(vec![Arc::downgrade(&reader)]),
                });
                flights.insert(hash.clone(), Arc::downgrade(&f));
                (f, true)
            }
        };
        if !owner {
            loop {
                let progress = *flight.progress.lock().map_err(|_| MobileError::Cache)?;
                if control
                    .as_mut()
                    .is_some_and(|c| c(progress.0, progress.1) != Control::Continue)
                {
                    reader.store(false, Ordering::Release);
                    return Ok(String::new());
                }
                let result = flight.result.lock().map_err(|_| MobileError::Cache)?;
                if let Some(result) = result.as_ref() {
                    return result.clone();
                }
                drop(
                    flight
                        .ready
                        .wait_timeout(result, Duration::from_millis(100))
                        .map_err(|_| MobileError::Cache)?,
                );
            }
        }
        let controlled = control.is_some();
        let mut stopped = Control::Continue;
        let mut poll = |received, total| {
            if let Ok(mut p) = flight.progress.lock() {
                *p = (received, total);
            }
            if stopped == Control::Continue {
                if let Some(c) = control.as_mut() {
                    stopped = c(received, total);
                }
                if stopped != Control::Continue {
                    reader.store(false, Ordering::Release);
                }
            }
            if stopped != Control::Continue {
                let peers = flight
                    .readers
                    .lock()
                    .map(|r| {
                        r.iter()
                            .filter_map(Weak::upgrade)
                            .any(|r| r.load(Ordering::Acquire))
                    })
                    .unwrap_or(false);
                if !peers {
                    return stopped;
                }
            }
            Control::Continue
        };
        let result = self.fetch(&url, &hash, controlled, &mut poll);
        *flight.result.lock().map_err(|_| MobileError::Cache)? = Some(result.clone());
        flight.ready.notify_all();
        if stopped != Control::Continue {
            Ok(String::new())
        } else {
            result
        }
    }
    /// Cancel only an idle, marker-owned partial; never a completed image or active preview.
    pub(crate) fn discard_partial(&self, input: &str) -> Result<()> {
        let url = urls::image_url(input)?;
        let hash = digest(url.as_str());
        let _gate = self.shared.gate.read().map_err(|_| MobileError::Cache)?;
        let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
        self.check_root()?;
        if self
            .shared
            .flights
            .lock()
            .map_err(|_| MobileError::Cache)?
            .get(&hash)
            .is_some_and(|f| f.strong_count() > 0)
        {
            return Ok(());
        }
        if self.entry(&hash)?.is_some_and(|e| !e.complete) {
            self.delete(&hash)?;
        }
        Ok(())
    }
    fn fetch(
        &self,
        url: &reqwest::Url,
        hash: &str,
        controlled: bool,
        poll: &mut dyn FnMut(u64, Option<u64>) -> Control,
    ) -> Result<String> {
        if let Some(path) = self.hit(hash)? {
            let entry = self.entry(hash)?.ok_or(MobileError::Cache)?;
            return if poll(entry.length, Some(entry.length)) == Control::Continue {
                Ok(path)
            } else {
                Ok(String::new())
            };
        }
        {
            let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
            self.check_root()?;
            if self.entry(hash)?.is_none() {
                // Refuse collisions/unowned files; never overwrite unrelated content.
                for suffix in ["json", "json.part", "part", "img"] {
                    if regular(&self.path(hash, suffix))? {
                        return Err(MobileError::Cache);
                    }
                }
                create(
                    &self.path(hash, "json"),
                    &serde_json::to_vec(&Entry {
                        owner: OWNER.into(),
                        url_hash: hash.into(),
                        length: 0,
                        sha256: hex(&Sha256::digest([])),
                        last_access: now(),
                        complete: false,
                        validator: None,
                        validator_header: None,
                        total: None,
                    })
                    .map_err(|_| MobileError::Cache)?,
                )?;
            }
        }
        let result = self.download(url, hash, poll);
        let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
        // Legacy preview errors still clean up. Controlled I/O failures retain a checkpoint.
        if result.is_err() && (!controlled || !matches!(result, Err(MobileError::Network))) {
            self.delete(hash)?;
        } else if result.as_ref().is_ok_and(|p| !p.is_empty()) {
            self.evict(Some(hash))?;
        }
        result
    }
    fn checkpoint(
        &self,
        hash: &str,
        file: &File,
        entry: &mut Entry,
        length: u64,
        sha: &Sha256,
    ) -> Result<()> {
        file.sync_all().map_err(|_| MobileError::Cache)?;
        entry.length = length;
        entry.sha256 = hex(&sha.clone().finalize());
        entry.last_access = now();
        let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
        self.check_root()?;
        self.update(hash, entry)
    }
    fn download(
        &self,
        url: &reqwest::Url,
        hash: &str,
        poll: &mut dyn FnMut(u64, Option<u64>) -> Control,
    ) -> Result<String> {
        use reqwest::header as h;
        let started = Instant::now();
        let mut entry = self.entry(hash)?.ok_or(MobileError::Cache)?;
        let part = self.path(hash, "part");
        let mut sha = Sha256::new();
        let mut length = 0u64;
        let mut buf = [0; 64 * 1024];
        if regular(&part)? && entry.length > 0 && resumable(&entry) {
            let mut prefix = File::open(&part)
                .map_err(|_| MobileError::Cache)?
                .take(entry.length);
            loop {
                let n = prefix.read(&mut buf).map_err(|_| MobileError::Cache)?;
                if n == 0 {
                    break;
                }
                sha.update(&buf[..n]);
                length += n as u64;
            }
            if length != entry.length || hex(&sha.clone().finalize()) != entry.sha256 {
                length = 0;
            }
        }
        if length == 0 {
            sha = Sha256::new();
            remove(&part)?;
        }
        let mut request = self
            .http
            .get(url.clone())
            .header(h::ACCEPT_ENCODING, "identity");
        if length > 0 {
            request = request.header(h::RANGE, format!("bytes={length}-")).header(
                h::IF_RANGE,
                entry.validator.as_deref().ok_or(MobileError::Response)?,
            );
        }
        let mut response = request.send().map_err(|_| MobileError::Network)?;
        if response.url() != url
            || response
                .headers()
                .get(h::CONTENT_ENCODING)
                .is_some_and(|v| v != "identity")
        {
            return Err(MobileError::Response);
        }
        let status = response.status().as_u16();
        if status == 206 && length > 0 {
            let range = response
                .headers()
                .get(h::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(content_range)
                .ok_or(MobileError::Response)?;
            let validator = response
                .headers()
                .get(
                    entry
                        .validator_header
                        .as_deref()
                        .ok_or(MobileError::Response)?,
                )
                .and_then(|v| v.to_str().ok());
            if range.0 != length
                || range.1.checked_add(1) != Some(range.2)
                || entry.total.is_some_and(|t| t != range.2)
                || validator != entry.validator.as_deref()
                || response
                    .content_length()
                    .is_some_and(|n| n != range.2 - length)
            {
                return Err(MobileError::Response);
            }
            entry.total = Some(range.2);
        } else if status == 200 {
            if response.headers().contains_key(h::CONTENT_RANGE) {
                return Err(MobileError::Response);
            }
            // If Range was ignored, discard the prefix before writing the full 200 body.
            length = 0;
            sha = Sha256::new();
            remove(&part)?;
            entry.total = response.content_length();
            let strong = response
                .headers()
                .get(h::ETAG)
                .and_then(|v| v.to_str().ok())
                .filter(|s| strong_etag(s));
            let modified = response
                .headers()
                .get(h::LAST_MODIFIED)
                .and_then(|v| v.to_str().ok())
                .filter(|s| httpdate::parse_http_date(s).is_ok());
            (entry.validator_header, entry.validator) = if let Some(v) = strong {
                (Some("etag".into()), Some(v.into()))
            } else if let Some(v) = modified {
                (Some("last-modified".into()), Some(v.into()))
            } else {
                (None, None)
            };
        } else {
            return Err(if status == 206 {
                MobileError::Response
            } else {
                MobileError::Http { status }
            });
        }
        let mut options = OpenOptions::new();
        options.write(true);
        let mut file = if length > 0 {
            if !regular(&part)? {
                return Err(MobileError::Cache);
            }
            let mut f = options.open(&part).map_err(|_| MobileError::Cache)?;
            f.set_len(length).map_err(|_| MobileError::Cache)?;
            f.seek(SeekFrom::Start(length))
                .map_err(|_| MobileError::Cache)?;
            f
        } else {
            options
                .create_new(true)
                .open(&part)
                .map_err(|_| MobileError::Cache)?
        };
        self.checkpoint(hash, &file, &mut entry, length, &sha)?;
        let mut checkpoint = Instant::now();
        loop {
            match poll(length, entry.total) {
                Control::Pause => {
                    self.checkpoint(hash, &file, &mut entry, length, &sha)?;
                    return Ok(String::new());
                }
                Control::Cancel => {
                    drop(file);
                    let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
                    self.delete(hash)?;
                    return Ok(String::new());
                }
                Control::Continue => (),
            }
            if started.elapsed() >= Duration::from_secs(300) {
                self.checkpoint(hash, &file, &mut entry, length, &sha)?;
                return Err(MobileError::Network);
            }
            let n = match response.read(&mut buf) {
                Ok(n) => n,
                Err(_) => {
                    match poll(length, entry.total) {
                        Control::Cancel => {
                            drop(file);
                            let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
                            self.delete(hash)?;
                            return Ok(String::new());
                        }
                        Control::Pause => {
                            self.checkpoint(hash, &file, &mut entry, length, &sha)?;
                            return Ok(String::new());
                        }
                        Control::Continue => (),
                    }
                    self.checkpoint(hash, &file, &mut entry, length, &sha)?;
                    return Err(MobileError::Network);
                }
            };
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|_| MobileError::Cache)?;
            sha.update(&buf[..n]);
            length = length.checked_add(n as u64).ok_or(MobileError::Response)?;
            if entry.total.is_some_and(|t| length > t) {
                return Err(MobileError::Response);
            }
            if checkpoint.elapsed() >= Duration::from_millis(500) {
                self.checkpoint(hash, &file, &mut entry, length, &sha)?;
                checkpoint = Instant::now();
            }
        }
        file.sync_all().map_err(|_| MobileError::Cache)?;
        drop(file);
        if length == 0 || entry.total.is_some_and(|n| n != length) {
            return Err(MobileError::Response);
        }
        image_format::validate(&part).map_err(|_| MobileError::Response)?;
        // Re-read the disk hash before commit, not just the network stream hash.
        let mut disk_sha = Sha256::new();
        let mut f = File::open(&part).map_err(|_| MobileError::Cache)?;
        loop {
            let n = f.read(&mut buf).map_err(|_| MobileError::Cache)?;
            if n == 0 {
                break;
            }
            disk_sha.update(&buf[..n]);
        }
        drop(f);
        let checksum = hex(&sha.finalize());
        if hex(&disk_sha.finalize()) != checksum {
            return Err(MobileError::Response);
        }
        let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
        self.check_root()?;
        if !regular(&part)? || regular(&self.path(hash, "img"))? {
            return Err(MobileError::Cache);
        }
        fs::rename(part, self.path(hash, "img")).map_err(|_| MobileError::Cache)?;
        entry.length = length;
        entry.sha256 = checksum;
        entry.complete = true;
        entry.last_access = now();
        self.update(hash, &entry)?;
        Ok(self
            .path(hash, "img")
            .to_str()
            .ok_or(MobileError::Cache)?
            .into())
    }
    fn evict(&self, protect: Option<&str>) -> Result<()> {
        let mut entries = self.entries()?;
        entries.sort_by_key(|(_, e)| e.last_access);
        let mut total: u64 = entries
            .iter()
            .filter(|(_, e)| e.complete)
            .map(|(_, e)| e.length)
            .sum();
        let active = self.shared.flights.lock().map_err(|_| MobileError::Cache)?;
        for (hash, entry) in entries {
            if total <= self.limit {
                break;
            }
            if !entry.complete
                || protect == Some(hash.as_str())
                || active.get(&hash).is_some_and(|f| f.strong_count() > 0)
            {
                continue;
            }
            self.delete(&hash)?;
            total = total.saturating_sub(entry.length);
        }
        // Soft cap: an active or single oversized original is retained intact until a later sweep.
        Ok(())
    }
    pub(crate) fn usage(&self) -> Result<Usage> {
        let _gate = self.shared.gate.read().map_err(|_| MobileError::Cache)?;
        let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
        self.check_root()?;
        let mut bytes = 0;
        let mut files = 0;
        for (hash, entry) in self.entries()? {
            if entry.complete && regular(&self.path(&hash, "img"))? {
                files += 1;
            }
            for suffix in ["img", "part", "json", "json.part"] {
                let path = self.path(&hash, suffix);
                if regular(&path)? {
                    bytes += fs::metadata(path).map_err(|_| MobileError::Cache)?.len();
                }
            }
        }
        Ok(Usage {
            bytes,
            files,
            path: self.root.to_str().ok_or(MobileError::Cache)?.into(),
        })
    }
    pub(crate) fn clear(&self) -> Result<()> {
        let _gate = self.shared.gate.write().map_err(|_| MobileError::Cache)?;
        let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
        self.check_root()?;
        for (hash, _) in self.entries()? {
            self.delete(&hash)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

fn content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    if ![start, end, total]
        .iter()
        .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let (start, end, total) = (start.parse().ok()?, end.parse().ok()?, total.parse().ok()?);
    if start > end || end >= total {
        return None;
    }
    Some((start, end, total))
}

fn strong_etag(value: &str) -> bool {
    value.len() >= 2
        && value.starts_with('"')
        && value.ends_with('"')
        && value.as_bytes()[1..value.len() - 1]
            .iter()
            .all(|b| *b == 0x21 || (0x23..=0x7e).contains(b) || *b >= 0x80)
}
fn resumable(entry: &Entry) -> bool {
    match (
        entry.validator_header.as_deref(),
        entry.validator.as_deref(),
    ) {
        (Some("etag"), Some(v)) => strong_etag(v),
        (Some("last-modified"), Some(v)) => httpdate::parse_http_date(v).is_ok(),
        _ => false,
    }
}
