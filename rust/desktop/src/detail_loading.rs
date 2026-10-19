//! Coordinate independent metadata and original-image reads.
use super::*;

#[cfg(test)]
pub(super) fn start(
    wall: Wallpaper,
    token: u64,
    loader: Arc<ImageLoader>,
    tx: mpsc::Sender<Event>,
    fetch_detail: impl FnOnce(String) -> Result<Wallpaper, String> + Send + 'static,
) {
    start_observed(wall, token, loader, tx, fetch_detail, |_, _| {});
}

pub(super) fn start_observed(
    wall: Wallpaper,
    token: u64,
    loader: Arc<ImageLoader>,
    tx: mpsc::Sender<Event>,
    fetch_detail: impl FnOnce(String) -> Result<Wallpaper, String> + Send + 'static,
    progress: impl Fn(u64, image_loader::OriginalProgress) + Send + Sync + 'static,
) {
    let progress = Arc::new(progress);
    let known_path = wall.path.clone().filter(|path| !path.trim().is_empty());
    if let Some(path) = known_path.clone() {
        let loader = loader.clone();
        let tx = tx.clone();
        let progress = progress.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Preview(
                token,
                loader.preview_observed(&path, |stage| progress(token, stage)),
            ));
        });
    }
    std::thread::spawn(move || {
        let result = fetch_detail(wall.id);
        let fallback_path = result.as_ref().ok().and_then(|wall| wall.path.clone());
        let _ = tx.send(Event::Detail(token, Box::new(result)));
        if known_path.is_none() {
            let preview = fallback_path
                .map(|path| loader.preview_observed(&path, |stage| progress(token, stage)))
                .unwrap_or_else(|| Err("原图链接为空".into()));
            let _ = tx.send(Event::Preview(token, preview));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn known_original_starts_before_slow_detail_metadata_finishes() {
        let body = {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                3,
                2,
                image::Rgba([30, 60, 90, 255]),
            ))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
            bytes.into_inner()
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let path = format!("http://{}/original", listener.local_addr().unwrap());
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
            stream.write_all(&body).unwrap();
        });
        let (release_tx, release_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let started = std::time::Instant::now();
        start(
            Wallpaper {
                id: "test".into(),
                path: Some(path),
                ..Default::default()
            },
            7,
            Arc::new(ImageLoader::new(1)),
            tx,
            move |_| {
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(Wallpaper {
                    id: "test".into(),
                    ..Default::default()
                })
            },
        );
        let early = rx.recv_timeout(Duration::from_secs(1));
        release_tx.send(()).unwrap();
        server.join().unwrap();
        assert!(
            matches!(early,Ok(Event::Preview(7,Ok(ref image))) if (image.w,image.h)==(3,2)),
            "original was blocked behind metadata for {:?}",
            started.elapsed()
        );
    }
    #[test]
    fn missing_original_url_uses_detail_response_and_keeps_generation() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            4,
            image::Rgba([90, 60, 30, 255]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        let body = bytes.into_inner();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let path = format!("http://{}/original", listener.local_addr().unwrap());
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
            stream.write_all(&body).unwrap();
        });
        let (tx, rx) = mpsc::channel();
        start(
            Wallpaper {
                id: "fallback".into(),
                ..Default::default()
            },
            12,
            Arc::new(ImageLoader::new(1)),
            tx,
            move |_| {
                Ok(Wallpaper {
                    id: "fallback".into(),
                    path: Some(path),
                    ..Default::default()
                })
            },
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Event::Detail(12, _))
        ));
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(2)),Ok(Event::Preview(12,Ok(ref image))) if (image.w,image.h)==(2,4))
        );
        server.join().unwrap();
    }
}
