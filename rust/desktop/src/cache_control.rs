//! Image-cache controls. Disk operations target only ImageCache's owned files.
use std::cell::RefCell;
use std::rc::{Rc, Weak as RcWeak};
use std::sync::{mpsc, Arc, Mutex, OnceLock, Weak};

pub(super) struct LoaderCache {
    size: Box<dyn Fn() -> u64 + Send + Sync>,
    clear: Box<dyn Fn() + Send + Sync>,
}
impl LoaderCache {
    #[cfg(test)]
    pub(super) fn bytes(&self) -> u64 {
        (self.size)()
    }
    #[cfg(test)]
    pub(super) fn clear(&self) {
        (self.clear)();
    }
}
struct UiCache {
    size: Box<dyn Fn() -> u64>,
    clear: Box<dyn Fn()>,
}
static LOADERS: OnceLock<Mutex<Vec<Weak<LoaderCache>>>> = OnceLock::new();
thread_local! { static UI_CACHES: RefCell<Vec<RcWeak<UiCache>>> = const { RefCell::new(Vec::new()) }; }
pub(super) fn register_loader(
    size: impl Fn() -> u64 + Send + Sync + 'static,
    clear: impl Fn() + Send + Sync + 'static,
) -> Arc<LoaderCache> {
    let registration = Arc::new(LoaderCache {
        size: Box::new(size),
        clear: Box::new(clear),
    });
    LOADERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .push(Arc::downgrade(&registration));
    registration
}
pub(super) struct UiRegistration {
    _cache: Rc<UiCache>,
}
pub(super) fn register_ui(
    size: impl Fn() -> u64 + 'static,
    clear: impl Fn() + 'static,
) -> UiRegistration {
    let registration = Rc::new(UiCache {
        size: Box::new(size),
        clear: Box::new(clear),
    });
    UI_CACHES.with(|registry| registry.borrow_mut().push(Rc::downgrade(&registration)));
    UiRegistration {
        _cache: registration,
    }
}
fn loaders() -> Vec<Arc<LoaderCache>> {
    let mut registry = LOADERS.get_or_init(Default::default).lock().unwrap();
    let caches = registry
        .iter()
        .filter_map(Weak::upgrade)
        .collect::<Vec<_>>();
    registry.retain(|entry| entry.strong_count() > 0);
    caches
}
fn ui_caches() -> Vec<Rc<UiCache>> {
    UI_CACHES.with(|registry| {
        let mut registry = registry.borrow_mut();
        let caches = registry
            .iter()
            .filter_map(RcWeak::upgrade)
            .collect::<Vec<_>>();
        registry.retain(|entry| entry.strong_count() > 0);
        caches
    })
}
pub(super) fn memory_bytes() -> u64 {
    loaders().iter().map(|cache| (cache.size)()).sum::<u64>()
        + ui_caches().iter().map(|cache| (cache.size)()).sum::<u64>()
}
pub(super) fn clear_all() {
    for cache in loaders() {
        (cache.clear)();
    }
    for cache in ui_caches() {
        (cache.clear)();
    }
}
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
enum CacheEvent {
    Measured(Result<crate::image_disk_cache::Usage, String>),
    Cleared(Result<u64, String>),
}
pub(super) struct CacheTools {
    _timer: slint::Timer,
}
pub(super) fn bind(settings: &crate::SettingsWindow) -> CacheTools {
    use slint::ComponentHandle;
    let cache = crate::image_disk_cache::shared();
    settings.set_cache_directory(
        cache
            .path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|error| error)
            .into(),
    );
    let (tx, rx) = mpsc::channel();
    let busy = Rc::new(std::cell::Cell::new(false));
    let weak = settings.as_weak();
    let current = cache.clone();
    let sender = tx.clone();
    let waiting = busy.clone();
    settings.on_refresh_image_cache(move || {
        if waiting.replace(true) {
            return;
        }
        if let Some(window) = weak.upgrade() {
            window.set_cache_busy(true);
            window.set_cache_memory(format_bytes(memory_bytes()).into());
        }
        let cache = current.clone();
        let tx = sender.clone();
        std::thread::spawn(move || {
            let _ = tx.send(CacheEvent::Measured(cache.usage()));
        });
    });
    let weak = settings.as_weak();
    let current = cache.clone();
    let sender = tx.clone();
    let waiting = busy.clone();
    settings.on_clear_image_cache(move || {
        if waiting.replace(true) {
            return;
        }
        clear_all();
        if let Some(window) = weak.upgrade() {
            window.set_cache_busy(true);
            window.set_cache_memory(format_bytes(memory_bytes()).into());
            window.set_cache_status("正在清除本地图片缓存…".into());
        }
        let cache = current.clone();
        let tx = sender.clone();
        std::thread::spawn(move || {
            let _ = tx.send(CacheEvent::Cleared(cache.clear()));
        });
    });
    let timer = slint::Timer::default();
    let weak = settings.as_weak();
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(5);
    timer.start(slint::TimerMode::Repeated,std::time::Duration::from_millis(100),move || {
        let Some(window)=weak.upgrade()else{return};
        for event in rx.try_iter(){
            busy.set(false);window.set_cache_busy(false);
            match event {
                CacheEvent::Measured(result)=>window.set_cache_disk(result.map(|usage|format!("{} · {} 个缓存文件",format_bytes(usage.bytes),usage.files)).unwrap_or_else(|error|format!("统计失败：{error}")).into()),
                CacheEvent::Cleared(result)=>{
                    window.set_cache_status(result.map(|bytes|format!("已清除 {} 磁盘图片缓存。当前显示的图片保留；下载文件、壁纸、账号和登录数据未改动。",format_bytes(bytes))).unwrap_or_else(|error|format!("清除未全部完成：{error}")).into());
                    last=std::time::Instant::now()-std::time::Duration::from_secs(5);
                }
            }
        }
        if window.window().is_visible(){
            window.set_cache_memory(format_bytes(memory_bytes()).into());
            if !busy.get()&&last.elapsed()>=std::time::Duration::from_secs(2){last=std::time::Instant::now();busy.set(true);window.set_cache_busy(true);let cache=cache.clone();let tx=tx.clone();std::thread::spawn(move||{let _=tx.send(CacheEvent::Measured(cache.usage()));});}
        }
    });
    CacheTools { _timer: timer }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_reports_real_cache_bytes_clears_and_releases_closed_views() {
        let bytes = Rc::new(std::cell::Cell::new(8192u64));
        let size = bytes.clone();
        let clear = bytes.clone();
        let registration = register_ui(move || size.get(), move || clear.set(0));
        assert_eq!((registration._cache.size)(), 8192);
        (registration._cache.clear)();
        assert_eq!(bytes.get(), 0);
        drop(registration);
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MiB");
    }
    #[test]
    fn settings_cache_action_clears_memory_without_saving_credentials() {
        use slint::ComponentHandle;
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::SettingsWindow::new().unwrap();
        let _tools = bind(&window);
        let bytes = Rc::new(std::cell::Cell::new(2 * 1024 * 1024u64));
        let size = bytes.clone();
        let clear = bytes.clone();
        let _registration = register_ui(move || size.get(), move || clear.set(0));
        window.invoke_clear_image_cache();
        assert_eq!(bytes.get(), 0);
        assert!(window.get_cache_busy());
        assert!(window.get_cache_status().contains("正在清除"));
        assert!(!window.get_cache_directory().is_empty());
        assert!(window.get_api_key_input().is_empty());
        window.hide().unwrap();
    }
}
