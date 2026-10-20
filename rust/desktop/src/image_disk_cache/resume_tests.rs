use super::*;
use std::net::{TcpListener, TcpStream};
fn request(s: &mut TcpStream) -> String {
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut b = Vec::new();
    while !b.ends_with(b"\r\n\r\n") {
        let mut c = [0];
        s.read_exact(&mut c).unwrap();
        b.push(c[0]);
    }
    String::from_utf8(b).unwrap()
}
fn accept(listener: &TcpListener) -> TcpStream {
    let end = Instant::now() + Duration::from_secs(6);
    loop {
        match listener.accept() {
            Ok((s, _)) => return s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < end, "expected resume request");
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("{e}"),
        }
    }
}
#[test]
fn real_pause_reopen_range_validator_200_restart_and_reject_mismatch() {
    for mode in 0..5 {
        let cache = DiskCache::temporary();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/full.png", listener.local_addr().unwrap());
        let body = (0..131072).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        let expected = body.clone();
        let server = std::thread::spawn(move || {
            let validator = if mode == 4 {
                "Last-Modified: Wed, 21 Oct 2015 07:28:00 GMT"
            } else {
                "ETag: \"stable\""
            };
            let mut first = accept(&listener);
            let initial = request(&mut first);
            assert!(!initial.to_lowercase().contains("range:"));
            write!(
                first,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{}\r\nConnection: close\r\n\r\n",
                body.len(),
                validator
            )
            .unwrap();
            first.write_all(&body[..32768]).unwrap();
            first.flush().unwrap();
            drop(first);
            let mut second = accept(&listener);
            let request = request(&mut second).to_lowercase();
            let range = request
                .lines()
                .find_map(|l| l.strip_prefix("range: bytes="))
                .unwrap();
            let start = range.trim_end_matches('-').parse::<usize>().unwrap();
            assert!(start > 0 && start <= 32768);
            assert!(request.contains(if mode == 4 {
                "if-range: wed, 21 oct 2015 07:28:00 gmt"
            } else {
                "if-range: \"stable\""
            }));
            if mode == 1 {
                write!(
                    second,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{}\r\nConnection: close\r\n\r\n",
                    body.len(),
                    validator
                )
                .unwrap();
                let _ = second.write_all(&body);
            } else {
                let first = if mode == 3 { start + 1 } else { start };
                let validator = if mode == 2 {
                    "ETag: \"changed\""
                } else {
                    validator
                };
                write!(second,"HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\n{}\r\nConnection: close\r\n\r\n",body.len()-start,first,body.len()-1,body.len(),validator).unwrap();
                let _ = second.write_all(&body[start..]);
            }
        });
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let paused = cache.load_controlled(&http, &url, |n, _| n == 0).unwrap();
        assert!(paused.is_none());
        let key = cache_key(&url).unwrap();
        let root = cache.path().unwrap().to_owned();
        let part = root.join(format!("{key}.resume.part"));
        assert!(part.is_file());
        let reopened = DiskCache::at(root.clone());
        let result = reopened.load_controlled(&http, &url, |_, _| true);
        if mode == 2 || mode == 3 {
            assert!(result.is_err(), "bad validator/range committed mode {mode}");
            assert!(!root.join(format!("{key}.png")).exists());
        } else {
            let data = result.unwrap().unwrap();
            assert_eq!(
                *data.bytes, expected,
                "response was incorrectly appended mode {mode}"
            );
            assert_eq!(
                *reopened.load(&http, &url, None, |_, _| {}).unwrap().bytes,
                expected
            );
        }
        server.join().unwrap();
    }
}
#[test]
fn cancellation_removes_paused_owned_partial_without_deleting_other_files() {
    let cache = DiskCache::temporary();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/full.png", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        request(&mut s);
        write!(s,"HTTP/1.1 200 OK\r\nContent-Length: 50000\r\nETag: \"stable\"\r\nConnection: close\r\n\r\n").unwrap();
        s.write_all(&[3; 4096]).unwrap();
    });
    let http = reqwest::blocking::Client::new();
    assert!(cache
        .load_controlled(&http, &url, |n, _| n == 0)
        .unwrap()
        .is_none());
    server.join().unwrap();
    let root = cache.path().unwrap();
    std::fs::write(root.join("unrelated.keep"), b"preserve").unwrap();
    let key = cache_key(&url).unwrap();
    cache.discard_partial(&url).unwrap();
    assert!(!root.join(format!("{key}.resume.part")).exists());
    assert!(!root.join(format!("{key}.resume.json")).exists());
    assert_eq!(
        std::fs::read(root.join("unrelated.keep")).unwrap(),
        b"preserve"
    );
}
