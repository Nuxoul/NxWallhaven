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
            s.set_nonblocking(false).unwrap();
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
fn interrupted_owned_parts_survive_restart_and_clear_is_owned() {
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
            validator: None,
            validator_header: None,
            total: None,
        })
        .unwrap(),
    )
    .unwrap();
    create(&c.path(&hash, "part"), b"partial").unwrap();
    drop(c);
    let c = cache(&dir);
    assert_eq!(fs::read(c.path(&hash, "part")).unwrap(), b"partial");
    assert!(c.hit(&hash).unwrap().is_none());
    c.clear().unwrap();
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
            validator: None,
            validator_header: None,
            total: None,
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

fn read_request(socket: &mut std::net::TcpStream) -> String {
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        request.push(byte[0]);
    }
    String::from_utf8(request).unwrap().to_lowercase()
}
fn seed_partial(c: &ImageCache, url: &str, body: &[u8], length: usize) {
    let hash = digest(url);
    create(
        &c.path(&hash, "json"),
        &serde_json::to_vec(&Entry {
            owner: OWNER.into(),
            url_hash: hash.clone(),
            length: length as u64,
            sha256: hex(&Sha256::digest(&body[..length])),
            last_access: now(),
            complete: false,
            validator: Some("\"original\"".into()),
            validator_header: Some("etag".into()),
            total: Some(body.len() as u64),
        })
        .unwrap(),
    )
    .unwrap();
    create(&c.path(&hash, "part"), &body[..length]).unwrap();
}
#[test]
fn resume_exact_range_checks_full_hash_and_offline_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/resume.png", listener.local_addr().unwrap());
    let c = cache(&dir);
    seed_partial(&c, &url, &body, 20);
    let sent = body.clone();
    let server = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let request = read_request(&mut s);
        assert!(request.contains("range: bytes=20-\r\n"));
        assert!(request.contains("if-range: \"original\"\r\n"));
        write!(s, "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 20-{}/{}\r\nETag: \"original\"\r\nConnection: close\r\n\r\n", sent.len()-20, sent.len()-1, sent.len()).unwrap();
        s.write_all(&sent[20..]).unwrap();
    });
    let path = c
        .acquire_controlled(&url, &mut |_, _| Control::Continue)
        .unwrap();
    server.join().unwrap();
    assert_eq!(fs::read(&path).unwrap(), body);
    let entry = c.entry(&digest(&url)).unwrap().unwrap();
    assert_eq!(entry.sha256, hex(&Sha256::digest(&body)));
    assert!(!c.path(&digest(&url), "part").exists());
    drop(c);
    assert_eq!(cache(&dir).acquire(&url).unwrap(), path);
}
#[test]
fn ignored_range_200_restarts_not_appends() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let (url, count, server) = server(body.clone(), body.len(), Duration::ZERO);
    let c = cache(&dir);
    seed_partial(&c, &url, &body, 20);
    let path = c
        .acquire_controlled(&url, &mut |_, _| Control::Continue)
        .unwrap();
    server.join().unwrap();
    assert_eq!(fs::read(path).unwrap(), body);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
#[test]
fn mismatched_validator_or_range_never_commits() {
    for variant in 0..4 {
        let dir = tempfile::tempdir().unwrap();
        let body = image_format::fixture();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/bad.png", listener.local_addr().unwrap());
        let c = cache(&dir);
        seed_partial(&c, &url, &body, 20);
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            read_request(&mut s);
            let range = match variant {
                1 => format!("bytes 21-{}/{}", body.len() - 1, body.len()),
                2 => "bytes nonsense".into(),
                3 => format!("bytes 20-{}/{}", body.len() - 1, body.len() + 1),
                _ => format!("bytes 20-{}/{}", body.len() - 1, body.len()),
            };
            let tag = if variant == 0 { "changed" } else { "original" };
            write!(s, "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: {range}\r\nETag: \"{tag}\"\r\nConnection: close\r\n\r\n",body.len()-20).unwrap();
            let _ = s.write_all(&body[20..]);
        });
        assert!(c
            .acquire_controlled(&url, &mut |_, _| Control::Continue)
            .is_err());
        server.join().unwrap();
        assert!(!c.path(&digest(&url), "img").exists());
        assert_eq!(c.usage().unwrap().files, 0);
    }
}
#[test]
fn real_pause_survives_new_instance_and_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/pause.png", listener.local_addr().unwrap());
    let sent = body.clone();
    let server = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        read_request(&mut s);
        write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"original\"\r\nConnection: close\r\n\r\n",sent.len()).unwrap();
        s.write_all(&sent[..20]).unwrap();
        s.flush().unwrap();
        thread::sleep(Duration::from_millis(150));
        let _ = s.write_all(&sent[20..]);
        drop(s);
        let (mut s, _) = listener.accept().unwrap();
        let request = read_request(&mut s);
        assert!(request.contains("range: bytes=20-\r\n"));
        assert!(request.contains("if-range: \"original\"\r\n"));
        write!(s, "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 20-{}/{}\r\nETag: \"original\"\r\nConnection: close\r\n\r\n",sent.len()-20,sent.len()-1,sent.len()).unwrap();
        s.write_all(&sent[20..]).unwrap();
    });
    let c = cache(&dir);
    assert!(c
        .acquire_controlled(&url, &mut |received, _| if received >= 20 {
            Control::Pause
        } else {
            Control::Continue
        })
        .unwrap()
        .is_empty());
    let entry = c.entry(&digest(&url)).unwrap().unwrap();
    assert!(!entry.complete);
    assert_eq!(entry.length, 20);
    assert_eq!(fs::read(c.path(&digest(&url), "part")).unwrap(), body[..20]);
    drop(c);
    let c = cache(&dir);
    assert!(c.hit(&digest(&url)).unwrap().is_none());
    let path = c
        .acquire_controlled(&url, &mut |_, _| Control::Continue)
        .unwrap();
    server.join().unwrap();
    assert_eq!(fs::read(path).unwrap(), body);
}
#[test]
fn cancel_removes_only_owned_partial_and_marker() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let (url, _, server) = server(body.clone(), body.len(), Duration::ZERO);
    let c = cache(&dir);
    let unrelated = c.root.join("keep.txt");
    fs::write(&unrelated, b"keep").unwrap();
    assert!(c
        .acquire_controlled(&url, &mut |_, _| Control::Cancel)
        .unwrap()
        .is_empty());
    server.join().unwrap();
    for suffix in ["part", "json", "json.part", "img"] {
        assert!(!c.path(&digest(&url), suffix).exists());
    }
    assert_eq!(fs::read(unrelated).unwrap(), b"keep");
}
#[test]
fn preview_flight_and_pausing_save_share_get_without_cancelling_preview() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/shared.png", listener.local_addr().unwrap());
    let (connected, ready) = std::sync::mpsc::channel();
    let sent = body.clone();
    let server = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        read_request(&mut s);
        connected.send(()).unwrap();
        thread::sleep(Duration::from_millis(250));
        write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            sent.len()
        )
        .unwrap();
        s.write_all(&sent).unwrap();
    });
    let c = Arc::new(cache(&dir));
    let preview = c.clone();
    let preview_url = url.clone();
    let thread = thread::spawn(move || preview.acquire(&preview_url).unwrap());
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(c
        .acquire_controlled(&url, &mut |_, _| Control::Pause)
        .unwrap()
        .is_empty());
    let path = thread.join().unwrap();
    server.join().unwrap();
    assert_eq!(fs::read(&path).unwrap(), body);
    // Listener is offline. A subsequent save reuses the preview's original.
    assert_eq!(
        c.acquire_controlled(&url, &mut |_, _| Control::Continue)
            .unwrap(),
        path
    );
}
#[test]
fn pausing_download_owner_keeps_joined_preview_alive() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let (url, count, server) = server(body.clone(), body.len(), Duration::from_millis(200));
    let c = Arc::new(cache(&dir));
    let owner = c.clone();
    let owner_url = url.clone();
    let started = Arc::new(AtomicBool::new(false));
    let signal = started.clone();
    let download = thread::spawn(move || {
        owner
            .acquire_controlled(&owner_url, &mut |_, _| {
                signal.store(true, Ordering::Release);
                // Wait deterministically for the preview to join this owner's flight.
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let flight = owner
                        .shared
                        .flights
                        .lock()
                        .unwrap()
                        .get(&digest(&owner_url))
                        .and_then(Weak::upgrade)
                        .unwrap();
                    if flight.readers.lock().unwrap().len() >= 2 {
                        break;
                    }
                    assert!(Instant::now() < deadline, "preview failed to join flight");
                    thread::sleep(Duration::from_millis(5));
                }
                Control::Pause
            })
            .unwrap()
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "owner did not begin");
        thread::sleep(Duration::from_millis(5));
    }
    let path = c.acquire(&url).unwrap();
    assert!(download.join().unwrap().is_empty());
    server.join().unwrap();
    assert!(started.load(Ordering::Acquire));
    assert_eq!(fs::read(path).unwrap(), body);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn missing_or_weak_validator_restarts_without_range() {
    for validator in [None, Some("W/\"weak\"")] {
        let dir = tempfile::tempdir().unwrap();
        let body = image_format::fixture();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/restart.png", listener.local_addr().unwrap());
        let c = cache(&dir);
        seed_partial(&c, &url, &body, 20);
        let mut entry = c.entry(&digest(&url)).unwrap().unwrap();
        entry.validator = validator.map(String::from);
        c.update(&digest(&url), &entry).unwrap();
        let sent = body.clone();
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let request = read_request(&mut s);
            assert!(!request.contains("range:"));
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                sent.len()
            )
            .unwrap();
            s.write_all(&sent).unwrap();
        });
        let path = c
            .acquire_controlled(&url, &mut |_, _| Control::Continue)
            .unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(path).unwrap(), body);
    }
}

#[test]
fn modified_validator_resumes_and_crash_tail_is_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/modified.png", listener.local_addr().unwrap());
    let c = cache(&dir);
    seed_partial(&c, &url, &body, 20);
    // Simulate a crash after bytes were written but before the next durable checkpoint.
    OpenOptions::new()
        .append(true)
        .open(c.path(&digest(&url), "part"))
        .unwrap()
        .write_all(b"uncheckpointed tail")
        .unwrap();
    let modified = "Wed, 21 Oct 2015 07:28:00 GMT";
    let mut entry = c.entry(&digest(&url)).unwrap().unwrap();
    entry.validator = Some(modified.into());
    entry.validator_header = Some("last-modified".into());
    c.update(&digest(&url), &entry).unwrap();
    drop(c);
    let sent = body.clone();
    let server = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let request = read_request(&mut s);
        assert!(request.contains("range: bytes=20-\r\n"));
        assert!(request.contains("if-range: wed, 21 oct 2015 07:28:00 gmt\r\n"));
        write!(s,"HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 20-{}/{}\r\nLast-Modified: {modified}\r\nConnection: close\r\n\r\n",sent.len()-20,sent.len()-1,sent.len()).unwrap();
        s.write_all(&sent[20..]).unwrap();
    });
    let path = cache(&dir)
        .acquire_controlled(&url, &mut |_, _| Control::Continue)
        .unwrap();
    server.join().unwrap();
    assert_eq!(fs::read(path).unwrap(), body);
}

#[test]
fn slow_original_verification_does_not_serialize_other_hits_or_misses() {
    // A deterministic stand-in for slow phone storage/SHA: suspend one full-file
    // verification, then require an unrelated hit and GET to finish before release.
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let (hot_url, hot_gets, hot_server) = server(body.clone(), body.len(), Duration::ZERO);
    let slow = Arc::new(cache(&dir));
    let hot_path = slow.acquire(&hot_url).unwrap();
    hot_server.join().unwrap();
    let (other_url, other_gets, other_server) = server(body.clone(), body.len(), Duration::ZERO);
    let other_path = slow.acquire(&other_url).unwrap();
    other_server.join().unwrap();
    let fast = Arc::new(cache(&dir));
    let (miss_url, miss_gets, miss_server) = server(body.clone(), body.len(), Duration::ZERO);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *slow.before_hit_hash.lock().unwrap() = Some(Box::new(move || {
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }));
    let owner = slow.clone();
    let slow_thread = thread::spawn(move || owner.acquire(&hot_url).unwrap());
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (hit_tx, hit_rx) = std::sync::mpsc::channel();
    let hit_cache = fast.clone();
    let hit_thread = thread::spawn(move || {
        let start = Instant::now();
        let path = hit_cache.acquire(&other_url).unwrap();
        hit_tx.send(start.elapsed()).unwrap();
        path
    });
    let (miss_tx, miss_rx) = std::sync::mpsc::channel();
    let miss_thread = thread::spawn(move || {
        let start = Instant::now();
        let path = fast.acquire(&miss_url).unwrap();
        miss_tx.send(start.elapsed()).unwrap();
        path
    });
    let hit_time = hit_rx.recv_timeout(Duration::from_millis(500));
    let miss_time = miss_rx.recv_timeout(Duration::from_millis(500));
    let hit_before_release = hit_time.is_ok();
    let miss_before_release = miss_time.is_ok();
    let gets_before_release = miss_gets.load(Ordering::SeqCst);
    release_tx.send(()).unwrap();
    assert_eq!(slow_thread.join().unwrap(), hot_path);
    assert_eq!(hit_thread.join().unwrap(), other_path);
    let miss_path = miss_thread.join().unwrap();
    miss_server.join().unwrap();
    let hit_elapsed = hit_time.unwrap_or_else(|_| hit_rx.recv().unwrap());
    let miss_elapsed = miss_time.unwrap_or_else(|_| miss_rx.recv().unwrap());
    eprintln!(
        "cache isolation timing: unrelated_hit={hit_elapsed:?} unrelated_miss={miss_elapsed:?}"
    );
    assert_eq!(fs::read(miss_path).unwrap(), body);
    assert_eq!(hot_gets.load(Ordering::SeqCst), 1);
    assert_eq!(other_gets.load(Ordering::SeqCst), 1);
    assert_eq!(miss_gets.load(Ordering::SeqCst), 1);
    eprintln!("cache isolation: unrelated_hit_before_release={hit_before_release} unrelated_miss_before_release={miss_before_release} miss_GETs_before_release={gets_before_release}");
    assert!(
        hit_before_release && miss_before_release && gets_before_release == 1,
        "full-file hit verification blocked unrelated cache hit/GET behind the disk mutex"
    );
}

#[test]
fn active_verification_still_blocks_clear_and_preserves_corruption_detection() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let (url, gets, server) = server(body.clone(), body.len(), Duration::ZERO);
    let mut instance = cache(&dir);
    instance.limit = 0; // Force eviction to consult the active-flight protection.
    let c = Arc::new(instance);
    let path = c.acquire(&url).unwrap();
    server.join().unwrap();
    // Same length, bad content: a metadata-only hit would incorrectly accept this.
    let mut damaged = body;
    damaged[45] ^= 1;
    fs::write(&path, damaged).unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *c.before_hit_hash.lock().unwrap() = Some(Box::new(move || {
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }));
    let checking = c.clone();
    let worker = thread::spawn(move || checking.acquire(&url));
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        c.shared.gate.try_write().is_err(),
        "clear must remain exclusive"
    );
    // Metadata operations can proceed, but eviction must retain this active file.
    {
        let _disk = c.shared.disk.lock().unwrap();
        c.evict(None).unwrap();
        assert!(Path::new(&path).exists());
    }
    release_tx.send(()).unwrap();
    assert!(worker.join().unwrap().is_err()); // Offline GET cannot replace corruption.
    assert!(!Path::new(&path).exists());
    assert_eq!(gets.load(Ordering::SeqCst), 1);
    c.clear().unwrap();
}

#[test]
fn distinct_images_reuse_one_client_connection() {
    let dir = tempfile::tempdir().unwrap();
    let body = image_format::fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        for index in 0..2 {
            let request = read_request(&mut socket);
            assert!(request.starts_with(&format!("get /{index}.png ")));
            assert!(!request.contains("cookie:"));
            assert!(!request.contains("x-api-key:"));
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .unwrap();
            socket.write_all(&body).unwrap();
        }
    });
    let c = cache(&dir);
    for index in 0..2 {
        assert_eq!(
            fs::read(c.acquire(&format!("http://{addr}/{index}.png")).unwrap()).unwrap(),
            image_format::fixture()
        );
    }
    serving.join().unwrap(); // Two GETs, exactly one accept; no per-image client rebuild.
}
