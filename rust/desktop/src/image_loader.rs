//! Bounded concurrent thumbnail loading and streaming original downloads.
use std::collections::HashMap;
#[cfg(test)]
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct DecodedImage {
    pub w: u32,
    pub h: u32,
    pub rgba: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginalProgress {
    Connecting,
    Downloading {
        received: u64,
        total: Option<u64>,
    },
    Decoding,
    Ready {
        network_ms: u64,
        decode_ms: u64,
        cache_hit: bool,
    },
}

struct OriginalState {
    epoch: u64,
    cached: Option<(String, DecodedImage)>,
    flights: HashMap<String, Arc<OriginalFlight>>,
}

struct OriginalFlight {
    state: Mutex<FlightState>,
    changed: Condvar,
}

struct FlightState {
    stage: OriginalProgress,
    result: Option<Result<DecodedImage, String>>,
}

impl OriginalFlight {
    fn new() -> Self {
        Self {
            state: Mutex::new(FlightState {
                stage: OriginalProgress::Connecting,
                result: None,
            }),
            changed: Condvar::new(),
        }
    }

    fn publish(&self, stage: OriginalProgress) {
        self.state.lock().unwrap().stage = stage;
        self.changed.notify_all();
    }

    fn wait<F: Fn(OriginalProgress)>(&self, progress: &F) -> Result<DecodedImage, String> {
        progress(OriginalProgress::Connecting);
        let mut previous: Option<OriginalProgress> = Some(OriginalProgress::Connecting);
        loop {
            let (stage, result) = {
                let state = self.state.lock().unwrap();
                let state = self
                    .changed
                    .wait_while(state, |state| {
                        previous.as_ref() == Some(&state.stage) && state.result.is_none()
                    })
                    .unwrap();
                (state.stage.clone(), state.result.clone())
            };
            if previous.as_ref() != Some(&stage) {
                progress(stage.clone());
                previous = Some(stage);
            }
            if let Some(result) = result {
                return result;
            }
        }
    }
}

type Sink = Arc<dyn Fn(usize, Result<DecodedImage, String>) + Send + Sync>;
struct Job {
    index: usize,
    url: String,
    generation: u64,
    sink: Sink,
}

pub struct ImageLoader {
    tx: mpsc::Sender<Job>,
    generation: Arc<AtomicU64>,
    http: reqwest::blocking::Client,
    original: Arc<Mutex<OriginalState>>,
    _cache_registration: Arc<crate::cache_control::LoaderCache>,
    disk: Arc<crate::image_disk_cache::DiskCache>,
}
// Shared by production and the ignored transport probe so they use identical
// timeouts, TLS settings, and user agent; only the probe can force HTTP/1.1.
fn http_client_builder() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder()
        .user_agent("WallHavenStudio/0.1")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
}

impl ImageLoader {
    pub fn new(threads: usize) -> Self {
        Self::with_disk_cache(threads, crate::image_disk_cache::shared())
    }
    fn with_disk_cache(threads: usize, disk: Arc<crate::image_disk_cache::DiskCache>) -> Self {
        let http = http_client_builder().build().expect("HTTP client");
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let generation = Arc::new(AtomicU64::new(0));
        let original = Arc::new(Mutex::new(OriginalState {
            epoch: 0,
            cached: None,
            flights: HashMap::new(),
        }));
        let size_original = Arc::downgrade(&original);
        let clear_original = Arc::downgrade(&original);
        let registration = crate::cache_control::register_loader(
            move || {
                size_original
                    .upgrade()
                    .and_then(|original| {
                        original
                            .lock()
                            .unwrap()
                            .cached
                            .as_ref()
                            .map(|(_, image)| image.rgba.as_bytes().len() as u64)
                    })
                    .unwrap_or(0)
            },
            move || {
                if let Some(original) = clear_original.upgrade() {
                    let mut original = original.lock().unwrap();
                    original.epoch = original.epoch.wrapping_add(1);
                    original.cached = None;
                    original.flights.clear();
                }
            },
        );
        for _ in 0..threads.clamp(1, 8) {
            let rx = rx.clone();
            let current = generation.clone();
            let disk = disk.clone();
            let http = http.clone();
            std::thread::spawn(move || loop {
                // Release queue lock before doing any network/decode work.
                let job = {
                    let guard = rx.lock().unwrap();
                    match guard.recv() {
                        Ok(j) => j,
                        Err(_) => return,
                    }
                };
                if current.load(Ordering::Acquire) != job.generation {
                    continue;
                }
                let result = disk
                    .load(&http, &job.url, Some(8 * 1024 * 1024), |_, _| {})
                    .and_then(|data| {
                        let result = decode(&data.bytes, 640);
                        if result.is_err() {
                            let _ = disk.discard(&job.url);
                        }
                        result
                    });
                if current.load(Ordering::Acquire) == job.generation {
                    (job.sink)(job.index, result);
                }
            });
        }
        Self {
            tx,
            generation,
            http,
            original,
            _cache_registration: registration,
            disk,
        }
    }
    pub fn invalidate(&self) -> u64 {
        self.clear_original();
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }
    pub fn clear_original(&self) {
        let mut original = self.original.lock().unwrap();
        original.epoch = original.epoch.wrapping_add(1);
        original.cached = None;
        original.flights.clear();
    }
    pub fn load_batch<F>(&self, items: Vec<(usize, String)>, generation: u64, sink: F)
    where
        F: Fn(usize, Result<DecodedImage, String>) + Send + Sync + 'static,
    {
        let sink: Sink = Arc::new(sink);
        for (index, url) in items {
            let _ = self.tx.send(Job {
                index,
                url,
                generation,
                sink: sink.clone(),
            });
        }
    }
    /// Load the original at its native resolution. Thumbnail budgets do not apply here.
    #[cfg(test)]
    pub fn preview(&self, url: &str) -> Result<DecodedImage, String> {
        self.preview_observed(url, |_| {})
    }

    pub fn preview_observed<F: Fn(OriginalProgress)>(
        &self,
        url: &str,
        progress: F,
    ) -> Result<DecodedImage, String> {
        let (epoch, flight, leader) = {
            let mut original = self.original.lock().unwrap();
            if let Some((cached_url, image)) = original.cached.as_ref() {
                if cached_url == url {
                    let image = image.clone();
                    drop(original);
                    progress(OriginalProgress::Ready {
                        network_ms: 0,
                        decode_ms: 0,
                        cache_hit: true,
                    });
                    return Ok(image);
                }
            }
            let epoch = original.epoch;
            if let Some(flight) = original.flights.get(url) {
                (epoch, flight.clone(), false)
            } else {
                let flight = Arc::new(OriginalFlight::new());
                original.flights.insert(url.to_owned(), flight.clone());
                (epoch, flight, true)
            }
        };
        if !leader {
            return flight.wait(&progress);
        }
        let emit = |stage: OriginalProgress| {
            flight.publish(stage.clone());
            progress(stage);
        };
        emit(OriginalProgress::Connecting);
        let result = self.fetch_original(url, &emit);
        let mut original = self.original.lock().unwrap();
        if original.epoch == epoch {
            if let Ok(image) = &result {
                original.cached = Some((url.to_owned(), image.clone()));
            }
            original.flights.remove(url);
        }
        // Completion is published under the original state lock so late callers
        // see either this flight or a cached successful result.
        {
            let mut state = flight.state.lock().unwrap();
            state.result = Some(result.clone());
        }
        flight.changed.notify_all();
        result
    }

    fn fetch_original<F: Fn(OriginalProgress)>(
        &self,
        url: &str,
        emit: &F,
    ) -> Result<DecodedImage, String> {
        let data = self.disk.load(&self.http, url, None, |received, total| {
            emit(OriginalProgress::Downloading { received, total })
        })?;
        let network_ms = data.network_ms;
        let decoding = Instant::now();
        emit(OriginalProgress::Decoding);
        let result = (|| {
            let mut reader = image::ImageReader::new(std::io::Cursor::new(data.bytes.as_slice()))
                .with_guessed_format()
                .map_err(|error| error.to_string())?;
            reader.no_limits();
            reader
                .decode()
                .map_err(|error| error.to_string())
                .map(|image| image.into_rgba8())
        })();
        let rgba = match result {
            Ok(rgba) => rgba,
            Err(error) => {
                let _ = self.disk.discard(url);
                return Err(error);
            }
        };
        let image = DecodedImage {
            w: rgba.width(),
            h: rgba.height(),
            rgba: slint::SharedPixelBuffer::clone_from_slice(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
            ),
        };
        emit(OriginalProgress::Ready {
            network_ms,
            decode_ms: decoding.elapsed().as_millis() as u64,
            cache_hit: data.disk_hit,
        });
        Ok(image)
    }
    pub fn download_to<F: Fn(u64, Option<u64>)>(
        &self,
        url: &str,
        path: &Path,
        progress: F,
    ) -> Result<(), String> {
        if path.exists() {
            return Err("文件已存在，未覆盖".into());
        }
        let data = self.disk.load(&self.http, url, None, &progress)?;
        crate::image_disk_cache::save_copy(&data.bytes, path, progress)
    }
}
fn decode(bytes: &[u8], edge: u32) -> Result<DecodedImage, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 * 1024 * 1024);
    limits.max_image_width = Some(20000);
    limits.max_image_height = Some(20000);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| e.to_string())?;
    let image = if image.width() > edge || image.height() > edge {
        image.resize(edge, edge, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    Ok(DecodedImage {
        w: rgba.width(),
        h: rgba.height(),
        rgba: slint::SharedPixelBuffer::clone_from_slice(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
        ),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bad_image_has_explicit_error() {
        assert!(decode(b"not an image", 640).is_err());
    }
    fn temporary_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wh-download-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn serve_once(response: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/image", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            stream.write_all(response).unwrap();
        });
        (url, handle)
    }
    fn serve_preview_image(body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/original", listener.local_addr().unwrap());
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
        (url, server)
    }
    #[test]
    fn original_preview_loads_large_jpeg_at_full_resolution_without_artificial_memory_limit() {
        let (url, server) = serve_preview_image(
            include_bytes!("../tests/fixtures/preview-12000x8000.jpg").to_vec(),
        );
        let result = ImageLoader::new(1).preview(&url);
        server.join().unwrap();
        let image = result
            .unwrap_or_else(|error| panic!("valid original must load, not fail with: {error}"));
        assert_eq!(
            (image.w, image.h),
            (12000, 8000),
            "original was silently downscaled"
        );
        assert_eq!(image.rgba.as_bytes().len(), 384_000_000);
        assert_eq!(image.rgba.as_bytes()[3], 255);
    }
    #[test]
    fn original_preview_reopen_reuses_the_decoded_original_without_a_second_http_request() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            7,
            5,
            image::Rgba([10, 20, 30, 255]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        let (url, server) = serve_preview_image(bytes.into_inner());
        let loader = ImageLoader::new(1);
        let stages = std::cell::RefCell::new(Vec::new());
        let first = loader
            .preview_observed(&url, |stage| stages.borrow_mut().push(stage))
            .unwrap();
        server.join().unwrap();
        let reopen = std::cell::RefCell::new(Vec::new());
        let second = loader
            .preview_observed(&url, |stage| reopen.borrow_mut().push(stage))
            .unwrap_or_else(|error| {
                panic!("reopening original unnecessarily requested the closed server: {error}")
            });
        assert_eq!((second.w, second.h), (7, 5));
        assert_eq!(second.rgba.as_bytes(), first.rgba.as_bytes());
        assert!(
            matches!(stages.borrow().as_slice(), [OriginalProgress::Connecting, OriginalProgress::Downloading { received: 0, total: Some(_) }, OriginalProgress::Downloading { received, total: Some(_) }, OriginalProgress::Decoding, OriginalProgress::Ready { cache_hit: false, .. }] if *received > 0)
        );
        assert!(matches!(
            reopen.borrow().as_slice(),
            [OriginalProgress::Ready {
                cache_hit: true,
                network_ms: 0,
                decode_ms: 0
            }]
        ));
        assert_eq!(loader._cache_registration.bytes(), 7 * 5 * 4);
        loader._cache_registration.clear();
        assert_eq!(loader._cache_registration.bytes(), 0);
        assert_eq!(
            first.rgba.as_bytes()[3],
            255,
            "clearing cache discarded active image pixels"
        );
        assert!(
            {
                let _ = loader.disk.clear();
                loader.preview(&url)
            }
            .is_err(),
            "invalidated original cache survived permission/query reset"
        );
    }
    #[test]
    fn preview_then_download_uses_the_cached_original_with_server_offline() {
        let body = tiny_png();
        let (url, server) = serve_preview_image(body.clone());
        let loader = ImageLoader::new(1);
        let original = loader.preview(&url).unwrap();
        server.join().unwrap(); // The only network response is gone before Save is clicked.
        let directory = temporary_dir();
        let destination = directory.join("saved.png");
        let result = loader.download_to(&url, &destination, |_, _| {});
        assert!(
            result.is_ok(),
            "Save re-requested the already loaded original: {result:?}"
        );
        assert_eq!(std::fs::read(&destination).unwrap(), body);
        assert_eq!((original.w, original.h), (7, 5));
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn restarting_loader_reads_disk_and_saves_original_without_network_or_reencoding() {
        let owner = crate::image_disk_cache::DiskCache::temporary();
        let body = tiny_png();
        let (url, server) = serve_preview_image(body.clone());
        let loader = ImageLoader::with_disk_cache(1, owner.clone());
        loader.preview(&url).unwrap();
        server.join().unwrap();
        drop(loader);
        let reopened_cache =
            crate::image_disk_cache::DiskCache::at(owner.path().unwrap().to_owned());
        let loader = ImageLoader::with_disk_cache(1, reopened_cache);
        let stages = std::cell::RefCell::new(Vec::new());
        let image = loader
            .preview_observed(&url, |stage| stages.borrow_mut().push(stage))
            .unwrap();
        assert_eq!((image.w, image.h), (7, 5));
        assert!(stages.borrow().iter().any(|stage| matches!(
            stage,
            OriginalProgress::Ready {
                cache_hit: true,
                network_ms: 0,
                ..
            }
        )));
        let directory = temporary_dir();
        let saved = directory.join("saved.png");
        loader.download_to(&url, &saved, |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), body);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    #[ignore = "helper process for disk-cache restart regression"]
    fn disk_cache_restart_child() {
        let Some(root) = std::env::var_os("WH_IMAGE_CACHE_CHILD_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("wh-image-cache-test-"));
        let url = std::env::var("WH_IMAGE_CACHE_CHILD_URL").unwrap();
        assert_eq!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("127.0.0.1")
        );
        let destination =
            std::path::PathBuf::from(std::env::var_os("WH_IMAGE_CACHE_CHILD_SAVE").unwrap());
        let loader = ImageLoader::with_disk_cache(1, crate::image_disk_cache::DiskCache::at(root));
        let image = loader
            .preview(&url)
            .expect("child must read original from disk without server");
        assert_eq!((image.w, image.h), (7, 5));
        loader
            .download_to(&url, &destination, |_, _| {})
            .expect("child save must reuse disk cache");
    }
    #[test]
    fn independent_process_reuses_cached_original_and_saves_with_server_offline() {
        let owner = crate::image_disk_cache::DiskCache::temporary();
        let body = tiny_png();
        let (url, server) = serve_preview_image(body.clone());
        let loader = ImageLoader::with_disk_cache(1, owner.clone());
        loader.preview(&url).unwrap();
        server.join().unwrap();
        drop(loader);
        let directory = temporary_dir();
        let destination = directory.join("child-save.png");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "image_loader::tests::disk_cache_restart_child",
                "--ignored",
                "--nocapture",
            ])
            .env("WH_IMAGE_CACHE_CHILD_ROOT", owner.path().unwrap())
            .env("WH_IMAGE_CACHE_CHILD_URL", url)
            .env("WH_IMAGE_CACHE_CHILD_SAVE", &destination)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child process failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read(&destination).unwrap(), body);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn thumbnail_cache_stats_and_clear_leave_delivered_image_valid() {
        let body = tiny_png();
        let expected = body.len() as u64;
        let (url, server) = serve_preview_image(body);
        let loader = ImageLoader::new(1);
        let token = loader.invalidate();
        let (tx, rx) = mpsc::channel();
        loader.load_batch(vec![(0, url)], token, move |_, result| {
            let _ = tx.send(result);
        });
        let image = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        server.join().unwrap();
        assert_eq!(
            loader._cache_registration.bytes(),
            0,
            "compressed thumbnail must live on disk"
        );
        assert!(loader.disk.usage().unwrap().bytes >= expected);
        loader.disk.clear().unwrap();
        assert_eq!(loader.disk.usage().unwrap().bytes, 0);
        assert_eq!((image.w, image.h), (7, 5));
        assert_eq!(image.rgba.as_bytes()[3], 255);
    }
    #[test]
    fn saving_while_original_is_loading_joins_the_existing_file_download() {
        let disk = crate::image_disk_cache::DiskCache::temporary();
        let loader = Arc::new(ImageLoader::with_disk_cache(1, disk));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/shared.png", listener.local_addr().unwrap());
        let (seen_tx, seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let body = tiny_png();
        let expected = body.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = stream.read(&mut req);
            seen_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            reply(&mut stream, "200 OK", &body);
            listener.set_nonblocking(true).unwrap();
            let until = Instant::now() + Duration::from_millis(300);
            let mut extra = 0;
            while Instant::now() < until {
                if let Ok((mut stream, _)) = listener.accept() {
                    extra += 1;
                    reply(&mut stream, "503 Service Unavailable", b"");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            extra
        });
        let a = loader.clone();
        let first_url = url.clone();
        let preview = std::thread::spawn(move || a.preview(&first_url).unwrap());
        seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let directory = temporary_dir();
        let target = directory.join("saved.png");
        let save_target = target.clone();
        let b = loader.clone();
        let save = std::thread::spawn(move || b.download_to(&url, &save_target, |_, _| {}));
        std::thread::sleep(Duration::from_millis(100));
        release_tx.send(()).unwrap();
        preview.join().unwrap();
        save.join().unwrap().unwrap();
        assert_eq!(
            server.join().unwrap(),
            0,
            "Save duplicated the in-flight original request"
        );
        assert_eq!(std::fs::read(target).unwrap(), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn gallery_invalidation_does_not_detach_another_windows_disk_download() {
        let disk = crate::image_disk_cache::DiskCache::temporary();
        let detail = Arc::new(ImageLoader::with_disk_cache(1, disk.clone()));
        let gallery = ImageLoader::with_disk_cache(1, disk);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/original.png", listener.local_addr().unwrap());
        let (seen_tx, seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let body = tiny_png();
        let expected = body.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            seen_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            reply(&mut stream, "200 OK", &body);
        });
        let loading = detail.clone();
        let loading_url = url.clone();
        let preview = std::thread::spawn(move || loading.preview(&loading_url));
        seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        gallery.invalidate();
        release_tx.send(()).unwrap();
        preview.join().unwrap().unwrap();
        server.join().unwrap();
        let directory = temporary_dir();
        let destination = directory.join("saved.png");
        let result = detail.download_to(&url, &destination, |_, _| {});
        assert!(
            result.is_ok(),
            "gallery invalidation lost disk cache: {result:?}"
        );
        assert_eq!(std::fs::read(destination).unwrap(), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }
    fn tiny_png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            7,
            5,
            image::Rgba([10, 20, 30, 255]),
        ))
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
        out.into_inner()
    }
    fn reply(stream: &mut std::net::TcpStream, status: &str, body: &[u8]) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    }
    #[test]
    fn concurrent_original_observers_share_one_request_and_pixels() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/original", listener.local_addr().unwrap());
        let (seen_tx, seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).unwrap();
            seen_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            reply(&mut stream, "200 OK", &tiny_png());
            listener.set_nonblocking(true).unwrap();
            let until = Instant::now() + Duration::from_millis(300);
            let mut extra = 0;
            while Instant::now() < until {
                if let Ok((mut stream, _)) = listener.accept() {
                    extra += 1;
                    reply(&mut stream, "503 Service Unavailable", b"");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            extra
        });
        let loader = Arc::new(ImageLoader::new(1));
        let first_loader = loader.clone();
        let first_url = url.clone();
        let first = std::thread::spawn(move || first_loader.preview(&first_url).unwrap());
        seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let second_loader = loader.clone();
        let second = std::thread::spawn(move || {
            let stages = std::cell::RefCell::new(Vec::new());
            let result =
                second_loader.preview_observed(&url, |stage| stages.borrow_mut().push(stage));
            (result, stages.into_inner())
        });
        std::thread::sleep(Duration::from_millis(150));
        release_tx.send(()).unwrap();
        let first = first.join().unwrap();
        let (second, stages) = second.join().unwrap();
        let second = second.unwrap();
        assert_eq!(server.join().unwrap(), 0, "duplicate original HTTP request");
        assert_eq!(
            first.rgba.as_bytes().as_ptr(),
            second.rgba.as_bytes().as_ptr()
        );
        assert!(stages.iter().any(|s| matches!(
            s,
            OriginalProgress::Ready {
                cache_hit: false,
                ..
            }
        )));
    }
    #[test]
    fn concurrent_original_errors_are_shared_but_retryable() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/original", listener.local_addr().unwrap());
        let (seen_tx, seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = first.read(&mut request).unwrap();
            seen_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            reply(&mut first, "503 Service Unavailable", b"");
            let (mut retry, _) = listener.accept().unwrap();
            let _ = retry.read(&mut request).unwrap();
            reply(&mut retry, "200 OK", &tiny_png());
        });
        let loader = Arc::new(ImageLoader::new(1));
        let a = loader.clone();
        let first_url = url.clone();
        let first = std::thread::spawn(move || a.preview(&first_url));
        seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let b = loader.clone();
        let second_url = url.clone();
        let (joined_tx, joined_rx) = mpsc::channel();
        let second = std::thread::spawn(move || {
            b.preview_observed(&second_url, |stage| {
                if stage == OriginalProgress::Connecting {
                    let _ = joined_tx.send(());
                }
            })
        });
        joined_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        release_tx.send(()).unwrap();
        assert!(first.join().unwrap().err().unwrap().contains("503"));
        assert!(second.join().unwrap().err().unwrap().contains("503"));
        assert_eq!(loader.preview(&url).unwrap().w, 7);
        server.join().unwrap();
    }
    #[test]
    fn invalidation_drops_stale_pixels_but_keeps_completed_disk_file() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/original", listener.local_addr().unwrap());
        let (seen_tx, seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).unwrap();
            seen_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            reply(&mut stream, "200 OK", &tiny_png());
        });
        let loader = Arc::new(ImageLoader::new(1));
        let a = loader.clone();
        let first_url = url.clone();
        let first = std::thread::spawn(move || a.preview(&first_url).unwrap());
        seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        loader.invalidate();
        release_tx.send(()).unwrap();
        let old_image = first.join().unwrap();
        server.join().unwrap();
        assert!(
            loader.original.lock().unwrap().cached.is_none(),
            "stale decoded pixels refilled the new view generation"
        );
        assert!(loader.disk.usage().unwrap().bytes > 0);
        let current = loader
            .preview(&url)
            .expect("new generation must reuse disk offline");
        assert_ne!(
            old_image.rgba.as_bytes().as_ptr(),
            current.rgba.as_bytes().as_ptr()
        );
        assert_eq!(
            loader.preview(&url).unwrap().rgba.as_bytes().as_ptr(),
            current.rgba.as_bytes().as_ptr()
        );
    }
    #[test]
    fn original_preview_has_no_legacy_edge_or_response_size_cap() {
        let original = image::RgbaImage::from_pixel(20001, 1, image::Rgba([40, 80, 120, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(original)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let mut bytes = bytes.into_inner();
        // PNG permits trailing bytes: exercise a response larger than the old 128 MiB cap.
        bytes.resize(128 * 1024 * 1024 + 1, 0);
        let (url, server) = serve_preview_image(bytes);
        let result = ImageLoader::new(1).preview(&url);
        server.join().unwrap();
        let image = result.unwrap_or_else(|error| panic!("original hit a legacy limit: {error}"));
        assert_eq!((image.w, image.h), (20001, 1));
        assert_eq!(&image.rgba.as_bytes()[..4], &[40, 80, 120, 255]);
    }
    #[test]
    fn original_preview_reports_bad_images_and_http_failures() {
        let (url, server) = serve_preview_image(b"not an image".to_vec());
        assert!(ImageLoader::new(1).preview(&url).is_err());
        server.join().unwrap();
        let (url, server) =
            serve_once(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert!(ImageLoader::new(1)
            .preview(&url)
            .err()
            .expect("404 must fail")
            .contains("404"));
        server.join().unwrap();
    }
    #[test]
    fn streaming_download_saves_complete_bytes_and_reports_progress() {
        let dir = temporary_dir();
        let path = dir.join("image.jpg");
        let (url, server) =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nimage");
        let loader = ImageLoader::new(1);
        let progress = std::cell::Cell::new(0);
        loader
            .download_to(&url, &path, |done, _| progress.set(done))
            .unwrap();
        server.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"image");
        assert_eq!(progress.get(), 5);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn failed_download_leaves_no_partial_or_final_file() {
        let dir = temporary_dir();
        let path = dir.join("image.jpg");
        for response in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort".as_slice(),
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".as_slice(),
        ] {
            let (url, server) = serve_once(response);
            assert!(ImageLoader::new(1)
                .download_to(&url, &path, |_, _| {})
                .is_err());
            server.join().unwrap();
            assert!(!path.exists());
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn existing_download_is_never_overwritten() {
        let dir = temporary_dir();
        let path = dir.join("existing.jpg");
        std::fs::write(&path, b"keep").unwrap();
        assert!(ImageLoader::new(1)
            .download_to("http://127.0.0.1:1/no-network", &path, |_, _| {})
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod original_speed_probe {
    use super::*;
    #[test]
    #[ignore = "read-only live original timing; downloads public SFW image into memory"]
    fn live_original_cold_and_reopen_timings() {
        let client = wh_core::WallhavenClient::new(None).unwrap();
        let wall = client.wallpaper("w5mqr6").unwrap().data.unwrap();
        let path = wall.path.unwrap();
        let loader = ImageLoader::new(1);
        let started = Instant::now();
        let phases = std::cell::RefCell::new(Vec::new());
        let first = loader
            .preview_observed(&path, |stage| phases.borrow_mut().push(stage))
            .unwrap();
        let cold = started.elapsed();
        let started = Instant::now();
        let second = loader.preview(&path).unwrap();
        let warm = started.elapsed();
        assert_eq!((first.w, first.h), (6240, 4160));
        assert_eq!((second.w, second.h), (6240, 4160));
        assert_eq!(
            first.rgba.as_bytes().as_ptr(),
            second.rgba.as_bytes().as_ptr(),
            "cached pixels were copied"
        );
        println!(
            "SFW_ORIGINAL=w5mqr6 SIZE={}x{} COLD_MS={} REOPEN_US={} PHASES={:?}",
            first.w,
            first.h,
            cold.as_millis(),
            warm.as_micros(),
            phases.into_inner()
        );
    }
    /// Run with `cargo test -p wh-desktop --release live_original_transport_comparison -- --ignored --nocapture`.
    /// Uses only the public SFW original; no credentials, cache headers, or on-disk cache.
    #[test]
    #[ignore = "live public SFW original: six full-size transfers and decodes"]
    fn live_original_transport_comparison() {
        let wall = wh_core::WallhavenClient::new(None)
            .unwrap()
            .wallpaper("w5mqr6")
            .unwrap()
            .data
            .unwrap();
        let url = wall.path.unwrap();
        let h1 = http_client_builder().http1_only().build().unwrap();
        let default = http_client_builder().build().unwrap();
        for round in 0..3 {
            // Alternate order to reduce bias from time-varying CDN throughput.
            let clients = if round % 2 == 0 {
                [("forced_h1", &h1), ("default", &default)]
            } else {
                [("default", &default), ("forced_h1", &h1)]
            };
            for (transport, client) in clients {
                let started = Instant::now();
                let mut response = client.get(&url).send().unwrap().error_for_status().unwrap();
                let version = response.version();
                let headers_ms = started.elapsed().as_millis();
                let mut bytes = Vec::new();
                response.read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes.len(), 4_315_271, "unexpected original response size");
                let network_ms = started.elapsed().as_millis();
                let decoding = Instant::now();
                let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes.as_slice()))
                    .with_guessed_format()
                    .unwrap();
                reader.no_limits();
                let rgba = reader.decode().unwrap().into_rgba8();
                let image = DecodedImage {
                    w: rgba.width(),
                    h: rgba.height(),
                    rgba: slint::SharedPixelBuffer::clone_from_slice(
                        rgba.as_raw(),
                        rgba.width(),
                        rgba.height(),
                    ),
                };
                assert_eq!((image.w, image.h), (6240, 4160));
                let decode_ms = decoding.elapsed().as_millis();
                println!(
                    "SFW_ORIGINAL=w5mqr6 round={} transport={} protocol={:?} bytes={} headers_ms={} network_ms={} decode_ms={}",
                    round + 1,
                    transport,
                    version,
                    bytes.len(),
                    headers_ms,
                    network_ms,
                    decode_ms,
                );
            }
        }
    }
}
