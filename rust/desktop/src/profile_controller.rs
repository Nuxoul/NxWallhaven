//! Independent native uploader profile state and controller.
use super::*;
use crate::public_profile::{ProfileClient, PublicProfile};
use wh_core::Collection;

#[derive(Clone, Copy, Default, PartialEq)]
enum Source {
    #[default]
    Uploads,
    Collections,
    Collection(u32),
}
#[derive(Clone, Copy)]
struct Filters {
    sfw: bool,
    sketchy: bool,
    nsfw: bool,
}
impl Default for Filters {
    fn default() -> Self {
        Self {
            sfw: true,
            sketchy: false,
            nsfw: false,
        }
    }
}
// Own the bounded image cache without changing the main gallery's Images type.
pub(super) struct ProfileState {
    pub(super) request: u64,
    epoch: u64,
    pub(super) preview_request: u64,
    filters: Filters,
    nsfw_allowed: bool,
    key: Option<String>,
    pub(super) username: String,
    pub(super) page: u32,
    pub(super) last: u32,
    pub(super) walls: Vec<Wallpaper>,
    collections: Vec<Collection>,
    source: Source,
    busy: bool,
    in_flight: Option<u32>,
    exhausted: bool,
    load_error: bool,
    width: f32,
    offset: f32,
    loader_generation: u64,
    pending: HashSet<String>,
    failed: HashSet<String>,
    images: Images,
}
impl Default for ProfileState {
    fn default() -> Self {
        Self {
            request: 0,
            epoch: 0,
            preview_request: 0,
            filters: Filters::default(),
            nsfw_allowed: false,
            key: None,
            username: String::new(),
            page: 0,
            last: 0,
            walls: Vec::new(),
            collections: Vec::new(),
            source: Source::default(),
            busy: false,
            in_flight: None,
            exhausted: false,
            load_error: false,
            width: 0.0,
            offset: 0.0,
            loader_generation: 0,
            pending: HashSet::new(),
            failed: HashSet::new(),
            images: Images::new(),
        }
    }
}
impl ProfileState {
    pub(super) fn purity(&self) -> String {
        let bits = format!(
            "{}{}{}",
            u8::from(self.filters.sfw),
            u8::from(self.filters.sketchy),
            u8::from(self.filters.nsfw && self.nsfw_allowed)
        );
        if bits == "000" {
            "100".into()
        } else {
            bits
        }
    }
    pub(super) fn set_filters(&mut self, sfw: bool, sketchy: bool, nsfw: bool) {
        self.filters = Filters {
            sfw,
            sketchy,
            nsfw: nsfw && self.nsfw_allowed,
        };
        if !self.filters.sfw && !self.filters.sketchy && !self.filters.nsfw {
            self.filters.sfw = true;
        }
        self.invalidate_content();
    }
    pub(super) fn sync_permissions(&mut self, settings: &Settings) -> bool {
        let key = settings
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let allowed = settings.nsfw_enabled && settings.has_api_key() && key.is_some();
        if self.key == key && self.nsfw_allowed == allowed {
            return false;
        }
        // A changed key never silently re-selects NSFW, even if the new key is valid.
        self.filters.nsfw = false;
        if !self.filters.sfw && !self.filters.sketchy {
            self.filters.sfw = true;
        }
        self.key = key;
        self.nsfw_allowed = allowed;
        self.invalidate_content();
        self.collections.clear();
        true
    }
    fn invalidate_content(&mut self) {
        self.request += 1;
        self.preview_request += 1;
        self.busy = false;
        self.in_flight = None;
        self.exhausted = false;
        self.load_error = false;
        self.walls.clear();
        self.images = Images::new();
        self.pending.clear();
        self.failed.clear();
        self.page = 0;
        self.last = 0;
        self.offset = 0.0;
    }
    /// At most one page per content generation may be in flight. An error pauses
    /// automatic retries until the user explicitly requests more.
    pub(super) fn start_page(&mut self) -> Option<(u64, u32)> {
        if self.in_flight.is_some()
            || self.exhausted
            || self.load_error
            || self.source == Source::Collections
            || self.busy
        {
            return None;
        }
        let page = self.page + 1;
        self.in_flight = Some(page);
        self.busy = true;
        Some((self.request, page))
    }
    fn permits(&self, wall: &Wallpaper) -> bool {
        match wall.purity.as_deref() {
            Some("sfw") => self.purity().as_bytes()[0] == b'1',
            Some("sketchy") => self.purity().as_bytes()[1] == b'1',
            Some("nsfw") => self.purity().as_bytes()[2] == b'1',
            _ => false,
        }
    }
    pub(super) fn open(&mut self, username: String) -> u64 {
        self.epoch += 1;
        self.filters = Filters::default();
        self.username = username;
        self.source = Source::Uploads;
        self.invalidate_content();
        self.collections.clear();
        self.request
    }
    pub(super) fn receive_page(
        &mut self,
        request: u64,
        page: u32,
        response: SearchResponse,
    ) -> bool {
        if request != self.request || self.in_flight != Some(page) {
            return false;
        }
        self.in_flight = None;
        self.busy = false;
        self.load_error = false;
        self.last = response.meta.last_page.unwrap_or(page).max(page);
        self.page = page;
        let mut seen: HashSet<String> = self.walls.iter().map(|w| w.id.clone()).collect();
        let before = self.walls.len();
        let additions = response
            .data
            .into_iter()
            .filter(|wall| {
                self.permits(wall) && !wall.id.is_empty() && seen.insert(wall.id.clone())
            })
            .collect::<Vec<_>>();
        self.walls.extend(additions);
        self.exhausted = self.walls.len() == before || page >= self.last;
        true
    }
}
pub(super) struct ProfileTools {
    pub(super) window: ProfileWindow,
    _timer: slint::Timer,
}
enum ResultEvent {
    Info(u64, Result<PublicProfile, String>),
    Avatar(u64, Result<DecodedImage, String>),
    Page(u64, u32, Result<SearchResponse, String>),
    Collections(u64, Result<wh_core::CollectionsResponse, String>),
    Thumb(u64, String, Result<DecodedImage, String>),
}
fn render(window: &ProfileWindow, st: &mut ProfileState) -> Vec<(usize, String)> {
    let width = window.get_gallery_width().max(400.0);
    let resized = st.width >= 200.0 && (st.width - width).abs() > 0.5;
    let offset = if resized {
        st.offset * width / st.width
    } else {
        window.get_gallery_offset().max(0.0)
    };
    st.width = width;
    st.offset = offset;
    let height = window.get_gallery_visible_height().max(500.0);
    let mut geometry = st
        .walls
        .iter()
        .map(|wall| CardModel {
            aspect: wall.aspect_ratio() as f32,
            ..Default::default()
        })
        .collect::<Vec<_>>();
    gallery_layout::arrange(
        &mut geometry,
        width,
        0,
        gallery_layout::PROFILE_CANVAS_WIDTH,
        180.0,
    );
    let content_height = geometry
        .iter()
        .map(|card| card.y + card.h)
        .fold(0.0, f32::max);
    let retained = window
        .get_cards()
        .iter()
        .filter(|card| card.loaded)
        .map(|card| (card.id.to_string(), card.thumb))
        .collect::<HashMap<_, _>>();
    let mut jobs = Vec::new();
    let cards = geometry
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            // Slint only holds image references for the visible/overscan cards.
            if item.y + item.h < offset - height || item.y > offset + 2.0 * height {
                return None;
            }
            let wall = &st.walls[index];
            let image = st
                .images
                .get(&wall.id)
                .or_else(|| retained.get(&wall.id).cloned());
            if image.is_none()
                && st.pending.len() < 24
                && !st.pending.contains(&wall.id)
                && !st.failed.contains(&wall.id)
            {
                if let Some(url) = wall.thumb_url().filter(|url| safe_thumb(url)) {
                    st.pending.insert(wall.id.clone());
                    jobs.push((index, url.to_owned()));
                } else {
                    st.failed.insert(wall.id.clone());
                }
            }
            Some(ProfileCard {
                id: wall.id.clone().into(),
                label: wall
                    .resolution
                    .clone()
                    .unwrap_or_else(|| wall.id.clone())
                    .into(),
                load_message: if st.failed.contains(&wall.id) {
                    "预览不可用"
                } else {
                    "正在加载缩略图"
                }
                .into(),
                thumb: image.clone().unwrap_or_default(),
                loaded: image.is_some(),
                x: item.x,
                y: item.y,
                w: item.w,
                h: item.h,
            })
        })
        .collect::<Vec<_>>();
    window.set_content_height(content_height);
    if resized {
        st.offset = offset.min((content_height - window.get_gallery_visible_height()).max(0.0));
        window.set_gallery_content_y(-st.offset);
    }
    let current = window.get_cards();
    let updated = gallery_layout::sync_model(current.clone(), cards, |card| card.id.as_str());
    if current != updated {
        window.set_cards(updated);
    }
    window.set_loaded_count(st.walls.len() as i32);
    window.set_has_next(!st.exhausted && st.page > 0);
    window.set_load_error(st.load_error);
    jobs
}

fn load_visible(
    window: &ProfileWindow,
    st: &mut ProfileState,
    loader: &ImageLoader,
    tx: &mpsc::Sender<ResultEvent>,
) {
    let jobs = render(window, st);
    if jobs.is_empty() {
        return;
    }
    let ids = st.walls.iter().map(|w| w.id.clone()).collect::<Vec<_>>();
    let generation = st.loader_generation;
    let tx = tx.clone();
    loader.load_batch(jobs, generation, move |index, result| {
        if let Some(id) = ids.get(index) {
            let _ = tx.send(ResultEvent::Thumb(generation, id.clone(), result));
        }
    });
}
fn info(
    window: &ProfileWindow,
    st: &mut ProfileState,
    main: &Rc<RefCell<State>>,
    tx: &mpsc::Sender<ResultEvent>,
) {
    st.epoch += 1;
    st.busy = true;
    window.set_busy(true);
    window.set_message("正在读取公开资料…".into());
    let epoch = st.epoch;
    let username = st.username.clone();
    let client = match ProfileClient::new(main.borrow().client.clone()) {
        Ok(client) => client,
        Err(error) => {
            st.busy = false;
            window.set_busy(false);
            window.set_message(error.into());
            return;
        }
    };
    let tx = tx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(ResultEvent::Info(epoch, client.profile(&username)));
    });
}
fn request(
    window: &ProfileWindow,
    st: &mut ProfileState,
    main: &Rc<RefCell<State>>,
    tx: &mpsc::Sender<ResultEvent>,
) {
    let source = st.source;
    let (token, page) = if source == Source::Collections {
        if st.busy {
            return;
        }
        st.busy = true;
        (st.request, 1)
    } else {
        let Some(next) = st.start_page() else {
            return;
        };
        next
    };
    window.set_busy(true);
    window.set_load_error(false);
    window.set_message("正在读取筛选后的公开内容…".into());
    let username = st.username.clone();
    let purity = st.purity();
    let current = main.borrow();
    let key = public_profile::content_key(&current.settings, &purity);
    let client = match ProfileClient::new(current.client.clone()) {
        Ok(client) => client,
        Err(error) => {
            st.busy = false;
            st.in_flight = None;
            st.load_error = true;
            window.set_busy(false);
            window.set_load_error(true);
            window.set_message(error.into());
            return;
        }
    };
    drop(current);
    let tx = tx.clone();
    std::thread::spawn(move || {
        let event = match source {
            Source::Uploads => {
                ResultEvent::Page(token, page, client.uploads(&username, page, &purity, key))
            }
            Source::Collection(id) => ResultEvent::Page(
                token,
                page,
                client.collection_page(&username, id, page, &purity, key),
            ),
            Source::Collections => ResultEvent::Collections(token, client.collections(&username)),
        };
        let _ = tx.send(event);
    });
}
fn clear_gallery(window: &ProfileWindow, st: &mut ProfileState, loader: &ImageLoader) {
    st.invalidate_content();
    st.loader_generation = loader.invalidate();
    window.set_gallery_content_y(0.0);
    window.set_busy(false);
    render(window, st);
}
fn show_info(window: &ProfileWindow, profile: PublicProfile) -> Option<String> {
    window.set_username(profile.username.into());
    window.set_group_name(profile.group.unwrap_or_else(|| "—".into()).into());
    window.set_biography(
        profile
            .bio
            .unwrap_or_else(|| "未公开个人简介".into())
            .into(),
    );
    window.set_fields(slint::ModelRc::new(VecModel::from(
        profile
            .fields
            .into_iter()
            .map(|(label, value)| ProfileField {
                label: label.into(),
                value: value.into(),
            })
            .collect::<Vec<_>>(),
    )));
    profile.avatar_url
}
pub(super) type OpenWallpaper = Rc<dyn Fn(Wallpaper, Option<Image>, &slint::Window)>;

pub(super) fn create(
    ui: &AppWindow,
    main: Rc<RefCell<State>>,
    username: String,
    parent: &slint::Window,
    open_wallpaper: OpenWallpaper,
) -> Result<ProfileTools, slint::PlatformError> {
    let window = ProfileWindow::new()?;
    window
        .window()
        .set_size(slint::LogicalSize::new(960.0, 740.0));
    let state = Rc::new(RefCell::new(ProfileState::default()));
    let loader = Arc::new(ImageLoader::new(4));
    let avatar_loader = Arc::new(ImageLoader::new(1));
    let (tx, rx) = mpsc::channel();
    {
        let mut st = state.borrow_mut();
        st.open(username.clone());
        st.sync_permissions(&main.borrow().settings);
        st.set_filters(ui.get_sfw(), ui.get_sketchy(), ui.get_nsfw());
        window.set_sfw(st.filters.sfw);
        window.set_sketchy(st.filters.sketchy);
        window.set_nsfw(st.filters.nsfw);
        window.set_nsfw_allowed(st.nsfw_allowed);
        window.set_username(username.into());
        clear_gallery(&window, &mut st, &loader);
        window_chrome::show_centered(&window, parent)?;
        info(&window, &mut st, &main, &tx);
    }
    {
        let state = state.clone();
        let loader = loader.clone();
        let weak = window.as_weak();
        window.window().on_close_requested(move || {
            let mut st = state.borrow_mut();
            st.collections.clear();
            st.epoch += 1;
            if let Some(window) = weak.upgrade() {
                window.set_preview_open(false);
                window.set_preview(Default::default());
                clear_gallery(&window, &mut st, &loader);
            }
            slint::CloseRequestResponse::HideWindow
        });
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        let main = main.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        window.on_select_tab(move |tab| {
            let Some(window) = weak.upgrade() else { return };
            if !(0..=2).contains(&tab) || window.get_tab() == tab {
                return;
            }
            let mut st = state.borrow_mut();
            // Tab switches restart the content at page one, not the main gallery.
            window.set_tab(tab);
            window.set_preview_open(false);
            window.set_preview(Default::default());
            clear_gallery(&window, &mut st, &loader);
            match tab {
                0 => {
                    window.set_message("公开资料 · 未公开字段不会补写为零".into());
                }
                1 => {
                    st.source = Source::Uploads;
                    window.set_gallery_title("上传作品".into());
                    request(&window, &mut st, &main, &tx);
                }
                _ => {
                    st.source = Source::Collections;
                    window.set_gallery_title("公开收藏 · 请选择收藏夹".into());
                    request(&window, &mut st, &main, &tx);
                }
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        let main = main.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        window.on_filters_changed(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            st.sync_permissions(&main.borrow().settings);
            window.set_nsfw_allowed(st.nsfw_allowed);
            st.set_filters(window.get_sfw(), window.get_sketchy(), window.get_nsfw());
            window.set_sfw(st.filters.sfw);
            window.set_nsfw(st.filters.nsfw);
            window.set_preview_open(false);
            window.set_preview(Default::default());
            clear_gallery(&window, &mut st, &loader);
            if window.get_tab() != 0 {
                request(&window, &mut st, &main, &tx);
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        let main = main.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        window.on_reload(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            if st.busy {
                return;
            }
            if window.get_tab() == 0 {
                info(&window, &mut st, &main, &tx);
            } else {
                clear_gallery(&window, &mut st, &loader);
                request(&window, &mut st, &main, &tx);
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        let main = main.clone();
        let tx = tx.clone();
        window.on_load_more(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            if !st.load_error
                || st.busy
                || window.get_tab() == 0
                || st.source == Source::Collections
            {
                return;
            }
            st.load_error = false;
            request(&window, &mut st, &main, &tx);
        });
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        let main = main.clone();
        let tx = tx.clone();
        let loader = loader.clone();
        window.on_open_collection(move |index| {
            let Some(window) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            if st.busy || index < 0 {
                return;
            }
            let Some(collection) = st.collections.get(index as usize).cloned() else {
                return;
            };
            let Some(id) = collection.id.filter(|id| *id > 0) else {
                return;
            };
            st.source = Source::Collection(id);
            clear_gallery(&window, &mut st, &loader);
            window.set_gallery_title(
                format!(
                    "{} · 筛选后内容",
                    collection.label.unwrap_or_else(|| "未命名收藏夹".into())
                )
                .into(),
            );
            request(&window, &mut st, &main, &tx);
        });
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        window.on_open_wallpaper(move |id| {
            let Some(window) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            let Some(wall) = st.walls.iter().find(|wall| wall.id == id.as_str()).cloned() else {
                return;
            };
            if !st.permits(&wall) {
                return;
            }
            let thumb = st.images.get(&wall.id);
            open_wallpaper(wall, thumb, window.window());
        });
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        window.on_close_preview(move || {
            if let Some(window) = weak.upgrade() {
                state.borrow_mut().preview_request += 1;
                window.set_preview_open(false);
                window.set_preview(Default::default());
            }
        });
    }
    let main_timer = main.clone();
    let timer_loader = loader.clone();
    let timer = slint::Timer::default();
    let weak = window.as_weak();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(40),
        move || {
            let Some(window) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            if !window.window().is_visible() || window.window().is_minimized() {
                return;
            }
            if st.sync_permissions(&main_timer.borrow().settings) {
                window.set_nsfw_allowed(st.nsfw_allowed);
                window.set_sfw(st.filters.sfw);
                window.set_sketchy(st.filters.sketchy);
                window.set_nsfw(false);
                window.set_busy(false);
                window.set_preview_open(false);
                window.set_preview(Default::default());
                clear_gallery(&window, &mut st, &timer_loader);
                if window.get_tab() == 2 {
                    st.source = Source::Collections;
                    window.set_gallery_title("公开收藏 · 请选择收藏夹".into());
                    window.set_collections(Default::default());
                    window.set_selected_collection(-1);
                    request(&window, &mut st, &main_timer, &tx);
                } else if window.get_tab() == 1 {
                    request(&window, &mut st, &main_timer, &tx);
                }
            }
            while let Ok(event) = rx.try_recv() {
                match event {
                    ResultEvent::Info(epoch, result) if epoch == st.epoch => {
                        if window.get_tab() == 0 {
                            st.busy = false;
                            window.set_busy(false);
                        }
                        match result {
                            Ok(profile) => {
                                let avatar = show_info(&window, profile);
                                if window.get_tab() == 0 {
                                    window
                                        .set_message("已读取公开资料 · 作品按所选分级筛选".into());
                                }
                                if let Some(url) = avatar {
                                    let tx = tx.clone();
                                    let generation = avatar_loader.invalidate();
                                    avatar_loader.load_batch(
                                        vec![(0, url)],
                                        generation,
                                        move |_, result| {
                                            let _ = tx.send(ResultEvent::Avatar(epoch, result));
                                        },
                                    );
                                }
                            }
                            Err(error) => {
                                window.set_biography("个人资料读取失败，可点击刷新重试".into());
                                if window.get_tab() == 0 {
                                    window.set_message(error.into());
                                }
                            }
                        }
                    }
                    ResultEvent::Avatar(epoch, Ok(image)) if epoch == st.epoch => {
                        window.set_avatar(to_image(image));
                        window.set_avatar_loaded(true);
                    }
                    ResultEvent::Page(token, page, result)
                        if token == st.request && st.in_flight == Some(page) =>
                    {
                        window.set_busy(false);
                        match result {
                            Ok(response) => {
                                if st.receive_page(token, page, response) {
                                    load_visible(&window, &mut st, &timer_loader, &tx);
                                    window.set_message(if st.walls.is_empty() {
                                        "没有符合筛选条件的作品".into()
                                    } else {
                                        format!("已加载 {} 张符合筛选的作品", st.walls.len()).into()
                                    });
                                }
                            }
                            Err(error) => {
                                st.in_flight = None;
                                st.busy = false;
                                st.load_error = true;
                                window.set_load_error(true);
                                window.set_message(
                                    format!("读取失败：{error} · 可点击重试加载更多").into(),
                                );
                            }
                        }
                    }
                    ResultEvent::Collections(token, result)
                        if token == st.request && st.source == Source::Collections && st.busy =>
                    {
                        st.busy = false;
                        window.set_busy(false);
                        match result {
                            Ok(response) => {
                                st.collections = response
                                    .data
                                    .into_iter()
                                    .filter(|c| {
                                        c.public != Some(false) && c.id.is_some_and(|id| id > 0)
                                    })
                                    .take(500)
                                    .collect();
                                let names = st
                                    .collections
                                    .iter()
                                    .map(|c| {
                                        format!(
                                            "{}  ({})",
                                            c.label.as_deref().unwrap_or("未命名收藏夹"),
                                            c.count
                                                .map(|n| n.to_string())
                                                .unwrap_or_else(|| "—".into())
                                        )
                                        .into()
                                    })
                                    .collect::<Vec<slint::SharedString>>();
                                window.set_collections(slint::ModelRc::new(VecModel::from(names)));
                                window.set_selected_collection(if st.collections.is_empty() {
                                    -1
                                } else {
                                    0
                                });
                                window.set_message(if st.collections.is_empty() {
                                    "此用户没有可读取的公开收藏夹".into()
                                } else {
                                    format!(
                                        "{} 个公开收藏夹 · 数量为网站总数，内容按所选分级筛选",
                                        st.collections.len()
                                    )
                                    .into()
                                });
                            }
                            Err(error) => {
                                window.set_message(format!("公开收藏读取失败：{error}").into())
                            }
                        }
                    }
                    ResultEvent::Thumb(token, id, result)
                        if token == st.loader_generation && st.pending.remove(&id) =>
                    {
                        match result {
                            Ok(image) => {
                                st.images.insert(id, to_image(image));
                            }
                            Err(_) => {
                                st.failed.insert(id);
                            }
                        }
                        load_visible(&window, &mut st, &timer_loader, &tx);
                    }
                    _ => {}
                }
            }
            if window.get_tab() == 0 || window.get_preview_open() || window.window().is_minimized()
            {
                return;
            }
            let offset = window.get_gallery_offset();
            if st.pending.len() >= 24
                && (offset - st.offset).abs() > window.get_gallery_visible_height().max(500.0)
            {
                st.loader_generation = timer_loader.invalidate();
                st.pending.clear();
            }
            if (window.get_gallery_width() - st.width).abs() > 1.0
                || (offset - st.offset).abs() > 1.0
            {
                load_visible(&window, &mut st, &timer_loader, &tx);
            }
            // Prefetch page two immediately; subsequent pages as the viewport approaches the end.
            if st.page > 0
                && !st.exhausted
                && !st.load_error
                && st.in_flight.is_none()
                && (st.page == 1
                    || window.get_gallery_offset() + window.get_gallery_visible_height() * 2.0
                        >= window.get_content_height())
            {
                request(&window, &mut st, &main_timer, &tx);
            }
        },
    );
    Ok(ProfileTools {
        window,
        _timer: timer,
    })
}
fn image_url(value: &str, host: &str, prefix: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some(host)
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path().starts_with(prefix)
    })
}
fn safe_thumb(value: &str) -> bool {
    image_url(value, "th.wallhaven.cc", "/")
}
