#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Windows desktop client. All UI state is owned by the Slint event-loop thread.
mod account;
mod account_controller;
mod browser_profile;
mod browser_session;
mod cache_control;
mod collection_management;
mod collection_ui_controller;
mod config;
#[cfg(test)]
mod default_collection_regression;
mod detail_loading;
mod gallery_layout;
mod image_disk_cache;
mod image_loader;
#[cfg(test)]
mod login_regression;
#[cfg(test)]
mod management_regression;
mod platform;
mod profile_controller;
#[cfg(test)]
mod profile_regression;
mod public_profile;
#[cfg(test)]
mod resize_regression;
mod search;
mod website;
mod website_collections;
mod website_controller;
mod window_chrome;
mod window_hub;
#[cfg(test)]
mod zoom_regression;
use config::Settings;
use image_loader::{DecodedImage, ImageLoader};
use slint::{ComponentHandle, Image, Model, VecModel};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use wh_core::{SearchQuery, SearchResponse, WallhavenClient, Wallpaper};
slint::include_modules!();
const GAP: f32 = 8.0;
#[derive(Clone, Default)]
struct CardModel {
    id: String,
    label: String,
    aspect: f32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}
fn relayout(cards: &mut [CardModel], width: f32, mode: i32) {
    gallery_layout::arrange(cards, width, mode, gallery_layout::MAIN_CANVAS_WIDTH, 210.0);
}
struct ImageStore {
    map: HashMap<String, Image>,
    order: VecDeque<String>,
    bytes: usize,
}
struct Images {
    store: Rc<RefCell<ImageStore>>,
    _cache_registration: cache_control::UiRegistration,
}
impl Images {
    fn new() -> Self {
        let store = Rc::new(RefCell::new(ImageStore {
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        }));
        let size = Rc::downgrade(&store);
        let clear = Rc::downgrade(&store);
        let registration = cache_control::register_ui(
            move || {
                size.upgrade()
                    .map(|store| store.borrow().bytes as u64)
                    .unwrap_or(0)
            },
            move || {
                if let Some(store) = clear.upgrade() {
                    let mut store = store.borrow_mut();
                    store.map.clear();
                    store.order.clear();
                    store.bytes = 0;
                }
            },
        );
        Self {
            store,
            _cache_registration: registration,
        }
    }
    fn get(&mut self, id: &str) -> Option<Image> {
        let mut store = self.store.borrow_mut();
        let image = store.map.get(id)?.clone();
        store.order.retain(|key| key != id);
        store.order.push_back(id.to_owned());
        Some(image)
    }
    fn insert(&mut self, id: String, image: Image) {
        let mut store = self.store.borrow_mut();
        if let Some(old) = store.map.remove(&id) {
            store.bytes -= image_bytes(&old);
        }
        store.order.retain(|key| key != &id);
        store.bytes += image_bytes(&image);
        store.map.insert(id.clone(), image);
        store.order.push_back(id);
        while store.bytes > 48 * 1024 * 1024 || store.map.len() > 150 {
            let Some(key) = store.order.pop_front() else {
                break;
            };
            if let Some(old) = store.map.remove(&key) {
                store.bytes -= image_bytes(&old);
            }
        }
    }
}
fn image_bytes(image: &Image) -> usize {
    let s = image.size();
    s.width as usize * s.height as usize * 4
}
struct State {
    settings: Settings,
    client: WallhavenClient,
    walls: Vec<Wallpaper>,
    cards: Vec<CardModel>,
    query: SearchQuery,
    page: u32,
    last: u32,
    seed: Option<String>,
    busy: bool,
    generation: u64,
    loader_generation: u64,
    width: f32,
    offset: f32,
    images: Images,
    pending: HashSet<String>,
    failed: HashSet<String>,
    detail: Option<Wallpaper>,
    detail_generation: u64,
    download_busy: bool,
    message: String,
    account: account::Session,
    collections: Vec<wh_core::models::Collection>,
    collections_generation: u64,
    collections_owner: String,
    collections_authenticated: bool,
    collections_website: bool,
    source: Option<account::CollectionSource>,
    web: website_controller::WebState,
}
impl State {
    fn height(&self) -> f32 {
        self.cards.iter().map(|c| c.y + c.h).fold(0.0, f32::max)
    }
    fn has_more(&self) -> bool {
        self.page > 0 && self.page < self.last
    }
    fn receive_page(&mut self, page: u32, response: SearchResponse) -> bool {
        self.last = response.meta.last_page.unwrap_or(page);
        if self.seed.is_none() {
            self.seed = response.meta.seed;
        }
        if response.data.is_empty() {
            self.last = self.page;
            self.message = "没有更多结果".into();
            return false;
        }
        let mut seen = self
            .walls
            .iter()
            .map(|w| w.id.clone())
            .collect::<HashSet<_>>();
        let previous_count = self.walls.len();
        for w in response.data {
            if !seen.insert(w.id.clone()) {
                continue;
            }
            self.cards.push(CardModel {
                id: w.id.clone(),
                label: w.resolution.clone().unwrap_or_default(),
                aspect: w.aspect_ratio() as f32,
                ..Default::default()
            });
            self.walls.push(w);
        }
        self.page = page;
        if self.walls.len() == previous_count {
            self.last = self.page;
            self.message = "接口返回重复页，已停止自动加载".into();
            return false;
        }
        self.message = "已载入结果".into();
        true
    }
}
enum Event {
    Page(u64, u32, Result<SearchResponse, String>),
    Thumb(u64, String, Result<DecodedImage, String>),
    Detail(u64, Box<Result<Wallpaper, String>>),
    Preview(u64, Result<DecodedImage, String>),
    Progress(String, u64, Option<u64>),
    Download(String, Result<PathBuf, String>),
    Wallpaper(u64, Result<(), String>),
    Account(u64, Result<(), String>),
    Collections(
        u64,
        String,
        Result<wh_core::models::CollectionsResponse, String>,
    ),
    WebBrowserReady(u64, Result<Arc<website::SiteClient>, website::SiteError>),
    WebLogin(
        u64,
        Result<(Arc<website::SiteClient>, String), website::SiteError>,
    ),
    WebNotice(u64, String),
    WebLogout(u64, Result<(), String>),
    WebFavorite(
        u64,
        u64,
        u64,
        bool,
        Result<website::FavoriteState, website::SiteError>,
    ),
}
fn to_image(data: DecodedImage) -> Image {
    Image::from_rgba8(data.rgba)
}
fn fmt_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / 1048576.0)
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}
fn wallpaper_filename(wall: &Wallpaper) -> Result<String, String> {
    if wall.id.is_empty() || !wall.id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("壁纸 ID 无效".into());
    }
    let extension = match wall.file_type.as_deref() {
        Some("image/png") => "png",
        Some("image/webp") => "webp",
        _ => "jpg",
    };
    Ok(format!("wallhaven-{}.{}", wall.id, extension))
}
fn wallpaper_link(id: &str) -> Result<String, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("壁纸 ID 无效".into());
    }
    Ok(format!("https://wallhaven.cc/w/{id}"))
}
fn query_from_ui(ui: &AppWindow) -> Result<SearchQuery, String> {
    let bits = |a, b, c| format!("{}{}{}", u8::from(a), u8::from(b), u8::from(c));
    let mut query = SearchQuery {
        q: ui.get_query_text().to_string(),
        categories: bits(
            ui.get_cat_general(),
            ui.get_cat_anime(),
            ui.get_cat_people(),
        ),
        purity: bits(
            ui.get_sfw(),
            ui.get_sketchy(),
            ui.get_nsfw() && ui.get_nsfw_enabled() && ui.get_has_api_key(),
        ),
        ..Default::default()
    };
    if query.categories == "000" {
        query.categories = "111".into();
    }
    if query.purity == "000" {
        query.purity = "100".into();
    }
    query.sorting = [
        "date_added",
        "relevance",
        "random",
        "views",
        "favorites",
        "toplist",
    ]
    .get(ui.get_sort_index().max(0) as usize)
    .unwrap_or(&"date_added")
    .to_string();
    let ratios = [
        (ui.get_ratio_wide(), "16x9"),
        (ui.get_ratio_desktop(), "16x10"),
        (ui.get_ratio_ultrawide(), "21x9"),
        (ui.get_ratio_classic(), "4x3"),
        (ui.get_ratio_portrait(), "9x16"),
        (ui.get_ratio_square(), "1x1"),
    ]
    .into_iter()
    .filter_map(|(selected, value)| selected.then_some(value))
    .collect();
    search::Filters {
        ascending: ui.get_order_index() == 1,
        resolution: ui.get_resolution_index(),
        width: ui.get_min_width_input().to_string(),
        height: ui.get_min_height_input().to_string(),
        ratios,
        color: ui.get_color_filter().to_string(),
        top_range: ui.get_top_range_index(),
    }
    .apply(&mut query)?;
    Ok(query)
}
fn reset_filter_controls(ui: &AppWindow) {
    ui.set_order_index(0);
    ui.set_resolution_index(0);
    ui.set_min_width_input("".into());
    ui.set_min_height_input("".into());
    ui.set_ratio_wide(false);
    ui.set_ratio_desktop(false);
    ui.set_ratio_ultrawide(false);
    ui.set_ratio_classic(false);
    ui.set_ratio_portrait(false);
    ui.set_ratio_square(false);
    ui.set_color_filter("".into());
    ui.set_top_range_index(3);
    ui.set_filter_error("".into());
}
fn swatches<'a>(colors: impl IntoIterator<Item = &'a str>) -> slint::ModelRc<Swatch> {
    let values = colors
        .into_iter()
        .filter_map(|value| {
            let value = value.trim_start_matches('#');
            if value.len() != 6 {
                return None;
            }
            let rgb = u32::from_str_radix(value, 16).ok()?;
            Some(Swatch {
                value: value.to_ascii_lowercase().into(),
                tint: slint::Brush::SolidColor(slint::Color::from_rgb_u8(
                    (rgb >> 16) as u8,
                    (rgb >> 8) as u8,
                    rgb as u8,
                )),
            })
        })
        .collect::<Vec<_>>();
    slint::ModelRc::new(VecModel::from(values))
}
fn update_status(ui: &AppWindow, st: &State) {
    ui.set_busy(st.busy);
    ui.set_has_more(st.has_more());
    ui.set_status(
        if st.page == 0 {
            st.message.clone()
        } else {
            format!("{} 张 · 第 {} 页 · {}", st.walls.len(), st.page, st.message)
        }
        .into(),
    );
}
/// Read the actual viewport once per pump, never mutate models inside a layout callback.
fn sync_gallery_geometry(ui: &AppWindow, st: &mut State) -> bool {
    let width = ui.get_browse_width();
    let offset = ui.get_scroll_offset().max(0.0);
    if width >= 200.0 && (width - st.width).abs() > 0.5 {
        let scale = width / st.width.max(1.0);
        st.width = width;
        relayout(&mut st.cards, width, ui.get_layout_mode());
        ui.set_content_height(st.height());
        let maximum = (st.height() + 16.0 - ui.get_viewport_height()).max(0.0);
        st.offset = (st.offset * scale).clamp(0.0, maximum);
        ui.set_browse_position(-st.offset);
        true
    } else if (offset - st.offset).abs() > 0.5 {
        st.offset = offset;
        true
    } else {
        false
    }
}
/// A small overscan window. No Image or UI object crosses a worker thread.
fn render(ui: &AppWindow, st: &mut State, loader: &ImageLoader, tx: &mpsc::Sender<Event>) {
    let top = st.offset.max(0.0);
    let viewport = ui.get_viewport_height().max(600.0);
    let low = (top - viewport).max(0.0);
    let high = top + viewport * 3.0;
    let mut model = Vec::<Card>::new();
    let retained = ui
        .get_cards()
        .iter()
        .filter(|card| card.loaded)
        .map(|card| (card.id.to_string(), card.thumb))
        .collect::<HashMap<_, _>>();
    let mut jobs = Vec::new();
    let generation = st.generation;
    for (i, c) in st.cards.iter().enumerate() {
        if c.y + c.h < low || c.y > high {
            continue;
        }
        let image = st
            .images
            .get(&c.id)
            .or_else(|| retained.get(&c.id).cloned());
        let failed = st.failed.contains(&c.id);
        if image.is_none() && !failed && st.pending.insert(c.id.clone()) {
            if let Some(url) = st.walls[i].thumb_url() {
                jobs.push((i, url.to_owned()));
            } else {
                st.failed.insert(c.id.clone());
            }
        }
        model.push(Card {
            thumb: image.clone().unwrap_or_default(),
            label: if failed {
                format!("{} · 加载失败", c.label).into()
            } else {
                c.label.clone().into()
            },
            x: c.x,
            y: c.y,
            w: c.w,
            h: c.h,
            loaded: image.is_some(),
            id: c.id.clone().into(),
            favorited: st.web.favorites.get(&c.id).copied().unwrap_or(false),
        });
    }
    let current = ui.get_cards();
    let updated = gallery_layout::sync_model(current.clone(), model, |card| card.id.as_str());
    if current != updated {
        ui.set_cards(updated);
    }
    ui.set_content_height(st.height());
    if !jobs.is_empty() {
        let ids = st.walls.iter().map(|w| w.id.clone()).collect::<Vec<_>>();
        let tx = tx.clone();
        loader.load_batch(jobs, st.loader_generation, move |i, result| {
            let _ = tx.send(Event::Thumb(generation, ids[i].clone(), result));
        });
    }
    update_status(ui, st);
}
fn should_prefetch(
    page: u32,
    last: u32,
    busy: bool,
    offset: f32,
    viewport: f32,
    height: f32,
) -> bool {
    !busy && page > 0 && page < last && (page == 1 || offset + viewport.max(1.0) * 2.5 >= height)
}
fn request_page(st: &mut State, tx: &mpsc::Sender<Event>, page: u32) {
    if st.busy {
        return;
    }
    st.busy = true;
    st.message = "正在加载…".into();
    let mut client = st.client.clone();
    let mut query = st.query.clone();
    query.seed = st.seed.clone();
    let source = st.source.clone();
    let site = st.web.client.clone();
    if source.as_ref().is_some_and(|source| !source.authenticated) {
        client.set_api_key(None);
    }
    let generation = st.generation;
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = match source {
            Some(source) if source.website => match site {
                Some(site) => site.read_collection_wallpapers(
                    &source.username,
                    source.id,
                    &query.purity,
                    page,
                ),
                None => Err("网站会话已退出，请重新登录".into()),
            },
            Some(source) => client
                .collection_wallpapers(&source.username, source.id, &query.purity, page)
                .map_err(|e| e.to_string()),
            None => client.search(&query, page).map_err(|e| e.to_string()),
        };
        let _ = tx.send(Event::Page(generation, page, result));
    });
}
fn fill_detail(ui: &AppWindow, w: &Wallpaper) {
    ui.set_detail_id(w.id.clone().into());
    ui.set_detail_resolution(
        w.resolution
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    ui.set_detail_size(
        w.file_size
            .map(fmt_size)
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    ui.set_detail_category(
        w.category
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    ui.set_detail_purity(
        w.purity
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    ui.set_detail_uploader(
        w.uploader
            .as_ref()
            .and_then(|u| u.username.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    ui.set_detail_uploader_available(public_profile::uploader_username(w).is_some());
    ui.set_detail_views(
        w.views
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    ui.set_detail_favorites(
        w.favorites
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    ui.set_detail_format(
        w.file_type
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    ui.set_detail_tag_choices(slint::ModelRc::new(VecModel::from(
        w.tags
            .iter()
            .filter_map(|tag| {
                let name = tag.name.as_ref()?;
                let query = tag
                    .id
                    .map(|id| format!("id:{id}"))
                    .unwrap_or_else(|| format!("+{}", name));
                Some(TagChoice {
                    name: name.clone().into(),
                    query: query.into(),
                })
            })
            .collect::<Vec<_>>(),
    )));
    ui.set_detail_palette(swatches(w.colors.iter().map(String::as_str)));
    ui.set_detail_date(
        w.created_at
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    ui.set_detail_colors(w.colors.join(" ").into());
    ui.set_detail_tags(
        w.tags
            .iter()
            .filter_map(|t| t.name.clone())
            .collect::<Vec<_>>()
            .join(" · ")
            .into(),
    );
}
fn reset_search(ui: &AppWindow, st: &mut State, loader: &ImageLoader) -> Result<(), String> {
    let query = query_from_ui(ui)?;
    reset_gallery(ui, st, loader);
    st.query = query;
    st.source = None;
    ui.set_collection_mode(false);
    ui.set_collection_selected(false);
    ui.set_selected_collection_index(-1);
    Ok(())
}
fn reset_gallery(ui: &AppWindow, st: &mut State, loader: &ImageLoader) {
    // Invalidate detail requests too: settings can change while a detail is open.
    st.detail_generation += 1;
    website_controller::reset_favorite(ui);
    st.detail = None;
    ui.set_detail_image(Image::default());
    ui.set_detail_loaded(false);
    ui.set_detail_busy(false);
    ui.set_detail_download_msg("".into());
    fill_detail(ui, &Wallpaper::default());
    ui.set_screen(0);
    st.generation += 1;
    st.loader_generation = loader.invalidate();
    st.busy = false;
    st.seed = None;
    st.walls.clear();
    st.cards.clear();
    st.pending.clear();
    st.failed.clear();
    st.page = 0;
    st.last = 0;
    st.offset = 0.0;
    st.width = ui.get_browse_width().max(800.0);
    ui.set_cards(Default::default());
    ui.set_content_height(0.0);
    ui.set_browse_position(0.0);
}
fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
    let settings_window = SettingsWindow::new()?;
    settings_window
        .window()
        .set_size(slint::LogicalSize::new(640.0, 780.0));
    ui.set_palette(swatches(search::COLORS.iter().copied()));
    let settings = Settings::load();
    ui.set_has_api_key(settings.has_api_key());
    ui.set_nsfw_enabled(settings.nsfw_enabled && settings.has_api_key());
    ui.set_download_dir_input(settings.download_dir.clone().unwrap_or_default().into());
    ui.set_layout_mode(settings.layout_mode);
    let client = WallhavenClient::new(settings.api_key.clone()).expect("HTTP client");
    let loader = Arc::new(ImageLoader::new(8));
    let state = Rc::new(RefCell::new(State {
        settings,
        client,
        walls: vec![],
        cards: vec![],
        query: SearchQuery::default(),
        account: account::Session::default(),
        collections: vec![],
        collections_generation: 0,
        collections_owner: String::new(),
        collections_authenticated: false,
        collections_website: false,
        source: None,
        web: website_controller::WebState::default(),
        page: 0,
        last: 0,
        seed: None,
        busy: false,
        generation: 0,
        loader_generation: 0,
        width: 1100.0,
        offset: 0.0,
        images: Images::new(),
        pending: HashSet::new(),
        failed: HashSet::new(),
        detail: None,
        detail_generation: 0,
        download_busy: false,
        message: "就绪".into(),
    }));
    let _cache_tools = cache_control::bind(&settings_window);
    let (tx, rx) = mpsc::channel::<Event>();
    account_controller::bind(
        &ui,
        &settings_window,
        state.clone(),
        loader.clone(),
        tx.clone(),
    );
    website_controller::bind(
        &ui,
        &settings_window,
        state.clone(),
        loader.clone(),
        tx.clone(),
    );
    let _collection_tools =
        collection_ui_controller::bind(&ui, state.clone(), loader.clone(), tx.clone())?;
    let window_hub = window_hub::WindowHub::new(
        &ui,
        state.clone(),
        loader.clone(),
        _collection_tools.quick_window(),
    );
    _collection_tools.bind_main_close(&ui, &settings_window, window_hub.clone());
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        ui.on_search(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            if let Err(error) = reset_search(&ui, &mut st, &loader) {
                ui.set_filter_error(error.clone().into());
                ui.set_status(format!("筛选无效：{error}").into());
                return;
            }
            ui.set_filter_error("".into());
            request_page(&mut st, &tx, 1);
            update_status(&ui, &st);
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_reset_filters(move || {
            if let Some(ui) = weak.upgrade() {
                reset_filter_controls(&ui);
                ui.set_cat_general(true);
                ui.set_cat_anime(true);
                ui.set_cat_people(true);
                ui.set_sfw(true);
                ui.set_sketchy(false);
                ui.set_nsfw(false);
                ui.set_sort_index(0);
                ui.invoke_search();
            }
        });
        let weak = ui.as_weak();
        ui.on_search_tag(move |query| {
            if let Some(ui) = weak.upgrade() {
                reset_filter_controls(&ui);
                ui.set_query_text(query);
                ui.set_sort_index(1);
                ui.invoke_search();
            }
        });
        let weak = ui.as_weak();
        ui.on_search_color(move |color| {
            if let Some(ui) = weak.upgrade() {
                reset_filter_controls(&ui);
                ui.set_query_text("".into());
                ui.set_color_filter(color);
                ui.set_sort_index(0);
                ui.set_advanced_open(true);
                ui.invoke_search();
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let tx = tx.clone();
        ui.on_load_more(move || {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.borrow_mut();
                if st.has_more() {
                    let page = st.page + 1;
                    request_page(&mut st, &tx, page);
                    update_status(&ui, &st);
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        ui.on_layout_changed(move |mode| {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            let width = st.width;
            relayout(&mut st.cards, width, mode);
            render(&ui, &mut st, &loader, &tx);
        });
    }
    {
        let dialog = settings_window.as_weak();
        let state = state.clone();
        let ui_weak = ui.as_weak();
        ui.on_go_settings(move || {
            if let Some(dialog) = dialog.upgrade() {
                let st = state.borrow();
                dialog.set_api_key_input("".into());
                dialog.set_has_api_key(st.settings.has_api_key());
                dialog.set_nsfw_enabled(st.settings.nsfw_enabled);
                dialog.set_layout_mode(st.settings.layout_mode);
                dialog.set_download_dir_input(
                    st.settings.download_dir.clone().unwrap_or_default().into(),
                );
                dialog.set_account_username_input(
                    st.settings
                        .account_username
                        .clone()
                        .unwrap_or_default()
                        .into(),
                );
                dialog.set_account_status(st.account.status.clone().into());
                dialog.set_account_busy(st.account.busy);
                dialog.set_settings_status("".into());
                let _ = window_chrome::show_centered(
                    &dialog,
                    ui_weak
                        .upgrade()
                        .as_ref()
                        .map(|ui| ui.window())
                        .unwrap_or(dialog.window()),
                );
            }
        });
        let dialog = settings_window.as_weak();
        settings_window.on_close_settings(move || {
            if let Some(dialog) = dialog.upgrade() {
                dialog.set_api_key_input("".into());
                let _ = dialog.hide();
            }
        });
        let dialog = settings_window.as_weak();
        settings_window.window().on_close_requested(move || {
            if let Some(dialog) = dialog.upgrade() {
                dialog.set_api_key_input("".into());
                let _ = dialog.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_go_back(move || {
            if let Some(ui) = weak.upgrade() {
                state.borrow_mut().detail_generation += 1;
                ui.set_detail_busy(false);
                ui.set_screen(0);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let dialog = settings_window.as_weak();
        let state = state.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        settings_window.on_save_settings(move || {
            let (Some(ui), Some(dialog)) = (weak.upgrade(), dialog.upgrade()) else {
                return;
            };
            let mut st = state.borrow_mut();
            let mut candidate = st.settings.clone();
            let input = dialog.get_api_key_input().trim().to_string();
            let key_changed = !input.is_empty();
            let username = dialog.get_account_username_input().trim().to_string();
            if !username.is_empty() {
                if let Err(error) = account::validate_username(&username) {
                    dialog.set_settings_status(error.into());
                    return;
                }
            }
            let username_changed =
                st.settings.account_username.as_deref().unwrap_or_default() != username;
            candidate.account_username = (!username.is_empty()).then_some(username);
            if key_changed {
                if let Err(error) = candidate.set_api_key(Some(input)) {
                    dialog.set_settings_status(format!("保存失败：{error}").into());
                    return;
                }
            }
            candidate.nsfw_enabled = dialog.get_nsfw_enabled();
            if candidate.nsfw_enabled && !candidate.has_api_key() {
                dialog.set_settings_status("请先填写 API Key，未启用 NSFW".into());
                return;
            }
            candidate.layout_mode = dialog.get_layout_mode().clamp(0, 1);
            candidate.download_dir = if dialog.get_download_dir_input().trim().is_empty() {
                None
            } else {
                Some(dialog.get_download_dir_input().trim().to_owned())
            };
            match candidate.save() {
                Ok(()) => {
                    let restrict = st.settings.nsfw_enabled && !candidate.nsfw_enabled;
                    st.client.set_api_key(candidate.api_key.clone());
                    st.settings = candidate;
                    ui.set_has_api_key(st.settings.has_api_key());
                    ui.set_nsfw_enabled(st.settings.nsfw_enabled);
                    ui.set_layout_mode(st.settings.layout_mode);
                    dialog.set_api_key_input("".into());
                    dialog.set_has_api_key(st.settings.has_api_key());
                    dialog.set_settings_status("已保存".into());
                    let width = st.width;
                    let mode = st.settings.layout_mode;
                    relayout(&mut st.cards, width, mode);
                    render(&ui, &mut st, &loader, &tx);
                    if restrict {
                        st.images = Images::new();
                        ui.set_nsfw(false);
                        ui.set_sketchy(false);
                        ui.set_sfw(true);
                    }
                    if restrict || key_changed || username_changed {
                        account_controller::invalidate(&ui, &dialog, &mut st);
                    }
                    drop(st);
                    if restrict || key_changed || username_changed {
                        reset_filter_controls(&ui);
                        ui.invoke_search();
                    }
                }
                Err(error) => dialog.set_settings_status(format!("保存失败：{error}").into()),
            }
        });
    }
    {
        let weak = ui.as_weak();
        let dialog = settings_window.as_weak();
        let state = state.clone();
        settings_window.on_clear_key(move || {
            let (Some(ui), Some(dialog)) = (weak.upgrade(), dialog.upgrade()) else {
                return;
            };
            let mut st = state.borrow_mut();
            let mut candidate = st.settings.clone();
            let result = candidate.set_api_key(None).and_then(|()| {
                candidate.nsfw_enabled = false;
                candidate.account_username = None;
                candidate.save()
            });
            match result {
                Ok(()) => {
                    st.settings = candidate;
                    st.client.set_api_key(None);
                    st.images = Images::new();
                    ui.set_has_api_key(false);
                    ui.set_nsfw_enabled(false);
                    ui.set_nsfw(false);
                    ui.set_sketchy(false);
                    ui.set_sfw(true);
                    dialog.set_has_api_key(false);
                    dialog.set_nsfw_enabled(false);
                    dialog.set_api_key_input("".into());
                    dialog.set_settings_status("Key 已清除".into());
                    dialog.set_account_username_input("".into());
                    account_controller::invalidate(&ui, &dialog, &mut st);
                    drop(st);
                    reset_filter_controls(&ui);
                    ui.invoke_search();
                }
                Err(error) => dialog.set_settings_status(format!("清除失败：{error}").into()),
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        ui.on_download_current(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            if st.download_busy {
                return;
            }
            let Some(wall) = st.detail.clone() else {
                return;
            };
            let filename = match wallpaper_filename(&wall) {
                Ok(filename) => filename,
                Err(error) => {
                    ui.set_detail_download_msg(error.into());
                    return;
                }
            };
            let Some(url) = wall.path else {
                ui.set_detail_download_msg("没有原图链接".into());
                return;
            };
            let path = st.settings.resolved_download_dir().join(filename);
            st.download_busy = true;
            ui.set_downloading(true);
            ui.set_detail_download_msg("开始下载…".into());
            let tx = tx.clone();
            let loader = loader.clone();
            std::thread::spawn(move || {
                let id = wall.id;
                let progress_id = id.clone();
                let progress_tx = tx.clone();
                let result = loader
                    .download_to(&url, &path, move |done, total| {
                        let _ = progress_tx.send(Event::Progress(progress_id.clone(), done, total));
                    })
                    .map(|()| path);
                let _ = tx.send(Event::Download(id, result));
            });
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_open_in_browser(move || {
            if let Some(ui) = weak.upgrade() {
                let id = ui.get_detail_id();
                if id.as_str().chars().all(|c| c.is_ascii_alphanumeric()) && !id.is_empty() {
                    let url = format!("https://wallhaven.cc/w/{id}");
                    use std::os::windows::process::CommandExt;
                    match std::process::Command::new("rundll32.exe")
                        .args(["url.dll,FileProtocolHandler", &url])
                        .creation_flags(0x08000000)
                        .spawn()
                    {
                        Ok(_) => ui.set_detail_download_msg("已交给系统浏览器打开".into()),
                        Err(e) => ui.set_detail_download_msg(format!("打开失败：{e}").into()),
                    }
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_copy_link(move || {
            if let Some(ui) = weak.upgrade() {
                let result = wallpaper_link(ui.get_detail_id().as_str())
                    .and_then(|url| platform::copy_text(&url));
                ui.set_detail_download_msg(match result {
                    Ok(()) => "链接已复制".into(),
                    Err(error) => format!("复制失败：{error}").into(),
                });
            }
        });
        let weak = ui.as_weak();
        let directory_state = state.clone();
        ui.on_open_download_dir(move || {
            if let Some(ui) = weak.upgrade() {
                let path = directory_state.borrow().settings.resolved_download_dir();
                let result = std::fs::create_dir_all(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|()| platform::open_folder(&path));
                ui.set_detail_download_msg(match result {
                    Ok(()) => "已打开下载目录".into(),
                    Err(error) => format!("打开目录失败：{error}").into(),
                });
            }
        });
        let weak = ui.as_weak();
        let state = state.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        ui.on_set_wallpaper(move || {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_wallpaper_busy() {
                return;
            }
            let st = state.borrow();
            let Some(wall) = st.detail.clone() else {
                return;
            };
            let filename = match wallpaper_filename(&wall) {
                Ok(filename) => filename,
                Err(error) => {
                    ui.set_detail_download_msg(error.into());
                    return;
                }
            };
            let Some(url) = wall.path else {
                ui.set_detail_download_msg("没有原图链接".into());
                return;
            };
            let token = st.detail_generation;
            ui.set_wallpaper_busy(true);
            ui.set_detail_download_msg("正在准备原图，完成后设置桌面壁纸…".into());
            let loader = loader.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let path = platform::wallpaper_cache_dir().join(filename);
                let result = (|| {
                    if !path.exists() {
                        loader.download_to(&url, &path, |_, _| {})?;
                    }
                    platform::set_wallpaper(&path)
                })();
                let _ = tx.send(Event::Wallpaper(token, result));
            });
        });
    }
    let pump = slint::Timer::default();
    {
        let weak = ui.as_weak();
        let account_dialog = settings_window.as_weak();
        let state = state.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        let mut previous_viewport_height = 0.0;
        pump.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(40),
            move || {
                let Some(ui) = weak.upgrade() else { return };
                let viewport_height = ui.get_viewport_height();
                let mut changed = sync_gallery_geometry(&ui, &mut state.borrow_mut())
                    || (viewport_height - previous_viewport_height).abs() > 0.5;
                previous_viewport_height = viewport_height;
                for event in rx.try_iter().take(24) {
                    let mut st = state.borrow_mut();
                    match event {
                        Event::WebBrowserReady(generation, result) => {
                            let dialog = account_dialog.upgrade();
                            website_controller::receive_browser(
                                &ui,
                                dialog.as_ref(),
                                &mut st,
                                generation,
                                result,
                            );
                        }
                        Event::WebLogin(generation, result) => {
                            let dialog = account_dialog.upgrade();
                            website_controller::receive_login(
                                &ui,
                                dialog.as_ref(),
                                &mut st,
                                generation,
                                result,
                            );
                        }
                        Event::WebNotice(generation, message)
                            if generation == st.web.generation =>
                        {
                            st.web.status = message;
                            let dialog = account_dialog.upgrade();
                            website_controller::sync(&ui, dialog.as_ref(), &st);
                        }
                        Event::WebLogout(generation, result) if generation == st.web.generation => {
                            let dialog = account_dialog.upgrade();
                            website_controller::receive_logout(
                                &ui,
                                dialog.as_ref(),
                                &mut st,
                                &loader,
                                result,
                            );
                            changed = true;
                        }
                        Event::WebFavorite(session, request, detail, mutation, result) => {
                            let dialog = account_dialog.upgrade();
                            website_controller::receive_favorite(
                                &ui,
                                dialog.as_ref(),
                                &mut st,
                                &loader,
                                (session, request, detail),
                                mutation,
                                result,
                            );
                        }
                        Event::Account(token, result) => {
                            if st.account.complete(token, result) {
                                let dialog = account_dialog.upgrade();
                                account_controller::sync_status(&ui, dialog.as_ref(), &st);
                            }
                        }
                        Event::Collections(token, owner, result)
                            if token == st.collections_generation =>
                        {
                            account_controller::receive_collections(&ui, &mut st, owner, result);
                        }
                        Event::Page(token, page, result) if token == st.generation => {
                            st.busy = false;
                            match result {
                                Ok(response) => {
                                    if st.receive_page(page, response) {
                                        let width = st.width;
                                        relayout(&mut st.cards, width, ui.get_layout_mode());
                                        if ui.get_screen() == 0
                                            && should_prefetch(
                                                st.page,
                                                st.last,
                                                st.busy,
                                                st.offset,
                                                ui.get_viewport_height().max(600.0),
                                                st.height(),
                                            )
                                        {
                                            let next_page = st.page + 1;
                                            request_page(&mut st, &tx, next_page);
                                        }
                                    }
                                    changed = true;
                                }
                                Err(error) => {
                                    st.message = format!("请求失败：{error}，可重试");
                                    update_status(&ui, &st);
                                }
                            }
                        }
                        Event::Thumb(token, id, result) if token == st.generation => {
                            st.pending.remove(&id);
                            match result {
                                Ok(data) => st.images.insert(id, to_image(data)),
                                Err(_) => {
                                    st.failed.insert(id);
                                }
                            }
                            st.message = if st.failed.is_empty() {
                                "预览已加载".into()
                            } else {
                                format!("{} 张预览失败", st.failed.len())
                            };
                            changed = true;
                        }
                        Event::Detail(token, result) if token == st.detail_generation => {
                            match *result {
                                Ok(wall) => {
                                    fill_detail(&ui, &wall);
                                    st.detail = Some(wall);
                                }
                                Err(error) => ui.set_detail_download_msg(
                                    format!("详情请求失败：{error}").into(),
                                ),
                            }
                        }
                        Event::Preview(token, result) if token == st.detail_generation => {
                            ui.set_detail_busy(false);
                            match result {
                                Ok(data) => {
                                    ui.set_detail_download_msg(
                                        format!("原图已加载 · {} × {}", data.w, data.h).into(),
                                    );
                                    ui.set_detail_image(to_image(data));
                                    ui.set_detail_loaded(true);
                                }
                                Err(error) => ui.set_detail_download_msg(
                                    format!("原图预览失败：{error}").into(),
                                ),
                            }
                        }
                        Event::Progress(id, done, total)
                            if st.detail.as_ref().is_some_and(|w| w.id == id) =>
                        {
                            ui.set_detail_download_msg(
                                total
                                    .map(|n| {
                                        format!(
                                            "下载 {:.0}%",
                                            done as f64 / n.max(1) as f64 * 100.0
                                        )
                                    })
                                    .unwrap_or_else(|| format!("下载 {}", fmt_size(done)))
                                    .into(),
                            );
                        }
                        Event::Download(id, result) => {
                            st.download_busy = false;
                            ui.set_downloading(false);
                            if st.detail.as_ref().is_some_and(|w| w.id == id) {
                                ui.set_detail_download_msg(
                                    match result {
                                        Ok(path) => format!("已保存：{}", path.display()),
                                        Err(error) => format!("下载失败：{error}"),
                                    }
                                    .into(),
                                );
                            }
                        }
                        Event::Wallpaper(token, result) => {
                            ui.set_wallpaper_busy(false);
                            if token == st.detail_generation {
                                ui.set_detail_download_msg(match result {
                                    Ok(()) => "已设为桌面壁纸".into(),
                                    Err(error) => format!("设置壁纸失败：{error}").into(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
                if changed {
                    let mut st = state.borrow_mut();
                    render(&ui, &mut st, &loader, &tx);
                    if ui.get_screen() == 0
                        && should_prefetch(
                            st.page,
                            st.last,
                            st.busy,
                            st.offset,
                            viewport_height,
                            st.height(),
                        )
                    {
                        let page = st.page + 1;
                        request_page(&mut st, &tx, page);
                        update_status(&ui, &st);
                    }
                }
            },
        );
    }
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        if let Some(ui) = weak.upgrade() {
            ui.invoke_search();
        }
    });
    ui.show()?;
    window_chrome::install(ui.window());
    let result = ui.run();
    website_controller::shutdown(&state.borrow());
    result
}

#[cfg(test)]
mod ui_regression_tests {
    use super::*;
    #[test]
    fn actual_browse_view_scrolls_and_preserves_position_across_screens() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1200.0, 800.0));
        ui.set_content_height(3000.0);
        ui.show().unwrap();
        slint::platform::update_timers_and_animations();
        assert!(
            ui.get_browse_width() > 1150.0,
            "browse viewport reserved a black region: {}",
            ui.get_browse_width()
        );
        ui.set_collection_mode(true);
        slint::platform::update_timers_and_animations();
        assert!(
            ui.get_browse_width() > 900.0,
            "collection sidebar consumed too much width: {}",
            ui.get_browse_width()
        );
        assert!(
            ui.get_collection_sidebar_left().abs() < 1.0,
            "collection sidebar was centered instead of left-aligned: {}",
            ui.get_collection_sidebar_left()
        );
        ui.set_collection_mode(false);
        slint::platform::update_timers_and_animations();
        let wheel = |delta| {
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                    position: slint::LogicalPosition::new(500.0, 400.0),
                    delta_x: 0.0,
                    delta_y: delta,
                });
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
            slint::platform::update_timers_and_animations();
        };
        for _ in 0..4 {
            wheel(-160.0);
        }
        let down = ui.get_scroll_offset();
        assert!(down > 100.0, "actual UI failed to scroll: {down}");
        ui.set_screen(1);
        slint::platform::update_timers_and_animations();
        ui.set_screen(0);
        slint::platform::update_timers_and_animations();
        assert!(
            (ui.get_scroll_offset() - down).abs() < 1.0,
            "back lost scroll position"
        );
        wheel(160.0);
        assert!(
            ui.get_scroll_offset() < down,
            "actual UI failed to scroll up"
        );
        let size_before = ui.window().size();
        ui.set_advanced_open(true);
        slint::platform::update_timers_and_animations();
        assert_eq!(
            ui.window().size(),
            size_before,
            "advanced filters resized main window"
        );
        assert!(
            ui.get_resolution_label_left() < 260.0,
            "short filter labels expanded across toolbar"
        );
        assert!(
            ui.get_filter_actions_bottom() <= ui.get_advanced_panel_height(),
            "filter actions exceed panel"
        );
        let filter_height = ui.get_advanced_panel_height();
        ui.window().set_size(slint::LogicalSize::new(860.0, 600.0));
        ui.set_resolution_index(4);
        ui.set_sort_index(5);
        ui.set_filter_error("颜色值无效，请输入 6 位十六进制颜色；原有搜索结果保持不变。".into());
        slint::platform::update_timers_and_animations();
        assert_eq!(
            ui.window().size(),
            slint::PhysicalSize::new(860, 600),
            "advanced fields forced narrow window to grow"
        );
        assert!(
            ui.get_advanced_panel_height() > filter_height,
            "extra filter content did not expand panel: before={filter_height}, after={}, sort={}, error={}",ui.get_advanced_panel_height(),ui.get_sort_index(),ui.get_filter_error()
        );
        assert!(
            ui.get_filter_actions_bottom() <= ui.get_advanced_panel_height(),
            "filter error overlaps gallery"
        );
        ui.set_sort_index(0);
        ui.set_resolution_index(0);
        ui.set_filter_error("".into());
        ui.window().set_size(size_before);
        ui.set_collection_mode(true);
        ui.set_collection_selected(false);
        ui.set_busy(false);
        assert!(
            ui.get_footer_message().is_empty(),
            "unselected collection claimed all results were loaded"
        );
        ui.set_collection_mode(false);
        ui.set_busy(true);
        assert!(ui.get_footer_message().contains("正在加载"));
        ui.set_busy(false);
        assert!(
            ui.get_footer_message().is_empty(),
            "empty results claimed end of a list"
        );
        ui.set_advanced_open(false);
        slint::platform::update_timers_and_animations();
        assert_eq!(
            ui.window().size(),
            size_before,
            "collapsing filters resized main window"
        );
        let dialog = SettingsWindow::new().unwrap();
        dialog.show().unwrap();
        dialog
            .window()
            .set_size(slint::LogicalSize::new(620.0, 580.0));
        slint::platform::update_timers_and_animations();
        dialog.hide().unwrap();
        assert_eq!(
            ui.window().size(),
            size_before,
            "settings resized main window"
        );
        assert!(
            ui.get_scroll_offset() > 0.0,
            "settings reset browse position"
        );
        fill_detail(
            &ui,
            &Wallpaper {
                id: "metadata-fixture".into(),
                ..Default::default()
            },
        );
        assert_eq!(
            ui.get_detail_size().as_str(),
            "—",
            "unknown file size was presented as 0 KB"
        );
        assert_eq!(
            ui.get_detail_views().as_str(),
            "—",
            "unknown view count was presented as zero"
        );
        assert_eq!(
            ui.get_detail_uploader().as_str(),
            "—",
            "unknown uploader left an empty row"
        );
        fill_detail(
            &ui,
            &Wallpaper {
                id: "metadata-fixture".into(),
                file_size: Some(0),
                views: Some(0),
                ..Default::default()
            },
        );
        assert_eq!(
            ui.get_detail_views().as_str(),
            "0",
            "actual zero must not be treated as missing"
        );
        ui.hide().unwrap();
        let mut st = test_state();
        st.detail = Some(Wallpaper {
            id: "old-detail".into(),
            ..Default::default()
        });
        st.detail_generation = 9;
        ui.set_screen(1);
        ui.set_detail_loaded(true);
        ui.set_detail_busy(true);
        ui.set_detail_id("old-detail".into());
        let loader = ImageLoader::new(1);
        reset_search(&ui, &mut st, &loader).unwrap();
        assert!(
            st.detail_generation > 9,
            "old detail results still accepted"
        );
        assert!(
            st.detail.is_none(),
            "old detail retained after search/auth reset"
        );
        assert_eq!(ui.get_screen(), 0);
        assert!(!ui.get_detail_loaded());
        assert!(!ui.get_detail_busy());
        assert!(ui.get_detail_id().is_empty());
        // The actual UI-to-query seam must include advanced controls.
        ui.set_sort_index(5);
        ui.set_top_range_index(2);
        ui.set_order_index(1);
        ui.set_resolution_index(4);
        ui.set_min_width_input("3440".into());
        ui.set_min_height_input("1440".into());
        ui.set_ratio_wide(true);
        ui.set_ratio_ultrawide(true);
        ui.set_color_filter("#66CCCC".into());
        let query = query_from_ui(&ui).unwrap();
        assert_eq!(query.atleast.as_deref(), Some("3440x1440"));
        assert_eq!(query.ratios.as_deref(), Some("16x9,21x9"));
        assert_eq!(query.colors.as_deref(), Some("66cccc"));
        assert_eq!(query.top_range.as_deref(), Some("1w"));
        assert_eq!(query.order, "asc");
        st.walls.push(Wallpaper {
            id: "retained".into(),
            ..Default::default()
        });
        let generation = st.generation;
        ui.set_min_width_input("bad".into());
        assert!(reset_search(&ui, &mut st, &loader).is_err());
        assert_eq!(st.generation, generation);
        assert_eq!(st.walls.len(), 1, "invalid filter cleared current results");
        reset_filter_controls(&ui);
        assert!(query_from_ui(&ui).unwrap().atleast.is_none());
        let wall = Wallpaper {
            id: "detail".into(),
            favorites: Some(42),
            file_type: Some("image/png".into()),
            tags: vec![wh_core::models::Tag {
                id: Some(123),
                name: Some("mountains".into()),
                ..Default::default()
            }],
            colors: vec!["#66cccc".into(), "invalid".into()],
            ..Default::default()
        };
        fill_detail(&ui, &wall);
        use slint::Model;
        assert_eq!(
            ui.get_detail_tag_choices().row_data(0).unwrap().query,
            "id:123"
        );
        assert_eq!(ui.get_detail_palette().row_count(), 1);
        assert_eq!(ui.get_detail_favorites(), "42");
        assert_eq!(ui.get_detail_format(), "image/png");
        let session_token = st.account.begin(true).unwrap();
        st.account.invalidate(false);
        assert!(!st.account.complete(session_token, Ok(())));
        account_controller::sync_status(&ui, Some(&dialog), &st);
        assert!(!ui.get_account_verified());
        let response: wh_core::CollectionsResponse = serde_json::from_str(
            r#"{"data":[{"id":"12","label":"Example","count":"24","public":0},{}]}"#,
        )
        .unwrap();
        account_controller::receive_collections(&ui, &mut st, "test-user".into(), Ok(response));
        assert_eq!(st.collections.len(), 1);
        assert_eq!(ui.get_collections().row_count(), 1);
        assert_eq!(ui.get_collections().row_data(0).unwrap().visibility, "私有");
        ui.set_collection_mode(true);
        ui.set_favorite_known(true);
        ui.set_is_favorite(true);
        let old_web_generation = st.web.generation;
        st.web.generation += 1;
        website_controller::receive_login(
            &ui,
            Some(&dialog),
            &mut st,
            old_web_generation,
            Err(website::SiteError::NotAuthenticated),
        );
        assert!(st.web.client.is_none());
        reset_search(&ui, &mut st, &loader).unwrap();
        assert!(!ui.get_collection_mode());
        assert!(st.source.is_none());
        assert!(!ui.get_favorite_known());
        account_controller::invalidate(&ui, &dialog, &mut st);
        assert_eq!(ui.get_collections().row_count(), 0);
        assert!(!ui.get_collections_busy());
        st.web.client = Some(Arc::new(website::SiteClient::test_session()));
        st.web.generation = 20;
        st.web.request = 3;
        st.web.busy = true;
        st.collections_website = true;
        st.walls.push(Wallpaper {
            id: "private-test".into(),
            ..Default::default()
        });
        ui.set_web_authenticated(true);
        ui.set_favorite_known(true);
        let detail_generation = st.detail_generation;
        website_controller::receive_favorite(
            &ui,
            Some(&dialog),
            &mut st,
            &loader,
            (20, 3, detail_generation),
            false,
            Err(website::SiteError::NotAuthenticated),
        );
        assert!(st.web.client.is_none());
        assert!(!ui.get_web_authenticated());
        assert!(!ui.get_favorite_known());
        assert!(
            st.walls.is_empty(),
            "expired website session retained private gallery"
        );
        let pending = Arc::new(website::SiteClient::test_session());
        st.web.generation = 25;
        st.web.busy = true;
        website_controller::receive_browser(&ui, Some(&dialog), &mut st, 25, Ok(pending.clone()));
        website_controller::receive_login(
            &ui,
            Some(&dialog),
            &mut st,
            25,
            Err(website::SiteError::LoginPending),
        );
        assert!(
            Arc::ptr_eq(st.web.pending.as_ref().unwrap(), &pending),
            "pending browser was discarded"
        );
        assert!(st.web.client.is_none());
        assert!(!st.web.busy);
        assert!(
            !ui.get_web_authenticated(),
            "unverified restored profile was accepted as logged in"
        );
        website_controller::receive_logout(
            &ui,
            Some(&dialog),
            &mut st,
            &loader,
            Err("fixture clear failed".into()),
        );
        assert!(
            st.web.pending.is_some(),
            "failed clear discarded the retryable browser"
        );
        assert!(st.web.status.contains("退出尚未确认"));
        website_controller::receive_login(
            &ui,
            Some(&dialog),
            &mut st,
            24,
            Ok((pending.clone(), "stale-user".into())),
        );
        assert!(
            st.web.client.is_none(),
            "cancelled login accepted stale success"
        );
        website_controller::receive_login(
            &ui,
            Some(&dialog),
            &mut st,
            25,
            Ok((pending.clone(), "Test_1".into())),
        );
        assert!(Arc::ptr_eq(st.web.client.as_ref().unwrap(), &pending));
        assert!(st.web.pending.is_none());
        assert!(ui.get_web_authenticated());
        ui.set_collection_mode(false);
        ui.set_screen(0);
        ui.set_browse_position(0.0);
        ui.set_content_height(300.0);
        ui.set_cards(slint::ModelRc::new(VecModel::from(vec![Card {
            id: "hit-test".into(),
            w: 300.0,
            h: 200.0,
            ..Default::default()
        }])));
        let favorites = Rc::new(std::cell::Cell::new(0));
        let details = Rc::new(std::cell::Cell::new(0));
        let f = favorites.clone();
        ui.on_card_favorite(move |_| f.set(f.get() + 1));
        let d = details.clone();
        ui.on_card_clicked(move |_| d.set(d.get() + 1));
        ui.show().unwrap();
        let gallery_top = ui.get_gallery_top();
        assert!(
            (150.0..190.0).contains(&gallery_top),
            "custom chrome displaced gallery: {gallery_top}"
        );
        slint::platform::update_timers_and_animations();
        let click = |x, y| {
            let position = slint::LogicalPosition::new(x, y);
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerPressed {
                    position,
                    button: slint::platform::PointerEventButton::Left,
                });
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                    position,
                    button: slint::platform::PointerEventButton::Left,
                });
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
            slint::platform::update_timers_and_animations();
        };
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(500.0, 400.0),
            });
        slint::platform::update_timers_and_animations();
        click(284.0, gallery_top + 16.0);
        assert_eq!(
            favorites.get(),
            0,
            "favorite remains clickable without hovering the card"
        );
        assert_eq!(details.get(), 1);
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(100.0, gallery_top + 80.0),
            });
        slint::platform::update_timers_and_animations();
        click(284.0, gallery_top + 16.0);
        assert_eq!(
            favorites.get(),
            1,
            "card heart did not trigger direct favorite"
        );
        assert_eq!(details.get(), 1, "card heart incorrectly opened detail");
        click(266.0, gallery_top + 16.0);
        assert_eq!(
            favorites.get(),
            1,
            "compact heart still intercepts the old larger area"
        );
        assert_eq!(
            details.get(),
            2,
            "image next to compact heart stopped opening detail"
        );
        click(100.0, gallery_top + 80.0);
        assert_eq!(details.get(), 3, "card image no longer opens detail");
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(500.0, 400.0),
            });
        slint::platform::update_timers_and_animations();
        click(284.0, gallery_top + 16.0);
        assert_eq!(
            favorites.get(),
            1,
            "favorite remained visible/clickable after pointer left card"
        );
        assert_eq!(details.get(), 4);
        click(1131.0, 20.0);
        assert!(ui.window().is_maximized(), "custom maximize button failed");
        click(1131.0, 20.0);
        assert!(!ui.window().is_maximized(), "custom restore button failed");
        click(1085.0, 20.0);
        assert!(ui.window().is_minimized(), "custom minimize button failed");
        ui.window().set_minimized(false);
        let closed = Rc::new(std::cell::Cell::new(false));
        let close_result = closed.clone();
        ui.window().on_close_requested(move || {
            close_result.set(true);
            slint::CloseRequestResponse::KeepWindowShown
        });
        click(1177.0, 20.0);
        assert!(closed.get(), "custom close bypassed close-requested");
        let navigation = Rc::new(std::cell::Cell::new(0));
        let navigation_result = navigation.clone();
        ui.on_go_collections(move || navigation_result.set(navigation_result.get() + 1));
        click(184.0, 72.0);
        assert_eq!(
            navigation.get(),
            1,
            "subtle collection tab lost its click action"
        );
        ui.set_busy(false);
        ui.set_has_more(false);
        assert_eq!(ui.get_footer_message().as_str(), "已到列表末尾");
        collection_ui_controller::assert_manager_guards();
        ui.hide().unwrap();
    }
    #[test]
    fn both_layout_modes_preserve_count_and_fit_width() {
        for mode in 0..=1 {
            let mut cards = (0..120)
                .map(|i| CardModel {
                    id: i.to_string(),
                    aspect: [0.3, 1.0, 1.78, 3.2][i % 4],
                    ..Default::default()
                })
                .collect::<Vec<_>>();
            relayout(&mut cards, 1100.0, mode);
            assert!(cards
                .iter()
                .all(|c| c.w > 0.0 && c.h > 0.0 && c.x + c.w <= 1100.1));
            if mode == 0 {
                assert!(
                    cards
                        .iter()
                        .all(|card| ((card.w / card.h) - card.aspect).abs() < 0.0001),
                    "adaptive cards must preserve original aspect ratios"
                );
            }
        }
    }
}

#[cfg(test)]
mod prefetch_tests {
    use super::*;
    #[test]
    fn prefetches_second_page_without_scrolling() {
        assert!(should_prefetch(1, 100, false, 0.0, 700.0, 4000.0));
    }
    #[test]
    fn prefetches_before_reaching_bottom() {
        assert!(should_prefetch(2, 100, false, 1300.0, 700.0, 3000.0));
    }
    #[test]
    fn no_duplicate_or_past_last_page_requests() {
        assert!(!should_prefetch(2, 100, true, 2000.0, 700.0, 3000.0));
        assert!(!should_prefetch(100, 100, false, 2000.0, 700.0, 3000.0));
        assert!(!should_prefetch(2, 100, false, 0.0, 700.0, 4000.0));
    }
}

#[cfg(test)]
fn test_state() -> State {
    State {
        settings: Settings::default(),
        client: WallhavenClient::new(None).unwrap(),
        walls: vec![],
        cards: vec![],
        query: SearchQuery::default(),
        page: 0,
        last: 0,
        seed: None,
        busy: false,
        generation: 0,
        loader_generation: 0,
        width: 1100.0,
        offset: 0.0,
        account: account::Session::default(),
        collections: vec![],
        collections_generation: 0,
        collections_owner: String::new(),
        collections_authenticated: false,
        collections_website: false,
        source: None,
        web: website_controller::WebState::default(),
        images: Images::new(),
        pending: HashSet::new(),
        failed: HashSet::new(),
        detail: None,
        detail_generation: 0,
        download_busy: false,
        message: String::new(),
    }
}

#[cfg(test)]
mod page_result_tests {
    use super::*;
    fn response(ids: &[&str]) -> SearchResponse {
        let mut response = SearchResponse::default();
        response.meta.last_page = Some(100);
        response.data = ids
            .iter()
            .map(|id| Wallpaper {
                id: (*id).into(),
                ..Default::default()
            })
            .collect();
        response
    }
    #[test]
    fn duplicate_only_page_stops_prefetch() {
        let mut st = test_state();
        assert!(st.receive_page(1, response(&["a", "b"])));
        assert!(!st.receive_page(2, response(&["a", "b"])));
        assert_eq!(st.walls.len(), 2);
        assert!(!st.has_more());
        assert!(!should_prefetch(
            st.page,
            st.last,
            false,
            0.0,
            700.0,
            st.height()
        ));
    }
    #[test]
    fn partially_overlapping_page_retains_unique_results() {
        let mut st = test_state();
        assert!(st.receive_page(1, response(&["a", "b"])));
        assert!(st.receive_page(2, response(&["b", "c", "c"])));
        assert_eq!(st.walls.len(), 3);
        assert_eq!(st.cards.len(), 3);
        assert_eq!(st.page, 2);
        assert!(st.has_more());
    }
}

#[cfg(test)]
mod file_action_tests {
    use super::*;
    #[test]
    fn filenames_and_links_reject_path_or_url_injection() {
        for id in ["", "../escape", "a/b", "a\\b", "id?key=value", "a&b"] {
            assert!(wallpaper_filename(&Wallpaper {
                id: id.into(),
                ..Default::default()
            })
            .is_err());
            assert!(wallpaper_link(id).is_err());
        }
        assert_eq!(
            wallpaper_filename(&Wallpaper {
                id: "abc123".into(),
                file_type: Some("image/webp".into()),
                ..Default::default()
            })
            .unwrap(),
            "wallhaven-abc123.webp"
        );
        assert_eq!(
            wallpaper_link("abc123").unwrap(),
            "https://wallhaven.cc/w/abc123"
        );
    }
}

#[cfg(test)]
mod titlebar_alignment_tests {
    use super::*;
    #[test]
    fn caption_icon_and_text_share_the_titlebar_vertical_center() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1200.0, 800.0));
        ui.show().unwrap();
        slint::platform::update_timers_and_animations();
        assert!(
            (ui.get_caption_icon_center_y() - 20.0).abs() < 0.5,
            "caption icon not vertically centered: {}",
            ui.get_caption_icon_center_y()
        );
        assert!(
            (ui.get_caption_text_center_y() - 20.0).abs() < 0.5,
            "caption text not vertically centered: {}",
            ui.get_caption_text_center_y()
        );
        ui.hide().unwrap();
    }
}
