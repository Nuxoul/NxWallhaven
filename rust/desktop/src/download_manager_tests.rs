use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;

struct Fixture {
    root: PathBuf,
    manager: Arc<DownloadManager>,
    loader: Arc<ImageLoader>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "wh-queue-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let disk = crate::image_disk_cache::DiskCache::temporary();
        let loader = Arc::new(ImageLoader::with_disk_cache(1, disk));
        let manager = DownloadManager::new(root.join("data"), loader.clone()).unwrap();
        Self {
            root,
            manager,
            loader,
        }
    }
    fn destination(&self) -> PathBuf {
        self.root.join("exports")
    }
    fn wait(&self, id: &str, expected: &str) -> DownloadTask {
        let until = Instant::now() + Duration::from_secs(8);
        loop {
            let task = self.manager.inner.db.get(id).unwrap();
            if task.state == expected {
                return task;
            }
            assert!(
                Instant::now() < until,
                "expected {expected}, got {}",
                task.state
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    // Reproduce a durable publication / SQLite seam without a live worker racing setup.
    fn published(&self, body: &[u8], receipt: bool) -> (DownloadTask, PathBuf) {
        let mut control = self.manager.inner.state.lock().unwrap();
        control.allow_nsfw = false;
        let wall = wall("abc123", "http://127.0.0.1:1/original.png", Some("sfw"));
        let task = self
            .manager
            .inner
            .db
            .enqueue(serde_json::to_value(&wall).unwrap())
            .unwrap();
        assert!(self.manager.inner.db.claim(&task.id).unwrap());
        std::fs::create_dir_all(self.destination()).unwrap();
        let directory = std::fs::canonicalize(self.destination()).unwrap();
        let record = Destination {
            directory: directory.clone(),
            filename: "wallhaven-abc123.png".into(),
            length: receipt.then_some(body.len() as u64),
            sha256: receipt.then(|| format!("{:x}", Sha256::digest(body))),
        };
        self.manager
            .inner
            .write_destination(&task.id, &record)
            .unwrap();
        let path = directory.join(&record.filename);
        std::fs::write(&path, body).unwrap();
        drop(control);
        (task, path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.manager.shutdown();
        let until = Instant::now() + Duration::from_secs(3);
        while self.manager.inner.state.lock().unwrap().active.is_some() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn wall(id: &str, url: &str, purity: Option<&str>) -> wh_core::Wallpaper {
    serde_json::from_value(serde_json::json!({"id":id,"path":url,"purity":purity})).unwrap()
}
fn png() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}
fn serve(body: Vec<u8>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/original.png", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"one\"\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        let _ = stream.write_all(&body);
    });
    (url, worker)
}
#[test]
fn crash_after_publication_before_index_reconciles_receipt_without_network() {
    let f = Fixture::new();
    let (task, path) = f.published(b"exact original", true);
    f.manager.inner.transfer(&task).unwrap();
    let completed = f.manager.inner.db.get(&task.id).unwrap();
    assert_eq!(completed.state, "completed");
    assert_eq!(completed.received, 14);
    assert_eq!(f.manager.library().unwrap().len(), 1);
    assert_eq!(std::fs::read(path).unwrap(), b"exact original");
    // Idempotent reconciliation cannot produce a second index or export.
    f.manager.inner.transfer(&task).unwrap();
    assert_eq!(f.manager.library().unwrap().len(), 1);
}
#[test]
fn reconciliation_never_adopts_or_overwrites_unrelated_existing_file() {
    for receipt in [false, true] {
        let f = Fixture::new();
        let (task, path) = f.published(b"original", receipt);
        std::fs::write(&path, b"unrelated user file").unwrap();
        assert!(f.manager.inner.transfer(&task).is_err());
        assert!(f.manager.library().unwrap().is_empty());
        assert_eq!(std::fs::read(path).unwrap(), b"unrelated user file");
    }
}
#[test]
fn offline_exact_export_dedup_missing_retry_and_reopen() {
    let f = Fixture::new();
    let expected = png();
    let (url, server) = serve(expected.clone());
    f.loader.preview(&url).unwrap();
    server.join().unwrap();
    let wall = wall("abc123", &url, Some("sfw"));
    f.manager.enqueue(&wall, &f.destination()).unwrap();
    f.wait("abc123", "completed");
    let path = f.destination().join("wallhaven-abc123.png");
    assert_eq!(std::fs::read(&path).unwrap(), expected);
    f.manager.enqueue(&wall, &f.root.join("different")).unwrap();
    assert!(!f.root.join("different").exists());
    std::fs::remove_file(&path).unwrap();
    assert_eq!(f.manager.tasks().unwrap()[0].state, "missing");
    f.manager.enqueue(&wall, &f.destination()).unwrap();
    f.wait("abc123", "completed");
    assert_eq!(std::fs::read(path).unwrap(), expected);
    assert_eq!(
        Library::open(&f.root.join("data"))
            .unwrap()
            .library()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn no_overwrite_failure_continues_and_delete_rejects_forged_uri() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.destination()).unwrap();
    let target = f.destination().join("wallhaven-abc123.png");
    std::fs::write(&target, b"user data").unwrap();
    f.manager
        .enqueue(
            &wall("abc123", "http://127.0.0.1:1/a.png", Some("sfw")),
            &f.destination(),
        )
        .unwrap();
    f.wait("abc123", "failed");
    let (url, server) = serve(png());
    f.manager
        .enqueue(&wall("def456", &url, Some("sfw")), &f.destination())
        .unwrap();
    f.wait("def456", "completed");
    server.join().unwrap();
    let uri = reqwest::Url::from_file_path(&target).unwrap();
    f.manager.inner.db.complete("def456", uri.as_str()).unwrap();
    assert!(f.manager.remove("def456", true).is_err());
    assert_eq!(std::fs::read(target).unwrap(), b"user data");
}
#[test]
fn permissions_default_deny_and_case_insensitive_safe_policy() {
    let f = Fixture::new();
    for (id, purity) in [
        ("abc123", None),
        ("def456", Some("NSFW")),
        ("ghi789", Some("unknown")),
    ] {
        f.manager
            .enqueue(
                &wall(id, "http://127.0.0.1:1/a.png", purity),
                &f.destination(),
            )
            .unwrap();
        assert_eq!(f.manager.inner.db.get(id).unwrap().state, "paused");
        assert!(f.manager.set_state(id, "queued").is_err());
    }
    let safe = f
        .manager
        .inner
        .db
        .enqueue(
            serde_json::to_value(wall("jkl012", "http://127.0.0.1:1/a.png", Some("SfW"))).unwrap(),
        )
        .unwrap();
    assert!(!restricted(&safe));
    f.manager.permissions_changed(true);
    assert_eq!(f.manager.inner.db.get("abc123").unwrap().state, "paused");
}
#[test]
fn queue_only_removal_retains_file_library_and_receipt_for_dedup() {
    let f = Fixture::new();
    let (task, path) = f.published(b"original", true);
    f.manager.inner.transfer(&task).unwrap();
    f.manager.remove(&task.id, false).unwrap();
    assert!(f.manager.tasks().unwrap().is_empty());
    assert_eq!(f.manager.library().unwrap().len(), 1);
    assert!(path.exists());
    assert!(f.manager.inner.destinations.join("abc123.json").exists());
}
#[test]
fn delete_requires_exact_receipt_and_retains_replaced_bytes() {
    let f = Fixture::new();
    let (task, path) = f.published(b"original", true);
    f.manager.inner.transfer(&task).unwrap();
    std::fs::write(&path, b"replacement").unwrap();
    assert!(f.manager.remove(&task.id, true).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    std::fs::write(&path, b"original").unwrap();
    f.manager.remove(&task.id, true).unwrap();
    assert!(!path.exists());
    assert!(f.manager.library().unwrap().is_empty());
}
#[test]
fn download_url_policy_rejects_unapproved_and_credentialed_endpoints() {
    for url in [
        "https://example.com/a.png",
        "https://w.wallhaven.cc/full/ab/wallhaven-abc123.png?key=secret",
        "https://user@w.wallhaven.cc/full/ab/wallhaven-abc123.png",
        "file:///original.png",
    ] {
        assert!(validate_download_url(url).is_err(), "{url}");
    }
    assert!(validate_download_url("https://w.wallhaven.cc/full/ab/wallhaven-abc123.png").is_ok());
}
#[test]
fn shutdown_returns_before_shared_preview_finishes_its_get() {
    let f = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/original.png", listener.local_addr().unwrap());
    let (seen_tx, seen_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let body = png();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(&body[..1]).unwrap();
        seen_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let _ = stream.write_all(&body[1..]);
    });
    f.manager
        .enqueue(&wall("abc123", &url, Some("sfw")), &f.destination())
        .unwrap();
    seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let loader = f.loader.clone();
    let preview = std::thread::spawn(move || loader.preview(&url));
    std::thread::sleep(Duration::from_millis(150));
    // Bound a broken implementation too: release after 1s, but require UI return <250ms.
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        release_tx.send(()).unwrap();
    });
    let start = Instant::now();
    f.manager.shutdown();
    let elapsed = start.elapsed();
    release.join().unwrap();
    assert!(
        preview.join().unwrap().is_ok(),
        "shutdown cancelled shared preview"
    );
    server.join().unwrap();
    assert!(
        elapsed < Duration::from_millis(250),
        "shutdown blocked for {elapsed:?}"
    );
    assert_eq!(f.manager.inner.db.get("abc123").unwrap().state, "paused");
    assert!(!f.destination().join("wallhaven-abc123.png").exists());
}
#[test]
fn managed_download_helper_obeys_cancel_without_exporting() {
    let f = Fixture::new();
    let path = f.destination().join("cancelled.png");
    assert!(!f
        .loader
        .download_managed("http://127.0.0.1:1/a.png", &path, |_, _| false)
        .unwrap());
    assert!(!path.exists());
}

#[test]
fn records_only_removal_clears_both_indexes_and_receipt_but_keeps_export() {
    let f = Fixture::new();
    let (task, path) = f.published(b"original", true);
    f.manager.inner.transfer(&task).unwrap();
    f.manager.remove_records(&task.id).unwrap();
    assert!(f.manager.tasks().unwrap().is_empty());
    assert!(f.manager.library().unwrap().is_empty());
    assert!(!f.manager.inner.destinations.join("abc123.json").exists());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    f.manager
        .enqueue(
            &wall("abc123", "http://127.0.0.1:1/original.png", Some("sfw")),
            &f.destination(),
        )
        .unwrap();
    f.wait("abc123", "failed");
    assert_eq!(std::fs::read(path).unwrap(), b"original");
}
