//! Human login and favorite requests share one application-owned browser instance.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(super) struct WebState {
    pub client: Option<Arc<website::SiteClient>>,
    pub pending: Option<Arc<website::SiteClient>>,
    pub cancel: Option<Arc<AtomicBool>>,
    pub generation: u64,
    pub request: u64,
    pub busy: bool,
    pub status: String,
    pub username: String,
    pub restoring: bool,
    pub favorites: HashMap<String, bool>,
}

pub(super) fn sync(ui: &AppWindow, dialog: Option<&SettingsWindow>, st: &State) {
    ui.set_web_authenticated(st.web.client.is_some());
    ui.set_web_busy(st.web.busy);
    let status = if st.web.status.is_empty() {
        "网站会话未登录"
    } else {
        &st.web.status
    };
    ui.set_web_status(status.into());
    if let Some(dialog) = dialog {
        dialog.set_web_authenticated(st.web.client.is_some());
        dialog.set_web_busy(st.web.busy);
        dialog.set_web_status(status.into());
        dialog.set_web_session_present(
            st.web.client.is_some() || st.web.pending.is_some() || st.web.busy,
        );
    }
}

pub(super) fn reset_favorite(ui: &AppWindow) {
    ui.set_favorite_known(false);
    ui.set_is_favorite(false);
    ui.set_favorite_targets(Default::default());
    ui.set_favorite_target_index(0);
}

pub(super) fn clear_session(
    ui: &AppWindow,
    dialog: Option<&SettingsWindow>,
    st: &mut State,
    loader: &ImageLoader,
    message: &str,
) {
    if let Some(cancel) = st.web.cancel.take() {
        cancel.store(true, Ordering::Release);
    }
    st.web.generation += 1;
    if let Some(client) = st.web.client.take() {
        client.hide_login();
    }
    if let Some(client) = st.web.pending.take() {
        client.hide_login();
    }
    st.web.username.clear();
    st.web.favorites.clear();
    st.web.busy = false;
    st.web.restoring = false;
    ui.set_favorite_busy(false);
    if st.collections_website {
        reset_gallery(ui, st, loader);
        st.collections_generation += 1;
        st.collections.clear();
        st.collections_owner.clear();
        st.collections_authenticated = false;
        st.collections_website = false;
        st.source = None;
        st.images = Images::new();
        st.message = "网站会话已结束".into();
        ui.set_collections(Default::default());
        ui.set_collections_busy(false);
        ui.set_collection_selected(false);
        ui.set_collection_title("网站会话已结束，请重新登录".into());
        ui.set_collections_status("网站会话已结束，请重新加载收藏夹".into());
        update_status(ui, st);
    }
    st.web.status = message.into();
    reset_favorite(ui);
    sync(ui, dialog, st);
}

fn start_favorite(
    ui: &AppWindow,
    dialog: Option<&SettingsWindow>,
    st: &mut State,
    tx: &mpsc::Sender<Event>,
    desired: Option<bool>,
) {
    if st.web.busy {
        return;
    }
    let Some(client) = st.web.client.clone() else {
        ui.set_detail_download_msg("请先在应用内网页窗口登录网站账号".into());
        return;
    };
    let Some(wall) = st.detail.as_ref() else {
        return;
    };
    let id = wall.id.clone();
    let detail = st.detail_generation;
    let session = st.web.generation;
    st.web.request += 1;
    let request = st.web.request;
    st.web.busy = true;
    ui.set_favorite_busy(true);
    ui.set_detail_download_msg(
        if desired.is_some() {
            "正在通过登录浏览器提交并核对收藏…"
        } else {
            "正在通过登录浏览器读取收藏状态…"
        }
        .into(),
    );
    let target = ui.get_favorite_target_index().max(0) as usize;
    sync(ui, dialog, st);
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = match desired {
            Some(desired) => client.set_favorite(&id, desired, target),
            None => client.favorite_state(&id),
        };
        let _ = tx.send(Event::WebFavorite(
            session,
            request,
            detail,
            desired.is_some(),
            result,
        ));
    });
}

pub(super) fn receive_browser(
    ui: &AppWindow,
    dialog: Option<&SettingsWindow>,
    st: &mut State,
    generation: u64,
    result: Result<Arc<website::SiteClient>, website::SiteError>,
) {
    if generation != st.web.generation {
        return;
    }
    match result {
        Ok(client) => {
            st.web.pending = Some(client);
            st.web.status = if st.web.restoring {
                "正在核对上次保存的网站登录；未确认前不会启用收藏"
            } else {
                "请在网站窗口完成验证和登录；若长时间停留，可手动重新加载登录页"
            }
            .into();
        }
        Err(error) => {
            st.web.busy = false;
            st.web.restoring = false;
            st.web.cancel = None;
            st.web.status = format!("无法打开网页登录窗口：{error}");
        }
    }
    sync(ui, dialog, st);
}

pub(super) fn receive_login(
    ui: &AppWindow,
    dialog: Option<&SettingsWindow>,
    st: &mut State,
    generation: u64,
    result: Result<(Arc<website::SiteClient>, String), website::SiteError>,
) {
    if generation != st.web.generation {
        return;
    }
    st.web.busy = false;
    st.web.restoring = false;
    st.web.cancel = None;
    match result {
        Ok((client, username)) => {
            client.hide_login();
            st.web.pending = None;
            st.web.client = Some(client);
            st.web.status =
                format!("网站账号已登录：{username}；已保留本应用登录资料（有效期由网站决定）");
            st.web.username = username.clone();
            ui.set_collection_owner_input(username.into());
        }
        Err(error) => {
            if st.web.pending.is_none() {
                st.web.pending = st.web.client.take();
            } else {
                st.web.client = None;
            }
            // Preserve the browser/profile for a person to continue verification.
            st.web.status = format!("网页登录未确认：{error}；已保留登录资料，可打开同一会话继续");
        }
    }
    sync(ui, dialog, st);
}

pub(super) fn receive_favorite(
    ui: &AppWindow,
    dialog: Option<&SettingsWindow>,
    st: &mut State,
    loader: &ImageLoader,
    versions: (u64, u64, u64),
    mutation: bool,
    result: Result<website::FavoriteState, website::SiteError>,
) {
    let (session, request, detail) = versions;
    if session != st.web.generation || request != st.web.request {
        return;
    }
    st.web.busy = false;
    ui.set_favorite_busy(false);
    if matches!(result, Err(website::SiteError::NotAuthenticated)) {
        clear_session(
            ui,
            dialog,
            st,
            loader,
            "网站会话已过期，请重新在应用内窗口登录；API Key 未更改",
        );
        return;
    }
    sync(ui, dialog, st);
    if detail != st.detail_generation {
        return;
    }
    match result {
        Ok(state) => {
            ui.set_favorite_known(true);
            if let Some(wall) = st.detail.as_ref() {
                st.web.favorites.insert(wall.id.clone(), state.is_favorite);
            }
            ui.set_is_favorite(state.is_favorite);
            ui.set_favorite_targets(slint::ModelRc::new(VecModel::from(
                state
                    .targets
                    .iter()
                    .map(|t| t.label.clone().into())
                    .collect::<Vec<slint::SharedString>>(),
            )));
            ui.set_favorite_target_index(0);
            ui.set_detail_download_msg(
                if state.is_favorite {
                    if mutation {
                        "已核对：该壁纸已在站内收藏"
                    } else {
                        "该壁纸已在站内收藏"
                    }
                } else if mutation {
                    "已核对：该壁纸已从站内收藏移除"
                } else {
                    "该壁纸尚未收藏到站内账号"
                }
                .into(),
            );
        }
        Err(error) => {
            reset_favorite(ui);
            ui.set_detail_download_msg(
                format!("收藏状态未确认：{error}。未自动重试写入，请刷新核对。 ").into(),
            );
        }
    }
}

fn wait_for_human_login(
    client: Arc<website::SiteClient>,
    cancel: &AtomicBool,
    notice: Option<(u64, &mpsc::Sender<Event>)>,
) -> Result<(Arc<website::SiteClient>, String), website::SiteError> {
    let started = std::time::Instant::now();
    let mut verification_reported = false;
    while started.elapsed() < Duration::from_secs(300) {
        if cancel.load(Ordering::Acquire) || client.login_window_closed() {
            return Err(website::SiteError::LoginWindowClosed);
        }
        match client.verify_login_window() {
            Ok(username) => return Ok((client, username)),
            Err(website::SiteError::HumanVerificationRequired) if !verification_reported => {
                verification_reported = true;
                if let Some((generation, tx)) = notice {
                    let _=tx.send(Event::WebNotice(generation,"网站仍在安全验证，并非已登录；请在网页窗口本人完成。长时间无变化可手动重载；不要反复换会话。".into()));
                }
            }
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Err(website::SiteError::LoginPending)
}

pub(super) fn bind(
    ui: &AppWindow,
    dialog: &SettingsWindow,
    state: Rc<RefCell<State>>,
    _loader: Arc<ImageLoader>,
    tx: mpsc::Sender<Event>,
) {
    sync(ui, Some(dialog), &state.borrow());
    if browser_profile::has_saved_profile() {
        begin_restore(ui, dialog, &state, &tx);
    }
    {
        let weak = ui.as_weak();
        let dialog_weak = dialog.as_weak();
        let state = state.clone();
        let tx = tx.clone();
        dialog.on_web_login(move || {
            let (Some(ui), Some(dialog)) = (weak.upgrade(), dialog_weak.upgrade()) else {
                return;
            };
            let mut st = state.borrow_mut();
            if st.web.busy {
                return;
            }
            let existing = st.web.client.clone().or_else(|| st.web.pending.clone());
            if let Some(cancel) = st.web.cancel.take() {
                cancel.store(true, Ordering::Release);
            }
            st.web.generation += 1;
            let generation = st.web.generation;
            let cancel = Arc::new(AtomicBool::new(false));
            st.web.cancel = Some(cancel.clone());
            st.web.busy = true;
            st.web.restoring = false;
            st.web.status = "正在打开应用内网页登录窗口…".into();
            reset_favorite(&ui);
            sync(&ui, Some(&dialog), &st);
            let tx = tx.clone();
            std::thread::spawn(move || {
                let opened = existing
                    .map(Ok)
                    .unwrap_or_else(|| website::SiteClient::new().map(Arc::new))
                    .and_then(|client| {
                        if cancel.load(Ordering::Acquire) {
                            return Err(website::SiteError::LoginWindowClosed);
                        }
                        client.show_login()?;
                        if cancel.load(Ordering::Acquire) {
                            client.hide_login();
                            return Err(website::SiteError::LoginWindowClosed);
                        }
                        Ok(client)
                    });
                match opened {
                    Ok(client) => {
                        let _ = tx.send(Event::WebBrowserReady(generation, Ok(client.clone())));
                        let result =
                            wait_for_human_login(client.clone(), &cancel, Some((generation, &tx)));
                        if cancel.load(Ordering::Acquire) {
                            client.hide_login();
                        }
                        let _ = tx.send(Event::WebLogin(generation, result));
                    }
                    Err(error) => {
                        let _ = tx.send(Event::WebBrowserReady(generation, Err(error)));
                    }
                }
            });
        });
    }
    {
        let dialog_weak = dialog.as_weak();
        dialog.on_open_web_login(move || {
            if let Some(dialog) = dialog_weak.upgrade() {
                dialog.invoke_web_login();
            }
        });
        {
            let weak = ui.as_weak();
            let dialog_weak = dialog.as_weak();
            let state = state.clone();
            let tx = tx.clone();
            dialog.on_reload_web_login(move || {
                let (Some(ui), Some(dialog)) = (weak.upgrade(), dialog_weak.upgrade()) else {
                    return;
                };
                let mut st = state.borrow_mut();
                if st.web.busy && st.web.cancel.is_none() {
                    return;
                }
                let Some(client) = st.web.pending.clone().or_else(|| st.web.client.clone()) else {
                    st.web.status = "请先打开应用内网页登录窗口".into();
                    sync(&ui, Some(&dialog), &st);
                    return;
                };
                let generation = st.web.generation;
                let resume = if st.web.cancel.is_none() && st.web.client.is_none() {
                    let cancel = Arc::new(AtomicBool::new(false));
                    st.web.cancel = Some(cancel.clone());
                    st.web.busy = true;
                    Some(cancel)
                } else {
                    None
                };
                st.web.status = "正在同一会话重新加载登录页；不会清除登录资料".into();
                sync(&ui, Some(&dialog), &st);
                let tx = tx.clone();
                std::thread::spawn(move || match client.reload_login() {
                    Ok(()) => {
                        let _ = tx.send(Event::WebNotice(
                            generation,
                            "已请求重载；请在网页窗口完成验证，未自动通过".into(),
                        ));
                        if let Some(cancel) = resume {
                            let result =
                                wait_for_human_login(client, &cancel, Some((generation, &tx)));
                            let _ = tx.send(Event::WebLogin(generation, result));
                        }
                    }
                    Err(error) => {
                        if resume.is_some() {
                            let _ = tx.send(Event::WebLogin(generation, Err(error)));
                        } else {
                            let _ = tx
                                .send(Event::WebNotice(generation, format!("重载未完成：{error}")));
                        }
                    }
                });
            });
        }
    }
    {
        let weak = ui.as_weak();
        let dialog_weak = dialog.as_weak();
        let state = state.clone();
        let tx = tx.clone();
        dialog.on_web_logout(move || {
            let (Some(ui), Some(dialog)) = (weak.upgrade(), dialog_weak.upgrade()) else {
                return;
            };
            let mut st = state.borrow_mut();
            // A submitted favorite write cannot be cancelled; human login polling can.
            if st.web.busy && st.web.pending.is_none() && st.web.client.is_some() {
                return;
            }
            if let Some(cancel) = st.web.cancel.take() {
                cancel.store(true, Ordering::Release);
            }
            st.web.generation += 1;
            let generation = st.web.generation;
            let client = st.web.client.clone().or_else(|| st.web.pending.clone());
            st.web.busy = true;
            st.web.status = "正在清除本应用保存的网站登录数据…".into();
            sync(&ui, Some(&dialog), &st);
            let tx = tx.clone();
            std::thread::spawn(move || {
                let result = client
                    .ok_or_else(|| "没有已打开的网站会话".to_string())
                    .and_then(|client| client.clear_data());
                let _ = tx.send(Event::WebLogout(generation, result));
            });
        });
    }
    {
        let weak = ui.as_weak();
        let dialog_weak = dialog.as_weak();
        let state = state.clone();
        let tx = tx.clone();
        ui.on_web_refresh_favorite(move || {
            let Some(ui) = weak.upgrade() else { return };
            let dialog = dialog_weak.upgrade();
            start_favorite(&ui, dialog.as_ref(), &mut state.borrow_mut(), &tx, None);
        });
    }
    {
        let weak = ui.as_weak();
        let dialog_weak = dialog.as_weak();
        let state = state.clone();
        let tx = tx.clone();
        ui.on_web_toggle_favorite(move || {
            let Some(ui) = weak.upgrade() else { return };
            if !ui.get_favorite_known() {
                return;
            }
            let desired = !ui.get_is_favorite();
            let dialog = dialog_weak.upgrade();
            start_favorite(
                &ui,
                dialog.as_ref(),
                &mut state.borrow_mut(),
                &tx,
                Some(desired),
            );
        });
    }
}

pub(super) fn receive_logout(
    ui: &AppWindow,
    dialog: Option<&SettingsWindow>,
    st: &mut State,
    loader: &ImageLoader,
    result: Result<(), String>,
) {
    st.web.busy = false;
    match result {
        Ok(()) => clear_session(
            ui,
            dialog,
            st,
            loader,
            "已清除本应用保存的网站登录与验证数据；API Key 未更改",
        ),
        Err(error) => {
            st.web.status = format!("退出尚未确认：{error}；请重试清除，未声称已退出");
            sync(ui, dialog, st);
        }
    }
}
pub(super) fn shutdown(st: &State) {
    if let Some(cancel) = &st.web.cancel {
        cancel.store(true, Ordering::Release);
    }
    if let Some(client) = st.web.client.as_ref().or(st.web.pending.as_ref()) {
        client.shutdown();
    }
}
fn begin_restore(
    ui: &AppWindow,
    dialog: &SettingsWindow,
    state: &Rc<RefCell<State>>,
    tx: &mpsc::Sender<Event>,
) {
    let mut st = state.borrow_mut();
    st.web.generation += 1;
    let generation = st.web.generation;
    let cancel = Arc::new(AtomicBool::new(false));
    st.web.cancel = Some(cancel.clone());
    st.web.busy = true;
    st.web.restoring = true;
    st.web.status = "正在恢复上次保存的网站登录…".into();
    sync(ui, Some(dialog), &st);
    let tx = tx.clone();
    std::thread::spawn(move || {
        let opened = website::SiteClient::new().map(Arc::new).and_then(|client| {
            client.prepare_restore()?;
            Ok(client)
        });
        match opened {
            Err(error) => {
                let _ = tx.send(Event::WebBrowserReady(generation, Err(error)));
            }
            Ok(client) => {
                let _ = tx.send(Event::WebBrowserReady(generation, Ok(client.clone())));
                let start = std::time::Instant::now();
                let mut result = Err(website::SiteError::LoginPending);
                while start.elapsed() < Duration::from_secs(15) && !cancel.load(Ordering::Acquire) {
                    match client.verify_login_window() {
                        Ok(name) => {
                            result = Ok((client.clone(), name));
                            break;
                        }
                        Err(
                            error @ (website::SiteError::NotAuthenticated
                            | website::SiteError::HumanVerificationRequired),
                        ) => {
                            result = Err(error);
                            break;
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(500)),
                    }
                }
                let _ = tx.send(Event::WebLogin(generation, result));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn login_wait_accepts_same_browser_and_honors_cancellation() {
        let client = Arc::new(website::SiteClient::test_session());
        client.show_login().unwrap();
        let (accepted, username) =
            wait_for_human_login(client.clone(), &AtomicBool::new(false), None).unwrap();
        assert!(Arc::ptr_eq(&client, &accepted));
        assert_eq!(username, "Test_1");
        assert!(matches!(
            wait_for_human_login(client, &AtomicBool::new(true), None),
            Err(website::SiteError::LoginWindowClosed)
        ));
    }
}
