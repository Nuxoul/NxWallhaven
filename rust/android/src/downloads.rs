//! Durable queue; Android owns scheduling and the final content:// or file:// export.
use crate::cache::{Control, ImageCache};
use crate::{invalid, json, urls, MobileError};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wh_core::library::{DownloadTask, Library, LibraryError};

#[uniffi::export(with_foreign)]
pub trait DownloadObserver: Send + Sync {
    /// Return false to pause. Callbacks occur without a database mutex held.
    fn progress(&self, received: u64, total: Option<u64>) -> bool;
}
#[derive(uniffi::Object)]
pub struct MobileDownloads {
    cache: ImageCache,
    db: Library,
}
impl From<LibraryError> for MobileError {
    fn from(e: LibraryError) -> Self {
        match e {
            LibraryError::Invalid => invalid("invalid download record"),
            LibraryError::NotFound => Self::NotFound,
            LibraryError::Storage => Self::Cache,
        }
    }
}
fn original(wall: &Value) -> Result<&str, MobileError> {
    wall.get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("wall must contain the public original path"))
}
#[uniffi::export]
impl MobileDownloads {
    #[uniffi::constructor]
    pub fn new(cache_dir: String, data_dir: String) -> Result<Arc<Self>, MobileError> {
        if data_dir.trim().is_empty() {
            return Err(invalid("data directory is empty"));
        }
        Ok(Arc::new(Self {
            cache: ImageCache::new(&cache_dir)?,
            db: Library::open(Path::new(&data_dir))?,
        }))
    }
    pub fn enqueue(&self, wall_json: String) -> Result<String, MobileError> {
        let wall: Value =
            serde_json::from_str(&wall_json).map_err(|_| invalid("invalid wall JSON"))?;
        urls::image_url(original(&wall)?)?;
        json(&self.db.enqueue(wall)?)
    }
    pub fn tasks(&self) -> Result<String, MobileError> {
        json(&self.db.tasks()?)
    }
    pub fn library(&self) -> Result<String, MobileError> {
        json(&self.db.library()?)
    }
    /// Call once at application startup, before scheduling; never while workers are live.
    pub fn recover(&self) -> Result<(), MobileError> {
        Ok(self.db.recover()?)
    }
    pub fn set_state(&self, id: String, state: String) -> Result<(), MobileError> {
        self.db.set_state(&id, &state)?;
        if state == "cancelled" {
            let task = self.db.get(&id)?;
            self.cache.discard_partial(original(&task.wall)?)?;
        }
        Ok(())
    }
    /// Returns only a verified complete cache path; an empty string means no export.
    /// A successful acquisition remains running until Android calls complete(uri) or fail().
    pub fn acquire(
        &self,
        id: String,
        observer: Arc<dyn DownloadObserver>,
    ) -> Result<String, MobileError> {
        let task = self.db.get(&id)?;
        if !self.db.claim(&id)? {
            return Ok(String::new());
        }
        let url = original(&task.wall)?;
        let mut last = None::<Instant>;
        let mut callback_error = None;
        let mut stopped = false;
        let mut latest = (task.received, task.total);
        let result = self.cache.acquire_controlled(url, &mut |received, total| {
            latest = (received, total);
            let current: DownloadTask = match self.db.get(&id) {
                Ok(t) => t,
                Err(LibraryError::NotFound) => {
                    stopped = true;
                    // remove() changes records only; preserve the cache-owned partial.
                    return Control::Pause;
                }
                Err(e) => {
                    callback_error = Some(MobileError::from(e));
                    return Control::Pause;
                }
            };
            if current.state != "running" {
                stopped = true;
                return if current.state == "cancelled" {
                    Control::Cancel
                } else {
                    Control::Pause
                };
            }
            if last.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) {
                last = Some(Instant::now());
                if let Err(e) = self.db.progress(&id, received, total) {
                    callback_error = Some(e.into());
                    return Control::Pause;
                }
                // db.progress and db.get have both released their locks before foreign code.
                if !observer.progress(received, total) {
                    if let Err(e) = self.db.pause(&id) {
                        callback_error = Some(e.into());
                    }
                    stopped = true;
                    return Control::Pause;
                }
            }
            Control::Continue
        });
        if let Some(e) = callback_error {
            return Err(e);
        }
        // Include the last chunk even if callback throttling suppressed the final notification.
        self.db.progress(&id, latest.0, latest.1)?;
        let current = match self.db.get(&id) {
            Ok(t) => t,
            Err(LibraryError::NotFound) => return Ok(String::new()),
            Err(e) => return Err(e.into()),
        };
        if current.state == "cancelled" {
            self.cache.discard_partial(url)?;
        }
        if current.state != "running" {
            return Ok(String::new());
        }
        if stopped {
            return Ok(String::new());
        }
        match result {
            Ok(path) => {
                // The path is not pinned after return: Android must copy it immediately.
                if !path.is_empty() {
                    let n = std::fs::metadata(&path)
                        .map_err(|_| MobileError::Cache)?
                        .len();
                    self.db.progress(&id, n, Some(n))?;
                } else {
                    self.db.progress(&id, latest.0, latest.1)?;
                    self.db.pause(&id)?;
                }
                Ok(path)
            }
            Err(e) => {
                self.db.fail(&id)?;
                Err(e)
            }
        }
    }
    pub fn complete(&self, id: String, uri: String) -> Result<(), MobileError> {
        Ok(self.db.complete(&id, &uri)?)
    }
    pub fn fail(&self, id: String) -> Result<(), MobileError> {
        Ok(self.db.fail(&id)?)
    }
    pub fn remove(&self, id: String, remove_library: bool) -> Result<(), MobileError> {
        Ok(self.db.remove(&id, remove_library)?)
    }
    pub fn mark_missing(&self, id: String) -> Result<(), MobileError> {
        Ok(self.db.mark_missing(&id)?)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Noop;
    impl DownloadObserver for Noop {
        fn progress(&self, _: u64, _: Option<u64>) -> bool {
            true
        }
    }
    #[test]
    fn exported_json_contract_and_states() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap().to_owned();
        let downloads = MobileDownloads::new(d.clone(), d.clone()).unwrap();
        let wall = serde_json::json!({"id":"abc123","path":"https://w.wallhaven.cc/full/ab/wallhaven-abc123.png","file_type":"image/png","dimension_x":10,"thumbs":{"small":"https://th.wallhaven.cc/small/a.png"}});
        let task: Value =
            serde_json::from_str(&downloads.enqueue(wall.to_string()).unwrap()).unwrap();
        assert_eq!(task["wall"], wall);
        assert_eq!(task["state"], "queued");
        assert!(task["uri"].is_null());
        downloads
            .set_state("abc123".into(), "paused".into())
            .unwrap();
        assert!(downloads
            .acquire("abc123".into(), Arc::new(Noop))
            .unwrap()
            .is_empty());
        downloads
            .complete("abc123".into(), "content://media/external/images/1".into())
            .unwrap();
        let duplicate: Value =
            serde_json::from_str(&downloads.enqueue(wall.to_string()).unwrap()).unwrap();
        assert_eq!(duplicate["state"], "completed");
        drop(downloads);
        let downloads = MobileDownloads::new(d.clone(), d).unwrap();
        let library: Vec<Value> = serde_json::from_str(&downloads.library().unwrap()).unwrap();
        assert_eq!(library.len(), 1);
        downloads.remove("abc123".into(), false).unwrap();
        assert_eq!(downloads.tasks().unwrap(), "[]");
        assert_ne!(downloads.library().unwrap(), "[]");
    }

    #[test]
    fn preview_loaded_original_is_exported_offline_and_callback_can_query_db() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/original.png", listener.local_addr().unwrap());
        let body = crate::image_format::fixture();
        let sent = body.clone();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                socket.read_exact(&mut b).unwrap();
                request.push(b[0]);
            }
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                sent.len()
            )
            .unwrap();
            socket.write_all(&sent).unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap().to_owned();
        let preview = ImageCache::new(&d).unwrap();
        let path = preview.acquire(&url).unwrap();
        server.join().unwrap(); // All later acquire calls must work with no HTTP server.
        let downloads = MobileDownloads::new(d.clone(), d).unwrap();
        downloads
            .enqueue(serde_json::json!({"id":"abc123","path":url}).to_string())
            .unwrap();
        struct Reentrant {
            downloads: Arc<MobileDownloads>,
            keep_going: bool,
        }
        impl DownloadObserver for Reentrant {
            fn progress(&self, _: u64, _: Option<u64>) -> bool {
                assert!(!self.downloads.tasks().unwrap().is_empty());
                self.keep_going
            }
        }
        let paused = downloads
            .acquire(
                "abc123".into(),
                Arc::new(Reentrant {
                    downloads: downloads.clone(),
                    keep_going: false,
                }),
            )
            .unwrap();
        assert!(paused.is_empty());
        assert_eq!(downloads.db.get("abc123").unwrap().state, "paused");
        downloads
            .set_state("abc123".into(), "queued".into())
            .unwrap();
        let acquired = downloads
            .acquire(
                "abc123".into(),
                Arc::new(Reentrant {
                    downloads: downloads.clone(),
                    keep_going: true,
                }),
            )
            .unwrap();
        assert_eq!(acquired, path);
        assert_eq!(std::fs::read(acquired).unwrap(), body);
        let task = downloads.db.get("abc123").unwrap();
        assert_eq!(task.state, "running");
        assert_eq!(task.received, body.len() as u64);
        assert_eq!(task.total, Some(body.len() as u64));
        downloads
            .complete("abc123".into(), "content://media/images/1".into())
            .unwrap();
        assert!(downloads
            .acquire("abc123".into(), Arc::new(Noop))
            .unwrap()
            .is_empty());
    }
}
