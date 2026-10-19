//! UI-thread account/collection actions; workers return generation-tagged results.
use super::*;
use wh_core::models::CollectionsResponse;

pub(super) fn sync_status(ui: &AppWindow, dialog: Option<&SettingsWindow>, st: &State) {
    ui.set_account_status(st.account.status.clone().into());
    ui.set_account_busy(st.account.busy);
    ui.set_account_verified(st.account.verified);
    if let Some(dialog) = dialog {
        dialog.set_account_status(st.account.status.clone().into());
        dialog.set_account_busy(st.account.busy);
    }
}

pub(super) fn invalidate(ui: &AppWindow, dialog: &SettingsWindow, st: &mut State) {
    st.account.invalidate(st.settings.has_api_key());
    st.collections_generation += 1;
    st.collections.clear();
    st.collections_owner.clear();
    st.collections_authenticated = false;
    st.collections_website = false;
    st.source = None;
    ui.set_collections(Default::default());
    ui.set_collections_busy(false);
    ui.set_collection_selected(false);
    ui.set_selected_collection_index(-1);
    ui.set_collection_title("请选择一个收藏夹".into());
    ui.set_collections_status("账号设置已变更，请重新加载收藏夹".into());
    ui.set_collection_owner_input(
        st.settings
            .account_username
            .clone()
            .unwrap_or_default()
            .into(),
    );
    sync_status(ui, Some(dialog), st);
}

pub(super) fn receive_collections(
    ui: &AppWindow,
    st: &mut State,
    owner: String,
    result: Result<CollectionsResponse, String>,
) {
    ui.set_collections_busy(false);
    match result {
        Ok(response) => {
            st.collections = response
                .data
                .into_iter()
                .filter(|c| c.id.is_some_and(|id| id > 0))
                .collect();
            st.collections_owner = owner;
            let selected = st
                .source
                .as_ref()
                .filter(|source| source.username.eq_ignore_ascii_case(&st.collections_owner))
                .and_then(|source| {
                    st.collections
                        .iter()
                        .position(|collection| collection.id == Some(source.id))
                })
                .map(|index| index as i32)
                .unwrap_or(-1);
            ui.set_selected_collection_index(selected);
            let rows = st
                .collections
                .iter()
                .map(|c| CollectionChoice {
                    label: c
                        .label
                        .clone()
                        .unwrap_or_else(|| "未命名收藏夹".into())
                        .into(),
                    count_label: c
                        .count
                        .map(|n| format!("{n} 张"))
                        .unwrap_or_else(|| "数量未知".into())
                        .into(),
                    visibility: match c.public {
                        Some(true) => "公开",
                        Some(false) => "私有",
                        None => "可见性未知",
                    }
                    .into(),
                })
                .collect::<Vec<_>>();
            ui.set_collections(slint::ModelRc::new(VecModel::from(rows)));
            ui.set_collections_status(if st.collections.is_empty() {
                "没有可访问的收藏夹".into()
            } else {
                format!(
                    "已加载 {} 个{}收藏夹，点击查看内容",
                    st.collections.len(),
                    if st.collections_website {
                        "自己的"
                    } else if st.collections_authenticated {
                        "账号"
                    } else {
                        "公开"
                    }
                )
                .into()
            });
        }
        Err(error) => {
            st.collections.clear();
            st.collections_owner.clear();
            ui.set_selected_collection_index(-1);
            ui.set_collections(Default::default());
            ui.set_collections_status(format!("收藏夹加载失败：{error}").into());
        }
    }
}

fn purity(ui: &AppWindow) -> String {
    let value = format!(
        "{}{}{}",
        u8::from(ui.get_sfw()),
        u8::from(ui.get_sketchy()),
        u8::from(ui.get_nsfw() && ui.get_nsfw_enabled() && ui.get_has_api_key())
    );
    if value == "000" {
        "100".into()
    } else {
        value
    }
}

pub(super) fn bind(
    ui: &AppWindow,
    dialog: &SettingsWindow,
    state: Rc<RefCell<State>>,
    loader: Arc<ImageLoader>,
    tx: mpsc::Sender<Event>,
) {
    {
        let mut st = state.borrow_mut();
        let has_key = st.settings.has_api_key();
        st.account.invalidate(has_key);
        ui.set_collection_owner_input(
            st.settings
                .account_username
                .clone()
                .unwrap_or_default()
                .into(),
        );
        sync_status(ui, Some(dialog), &st);
    }
    {
        let weak = ui.as_weak();
        let dialog_weak = dialog.as_weak();
        let state = state.clone();
        let tx = tx.clone();
        ui.on_verify_account(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            let has_key = st.settings.has_api_key();
            let token = st.account.begin(has_key);
            let dialog = dialog_weak.upgrade();
            sync_status(&ui, dialog.as_ref(), &st);
            let Some(token) = token else { return };
            let client = st.client.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let result = client
                    .user_settings()
                    .map_err(|e| e.to_string())
                    .and_then(|r| {
                        r.data
                            .map(|_| ())
                            .ok_or_else(|| "账号设置响应为空".to_string())
                    });
                let _ = tx.send(Event::Account(token, result));
            });
        });
        let weak = ui.as_weak();
        dialog.on_verify_account(move || {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_verify_account();
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_go_browse(move || {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_search();
            }
        });
        let weak = ui.as_weak();
        let state = state.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        ui.on_go_collections(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            if !ui.get_collection_mode() {
                reset_gallery(&ui, &mut st, &loader);
                st.source = None;
                st.message = "请选择收藏夹".into();
            }
            ui.set_collection_mode(true);
            ui.set_screen(0);
            render(&ui, &mut st, &loader, &tx);
            let load_own = (st.settings.has_api_key() || st.web.client.is_some())
                && (!ui.get_collection_owner_input().trim().is_empty()
                    || !st.web.username.is_empty())
                && st.collections.is_empty();
            drop(st);
            if load_own {
                ui.invoke_load_collections(true);
            } else if ui.get_collection_owner_input().is_empty() {
                ui.set_collections_status(
                    "填写用户名后加载公开收藏；自己的私有收藏需网站登录或 API Key".into(),
                );
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        ui.on_load_collections(move |own| {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            let site = if own { st.web.client.clone() } else { None };
            let owner = if site.is_some() {
                st.web.username.clone()
            } else {
                ui.get_collection_owner_input().trim().to_string()
            };
            if let Err(error) = account::validate_username(&owner) {
                ui.set_collections_status(error.into());
                return;
            }
            if own && !st.settings.has_api_key() && site.is_none() {
                ui.set_collections_status("请先登录网站账号，或在账号设置保存 API Key".into());
                return;
            }
            ui.set_collection_owner_input(owner.clone().into());
            st.collections_generation += 1;
            let token = st.collections_generation;
            st.collections.clear();
            st.collections_owner = owner.clone();
            st.collections_authenticated = own;
            st.collections_website = site.is_some();
            reset_gallery(&ui, &mut st, &loader);
            st.source = None;
            st.message = "请选择收藏夹".into();
            ui.set_collection_selected(false);
            ui.set_collection_mode(true);
            ui.set_collection_title("请选择一个收藏夹".into());
            ui.set_collections(Default::default());
            ui.set_collections_busy(true);
            ui.set_collections_status("正在加载收藏夹…".into());
            render(&ui, &mut st, &loader, &tx);
            let mut client = st.client.clone();
            if !own {
                client.set_api_key(None);
            }
            let tx = tx.clone();
            std::thread::spawn(move || {
                let result = if let Some(site) = site {
                    site.read_collections(&owner)
                } else {
                    client
                        .collections(if own { None } else { Some(owner.as_str()) })
                        .map_err(|e| e.to_string())
                };
                let _ = tx.send(Event::Collections(token, owner, result));
            });
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        ui.on_open_collection(move |index| {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            let Some(collection) = st.collections.get(index as usize) else {
                return;
            };
            let Some(id) = collection.id else { return };
            let label = collection
                .label
                .clone()
                .unwrap_or_else(|| "未命名收藏夹".into());
            let mut source = match account::CollectionSource::new(&st.collections_owner, id, label)
            {
                Ok(source) => source,
                Err(error) => {
                    ui.set_collections_status(error.into());
                    return;
                }
            };
            source.authenticated = st.collections_authenticated;
            source.website = st.collections_website;
            reset_gallery(&ui, &mut st, &loader);
            ui.set_selected_collection_index(index);
            ui.set_collection_title(format!("{} / {}", source.username, source.label).into());
            st.source = Some(source);
            st.query = SearchQuery {
                purity: purity(&ui),
                ..Default::default()
            };
            ui.set_collection_mode(true);
            ui.set_collection_selected(true);
            request_page(&mut st, &tx, 1);
            update_status(&ui, &st);
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let loader = loader.clone();
        let tx = tx.clone();
        ui.on_refresh_collection(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            if st.source.is_none() {
                return;
            }
            reset_gallery(&ui, &mut st, &loader);
            st.query.purity = purity(&ui);
            request_page(&mut st, &tx, 1);
            update_status(&ui, &st);
        });
    }
    {
        let weak = ui.as_weak();
        dialog.on_open_account_website(move || {
            if let Some(ui) = weak.upgrade() {
                if let Err(error) = open_web("https://wallhaven.cc/settings/account") {
                    ui.set_account_status(error.into());
                }
            }
        });
        let weak = ui.as_weak();
        ui.on_favorite_on_website(move || {
            let Some(ui) = weak.upgrade() else { return };
            let result = wallpaper_link(ui.get_detail_id().as_str()).and_then(|url| open_web(&url));
            ui.set_detail_download_msg(match result {
                Ok(()) => "已交给系统浏览器；在网站登录后收藏或取消收藏，客户端未修改收藏".into(),
                Err(error) => format!("打开网站失败：{error}").into(),
            });
        });
    }
}

fn open_web(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(0x08000000)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Err("此平台暂不支持打开浏览器".into())
    }
}
