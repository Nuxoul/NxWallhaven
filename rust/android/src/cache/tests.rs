use super::*;
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Barrier;
use std::thread;

fn server(
    body: Vec<u8>,
    advertised: usize,
    delay: Duration,
) -> (String, Arc<AtomicUsize>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/original.png", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let counter = count.clone();
    let handle = thread::spawn(move || {
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
        let request = String::from_utf8(request).unwrap().to_lowercase();
        assert!(!request.contains("x-api-key"));
        assert!(!request.contains("cookie:"));
        counter.fetch_add(1, Ordering::SeqCst);
        thread::sleep(delay);
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {advertised}\r\nConnection: close\r\n\r\n").unwrap();
        socket.write_all(&body).unwrap();
    });
    (url, count, handle)
}
fn cache(dir: &tempfile::TempDir) -> ImageCache {
    ImageCache::new(dir.path().to_str().unwrap()).unwrap()
}

#[test]
fn offline_new_instance_reuses_complete_original_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let (url, count, server) = server(body.clone(), body.len(), Duration::ZERO);
    let first = cache(&dir);
    let path = first.acquire(&url).unwrap();
    server.join().unwrap();
    assert_eq!(fs::read(&path).unwrap(), body);
    assert!(Path::new(&path).is_absolute());
    let meta = first.entry(&digest(&url)).unwrap().unwrap();
    assert!(meta.complete);
    assert_eq!(meta.length, body.len() as u64);
    assert_eq!(meta.sha256, hex(&Sha256::digest(&body)));
    drop(first); // No live client/root state; the listener is also gone.
    let second = cache(&dir);
    assert_eq!(second.acquire(&url).unwrap(), path);
    assert_eq!(second.usage().unwrap().files, 1);
    let metadata_bytes = fs::metadata(second.path(&digest(&url), "json"))
        .unwrap()
        .len();
    assert_eq!(
        second.usage().unwrap().bytes,
        body.len() as u64 + metadata_bytes
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
#[test]
fn concurrent_acquires_across_instances_coalesce_one_get() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let (url, count, server) = server(body.clone(), body.len(), Duration::from_millis(150));
    let a = Arc::new(cache(&dir));
    let b = Arc::new(cache(&dir));
    let barrier = Arc::new(Barrier::new(12));
    let threads = (0..12)
        .map(|i| {
            let c = if i % 2 == 0 { a.clone() } else { b.clone() };
            let url = url.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                c.acquire(&url).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let paths = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    server.join().unwrap();
    assert!(paths.iter().all(|p| p == &paths[0]));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
#[test]
fn different_urls_download_independently_without_global_network_mutex() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let serving = thread::spawn(move || {
        let start = std::time::Instant::now();
        let mut sockets = Vec::new();
        while sockets.len() < 2 && start.elapsed() < Duration::from_secs(5) {
            match listener.accept() {
                Ok((socket, _)) => sockets.push(socket),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(e) => panic!("{e}"),
            }
        }
        let n = sockets.len();
        for mut s in sockets {
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                s.read_exact(&mut b).unwrap();
                request.push(b[0]);
            }
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            s.write_all(&body).unwrap();
        }
        assert_eq!(n, 2, "both URLs must connect before either receives a body");
    });
    let c = Arc::new(cache(&dir));
    let a = c.clone();
    let first = thread::spawn(move || a.acquire(&format!("http://{addr}/a.png")));
    let second = c.acquire(&format!("http://{addr}/b.png"));
    assert!(first.join().unwrap().is_ok());
    assert!(second.is_ok());
    serving.join().unwrap();
}
#[test]
fn clear_preserves_unrelated_files_and_saved_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let (url, _, server) = server(body.clone(), body.len(), Duration::ZERO);
    let c = cache(&dir);
    let image = c.acquire(&url).unwrap();
    server.join().unwrap();
    let unrelated = c.root.join("family.jpg");
    fs::write(&unrelated, b"keep").unwrap();
    let collision = c.path(&"a".repeat(64), "img");
    fs::write(&collision, b"unowned").unwrap();
    let invalid_marker = c.path(&"b".repeat(64), "json");
    fs::write(&invalid_marker, b"{}").unwrap();
    let downloads = dir.path().join("downloads");
    fs::create_dir(&downloads).unwrap();
    fs::write(downloads.join("saved.jpg"), b"saved").unwrap();
    let nested = c.root.join("unrelated");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("keep"), b"keep").unwrap();
    c.clear().unwrap();
    assert!(!Path::new(&image).exists());
    assert_eq!(c.usage().unwrap().files, 0);
    assert_eq!(fs::read(unrelated).unwrap(), b"keep");
    assert_eq!(fs::read(collision).unwrap(), b"unowned");
    assert!(invalid_marker.exists());
    assert!(downloads.join("saved.jpg").exists());
    assert!(nested.join("keep").exists());
}
#[test]
fn truncated_and_malformed_responses_leave_no_parts_or_commits() {
    for (body, advertised) in [
        (image_format::fixture(), 500),
        (b"<html>not image</html>".to_vec(), 22),
        (image_format::fixture()[..40].to_vec(), 40),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (url, _, server) = server(body, advertised, Duration::ZERO);
        let c = cache(&dir);
        assert!(c.acquire(&url).is_err());
        server.join().unwrap();
        assert_eq!(c.usage().unwrap().files, 0);
        assert_eq!(fs::read_dir(&c.root).unwrap().count(), 1); // root marker only
    }
}
#[test]
fn interrupted_owned_parts_are_cleaned_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let c = cache(&dir);
    let hash = digest("interrupted");
    create(
        &c.path(&hash, "json"),
        &serde_json::to_vec(&Entry {
            owner: OWNER.into(),
            url_hash: hash.clone(),
            length: 0,
            sha256: String::new(),
            last_access: now(),
            complete: false,
        })
        .unwrap(),
    )
    .unwrap();
    create(&c.path(&hash, "part"), b"partial").unwrap();
    drop(c);
    let c = cache(&dir);
    assert_eq!(fs::read_dir(&c.root).unwrap().count(), 1);
}
#[test]
fn lru_evicts_oldest_but_preserves_new_original() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let mut c = cache(&dir);
    c.limit = body.len() as u64 * 2;
    let mut paths = Vec::new();
    let mut urls = Vec::new();
    for _ in 0..2 {
        let (url, _, s) = server(body.clone(), body.len(), Duration::ZERO);
        paths.push(c.acquire(&url).unwrap());
        urls.push(url);
        s.join().unwrap();
        thread::sleep(Duration::from_millis(10));
    }
    c.acquire(&urls[0]).unwrap();
    thread::sleep(Duration::from_millis(10));
    let (url, _, s) = server(body.clone(), body.len(), Duration::ZERO);
    let third = c.acquire(&url).unwrap();
    s.join().unwrap();
    assert!(Path::new(&paths[0]).exists());
    assert!(!Path::new(&paths[1]).exists());
    assert!(Path::new(&third).exists());
    assert_eq!(c.usage().unwrap().files, 2);
}
#[test]
fn symlink_and_reparse_roots_and_files_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let c = cache(&dir);
    let target = dir.path().join("keep");
    fs::write(&target, b"keep").unwrap();
    let hash = digest("link");
    let link = c.path(&hash, "img");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&target, &link);
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&target, &link);
    if let Err(e) = made {
        #[cfg(windows)]
        if e.raw_os_error() == Some(1314) {
            return;
        } // No developer-mode symlink privilege.
        panic!("symlink fixture failed: {e}");
    }
    assert!(regular(&link).is_err());
    create(
        &c.path(&hash, "json"),
        &serde_json::to_vec(&Entry {
            owner: OWNER.into(),
            url_hash: hash.clone(),
            length: 4,
            sha256: digest("keep"),
            last_access: now(),
            complete: true,
        })
        .unwrap(),
    )
    .unwrap();
    assert!(c.clear().is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    fs::remove_file(&link).unwrap();
    c.clear().unwrap();
    drop(c);
    let linked = dir.path().join("linked");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(dir.path().join("images"), &linked).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path().join("images"), &linked).unwrap();
    assert!(ImageCache::new(linked.to_str().unwrap()).is_err());
}
