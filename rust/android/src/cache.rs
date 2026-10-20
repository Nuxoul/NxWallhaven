//! Marker-owned compressed-original disk cache. No image bytes cross UniFFI.
use crate::{image_format, urls, MobileError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
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
        };
        {
            let _gate = cache.shared.gate.write().map_err(|_| MobileError::Cache)?;
            let _disk = cache.shared.disk.lock().map_err(|_| MobileError::Cache)?;
            cache.check_root()?;
            for (hash, entry) in cache.entries()? {
                if !entry.complete {
                    cache.delete(&hash)?;
                }
            }
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
        remove(&self.path(hash, "json"))?;
        fs::rename(part, self.path(hash, "json")).map_err(|_| MobileError::Cache)
    }
    fn hit(&self, hash: &str) -> Result<Option<String>> {
        let Some(mut entry) = self.entry(hash)? else {
            return Ok(None);
        };
        if !entry.complete {
            self.delete(hash)?;
            return Ok(None);
        }
        let path = self.path(hash, "img");
        if !regular(&path)? {
            self.delete(hash)?;
            return Ok(None);
        }
        let mut file = File::open(&path).map_err(|_| MobileError::Cache)?;
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
        if length != entry.length || hex(&sha.finalize()) != entry.sha256 {
            self.delete(hash)?;
            return Ok(None);
        }
        entry.last_access = now();
        self.update(hash, &entry)?;
        Ok(Some(path.to_str().ok_or(MobileError::Cache)?.into()))
    }
    pub(crate) fn acquire(&self, input: &str) -> Result<String> {
        let url = urls::image_url(input)?;
        let hash = digest(url.as_str());
        let _gate = self.shared.gate.read().map_err(|_| MobileError::Cache)?;
        let (flight, owner) = {
            let mut flights = self.shared.flights.lock().map_err(|_| MobileError::Cache)?;
            flights.retain(|_, v| v.strong_count() > 0);
            if let Some(f) = flights.get(&hash).and_then(Weak::upgrade) {
                (f, false)
            } else {
                let f = Arc::new(Flight {
                    result: Mutex::new(None),
                    ready: Condvar::new(),
                });
                flights.insert(hash.clone(), Arc::downgrade(&f));
                (f, true)
            }
        };
        if !owner {
            let mut result = flight.result.lock().map_err(|_| MobileError::Cache)?;
            while result.is_none() {
                result = flight.ready.wait(result).map_err(|_| MobileError::Cache)?;
            }
            return result.as_ref().unwrap().clone();
        }
        let result = self.fetch(&url, &hash);
        *flight.result.lock().map_err(|_| MobileError::Cache)? = Some(result.clone());
        flight.ready.notify_all();
        result
    }
    fn fetch(&self, url: &reqwest::Url, hash: &str) -> Result<String> {
        {
            let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
            self.check_root()?;
            if let Some(path) = self.hit(hash)? {
                return Ok(path);
            }
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
                    sha256: String::new(),
                    last_access: now(),
                    complete: false,
                })
                .map_err(|_| MobileError::Cache)?,
            )?;
        }
        let result = self.download(url, hash);
        let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
        if result.is_err() {
            self.delete(hash)?;
        } else {
            self.evict(Some(hash))?;
        }
        result
    }
    fn download(&self, url: &reqwest::Url, hash: &str) -> Result<String> {
        let mut response = self
            .http
            .get(url.clone())
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .map_err(|_| MobileError::Network)?;
        if response.status().as_u16() != 200 {
            return Err(MobileError::Http {
                status: response.status().as_u16(),
            });
        }
        if response.url() != url
            || response
                .headers()
                .contains_key(reqwest::header::CONTENT_RANGE)
            || response
                .headers()
                .get(reqwest::header::CONTENT_ENCODING)
                .is_some_and(|v| v != "identity")
        {
            return Err(MobileError::Response);
        }
        let expected = response.content_length();
        let part = self.path(hash, "part");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(|_| MobileError::Cache)?;
        let mut length = 0u64;
        let mut sha = Sha256::new();
        let mut buf = [0; 64 * 1024];
        // No body-size cap, resize, or decoded pixel allocation: keep the complete original.
        loop {
            let n = response.read(&mut buf).map_err(|_| MobileError::Response)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|_| MobileError::Cache)?;
            sha.update(&buf[..n]);
            length = length.checked_add(n as u64).ok_or(MobileError::Response)?;
        }
        file.sync_all().map_err(|_| MobileError::Cache)?;
        drop(file);
        if length == 0 || expected.is_some_and(|n| n != length) {
            return Err(MobileError::Response);
        }
        image_format::validate(&part).map_err(|_| MobileError::Response)?;
        let _disk = self.shared.disk.lock().map_err(|_| MobileError::Cache)?;
        self.check_root()?;
        if !regular(&part)? || regular(&self.path(hash, "img"))? {
            return Err(MobileError::Cache);
        }
        fs::rename(part, self.path(hash, "img")).map_err(|_| MobileError::Cache)?;
        self.update(
            hash,
            &Entry {
                owner: OWNER.into(),
                url_hash: hash.into(),
                length,
                sha256: hex(&sha.finalize()),
                last_access: now(),
                complete: true,
            },
        )?;
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
