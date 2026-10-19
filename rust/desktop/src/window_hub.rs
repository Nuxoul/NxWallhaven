//! Native non-modal windows: each detail/profile owns independent view state.
use super::*;
use image_loader::OriginalProgress;

pub(super) struct WindowHub {
    ui: slint::Weak<AppWindow>,
    main: Rc<RefCell<State>>,
    loader: Arc<ImageLoader>,
    quick: slint::Weak<QuickFavoriteWindow>,
    details: RefCell<Vec<DetailEntry>>,
    profiles: RefCell<Vec<profile_controller::ProfileTools>>,
    next: std::cell::Cell<u64>,
    timer: slint::Timer,
}
struct DetailEntry {
    id: u64,
    window: DetailWindow,
    wall: Wallpaper,
    generation: u64,
    rx: mpsc::Receiver<DetailEvent>,
    tx: mpsc::Sender<DetailEvent>,
    key: Option<String>,
    nsfw_allowed: bool,
}
enum DetailEvent {
    Metadata(u64, Box<Result<Wallpaper, String>>),
    Progress(u64, OriginalProgress),
    Original(u64, Result<DecodedImage, String>),
    DownloadProgress(u64, Option<u64>),
    Download(Result<PathBuf, String>),
    Wallpaper(Result<(), String>),
}
impl WindowHub {
    pub(super) fn new(
        ui: &AppWindow,
        main: Rc<RefCell<State>>,
        loader: Arc<ImageLoader>,
        quick: slint::Weak<QuickFavoriteWindow>,
    ) -> Rc<Self> {
        let hub = Rc::new(Self {
            ui: ui.as_weak(),
            main,
            loader,
            quick,
            details: RefCell::new(Vec::new()),
            profiles: RefCell::new(Vec::new()),
            next: std::cell::Cell::new(0),
            timer: slint::Timer::default(),
        });
        let weak = Rc::downgrade(&hub);
        hub.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(30),
            move || {
                if let Some(hub) = weak.upgrade() {
                    hub.pump();
                }
            },
        );
        let weak = Rc::downgrade(&hub);
        ui.on_card_clicked(move |id| {
            let Some(hub) = weak.upgrade() else { return };
            let item = {
                let mut state = hub.main.borrow_mut();
                let wall = state
                    .walls
                    .iter()
                    .find(|wall| wall.id == id.as_str())
                    .cloned();
                wall.map(|wall| {
                    let thumb = state.images.get(&wall.id);
                    (wall, thumb)
                })
            };
            if let (Some((wall, thumb)), Some(ui)) = (item, hub.ui.upgrade()) {
                hub.open_detail(wall, thumb, ui.window());
            }
        });
        hub
    }
    pub(super) fn close_all(&self) {
        for entry in self.details.borrow().iter() {
            let _ = entry.window.hide();
            entry.window.set_original(Default::default());
        }
        for entry in self.profiles.borrow().iter() {
            let _ = entry.window.hide();
        }
        self.details.borrow_mut().clear();
        self.profiles.borrow_mut().clear();
    }
    pub(super) fn open_detail(
        self: &Rc<Self>,
        wall: Wallpaper,
        thumb: Option<Image>,
        parent: &slint::Window,
    ) {
        if let Err(error) = self.add_detail(wall, thumb, parent, true) {
            if let Some(ui) = self.ui.upgrade() {
                ui.set_status(format!("无法打开详情窗口：{error}").into());
            }
        }
    }
    fn add_detail(
        self: &Rc<Self>,
        wall: Wallpaper,
        thumb: Option<Image>,
        parent: &slint::Window,
        load: bool,
    ) -> Result<u64, slint::PlatformError> {
        if wall.purity.as_deref() == Some("nsfw") {
            let settings = &self.main.borrow().settings;
            if !settings.nsfw_enabled || !settings.has_api_key() {
                return Err(slint::PlatformError::Other("未启用 NSFW 内容权限".into()));
            }
        }
        let window = DetailWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(1120.0, 760.0));
        fill(&window, &wall);
        if let Some(thumb) = thumb {
            window.set_original(thumb);
            window.set_image_loaded(true);
        }
        let id = self.next.get() + 1;
        self.next.set(id);
        let (tx, rx) = mpsc::channel();
        let state = self.main.borrow();
        let key = state.settings.api_key.clone();
        let nsfw_allowed = state.settings.nsfw_enabled && state.settings.has_api_key();
        drop(state);
        let weak_window = window.as_weak();
        window.window().on_close_requested(move || {
            if let Some(window) = weak_window.upgrade() {
                window.set_original(Default::default());
            }
            slint::CloseRequestResponse::HideWindow
        });
        let weak = Rc::downgrade(self);
        window.on_retry(move || {
            if let Some(hub) = weak.upgrade() {
                hub.request(id);
            }
        });
        let weak = Rc::downgrade(self);
        window.on_open_uploader(move || {
            let Some(hub) = weak.upgrade() else { return };
            let info = hub
                .details
                .borrow()
                .iter()
                .find(|e| e.id == id)
                .and_then(|entry| {
                    public_profile::uploader_username(&entry.wall)
                        .map(|username| (username, entry.window.as_weak()))
                });
            if let Some((username, window)) = info {
                if let Some(window) = window.upgrade() {
                    hub.open_profile(username, window.window());
                }
            }
        });
        let weak = Rc::downgrade(self);
        window.on_copy_link(move || {
            if let Some(hub) = weak.upgrade() {
                hub.with_entry(id, |entry| {
                    let result =
                        wallpaper_link(&entry.wall.id).and_then(|url| platform::copy_text(&url));
                    entry.window.set_action_message(
                        result
                            .map(|_| "链接已复制".into())
                            .unwrap_or_else(|e| format!("复制失败：{e}"))
                            .into(),
                    );
                });
            }
        });
        let weak = Rc::downgrade(self);
        window.on_browser_open(move || {
            if let Some(hub) = weak.upgrade() {
                hub.with_entry(id, |entry| {
                    if let Ok(url) = wallpaper_link(&entry.wall.id) {
                        use std::os::windows::process::CommandExt;
                        let result = std::process::Command::new("rundll32.exe")
                            .args(["url.dll,FileProtocolHandler", &url])
                            .creation_flags(0x08000000)
                            .spawn();
                        entry.window.set_action_message(if result.is_ok() {
                            "已交给系统浏览器".into()
                        } else {
                            "无法打开浏览器".into()
                        });
                    }
                });
            }
        });
        let weak = Rc::downgrade(self);
        window.on_open_folder(move || {
            if let Some(hub) = weak.upgrade() {
                let path = hub.main.borrow().settings.resolved_download_dir();
                let result = std::fs::create_dir_all(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|_| platform::open_folder(&path));
                hub.message(
                    id,
                    result
                        .map(|_| "已打开下载目录".into())
                        .unwrap_or_else(|e| format!("打开失败：{e}")),
                );
            }
        });
        let weak = Rc::downgrade(self);
        window.on_download(move || {
            if let Some(hub) = weak.upgrade() {
                hub.download(id, false);
            }
        });
        let weak = Rc::downgrade(self);
        window.on_set_wallpaper(move || {
            if let Some(hub) = weak.upgrade() {
                hub.download(id, true);
            }
        });
        let weak = Rc::downgrade(self);
        window.on_favorite(move || {
            let Some(hub) = weak.upgrade() else { return };
            let Some(ui) = hub.ui.upgrade() else { return };
            let info = hub
                .details
                .borrow()
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| (entry.wall.id.clone(), entry.window.as_weak()));
            if let Some((wall, window)) = info {
                ui.invoke_card_favorite(wall.into());
                if let (Some(quick), Some(window)) = (hub.quick.upgrade(), window.upgrade()) {
                    window_chrome::center_on_parent(quick.window(), window.window());
                }
            }
        });
        let weak = Rc::downgrade(self);
        window.on_search_tag(move |query| {
            if let Some(hub) = weak.upgrade() {
                if let Some(ui) = hub.ui.upgrade() {
                    ui.invoke_search_tag(query);
                    hub.message(id, "已在主窗口搜索标签".into());
                }
            }
        });
        let weak = Rc::downgrade(self);
        window.on_search_color(move |color| {
            if let Some(hub) = weak.upgrade() {
                if let Some(ui) = hub.ui.upgrade() {
                    ui.invoke_search_color(color);
                    hub.message(id, "已在主窗口搜索颜色".into());
                }
            }
        });
        window_chrome::show_centered(&window, parent)?;
        self.details.borrow_mut().push(DetailEntry {
            id,
            window,
            wall,
            generation: 0,
            rx,
            tx,
            key,
            nsfw_allowed,
        });
        if load {
            self.request(id);
        }
        Ok(id)
    }
    pub(super) fn open_profile(self: &Rc<Self>, username: String, parent: &slint::Window) {
        let Some(ui) = self.ui.upgrade() else { return };
        let weak = Rc::downgrade(self);
        let open: profile_controller::OpenWallpaper = Rc::new(move |wall, thumb, parent| {
            if let Some(hub) = weak.upgrade() {
                hub.open_detail(wall, thumb, parent);
            }
        });
        match profile_controller::create(&ui, self.main.clone(), username, parent, open) {
            Ok(profile) => self.profiles.borrow_mut().push(profile),
            Err(error) => ui.set_status(format!("无法打开作者主页：{error}").into()),
        }
    }
    fn with_entry(&self, id: u64, action: impl FnOnce(&DetailEntry)) {
        if let Some(entry) = self.details.borrow().iter().find(|entry| entry.id == id) {
            action(entry);
        }
    }
    fn message(&self, id: u64, message: String) {
        self.with_entry(id, |entry| entry.window.set_action_message(message.into()));
    }
    fn request(&self, id: u64) {
        let info = {
            let mut entries = self.details.borrow_mut();
            let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) else {
                return;
            };
            entry.generation += 1;
            entry.window.set_image_busy(true);
            entry.window.set_failed(false);
            entry.window.set_metadata_busy(true);
            entry.window.set_load_message("正在连接原图…".into());
            entry.window.set_progress(-1.0);
            (entry.wall.clone(), entry.generation, entry.tx.clone())
        };
        let (wall, generation, tx) = info;
        let client = self.main.borrow().client.clone();
        let loader = self.loader.clone();
        let (events, results) = mpsc::channel();
        let progress = tx.clone();
        detail_loading::start_observed(
            wall,
            generation,
            loader,
            events,
            move |id| {
                client
                    .wallpaper(&id)
                    .map_err(|error| error.to_string())
                    .and_then(|response| response.data.ok_or("详情为空".into()))
            },
            move |generation, stage| {
                let _ = progress.send(DetailEvent::Progress(generation, stage));
            },
        );
        std::thread::spawn(move || {
            for event in results {
                let event = match event {
                    Event::Detail(generation, result) => DetailEvent::Metadata(generation, result),
                    Event::Preview(generation, result) => DetailEvent::Original(generation, result),
                    _ => continue,
                };
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
    }
    fn download(&self, id: u64, set_wallpaper: bool) {
        let directory = self.main.borrow().settings.resolved_download_dir();
        let info = {
            let entries = self.details.borrow();
            let Some(entry) = entries.iter().find(|entry| entry.id == id) else {
                return;
            };
            if entry.window.get_downloading() || entry.window.get_wallpaper_busy() {
                return;
            }
            let filename = match wallpaper_filename(&entry.wall) {
                Ok(name) => name,
                Err(error) => {
                    entry.window.set_action_message(error.into());
                    return;
                }
            };
            let Some(url) = entry.wall.path.clone() else {
                entry
                    .window
                    .set_action_message("原图链接尚未加载，请稍后重试".into());
                return;
            };
            if set_wallpaper {
                entry.window.set_wallpaper_busy(true);
            } else {
                entry.window.set_downloading(true);
            }
            entry.window.set_action_message("正在准备原图…".into());
            (filename, url, entry.tx.clone())
        };
        let (filename, url, tx) = info;
        let loader = self.loader.clone();
        std::thread::spawn(move || {
            let path = if set_wallpaper {
                platform::wallpaper_cache_dir().join(filename)
            } else {
                directory.join(filename)
            };
            let progress = tx.clone();
            let result = if set_wallpaper && path.exists() {
                Ok(())
            } else {
                loader.download_to(&url, &path, move |done, total| {
                    let _ = progress.send(DetailEvent::DownloadProgress(done, total));
                })
            };
            if set_wallpaper {
                let result = result.and_then(|_| platform::set_wallpaper(&path));
                let _ = tx.send(DetailEvent::Wallpaper(result));
            } else {
                let _ = tx.send(DetailEvent::Download(result.map(|_| path)));
            }
        });
    }
    fn pump(&self) {
        let current = self.main.borrow();
        let key = current.settings.api_key.clone();
        let allowed = current.settings.nsfw_enabled && current.settings.has_api_key();
        drop(current);
        let mut entries = self.details.borrow_mut();
        for entry in entries.iter_mut() {
            if !entry.window.window().is_visible() {
                continue;
            }
            if entry.key != key || entry.nsfw_allowed != allowed {
                entry.key = key.clone();
                entry.nsfw_allowed = allowed;
                if entry.wall.purity.as_deref() == Some("nsfw") {
                    entry.generation += 1;
                    entry.window.set_original(Default::default());
                    let _ = entry.window.hide();
                    continue;
                }
            }
            for event in entry.rx.try_iter().take(32) {
                match event {
                    DetailEvent::Metadata(generation, result) if generation == entry.generation => {
                        entry.window.set_metadata_busy(false);
                        match *result {
                            Ok(wall) => {
                                fill(&entry.window, &wall);
                                entry.wall = wall;
                            }
                            Err(error) => entry
                                .window
                                .set_action_message(format!("详细信息读取失败：{error}").into()),
                        }
                    }
                    DetailEvent::Progress(generation, progress)
                        if generation == entry.generation =>
                    {
                        apply_progress(&entry.window, progress)
                    }
                    DetailEvent::Original(generation, result) if generation == entry.generation => {
                        entry.window.set_image_busy(false);
                        match result {
                            Ok(image) => {
                                let size = format!(" · {} × {}", image.w, image.h);
                                entry.window.set_load_message(
                                    format!("{}{}", entry.window.get_load_message(), size).into(),
                                );
                                entry.window.set_original(to_image(image));
                                entry.window.set_image_loaded(true);
                                entry.window.set_failed(false);
                            }
                            Err(error) => {
                                entry.window.set_failed(true);
                                entry
                                    .window
                                    .set_load_message(format!("原图加载失败：{error}").into());
                            }
                        }
                    }
                    DetailEvent::DownloadProgress(done, total) => entry.window.set_action_message(
                        total
                            .map(|total| {
                                format!("下载 {:.0}%", done as f64 / total.max(1) as f64 * 100.0)
                            })
                            .unwrap_or_else(|| format!("已接收 {}", fmt_size(done)))
                            .into(),
                    ),
                    DetailEvent::Download(result) => {
                        entry.window.set_downloading(false);
                        entry.window.set_action_message(
                            result
                                .map(|path| format!("已保存：{}", path.display()))
                                .unwrap_or_else(|e| format!("下载失败：{e}"))
                                .into(),
                        );
                    }
                    DetailEvent::Wallpaper(result) => {
                        entry.window.set_wallpaper_busy(false);
                        entry.window.set_action_message(
                            result
                                .map(|_| "已设置桌面壁纸".into())
                                .unwrap_or_else(|e| format!("设置失败：{e}"))
                                .into(),
                        );
                    }
                    _ => {}
                }
            }
        }
        entries.retain(|entry| entry.window.window().is_visible());
        drop(entries);
        self.profiles
            .borrow_mut()
            .retain(|entry| entry.window.window().is_visible());
    }
}
fn apply_progress(window: &DetailWindow, progress: OriginalProgress) {
    match progress {
        OriginalProgress::Connecting => {
            window.set_progress(-1.0);
            window.set_load_message("正在连接原图…".into());
        }
        OriginalProgress::Downloading { received, total } => {
            window.set_progress(
                total
                    .map(|total| received as f32 / total.max(1) as f32)
                    .unwrap_or(-1.0),
            );
            window.set_load_message(
                total
                    .map(|total| {
                        format!(
                            "正在传输原图 · {} / {}",
                            fmt_size(received),
                            fmt_size(total)
                        )
                    })
                    .unwrap_or_else(|| format!("正在传输原图 · {}", fmt_size(received)))
                    .into(),
            );
        }
        OriginalProgress::Decoding => {
            window.set_progress(-1.0);
            window.set_load_message("传输完成，正在解码原图…".into());
        }
        OriginalProgress::Ready {
            network_ms,
            decode_ms,
            cache_hit,
        } => window.set_load_message(if cache_hit {
            "原图已加载 · 已复用本地缓存".into()
        } else {
            format!(
                "原图已加载 · 网络 {:.2}s · 解码 {:.2}s",
                network_ms as f64 / 1000.0,
                decode_ms as f64 / 1000.0
            )
            .into()
        }),
    }
}
fn value(value: &Option<String>) -> String {
    value
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("—")
        .to_owned()
}
fn fill(window: &DetailWindow, wall: &Wallpaper) {
    window.set_wallpaper_id(wall.id.clone().into());
    window.set_resolution(value(&wall.resolution).into());
    window.set_file_size(
        wall.file_size
            .map(fmt_size)
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    window.set_file_format(value(&wall.file_type).into());
    window.set_category(format!("{} / {}", value(&wall.category), value(&wall.purity)).into());
    window.set_uploader(
        wall.uploader
            .as_ref()
            .and_then(|u| u.username.clone())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    window.set_uploader_available(public_profile::uploader_username(wall).is_some());
    window.set_views(
        wall.views
            .map(|v| v.to_string())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    window.set_favorites(
        wall.favorites
            .map(|v| v.to_string())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    window.set_created_at(value(&wall.created_at).into());
    window.set_tags(slint::ModelRc::new(VecModel::from(
        wall.tags
            .iter()
            .filter_map(|tag| {
                Some(DetailTag {
                    name: tag.name.clone()?.into(),
                    query: tag
                        .id
                        .map(|id| format!("id:{id}"))
                        .unwrap_or_else(|| format!("+{}", tag.name.as_deref().unwrap_or_default()))
                        .into(),
                })
            })
            .collect::<Vec<_>>(),
    )));
    let colors = wall
        .colors
        .iter()
        .filter_map(|color| {
            let color = color.trim_start_matches('#');
            if color.len() != 6 {
                return None;
            }
            let rgb = u32::from_str_radix(color, 16).ok()?;
            Some(DetailSwatch {
                value: color.into(),
                tint: slint::Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
                    .into(),
            })
        })
        .collect::<Vec<_>>();
    window.set_palette(slint::ModelRc::new(VecModel::from(colors)));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiple_detail_windows_keep_main_gallery_and_each_other_independent() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
        ui.show().unwrap();
        ui.set_content_height(3000.0);
        ui.set_query_text("keep query".into());
        ui.set_browse_position(420.0);
        slint::platform::update_timers_and_animations();
        let before = ui.get_scroll_offset();
        let hub = WindowHub::new(
            &ui,
            Rc::new(RefCell::new(test_state())),
            Arc::new(ImageLoader::new(1)),
            Default::default(),
        );
        let a = hub
            .add_detail(
                Wallpaper {
                    id: "first1".into(),
                    ..Default::default()
                },
                None,
                ui.window(),
                false,
            )
            .unwrap();
        let b = hub
            .add_detail(
                Wallpaper {
                    id: "second".into(),
                    ..Default::default()
                },
                None,
                ui.window(),
                false,
            )
            .unwrap();
        assert_ne!(a, b);
        assert_eq!(hub.details.borrow().len(), 2);
        assert_eq!(ui.get_screen(), 0);
        assert_eq!(ui.get_query_text().as_str(), "keep query");
        assert!((ui.get_scroll_offset() - before).abs() < 1.0);
        hub.details.borrow()[0].window.set_zoom(2.0);
        assert_eq!(hub.details.borrow()[1].window.get_zoom(), 1.0);
        hub.details.borrow()[0].window.hide().unwrap();
        hub.pump();
        assert_eq!(hub.details.borrow().len(), 1);
        assert_eq!(
            hub.details.borrow()[0].window.get_wallpaper_id().as_str(),
            "second"
        );
        hub.close_all();
        assert!(hub.details.borrow().is_empty());
        ui.hide().unwrap();
    }
    #[test]
    fn detail_events_are_scoped_and_permissions_close_only_restricted_windows() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.show().unwrap();
        let state = Rc::new(RefCell::new(test_state()));
        state.borrow_mut().settings.api_key = Some("fixture-key".into());
        state.borrow_mut().settings.nsfw_enabled = true;
        let hub = WindowHub::new(
            &ui,
            state.clone(),
            Arc::new(ImageLoader::new(1)),
            Default::default(),
        );
        hub.add_detail(
            Wallpaper {
                id: "safe01".into(),
                purity: Some("sfw".into()),
                ..Default::default()
            },
            None,
            ui.window(),
            false,
        )
        .unwrap();
        hub.add_detail(
            Wallpaper {
                id: "other1".into(),
                purity: Some("nsfw".into()),
                ..Default::default()
            },
            None,
            ui.window(),
            false,
        )
        .unwrap();
        hub.details.borrow()[0]
            .tx
            .send(DetailEvent::Metadata(
                99,
                Box::new(Ok(Wallpaper {
                    id: "wrong1".into(),
                    ..Default::default()
                })),
            ))
            .unwrap();
        hub.pump();
        assert_eq!(
            hub.details.borrow()[0].window.get_wallpaper_id().as_str(),
            "safe01"
        );
        state.borrow_mut().settings.nsfw_enabled = false;
        hub.pump();
        assert_eq!(hub.details.borrow().len(), 1);
        assert_eq!(hub.details.borrow()[0].wall.id, "safe01");
        assert!(hub
            .add_detail(
                Wallpaper {
                    id: "denied".into(),
                    purity: Some("nsfw".into()),
                    ..Default::default()
                },
                None,
                ui.window(),
                false
            )
            .is_err());
        hub.close_all();
        ui.hide().unwrap();
    }
}
