//! Compressed image files in an app-owned directory, separate from user downloads.
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(test))]
use std::sync::OnceLock;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const MARKER: &str = "wallhaven-image-cache-v1";
const MARKER_BYTES: &[u8] = b"WallHaven Studio image cache v1\n";
static SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(super) struct CachedData {
    pub bytes: Arc<Vec<u8>>,
    pub disk_hit: bool,
    pub network_ms: u64,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct EntryInfo {
    version: u8,
    length: u64,
    sha256: String,
    extension: String,
}
struct State {
    epoch: u64,
    flights: HashMap<String, Arc<Flight>>,
}
struct Flight {
    state: Mutex<FlightState>,
    changed: Condvar,
}
struct FlightState {
    progress: Option<(u64, Option<u64>)>,
    result: Option<Result<Option<CachedData>, String>>,
    legacy_waiters: usize,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct ResumeInfo {
    length: u64,
    total: Option<u64>,
    sha256: String,
    validator_header: String,
    validator: String,
}
pub(super) struct DiskCache {
    root: Result<PathBuf, String>,
    state: Mutex<State>,
    temporary: bool,
}
struct PartialFile(PathBuf);
impl PartialFile {
    fn keep(mut self) {
        self.0.clear();
    }
}
impl Drop for PartialFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
#[derive(Default)]
pub(super) struct Usage {
    pub bytes: u64,
    pub files: usize,
}

impl DiskCache {
    pub(super) fn at(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            root: Ok(root),
            state: Mutex::new(State {
                epoch: 0,
                flights: HashMap::new(),
            }),
            temporary: false,
        })
    }
    #[cfg(test)]
    pub(super) fn temporary() -> Arc<Self> {
        let root = std::env::temp_dir().join(format!(
            "wh-image-cache-test-{}-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Arc::new(Self {
            root: Ok(root),
            state: Mutex::new(State {
                epoch: 0,
                flights: HashMap::new(),
            }),
            temporary: true,
        })
    }
    pub(super) fn path(&self) -> Result<&Path, String> {
        self.root.as_deref().map_err(Clone::clone)
    }
    /// No recursive traversal: refuse symlinks/junctions and require our exact marker.
    fn directory(&self, create: bool) -> Result<Option<&Path>, String> {
        let root = self.path()?;
        if !root.exists() {
            if !create {
                return Ok(None);
            }
            std::fs::create_dir_all(root).map_err(|e| format!("无法创建图片缓存目录：{e}"))?;
        }
        let meta = std::fs::symlink_metadata(root).map_err(|e| e.to_string())?;
        if !meta.is_dir() || is_link(&meta) {
            return Err("图片缓存位置不是普通目录，未读取或清除内容".into());
        }
        let marker = root.join(MARKER);
        if !marker.exists() {
            if !create {
                return Ok(None);
            }
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker)
            {
                Ok(mut file) => file.write_all(MARKER_BYTES).map_err(|e| e.to_string())?,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let meta = std::fs::symlink_metadata(&marker).map_err(|e| e.to_string())?;
        if !meta.is_file()
            || is_link(&meta)
            || meta.len() != MARKER_BYTES.len() as u64
            || std::fs::read(marker).map_err(|e| e.to_string())? != MARKER_BYTES
        {
            return Err("图片缓存目录标记无效，未读取或清除内容".into());
        }
        Ok(Some(root))
    }
    pub(super) fn usage(&self) -> Result<Usage, String> {
        let _guard = self.state.lock().unwrap();
        let Some(root) = self.directory(false)? else {
            return Ok(Usage::default());
        };
        let mut usage = Usage::default();
        for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !owned_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let meta = match std::fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.to_string()),
            };
            if meta.is_file() && !is_link(&meta) {
                usage.bytes = usage.bytes.saturating_add(meta.len());
                usage.files += 1;
            }
        }
        Ok(usage)
    }
    pub(super) fn clear(&self) -> Result<u64, String> {
        let mut state = self.state.lock().unwrap();
        state.epoch = state.epoch.wrapping_add(1);
        // Keep live flights attached: clearing is not transport cancellation, and
        // a second writer must not reuse an active partial's name after unlink.
        let Some(root) = self.directory(false)? else {
            return Ok(0);
        };
        let mut removed = 0u64;
        for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Windows file sharing allows unlinking active partials; their handles
            // can finish reading, while the changed epoch prevents a later commit.
            if !owned_name(&name) {
                continue;
            }
            let meta = match std::fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.to_string()),
            };
            if !meta.is_file() || is_link(&meta) {
                continue;
            }
            match std::fs::remove_file(entry.path()) {
                Ok(()) => removed = removed.saturating_add(meta.len()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("部分图片缓存未能清除：{e}")),
            }
        }
        Ok(removed)
    }
    fn read_entry(&self, key: &str) -> Result<Option<CachedData>, String> {
        let Some(root) = self.directory(false)? else {
            return Ok(None);
        };
        let info_path = root.join(format!("{key}.json"));
        let data = (|| {
            let meta = std::fs::symlink_metadata(&info_path).ok()?;
            if !meta.is_file() || is_link(&meta) || meta.len() > 1024 {
                return None;
            }
            let info: EntryInfo = serde_json::from_slice(&std::fs::read(info_path).ok()?).ok()?;
            if info.version != 1 || info.length == 0 || !valid_extension(&info.extension) {
                return None;
            }
            let path = root.join(format!("{key}.{}", info.extension));
            let meta = std::fs::symlink_metadata(&path).ok()?;
            if !meta.is_file() || is_link(&meta) || meta.len() != info.length {
                return None;
            }
            let bytes = std::fs::read(path).ok()?;
            if digest(&bytes) != info.sha256 {
                return None;
            }
            Some(CachedData {
                bytes: Arc::new(bytes),
                disk_hit: true,
                network_ms: 0,
            })
        })();
        Ok(data)
    }
    pub(super) fn load<F: Fn(u64, Option<u64>)>(
        &self,
        http: &reqwest::blocking::Client,
        url: &str,
        max: Option<u64>,
        progress: F,
    ) -> Result<CachedData, String> {
        // A managed leader may have paused just before this preview joined it.
        loop {
            if let Some(data) = self.acquire(http, url, max, false, &mut |n, t| {
                progress(n, t);
                true
            })? {
                return Ok(data);
            }
        }
    }
    pub(super) fn load_controlled<F: FnMut(u64, Option<u64>) -> bool>(
        &self,
        http: &reqwest::blocking::Client,
        url: &str,
        mut progress: F,
    ) -> Result<Option<CachedData>, String> {
        self.acquire(http, url, None, true, &mut progress)
    }
    fn acquire(
        &self,
        http: &reqwest::blocking::Client,
        url: &str,
        max: Option<u64>,
        controlled: bool,
        progress: &mut dyn FnMut(u64, Option<u64>) -> bool,
    ) -> Result<Option<CachedData>, String> {
        let key = cache_key(url)?;
        if controlled && !progress(0, None) {
            return Ok(None);
        }
        let (epoch, flight, leader) = {
            let mut state = self.state.lock().unwrap();
            if let Some(data) = self.read_entry(&key)? {
                check_size(data.bytes.len() as u64, max)?;
                drop(state);
                return Ok((!controlled
                    || progress(data.bytes.len() as u64, Some(data.bytes.len() as u64)))
                .then_some(data));
            }
            if let Some(flight) = state.flights.get(&key) {
                if !controlled {
                    flight.state.lock().unwrap().legacy_waiters += 1;
                }
                (state.epoch, flight.clone(), false)
            } else {
                let flight = Arc::new(Flight {
                    state: Mutex::new(FlightState {
                        progress: None,
                        result: None,
                        legacy_waiters: usize::from(!controlled),
                    }),
                    changed: Condvar::new(),
                });
                state.flights.insert(key.clone(), flight.clone());
                (state.epoch, flight, true)
            }
        };
        if !leader {
            let mut last = None;
            loop {
                let (stage, result) = {
                    let state = flight.state.lock().unwrap();
                    let (state, _) = flight
                        .changed
                        .wait_timeout_while(state, Duration::from_millis(100), |s| {
                            s.progress == last && s.result.is_none()
                        })
                        .unwrap();
                    (state.progress, state.result.clone())
                };
                if controlled || stage != last {
                    let (n, t) = stage.unwrap_or((0, None));
                    if !progress(n, t) {
                        return Ok(None); // Never cancel a preview's transport.
                    }
                    last = stage;
                }
                if let Some(result) = result {
                    let data = result?;
                    if let Some(ref data) = data {
                        check_size(data.bytes.len() as u64, max)?;
                    }
                    return Ok(data);
                }
            }
        }
        let mut stopped = false;
        let mut previous = None;
        let mut last = Instant::now();
        let result = self.fetch(
            http,
            url,
            &key,
            epoch,
            (max, controlled),
            |done, total, final_chunk| {
                let emit = previous.is_none()
                    || final_chunk
                    || last.elapsed() >= Duration::from_millis(100);
                if emit {
                    flight.state.lock().unwrap().progress = Some((done, total));
                    flight.changed.notify_all();
                    previous = Some((done, total));
                    last = Instant::now();
                }
                if !stopped && (controlled || emit) && !progress(done, total) {
                    stopped = true;
                }
                !stopped || flight.state.lock().unwrap().legacy_waiters > 0
            },
        );
        let mut state = self.state.lock().unwrap();
        if state
            .flights
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, &flight))
        {
            state.flights.remove(&key);
        }
        flight.state.lock().unwrap().result = Some(result.clone());
        flight.changed.notify_all();
        if stopped {
            Ok(None)
        } else {
            result
        }
    }
    fn fetch<F: FnMut(u64, Option<u64>, bool) -> bool>(
        &self,
        http: &reqwest::blocking::Client,
        url: &str,
        key: &str,
        epoch: u64,
        limits: (Option<u64>, bool),
        mut progress: F,
    ) -> Result<Option<CachedData>, String> {
        let (max, controlled) = limits;
        use reqwest::header as h;
        let started = Instant::now();
        let root = {
            let _guard = self.state.lock().unwrap();
            self.directory(true)?.unwrap().to_owned()
        };
        let partial = PartialFile(root.join(format!("{key}.resume.part")));
        let resume_path = root.join(format!("{key}.resume.json"));
        let mut hash = Sha256::new();
        let mut done = 0u64;
        let saved = read_resume(&partial.0, &resume_path);
        let mut resume = saved.filter(|info| {
            let Ok(mut f) = std::fs::File::open(&partial.0) else {
                return false;
            };
            let mut buf = [0u8; 65536];
            let mut prefix = (&mut f).take(info.length);
            while let Ok(n) = prefix.read(&mut buf) {
                if n == 0 {
                    break;
                }
                hash.update(&buf[..n]);
                done += n as u64;
            }
            done == info.length && format!("{:x}", hash.clone().finalize()) == info.sha256
        });
        if resume.is_none() {
            done = 0;
            hash = Sha256::new();
            remove_regular(&partial.0)?;
            remove_regular(&resume_path)?;
        }
        let mut request = http.get(url).header(h::ACCEPT_ENCODING, "identity");
        if let Some(info) = &resume {
            request = request
                .header(h::RANGE, format!("bytes={done}-"))
                .header(h::IF_RANGE, &info.validator);
        }
        let mut response = match request.send() {
            Ok(response) => response,
            Err(e) => {
                if controlled && resume.is_some() {
                    partial.keep();
                }
                return Err(e.to_string());
            }
        };
        if response.url().as_str() != url
            || response
                .headers()
                .get(h::CONTENT_ENCODING)
                .is_some_and(|v| v != "identity")
        {
            return Err("不支持重定向或编码后的原图响应".into());
        }
        let total;
        if let (206, Some(info)) = (response.status().as_u16(), resume.as_ref()) {
            let range = response
                .headers()
                .get(h::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(content_range)
                .ok_or("续传 Content-Range 无效")?;
            if range.0 != done
                || range.1.checked_add(1) != Some(range.2)
                || info.total.is_some_and(|n| n != range.2)
                || response
                    .headers()
                    .get(info.validator_header.as_str())
                    .and_then(|v| v.to_str().ok())
                    != Some(info.validator.as_str())
                || response
                    .content_length()
                    .is_some_and(|n| n != range.2 - done)
            {
                remove_regular(&resume_path)?;
                return Err("续传验证失败，未合并原图".into());
            }
            total = Some(range.2);
        } else if response.status().as_u16() == 200
            && !response.headers().contains_key(h::CONTENT_RANGE)
        {
            // Ignored Range / changed validator: restart from zero, never append a 200.
            done = 0;
            hash = Sha256::new();
            remove_regular(&partial.0)?;
            remove_regular(&resume_path)?;
            total = response.content_length();
            resume = response
                .headers()
                .get(h::ETAG)
                .and_then(|v| v.to_str().ok())
                .filter(|s| strong_etag(s))
                .map(|s| ("etag", s))
                .or_else(|| {
                    response
                        .headers()
                        .get(h::LAST_MODIFIED)
                        .and_then(|v| v.to_str().ok())
                        .filter(|s| http_date(s))
                        .map(|s| ("last-modified", s))
                })
                .map(|(header, value)| ResumeInfo {
                    length: 0,
                    total,
                    sha256: String::new(),
                    validator_header: header.into(),
                    validator: value.into(),
                });
        } else {
            return Err(format!("原图 HTTP 响应无效：{}", response.status()));
        }
        if let Some(n) = total {
            check_size(n, max)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        let mut file = if done > 0 {
            let f = options.open(&partial.0).map_err(|e| e.to_string())?;
            f.set_len(done).map_err(|e| e.to_string())?;
            f
        } else {
            options
                .create_new(true)
                .open(&partial.0)
                .map_err(|e| e.to_string())?
        };
        file.seek(SeekFrom::Start(done))
            .map_err(|e| e.to_string())?;
        let mut checkpoint = Instant::now();
        let mut buffer = [0u8; 65536];
        loop {
            if !progress(done, total, false) {
                // All advertised bytes already arrived: finish cache validation,
                // but the stopped observer still receives no export permission.
                if total == Some(done) && done > 0 {
                    break;
                }
                if let Some(ref mut info) = resume {
                    self.checkpoint(epoch, &file, &resume_path, info, done, &hash)?;
                    partial.keep();
                }
                return Ok(None);
            }
            let count = match response.read(&mut buffer) {
                Ok(n) => n,
                Err(e) => {
                    if controlled {
                        if let Some(ref mut info) = resume {
                            self.checkpoint(epoch, &file, &resume_path, info, done, &hash)?;
                            partial.keep();
                        }
                    }
                    return Err(e.to_string());
                }
            };
            if count == 0 {
                break;
            }
            done += count as u64;
            check_size(done, max)?;
            if total.is_some_and(|n| done > n) {
                return Err("原图响应长度无效".into());
            }
            file.write_all(&buffer[..count])
                .map_err(|e| e.to_string())?;
            hash.update(&buffer[..count]);
            if controlled && checkpoint.elapsed() >= Duration::from_secs(1) {
                if let Some(ref mut info) = resume {
                    self.checkpoint(epoch, &file, &resume_path, info, done, &hash)?;
                }
                checkpoint = Instant::now();
            }
        }
        if done == 0 || total.is_some_and(|total| total != done) {
            return Err("图片下载不完整，未提交缓存".into());
        }
        file.sync_all().map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() as u64 != done {
            return Err("图片缓存长度校验失败".into());
        }
        drop(file);
        let extension = extension(url);
        let info = EntryInfo {
            version: 1,
            length: done,
            sha256: format!("{:x}", hash.finalize()),
            extension: extension.into(),
        };
        let guard = self.state.lock().unwrap();
        if guard.epoch == epoch {
            let info_partial = PartialFile(root.join(format!(
                "{key}.{}.{}.part",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            )));
            std::fs::write(
                &info_partial.0,
                serde_json::to_vec(&info).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            for path in [
                root.join(format!("{key}.{extension}")),
                root.join(format!("{key}.json")),
            ] {
                remove_regular(&path)?;
            }
            std::fs::rename(&partial.0, root.join(format!("{key}.{extension}")))
                .map_err(|e| e.to_string())?;
            std::fs::rename(&info_partial.0, root.join(format!("{key}.json")))
                .map_err(|e| e.to_string())?;
            remove_regular(&resume_path)?;
        }
        drop(guard);
        progress(done, total, true);
        Ok(Some(CachedData {
            bytes: Arc::new(bytes),
            disk_hit: false,
            network_ms: started.elapsed().as_millis() as u64,
        }))
    }
    fn checkpoint(
        &self,
        epoch: u64,
        file: &std::fs::File,
        path: &Path,
        info: &mut ResumeInfo,
        done: u64,
        hash: &Sha256,
    ) -> Result<(), String> {
        let guard = self.state.lock().unwrap();
        if guard.epoch != epoch {
            return Ok(());
        }
        file.sync_all().map_err(|e| e.to_string())?;
        info.length = done;
        info.sha256 = format!("{:x}", hash.clone().finalize());
        let temp = PartialFile(path.with_extension(format!(
            "{}.{}.part",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        )));
        std::fs::write(
            &temp.0,
            serde_json::to_vec(info).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        // A crash between remove and rename only loses resumability, never validates bad bytes.
        remove_regular(path)?;
        std::fs::rename(&temp.0, path).map_err(|e| e.to_string())
    }
    pub(super) fn discard_partial(&self, url: &str) -> Result<(), String> {
        let key = cache_key(url)?;
        let state = self.state.lock().unwrap();
        if state.flights.contains_key(&key) {
            return Ok(());
        }
        if let Some(root) = self.directory(false)? {
            remove_regular(&root.join(format!("{key}.resume.part")))?;
            remove_regular(&root.join(format!("{key}.resume.json")))?;
        }
        Ok(())
    }
    pub(super) fn discard(&self, url: &str) -> Result<(), String> {
        let key = cache_key(url)?;
        let _guard = self.state.lock().unwrap();
        let Some(root) = self.directory(false)? else {
            return Ok(());
        };
        for suffix in [
            "json", "jpg", "jpeg", "png", "webp", "gif", "bmp", "avif", "img",
        ] {
            let path = root.join(format!("{key}.{suffix}"));
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if meta.is_file() && !is_link(&meta) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        Ok(())
    }
}
impl Drop for DiskCache {
    fn drop(&mut self) {
        if self.temporary {
            if let Ok(root) = &self.root {
                let _ = std::fs::remove_dir_all(root);
            }
        }
    }
}
fn remove_regular(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !is_link(&meta) => {
            std::fs::remove_file(path).map_err(|e| e.to_string())
        }
        Ok(_) => Err("缓存目标不是普通文件".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
fn strong_etag(s: &str) -> bool {
    s.len() >= 2
        && s.starts_with('"')
        && s.ends_with('"')
        && s[1..s.len() - 1]
            .bytes()
            .all(|b| b >= 0x21 && b != b'"' && b != 0x7f)
}
fn http_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 29
        && &b[3..5] == b", "
        && b[7] == b' '
        && b[11] == b' '
        && b[16] == b' '
        && b[19] == b':'
        && b[22] == b':'
        && &b[25..] == b" GMT"
        && [5, 6, 12, 13, 14, 15, 17, 18, 20, 21, 23, 24]
            .iter()
            .all(|&i| b[i].is_ascii_digit())
        && [b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat", b"Sun"]
            .iter()
            .any(|day| day.as_slice() == &b[..3])
        && [
            b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov",
            b"Dec",
        ]
        .iter()
        .any(|month| month.as_slice() == &b[8..11])
        && valid_date_fields(b)
}
fn valid_date_fields(b: &[u8]) -> bool {
    let number = |start: usize, end: usize| {
        std::str::from_utf8(&b[start..end])
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
    };
    let (Some(day), Some(year), Some(hour), Some(minute), Some(second)) = (
        number(5, 7),
        number(12, 16),
        number(17, 19),
        number(20, 22),
        number(23, 25),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match &b[8..11] {
        b"Feb" => {
            if leap {
                29
            } else {
                28
            }
        }
        b"Apr" | b"Jun" | b"Sep" | b"Nov" => 30,
        _ => 31,
    };
    year >= 1601 && day > 0 && day <= days && hour < 24 && minute < 60 && second < 60
}
fn content_range(s: &str) -> Option<(u64, u64, u64)> {
    let (span, total) = s.strip_prefix("bytes ")?.split_once('/')?;
    let (first, last) = span.split_once('-')?;
    if [first, last, total]
        .iter()
        .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let (first, last, total) = (first.parse().ok()?, last.parse().ok()?, total.parse().ok()?);
    (first <= last && last < total).then_some((first, last, total))
}
fn read_resume(part: &Path, info_path: &Path) -> Option<ResumeInfo> {
    let meta = std::fs::symlink_metadata(info_path).ok()?;
    if !meta.is_file() || is_link(&meta) || meta.len() > 2048 {
        return None;
    }
    let info: ResumeInfo = serde_json::from_slice(&std::fs::read(info_path).ok()?).ok()?;
    let meta = std::fs::symlink_metadata(part).ok()?;
    if !meta.is_file()
        || is_link(&meta)
        || info.length == 0
        || meta.len() < info.length
        || info.total.is_some_and(|n| info.length >= n)
        || !match info.validator_header.as_str() {
            "etag" => strong_etag(&info.validator),
            "last-modified" => http_date(&info.validator),
            _ => false,
        }
    {
        return None;
    }
    Some(info)
}
fn check_size(bytes: u64, max: Option<u64>) -> Result<(), String> {
    if max.is_some_and(|max| bytes > max) {
        Err("图片超过大小上限".into())
    } else {
        Ok(())
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn cache_key(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "图片地址无效")?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err("图片地址不受支持".into());
    }
    Ok(digest(url.as_bytes()))
}
fn valid_extension(value: &str) -> bool {
    matches!(
        value,
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "avif" | "img"
    )
}
fn extension(url: &str) -> &str {
    let candidate = url
        .split('?')
        .next()
        .unwrap_or(url)
        .rsplit('.')
        .next()
        .unwrap_or("");
    if valid_extension(candidate) {
        candidate
    } else {
        "img"
    }
}
fn owned_name(name: &str) -> bool {
    let Some((key, suffix)) = name.split_once('.') else {
        return false;
    };
    key.len() == 64
        && key.bytes().all(|b| b.is_ascii_hexdigit())
        && (suffix == "json"
            || suffix == "resume.json"
            || valid_extension(suffix)
            || (suffix.ends_with(".part")
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'.' || b.is_ascii_lowercase())))
}
fn is_link(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
#[cfg(not(test))]
static SHARED: OnceLock<Arc<DiskCache>> = OnceLock::new();
pub(super) fn shared() -> Arc<DiskCache> {
    #[cfg(not(test))]
    {
        SHARED
            .get_or_init(|| match cache_root() {
                Ok(root) => DiskCache::at(root),
                Err(error) => Arc::new(DiskCache {
                    root: Err(error),
                    state: Mutex::new(State {
                        epoch: 0,
                        flights: HashMap::new(),
                    }),
                    temporary: false,
                }),
            })
            .clone()
    }
    #[cfg(test)]
    {
        thread_local! { static CACHE: Arc<DiskCache> = DiskCache::temporary(); }
        CACHE.with(Clone::clone)
    }
}
#[cfg(not(test))]
fn cache_root() -> Result<PathBuf, String> {
    if let Some(base) = std::env::var_os("WH_STUDIO_CONFIG_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(base).join("image-cache-v1"));
    }
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
    };
    let mut value = core::ptr::null_mut();
    let status = unsafe {
        SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, core::ptr::null_mut(), &mut value)
    };
    if status < 0 || value.is_null() {
        return Err("无法确定本地图片缓存目录".into());
    }
    let mut length = 0;
    while unsafe { *value.add(length) } != 0 {
        length += 1;
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(unsafe {
        std::slice::from_raw_parts(value, length)
    }));
    unsafe {
        CoTaskMemFree(value.cast());
    }
    Ok(path.join("WallHavenStudio").join("Cache").join("Images"))
}
/// Save original compressed bytes with a no-replace atomic commit, never re-encode pixels.
pub(super) fn save_copy<F: Fn(u64, Option<u64>)>(
    bytes: &[u8],
    destination: &Path,
    progress: F,
) -> Result<(), String> {
    if destination.exists() {
        return Err("文件已存在，未覆盖".into());
    }
    let directory = destination.parent().ok_or("下载目录无效")?;
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let name = destination
        .file_name()
        .ok_or("下载文件名无效")?
        .to_string_lossy();
    let partial = PartialFile(directory.join(format!(
        ".{name}.{}.{}.part",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial.0)
        .map_err(|e| e.to_string())?;
    let mut done = 0u64;
    for chunk in bytes.chunks(65536) {
        file.write_all(chunk).map_err(|e| e.to_string())?;
        done += chunk.len() as u64;
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let from = partial
            .0
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let to = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
        } == 0
        {
            return Err(if destination.exists() {
                "文件已存在，未覆盖".into()
            } else {
                format!("保存缓存图片失败：{}", std::io::Error::last_os_error())
            });
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::hard_link(&partial.0, destination).map_err(|e| e.to_string())?;
    }
    progress(done, Some(done));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn serve(body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/picture.png", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = stream.read(&mut req);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });
        (url, server)
    }
    #[test]
    fn new_cache_instance_reuses_file_offline_and_clear_preserves_unrelated_files() {
        let owner = DiskCache::temporary();
        let http = reqwest::blocking::Client::new();
        let body = b"original compressed file bytes".to_vec();
        let (url, server) = serve(body.clone());
        assert!(!owner.load(&http, &url, None, |_, _| {}).unwrap().disk_hit);
        server.join().unwrap();
        let root = owner.path().unwrap();
        let reopened = DiskCache::at(root.to_owned());
        let cached = reopened.load(&http, &url, None, |_, _| {}).unwrap();
        assert!(cached.disk_hit);
        assert_eq!(*cached.bytes, body);
        let saved = root.parent().unwrap().join(format!(
            "wh-saved-{}.png",
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        save_copy(&cached.bytes, &saved, |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), body);
        assert!(save_copy(b"replace", &saved, |_, _| {}).is_err());
        std::fs::write(root.join("keep.txt"), b"user file").unwrap();
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/keep.txt"), b"nested").unwrap();
        let usage = reopened.usage().unwrap();
        assert!(usage.bytes > body.len() as u64);
        assert_eq!(usage.files, 2);
        assert!(reopened.clear().unwrap() > 0);
        assert_eq!(reopened.usage().unwrap().bytes, 0);
        assert!(root.join("keep.txt").is_file());
        assert!(root.join("nested/keep.txt").is_file());
        assert_eq!(std::fs::read(&saved).unwrap(), body);
        std::fs::remove_file(saved).unwrap();
        assert!(reopened.load(&http, &url, None, |_, _| {}).is_err());
    }
    #[test]
    fn truncated_response_and_partial_file_never_become_cache_hits() {
        let cache = DiskCache::temporary();
        let http = reqwest::blocking::Client::new();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/incomplete.jpg", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = stream.read(&mut req);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort",
                )
                .unwrap();
        });
        assert!(cache.load(&http, &url, None, |_, _| {}).is_err());
        server.join().unwrap();
        assert_eq!(cache.usage().unwrap().bytes, 0);
        assert_eq!(std::fs::read_dir(cache.path().unwrap()).unwrap().count(), 1);
    }
    #[test]
    fn clearing_in_flight_data_prevents_old_download_repopulating_disk_cache() {
        let cache = DiskCache::temporary();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/inflight.png", listener.local_addr().unwrap());
        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = stream.read(&mut req);
            seen_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata")
                .unwrap();
        });
        let running = cache.clone();
        let worker = std::thread::spawn(move || {
            running
                .load(&reqwest::blocking::Client::new(), &url, None, |_, _| {})
                .unwrap()
        });
        seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        cache.clear().unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(&**worker.join().unwrap().bytes, b"data");
        server.join().unwrap();
        assert_eq!(cache.usage().unwrap().bytes, 0);
    }
    #[test]
    fn same_length_corruption_is_rejected_by_checksum_and_refetched() {
        let cache = DiskCache::temporary();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/hash.jpg", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for body in [b"first", b"fixed"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut req = [0u8; 4096];
                let _ = stream.read(&mut req);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                stream.write_all(body).unwrap();
            }
        });
        let http = reqwest::blocking::Client::new();
        cache.load(&http, &url, None, |_, _| {}).unwrap();
        std::fs::write(
            cache
                .path()
                .unwrap()
                .join(format!("{}.jpg", cache_key(&url).unwrap())),
            b"wrong",
        )
        .unwrap();
        let result = cache.load(&http, &url, None, |_, _| {}).unwrap();
        assert!(!result.disk_hit);
        assert_eq!(&**result.bytes, b"fixed");
        server.join().unwrap();
    }
    #[test]
    fn clear_removes_crash_leftover_partial_without_touching_unrelated_files() {
        let cache = DiskCache::temporary();
        let root = cache.directory(true).unwrap().unwrap();
        let partial = root.join(format!("{}.99999.0.part", "a".repeat(64)));
        std::fs::write(&partial, b"incomplete image").unwrap();
        std::fs::write(root.join("unrelated.part"), b"keep").unwrap();
        assert!(cache.usage().unwrap().bytes > 0);
        cache.clear().unwrap();
        assert!(!partial.exists(), "clear left an owned partial on disk");
        assert_eq!(cache.usage().unwrap().bytes, 0);
        assert_eq!(std::fs::read(root.join("unrelated.part")).unwrap(), b"keep");
    }
    #[test]
    fn clear_unlinks_open_partial_and_download_finishes_without_refilling_cache() {
        let cache = DiskCache::temporary();
        let body = b"complete original bytes".to_vec();
        let (url, server) = serve(body.clone());
        let cleared = std::cell::Cell::new(false);
        let result = cache
            .load(&reqwest::blocking::Client::new(), &url, None, |done, _| {
                if done == 0 && !cleared.replace(true) {
                    assert!(
                        cache.usage().unwrap().files > 0,
                        "partial must already be open"
                    );
                    cache.clear().unwrap();
                    assert_eq!(cache.usage().unwrap().bytes, 0);
                }
            })
            .unwrap();
        server.join().unwrap();
        assert!(cleared.get());
        assert_eq!(*result.bytes, body);
        assert_eq!(cache.usage().unwrap().bytes, 0);
    }
}
#[cfg(test)]
mod resume_tests;
