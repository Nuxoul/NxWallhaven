//! Native local management. Only file paths/metadata cross worker channels; UI images stay on UI thread.
use super::*;
use crate::download_manager::DownloadManager;
use std::path::Path;
use wh_core::catalog::{Catalog, ImportPreview};
use wh_core::library::DownloadTask;

#[derive(Clone)]
struct Record {
    key: String,
    id: String,
    title: String,
    subtitle: String,
    detail: String,
    tags: Vec<String>,
    restricted: bool,
    path: Option<PathBuf>,
    state: String,
    progress: f32,
    bytes: u64,
    created: u64,
    imported: bool,
    available: bool,
}
#[derive(Default)]
struct ViewState {
    records: Vec<Record>,
    selected: Option<String>,
    pending: Option<Pending>,
    preview: Option<ImportPreview>,
    refreshing: bool,
    ticks: u32,
    generation: u64,
    thumbs: HashMap<String, Image>,
    loading_thumbs: HashSet<String>,
    failed_thumbs: HashSet<String>,
    edited_key: Option<String>,
    viewers: Vec<LocalOriginalWindow>,
}
enum Pending {
    Batch(Vec<Wallpaper>, PathBuf),
    Remove(Record, bool),
    Tags(String, Vec<String>),
    Import,
}
enum ResultEvent {
    Snapshot(u64, Result<Vec<Record>, String>),
    Scan(Result<ImportPreview, String>),
    Message(Result<String, String>),
    Duplicates(Result<String, String>),
    Thumbnail(u64, String, Result<DecodedImage, String>),
    Original(usize, Result<DecodedImage, String>),
}
struct Inner {
    window: LocalWindow,
    main: Rc<RefCell<State>>,
    manager: Arc<DownloadManager>,
    data: PathBuf,
    view: RefCell<ViewState>,
    tx: mpsc::Sender<ResultEvent>,
    rx: mpsc::Receiver<ResultEvent>,
    timer: slint::Timer,
    permission: RefCell<(Option<String>, bool)>,
}
#[derive(Clone)]
pub(super) struct LocalTools(Rc<Inner>);
impl LocalTools {
    pub(super) fn hide_all(&self) {
        self.0.view.borrow_mut().pending = None;
        self.0.window.set_confirmation("".into());
        let _ = self.0.window.hide();
        for viewer in &self.0.view.borrow().viewers {
            let _ = viewer.hide();
            viewer.set_original(Default::default());
        }
    }
    pub(super) fn manager(&self) -> Arc<DownloadManager> {
        self.0.manager.clone()
    }
}

pub(super) fn data_directory() -> PathBuf {
    Settings::config_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("local-data")
}
pub(super) fn bind(
    ui: &AppWindow,
    main: Rc<RefCell<State>>,
    loader: Arc<ImageLoader>,
) -> Result<LocalTools, String> {
    bind_at(ui, main, loader, data_directory())
}
fn bind_at(
    ui: &AppWindow,
    main: Rc<RefCell<State>>,
    loader: Arc<ImageLoader>,
    data: PathBuf,
) -> Result<LocalTools, String> {
    let manager = DownloadManager::new(data.clone(), loader)?;
    let window = LocalWindow::new().map_err(|e| e.to_string())?;
    window
        .window()
        .set_size(slint::LogicalSize::new(1060.0, 760.0));
    window.set_import_path(
        main.borrow()
            .settings
            .resolved_download_dir()
            .to_string_lossy()
            .into_owned()
            .into(),
    );
    let (tx, rx) = mpsc::channel();
    let permission = {
        let s = &main.borrow().settings;
        (s.api_key.clone(), s.nsfw_enabled && s.has_api_key())
    };
    manager.permissions_changed(permission.1);
    let inner = Rc::new(Inner {
        window,
        main,
        manager,
        data,
        view: RefCell::new(ViewState::default()),
        tx,
        rx,
        timer: slint::Timer::default(),
        permission: RefCell::new(permission),
    });
    let weak = Rc::downgrade(&inner);
    inner.timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(150),
        move || {
            if let Some(i) = weak.upgrade() {
                i.pump();
            }
        },
    );
    let weak = Rc::downgrade(&inner);
    let parent = ui.as_weak();
    ui.on_go_local(move || {
        if let (Some(i), Some(p)) = (weak.upgrade(), parent.upgrade()) {
            let _ = window_chrome::show_centered(&i.window, p.window());
            i.refresh();
        }
    });
    let weak = Rc::downgrade(&inner);
    let parent = ui.as_weak();
    ui.on_batch_download(move||{if let (Some(i),Some(p))=(weak.upgrade(),parent.upgrade()){
        let snapshot=i.main.borrow().walls.clone();let destination=i.main.borrow().settings.resolved_download_dir();
        if snapshot.is_empty(){return}
        i.window.set_tab(0);i.window.set_confirmation(format!("将当前已加载的 {} 张壁纸加入下载队列？不会读取其他页面，已完成的图片不重复保存。\n目标：{}",snapshot.len(),destination.display()).into());
        i.window.set_confirmation_button("加入下载队列".into());i.view.borrow_mut().pending=Some(Pending::Batch(snapshot,destination));
        let _=window_chrome::show_centered(&i.window,p.window());i.refresh();
    }});
    let weak = Rc::downgrade(&inner);
    inner.window.on_refresh(move || {
        if let Some(i) = weak.upgrade() {
            i.refresh();
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_filter(move || {
        if let Some(i) = weak.upgrade() {
            i.render();
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_change_tab(move |tab| {
        if let Some(i) = weak.upgrade() {
            i.window.set_tab(tab);
            let mut view = i.view.borrow_mut();
            view.selected = None;
            view.edited_key = None;
            drop(view);
            i.render();
            i.refresh();
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_select_row(move |key| {
        if let Some(i) = weak.upgrade() {
            i.view.borrow_mut().selected = Some(key.to_string());
            i.render();
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_queue_action(move |key, state| {
        if let Some(i) = weak.upgrade() {
            let id = key.strip_prefix("download:").unwrap_or("").to_owned();
            let state = state.to_string();
            let manager = i.manager.clone();
            let tx = i.tx.clone();
            std::thread::spawn(move || {
                let r = manager
                    .set_state(&id, &state)
                    .map(|_| "下载状态已更新".to_owned());
                let _ = tx.send(ResultEvent::Message(r));
            });
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_scan(move || {
        if let Some(i) = weak.upgrade() {
            if i.window.get_busy() {
                return;
            }
            i.window.set_busy(true);
            i.window.set_import_ready(false);
            i.view.borrow_mut().preview = None;
            let path = PathBuf::from(i.window.get_import_path().trim());
            let recursive = i.window.get_recursive();
            let tx = i.tx.clone();
            i.window.set_status("正在只读扫描，未修改任何图片…".into());
            std::thread::spawn(move || {
                let _ = tx.send(ResultEvent::Scan(Catalog::scan(&path, recursive)));
            });
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_commit_import(move||{if let Some(i)=weak.upgrade(){
        if i.view.borrow().preview.is_none(){return}i.window.set_confirmation("将上次扫描预览中的图片加入本地索引？提交前会重新校验，扫描后变化的文件不会被静默导入。源文件保留原位。".into());i.window.set_confirmation_button("确认导入".into());i.view.borrow_mut().pending=Some(Pending::Import);
    }});
    let weak = Rc::downgrade(&inner);
    inner.window.on_duplicates(move || {
        if let Some(i) = weak.upgrade() {
            if i.window.get_busy() {
                return;
            }
            i.window.set_busy(true);
            let data = i.data.clone();
            let tx = i.tx.clone();
            std::thread::spawn(move || {
                let r = Catalog::open(&data)
                    .and_then(|c| c.duplicate_groups())
                    .map(|groups| {
                        if groups.is_empty() {
                            return "未发现同内容的多个来源。此检查不删除任何文件。".to_owned();
                        }
                        let mut text = format!(
                            "发现 {} 组相同内容；以下仅为报告，未删除文件。\n",
                            groups.len()
                        );
                        for (n, g) in groups.iter().enumerate() {
                            text.push_str(&format!("\n第 {} 组\n{}\n", n + 1, g.join("\n")));
                        }
                        text
                    });
                let _ = tx.send(ResultEvent::Duplicates(r));
            });
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_save_tags(move || {
        if let Some(i) = weak.upgrade() {
            if let Some(record) = i.selected() {
                if !record.imported {
                    return;
                }
                let raw = i
                    .window
                    .get_tags_input()
                    .split([',', '，', ';', '；', '\n'])
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let tags = wh_core::catalog::normalize_tags(&raw);
                i.window.set_confirmation(
                    format!(
                        "仅更新本地标签，不改图片。\n原标签：{}\n规范化后：{}",
                        record.tags.join(", "),
                        tags.join(", ")
                    )
                    .into(),
                );
                i.window.set_confirmation_button("保存规范化标签".into());
                i.view.borrow_mut().pending = Some(Pending::Tags(record.id, tags));
            }
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_remove_selected(move |delete| {
        if let Some(i) = weak.upgrade() {
            if let Some(record) = i.selected() {
                if delete && record.imported {
                    return;
                }
                i.window.set_confirmation(
                    format!(
                        "{}\n{}\n{}",
                        if delete {
                            "永久删除此已下载原图及记录？不能撤销。"
                        } else {
                            "仅移除此项本地记录？原始文件将保留。"
                        },
                        record.title,
                        record
                            .path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default()
                    )
                    .into(),
                );
                i.window.set_confirmation_button(if delete {
                    "删除文件".into()
                } else {
                    "仅移除记录".into()
                });
                i.view.borrow_mut().pending = Some(Pending::Remove(record, delete));
            }
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_dismiss_confirmation(move || {
        if let Some(i) = weak.upgrade() {
            i.window.set_confirmation("".into());
            i.view.borrow_mut().pending = None;
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_confirm(move || {
        if let Some(i) = weak.upgrade() {
            i.confirm();
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_open_selected(move || {
        if let Some(i) = weak.upgrade() {
            i.open_original();
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_open_folder(move || {
        if let Some(i) = weak.upgrade() {
            if let Some(record) = i.selected() {
                if let Some(path) = record.path.and_then(|p| p.parent().map(Path::to_owned)) {
                    let r = platform::open_folder(&path);
                    i.window.set_status(
                        r.map(|_| "已打开所在目录".to_owned())
                            .unwrap_or_else(|e| e)
                            .into(),
                    );
                }
            }
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.on_export_json(move || {
        if let Some(i) = weak.upgrade() {
            if i.window.get_busy() {
                return;
            }
            i.window.set_busy(true);
            let path = PathBuf::from(i.window.get_export_path().trim());
            let data = i.data.clone();
            let tx = i.tx.clone();
            std::thread::spawn(move || {
                let r = Catalog::open(&data)
                    .and_then(|c| c.export_json())
                    .and_then(|json| write_new(&path, json.as_bytes()))
                    .map(|_| "本地整理记录已导出；未覆盖已有文件".to_owned());
                let _ = tx.send(ResultEvent::Message(r));
            });
        }
    });
    let weak = Rc::downgrade(&inner);
    inner.window.window().on_close_requested(move || {
        if let Some(i) = weak.upgrade() {
            i.view.borrow_mut().pending = None;
            i.window.set_confirmation("".into());
            for viewer in &i.view.borrow().viewers {
                let _ = viewer.hide();
                viewer.set_original(Default::default());
            }
        }
        slint::CloseRequestResponse::HideWindow
    });
    inner.refresh();
    Ok(LocalTools(inner))
}
impl Inner {
    fn selected(&self) -> Option<Record> {
        let v = self.view.borrow();
        v.selected
            .as_ref()
            .and_then(|k| v.records.iter().find(|r| &r.key == k))
            .cloned()
    }
    fn refresh(&self) {
        self.refresh_with_catalog(true);
    }
    fn refresh_with_catalog(&self, include_catalog: bool) {
        if self.view.borrow().refreshing {
            return;
        }
        let generation = self.view.borrow().generation;
        self.view.borrow_mut().refreshing = true;
        let imported = if include_catalog {
            Vec::new()
        } else {
            self.view
                .borrow()
                .records
                .iter()
                .filter(|r| r.imported)
                .cloned()
                .collect::<Vec<_>>()
        };
        let manager = self.manager.clone();
        let data = self.data.clone();
        let tx = self.tx.clone();
        let allowed = self.permission.borrow().1;
        std::thread::spawn(move || {
            let result = (|| {
                let mut rows = imported;
                for t in manager.tasks()? {
                    if allowed || !crate::download_manager::restricted(&t) {
                        rows.push(task_record(t));
                    }
                }
                if include_catalog {
                    for item in Catalog::open(&data)?.items()? {
                        rows.push(Record {
                            key: format!("import:{}", item.id),
                            id: item.id,
                            title: Path::new(&item.path)
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                            subtitle: format!(
                                "{} × {} · {} · {} 个来源",
                                item.width,
                                item.height,
                                bytes(item.bytes),
                                item.paths.len()
                            ),
                            detail: item.tags.join(", "),
                            tags: item.tags,
                            restricted: false,
                            path: Some(PathBuf::from(item.path)),
                            state: "imported".into(),
                            progress: -1.0,
                            bytes: item.bytes,
                            created: item.created,
                            imported: true,
                            available: item.available,
                        });
                    }
                }
                // Queue-only removal can retain a library record. Surface it without re-enqueuing.
                for t in manager.library()? {
                    let key = format!("download:{}", t.id);
                    if rows.iter().all(|r| r.key != key)
                        && (allowed || !crate::download_manager::restricted(&t))
                    {
                        rows.push(task_record(t));
                    }
                }
                Ok(rows)
            })();
            let _ = tx.send(ResultEvent::Snapshot(generation, result));
        });
    }
    fn confirm(&self) {
        if self.window.get_busy() {
            return;
        }
        let Some(pending) = self.view.borrow_mut().pending.take() else {
            return;
        };
        self.window.set_confirmation("".into());
        self.window.set_busy(true);
        let manager = self.manager.clone();
        let data = self.data.clone();
        let tx = self.tx.clone();
        let preview = if matches!(pending, Pending::Import) {
            self.view.borrow_mut().preview.take()
        } else {
            None
        };
        std::thread::spawn(move || {
            let result = match pending {
                Pending::Batch(walls, directory) => {
                    let mut count = 0;
                    let mut failed = 0;
                    for wall in walls {
                        match manager.enqueue(&wall, &directory) {
                            Ok(()) => count += 1,
                            Err(_) => failed += 1,
                        }
                    }
                    Ok(format!(
                        "已处理 {count} 个任务，{failed} 个未加入；已完成的图片不重复保存"
                    ))
                }
                Pending::Remove(r, delete) => {
                    if r.imported {
                        Catalog::open(&data)
                            .and_then(|c| c.remove(&r.id))
                            .map(|_| "已移除记录，源文件保留".to_owned())
                    } else {
                        (if delete {
                            manager.remove(&r.id, true)
                        } else {
                            manager.remove_records(&r.id)
                        })
                        .map(|_| {
                            if delete {
                                "已删除确认的下载文件和记录".to_owned()
                            } else {
                                "已移除队列/图库记录，文件保留；重新下载不会覆盖原文件".to_owned()
                            }
                        })
                    }
                }
                Pending::Tags(id, tags) => Catalog::open(&data)
                    .and_then(|c| c.set_tags(&id, tags))
                    .map(|_| "本地标签已规范化，图片文件未改动".to_owned()),
                Pending::Import => preview
                    .ok_or_else(|| "请重新扫描预览".to_owned())
                    .and_then(|p| Catalog::open(&data)?.import(&p))
                    .map(|count| format!("已导入 {count} 项索引，源文件未改动")),
            };
            let _ = tx.send(ResultEvent::Message(result));
        });
    }
    fn open_original(&self) {
        let Some(record) = self.selected() else {
            return;
        };
        let Some(path) = record.path else { return };
        let Ok(viewer) = LocalOriginalWindow::new() else {
            return;
        };
        viewer
            .window()
            .set_size(slint::LogicalSize::new(1000.0, 760.0));
        let _ = window_chrome::show_centered(&viewer, self.window.window());
        let weak = viewer.as_weak();
        viewer.window().on_close_requested(move || {
            if let Some(v) = weak.upgrade() {
                v.set_original(Default::default());
            }
            slint::CloseRequestResponse::HideWindow
        });
        let index = {
            let mut v = self.view.borrow_mut();
            v.viewers.push(viewer);
            v.viewers.len() - 1
        };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(ResultEvent::Original(index, decode_file(&path, false)));
        });
    }
    fn pump(&self) {
        let permission = {
            let s = &self.main.borrow().settings;
            (s.api_key.clone(), s.nsfw_enabled && s.has_api_key())
        };
        if *self.permission.borrow() != permission {
            // Even a different present Key invalidates old restricted work before granting new permission.
            self.manager.permissions_changed(false);
            self.manager.permissions_changed(permission.1);
            *self.permission.borrow_mut() = permission;
            {
                let mut view = self.view.borrow_mut();
                view.generation = view.generation.wrapping_add(1);
                view.records.retain(|r| !r.restricted);
                view.thumbs.clear();
                view.loading_thumbs.clear();
                view.failed_thumbs.clear();
                view.selected = None;
                view.edited_key = None;
                view.pending = None;
            }
            self.window.set_confirmation("".into());
            self.render();
            for viewer in &self.view.borrow().viewers {
                let _ = viewer.hide();
                viewer.set_original(Default::default());
            }
            self.refresh();
        }
        while let Ok(event) = self.rx.try_recv() {
            match event {
                ResultEvent::Snapshot(generation, result) => {
                    self.view.borrow_mut().refreshing = false;
                    if generation != self.view.borrow().generation {
                        self.refresh();
                        continue;
                    }
                    match result {
                        Ok(records) => {
                            let mut view = self.view.borrow_mut();
                            let selected_tags_changed = view.selected.as_ref().is_some_and(|key| {
                                let old = view.records.iter().find(|r| &r.key == key);
                                let new = records.iter().find(|r| &r.key == key);
                                old.map(|r| &r.tags) != new.map(|r| &r.tags)
                            });
                            if selected_tags_changed {
                                view.edited_key = None;
                            }
                            let keys = records
                                .iter()
                                .map(|r| r.key.as_str())
                                .collect::<HashSet<_>>();
                            view.thumbs.retain(|key, _| keys.contains(key.as_str()));
                            view.loading_thumbs
                                .retain(|key| keys.contains(key.as_str()));
                            view.failed_thumbs.retain(|key| keys.contains(key.as_str()));
                            drop(view);
                            self.view.borrow_mut().records = records;
                            self.render();
                        }
                        Err(e) => self.window.set_status(format!("索引读取失败：{e}").into()),
                    }
                }
                ResultEvent::Scan(result) => {
                    self.window.set_busy(false);
                    match result {
                        Ok(preview) => {
                            let mut text = format!(
                                "发现 {} 个图片文件，跳过 {} 项。\n预览（最多列出前 100 项）：\n",
                                preview.files.len(),
                                preview.skipped
                            );
                            for f in preview.files.iter().take(100) {
                                text.push_str(&format!(
                                    "{} × {} · {}\n{}\n",
                                    f.width,
                                    f.height,
                                    bytes(f.bytes),
                                    f.path
                                ));
                            }
                            self.window.set_import_summary(text.into());
                            self.window.set_import_ready(!preview.files.is_empty());
                            self.view.borrow_mut().preview = Some(preview);
                            self.window
                                .set_status("扫描完成，等待确认；文件未修改".into());
                        }
                        Err(e) => self.window.set_status(format!("扫描失败：{e}").into()),
                    }
                }
                ResultEvent::Message(result) => {
                    // Invalidate snapshots that started before this mutation completed.
                    {
                        let mut view = self.view.borrow_mut();
                        view.generation = view.generation.wrapping_add(1);
                        view.loading_thumbs.clear();
                    }
                    self.window.set_busy(false);
                    self.window
                        .set_status(result.unwrap_or_else(|e| format!("操作未完成：{e}")).into());
                    self.window.set_import_ready(false);
                    self.refresh();
                }
                ResultEvent::Duplicates(result) => {
                    self.window.set_busy(false);
                    self.window.set_import_summary(
                        result.unwrap_or_else(|e| format!("查重失败：{e}")).into(),
                    );
                    self.window
                        .set_status("查重仅生成报告，没有删除文件".into());
                }
                ResultEvent::Thumbnail(generation, key, result) => {
                    if generation != self.view.borrow().generation
                        || !self.view.borrow().records.iter().any(|r| r.key == key)
                    {
                        continue;
                    }
                    self.view.borrow_mut().loading_thumbs.remove(&key);
                    match result {
                        Ok(image) => {
                            let mut v = self.view.borrow_mut();
                            if v.thumbs.len() < 256 {
                                v.thumbs.insert(key, Image::from_rgba8(image.rgba));
                            }
                        }
                        Err(_) => {
                            self.view.borrow_mut().failed_thumbs.insert(key);
                        }
                    }
                    self.render();
                }
                ResultEvent::Original(index, result) => {
                    if let Some(viewer) = self.view.borrow().viewers.get(index) {
                        if !viewer.window().is_visible() {
                            continue;
                        }
                        match result {
                            Ok(image) => {
                                viewer.set_message(
                                    format!(
                                        "完整原图 {} × {} · 滚轮缩放 / 拖动平移 · 仅本地读取",
                                        image.w, image.h
                                    )
                                    .into(),
                                );
                                viewer.set_original(Image::from_rgba8(image.rgba));
                                viewer.set_loaded(true);
                            }
                            Err(e) => viewer.set_message(format!("无法读取本地原图：{e}").into()),
                        }
                    }
                }
            }
        }
        let should_refresh = {
            let mut v = self.view.borrow_mut();
            v.ticks += 1;
            v.ticks.is_multiple_of(7)
                && self.window.window().is_visible()
                && self.window.get_tab() == 0
        };
        if should_refresh {
            self.refresh_with_catalog(false);
        }
    }
    fn render(&self) {
        let query = self.window.get_query().trim().to_lowercase();
        let tab = self.window.get_tab();
        let sort = self.window.get_sort_index();
        let mut view = self.view.borrow_mut();
        let mut records = view
            .records
            .iter()
            .filter(|r| {
                if tab == 0 {
                    !r.imported
                } else {
                    r.imported || matches!(r.state.as_str(), "completed" | "missing")
                }
            })
            .filter(|r| {
                query.is_empty()
                    || format!(
                        "{} {} {} {}",
                        r.title,
                        r.subtitle,
                        r.detail,
                        r.tags.join(" ")
                    )
                    .to_lowercase()
                    .contains(&query)
            })
            .cloned()
            .collect::<Vec<_>>();
        match sort {
            1 => records.sort_by_key(|r| r.created),
            2 => records.sort_by_key(|r| std::cmp::Reverse(r.bytes)),
            _ => records.sort_by_key(|r| std::cmp::Reverse(r.created)),
        }
        if view
            .selected
            .as_ref()
            .is_some_and(|key| !records.iter().any(|r| &r.key == key))
        {
            view.selected = None;
            view.edited_key = None;
        }
        let selected = view.selected.clone();
        let generation = view.generation;
        let mut jobs = Vec::new();
        let rows = records
            .iter()
            .map(|r| {
                if tab == 1
                    && r.available
                    && !view.thumbs.contains_key(&r.key)
                    && !view.failed_thumbs.contains(&r.key)
                    && view.thumbs.len() < 256
                    && view.loading_thumbs.len() < 3
                    && view.loading_thumbs.insert(r.key.clone())
                {
                    if let Some(p) = &r.path {
                        jobs.push((r.key.clone(), p.clone()));
                    }
                }
                let image = view.thumbs.get(&r.key).cloned();
                LocalRow {
                    key: r.key.clone().into(),
                    title: r.title.clone().into(),
                    subtitle: r.subtitle.clone().into(),
                    detail: r.detail.clone().into(),
                    state: r.state.clone().into(),
                    progress: r.progress,
                    thumbnail: image.clone().unwrap_or_default(),
                    loaded: image.is_some(),
                    selected: selected.as_ref() == Some(&r.key),
                    can_open: r.available,
                    can_delete: !r.imported && r.available,
                }
            })
            .collect::<Vec<_>>();
        let model =
            gallery_layout::sync_model(self.window.get_rows(), rows, |row| row.key.as_str());
        self.window.set_rows(model);
        if let Some(r) = selected
            .as_ref()
            .and_then(|key| view.records.iter().find(|r| &r.key == key))
        {
            self.window.set_selected_title(r.title.clone().into());
            self.window.set_selected_detail(
                format!(
                    "{}\n{}\n{}",
                    r.subtitle,
                    r.detail,
                    r.path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "文件尚未保存".into())
                )
                .into(),
            );
            self.window.set_selected_available(r.available);
            self.window.set_selected_imported(r.imported);
            // Do not overwrite text while a user is editing the selected item.
            if view.edited_key.as_ref() != Some(&r.key) {
                self.window.set_tags_input(r.tags.join(", ").into());
            }
        } else {
            self.window.set_selected_title("选择一张图片".into());
            self.window.set_selected_detail("".into());
            self.window.set_selected_available(false);
            self.window.set_selected_imported(false);
            self.window.set_tags_input("".into());
        }
        view.edited_key = selected;
        drop(view);
        for (key, path) in jobs {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(ResultEvent::Thumbnail(
                    generation,
                    key,
                    decode_file(&path, true),
                ));
            });
        }
    }
}
fn task_record(t: DownloadTask) -> Record {
    let restricted = crate::download_manager::restricted(&t);
    let path = t
        .uri
        .as_deref()
        .and_then(|u| reqwest::Url::parse(u).ok())
        .and_then(|u| u.to_file_path().ok());
    let available = path.as_ref().is_some_and(|p| regular_file(p).is_ok());
    let wall = serde_json::from_value::<Wallpaper>(t.wall).unwrap_or_default();
    let tags = wall
        .tags
        .iter()
        .filter_map(|t| t.name.clone())
        .collect::<Vec<_>>();
    Record {
        key: format!("download:{}", t.id),
        id: t.id.clone(),
        title: format!("壁纸 {}", t.id),
        subtitle: format!(
            "{} · {} · {}",
            wall.resolution.as_deref().unwrap_or("—"),
            bytes(t.total.unwrap_or(t.received)),
            state_label(&t.state)
        ),
        detail: format!(
            "已接收 {}{}",
            bytes(t.received),
            t.total
                .map(|n| format!(" / {}", bytes(n)))
                .unwrap_or_default()
        ),
        tags,
        restricted,
        path,
        state: t.state,
        progress: t
            .total
            .filter(|n| *n > 0)
            .map(|n| (t.received as f64 / n as f64).clamp(0.0, 1.0) as f32)
            .unwrap_or(-1.0),
        bytes: t.total.unwrap_or(t.received),
        created: t.created,
        imported: false,
        available,
    }
}
fn state_label(state: &str) -> &str {
    match state {
        "queued" => "排队中",
        "running" => "下载中",
        "paused" => "已暂停",
        "cancelled" => "已取消",
        "failed" => "失败，可重试",
        "completed" => "已完成",
        "missing" => "文件已丢失",
        _ => state,
    }
}
fn bytes(n: u64) -> String {
    if n >= 1_048_576 {
        format!("{:.1} MiB", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}
fn regular_file(path: &Path) -> Result<(), String> {
    let m = std::fs::symlink_metadata(path).map_err(|_| "文件不存在或无权读取".to_owned())?;
    if !m.is_file() || m.file_type().is_symlink() {
        return Err("不是普通图片文件".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return Err("不读取链接或重解析文件".into());
        }
    }
    Ok(())
}
fn decode_file(path: &Path, thumbnail: bool) -> Result<DecodedImage, String> {
    regular_file(path)?;
    let mut reader = image::ImageReader::open(path)
        .map_err(|_| "无法打开文件".to_owned())?
        .with_guessed_format()
        .map_err(|_| "无法识别图片".to_owned())?;
    if thumbnail {
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(96 * 1024 * 1024);
        reader.limits(limits);
    } else {
        reader.no_limits();
    }
    let image = reader
        .decode()
        .map_err(|_| "图片解码失败或缩略图解码预算不足；原文件保留".to_owned())?;
    let image = if thumbnail {
        image.thumbnail(260, 190)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(DecodedImage {
        w,
        h,
        rgba: slint::SharedPixelBuffer::clone_from_slice(rgba.as_raw(), w, h),
    })
}
fn write_new(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "无法创建导出文件（不会覆盖已有文件）".to_owned())?;
    let result = file.write_all(data).and_then(|_| file.sync_all());
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err("导出失败，未完成文件已清理".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests;
