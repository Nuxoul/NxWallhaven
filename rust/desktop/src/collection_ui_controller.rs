//! Native quick-favorite and collection management windows, independent of details.
use super::*;
use crate::collection_management::ManageOperation;
use wh_core::{Collection, CollectionsResponse};

pub(super) struct CollectionTools {
    _quick: QuickFavoriteWindow,
    _manager: CollectionManagerWindow,
    _timer: slint::Timer,
}
impl CollectionTools {
    pub(super) fn bind_main_close(
        &self,
        ui: &AppWindow,
        settings: &SettingsWindow,
        hub: Rc<window_hub::WindowHub>,
        local: crate::local_controller::LocalTools,
        downloads: Arc<crate::download_manager::DownloadManager>,
    ) {
        let quick = self._quick.as_weak();
        let manager = self._manager.as_weak();
        let settings = settings.as_weak();
        ui.window().on_close_requested(move || {
            hub.close_all();
            downloads.shutdown();
            local.hide_all();
            if let Some(window) = quick.upgrade() {
                let _ = window.hide();
            }
            if let Some(window) = manager.upgrade() {
                let _ = window.hide();
            }
            if let Some(window) = settings.upgrade() {
                window.set_api_key_input("".into());
                let _ = window.hide();
            }
            slint::CloseRequestResponse::HideWindow
        });
    }
    pub(super) fn quick_window(&self) -> slint::Weak<QuickFavoriteWindow> {
        self._quick.as_weak()
    }
}
#[derive(Default)]
struct ToolsState {
    request: u64,
    busy: bool,
    quick_id: String,
    manager_owner: String,
    manager_rows: Vec<Collection>,
    quick_session: u64,
    manager_session: u64,
}
enum ResultEvent {
    Favorite {
        session: u64,
        request: u64,
        id: String,
        result: Result<website::FavoriteState, website::SiteError>,
    },
    Manager {
        session: u64,
        request: u64,
        owner: String,
        message: String,
        result: Result<CollectionsResponse, String>,
    },
}
fn strings(values: Vec<String>) -> slint::ModelRc<slint::SharedString> {
    slint::ModelRc::new(VecModel::from(
        values.into_iter().map(Into::into).collect::<Vec<_>>(),
    ))
}

fn request_favorite(
    ui: &AppWindow,
    quick: &QuickFavoriteWindow,
    st: &mut State,
    tools: &mut ToolsState,
    tx: &mpsc::Sender<ResultEvent>,
    desired: Option<bool>,
) {
    if tools.busy || st.web.busy {
        quick.set_message("其他站内操作正在进行，请稍后刷新".into());
        return;
    }
    if desired.is_some() && tools.quick_session != st.web.generation {
        quick.set_ready(false);
        quick.set_message("账号已变化，请先刷新当前图片收藏状态".into());
        return;
    }
    let Some(client) = st.web.client.clone() else {
        quick.set_ready(false);
        quick.set_message("请先在账号设置完成网站登录，再刷新状态；无需打开图片详情".into());
        return;
    };
    if wallpaper_link(&tools.quick_id).is_err() {
        quick.set_message("壁纸 ID 无效".into());
        return;
    }
    tools.request += 1;
    tools.busy = true;
    st.web.busy = true;
    let request = tools.request;
    let session = st.web.generation;
    let id = tools.quick_id.clone();
    quick.set_busy(true);
    quick.set_message(
        if desired.is_some() {
            "正在提交并核对站内收藏…"
        } else {
            "正在读取站内收藏状态…"
        }
        .into(),
    );
    website_controller::sync(ui, None, st);
    let target = quick.get_target_index().max(0) as usize;
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = match desired {
            Some(value) => client.set_favorite(&id, value, target),
            None => client.favorite_state(&id),
        };
        let _ = tx.send(ResultEvent::Favorite {
            session,
            request,
            id,
            result,
        });
    });
}
fn select_manager(manager: &CollectionManagerWindow, tools: &ToolsState, index: i32) {
    manager.set_delete_confirmed(false);
    let row = tools.manager_rows.get(index.max(0) as usize);
    manager.set_editable(row.is_some_and(|row| row.id.is_some_and(|id| id > 0)));
    manager.set_label_input(
        row.and_then(|row| row.label.clone())
            .unwrap_or_default()
            .into(),
    );
    manager.set_public_input(row.and_then(|row| row.public).unwrap_or(false));
    manager.set_visibility_unknown(row.is_some_and(|row| row.public.is_none()));
    manager.set_visibility_confirmed(row.and_then(|row| row.public).is_some());
}
fn request_manager(
    ui: &AppWindow,
    manager: &CollectionManagerWindow,
    st: &mut State,
    tools: &mut ToolsState,
    tx: &mpsc::Sender<ResultEvent>,
    operation: Option<ManageOperation>,
) {
    if tools.busy || st.web.busy {
        manager.set_message("其他站内操作正在进行，请稍后重试".into());
        return;
    }
    let Some(client) = st.web.client.clone() else {
        manager.set_editable(false);
        manager
            .set_message("请先在账号设置登录网站账号；API Key 只用于读取，不能管理收藏夹".into());
        return;
    };
    let owner = st.web.username.clone();
    if operation.is_some()
        && (!tools.manager_owner.eq_ignore_ascii_case(&owner)
            || tools.manager_session != st.web.generation)
    {
        manager.set_editable(false);
        manager.set_message("账号已变化，请先刷新自己的收藏夹再提交".into());
        return;
    }
    if let Err(error) = account::validate_username(&owner) {
        manager.set_message(error.into());
        return;
    }
    tools.request += 1;
    tools.busy = true;
    st.web.busy = true;
    let request = tools.request;
    let session = st.web.generation;
    let tx = tx.clone();
    manager.set_busy(true);
    manager.set_delete_confirmed(false);
    manager.set_message(
        if operation.is_some() {
            "正在提交并重新读取核对收藏夹…"
        } else {
            "正在加载自己的全部收藏夹…"
        }
        .into(),
    );
    website_controller::sync(ui, None, st);
    std::thread::spawn(move || {
        let mut message = "收藏夹已刷新".to_string();
        let result = (|| {
            if let Some(operation) = operation {
                client.manage_collection(&owner, operation)?;
                message = "操作已完成，并重新读取收藏夹核对".into();
            }
            client.read_collections(&owner)
        })();
        let _ = tx.send(ResultEvent::Manager {
            session,
            request,
            owner,
            message,
            result,
        });
    });
}
fn manager_operation(
    manager: &CollectionManagerWindow,
    tools: &ToolsState,
    kind: u8,
) -> Result<ManageOperation, String> {
    if kind == 2 {
        if !manager.get_delete_confirmed() {
            return Err("删除前请勾选确认，未发送删除请求".into());
        }
        let id = tools
            .manager_rows
            .get(manager.get_selected_index().max(0) as usize)
            .and_then(|row| row.id)
            .ok_or("请选择要删除的收藏夹")?;
        return Ok(ManageOperation::Delete { id });
    }
    let label = manager.get_label_input().trim().to_string();
    if label.is_empty() || label.chars().count() > 100 || label.chars().any(char::is_control) {
        return Err("名称需为 1–100 个可显示字符".into());
    }
    let public = manager.get_public_input();
    if kind == 0 {
        if tools
            .manager_rows
            .iter()
            .any(|row| row.label.as_deref() == Some(label.as_str()))
        {
            return Err("已存在同名收藏夹，请选择修改或使用其他名称".into());
        }
        Ok(ManageOperation::Create { label, public })
    } else {
        if !manager.get_visibility_confirmed() {
            return Err("当前可见性未知，请先确认公开/私有选择".into());
        }
        let id = tools
            .manager_rows
            .get(manager.get_selected_index().max(0) as usize)
            .and_then(|row| row.id)
            .ok_or("请选择要修改的收藏夹")?;
        Ok(ManageOperation::Update { id, label, public })
    }
}

pub(super) fn bind(
    ui: &AppWindow,
    state: Rc<RefCell<State>>,
    loader: Arc<ImageLoader>,
    tx_events: mpsc::Sender<Event>,
) -> Result<CollectionTools, slint::PlatformError> {
    let quick = QuickFavoriteWindow::new()?;
    let manager = CollectionManagerWindow::new()?;
    quick
        .window()
        .set_size(slint::LogicalSize::new(520.0, 380.0));
    manager
        .window()
        .set_size(slint::LogicalSize::new(660.0, 640.0));
    let tools = Rc::new(RefCell::new(ToolsState::default()));
    let (tx, rx) = mpsc::channel();
    {
        let weak = ui.as_weak();
        let q = quick.as_weak();
        let state = state.clone();
        let tools = tools.clone();
        let tx = tx.clone();
        ui.on_card_favorite(move |id| {
            let (Some(ui), Some(quick)) = (weak.upgrade(), q.upgrade()) else {
                return;
            };
            let mut st = state.borrow_mut();
            let mut tools = tools.borrow_mut();
            if tools.busy || st.web.busy {
                ui.set_status("站内操作正在进行，请稍后再试".into());
                return;
            }
            tools.quick_id = id.to_string();
            quick.set_wallpaper_id(id);
            quick.set_ready(false);
            quick.set_targets(Default::default());
            quick.set_target_index(0);
            quick.set_busy(false);
            let _ = window_chrome::show_centered(&quick, ui.window());
            request_favorite(&ui, &quick, &mut st, &mut tools, &tx, None);
        });
    }
    for apply in [false, true] {
        let weak = ui.as_weak();
        let q = quick.as_weak();
        let state = state.clone();
        let tools = tools.clone();
        let tx = tx.clone();
        let action = move || {
            let (Some(ui), Some(quick)) = (weak.upgrade(), q.upgrade()) else {
                return;
            };
            if apply && !quick.get_ready() {
                return;
            }
            let desired = apply.then(|| !quick.get_favorited());
            request_favorite(
                &ui,
                &quick,
                &mut state.borrow_mut(),
                &mut tools.borrow_mut(),
                &tx,
                desired,
            );
        };
        if apply {
            quick.on_apply_favorite(action);
        } else {
            quick.on_retry(action);
        }
    }
    {
        let q = quick.as_weak();
        quick.on_close_dialog(move || {
            if let Some(q) = q.upgrade() {
                let _ = q.hide();
            }
        });
        let weak = ui.as_weak();
        let q = quick.as_weak();
        quick.on_login(move || {
            if let Some(q) = q.upgrade() {
                let _ = q.hide();
            }
            if let Some(ui) = weak.upgrade() {
                ui.invoke_go_settings();
            }
        });
        let q = quick.as_weak();
        quick.window().on_close_requested(move || {
            if let Some(q) = q.upgrade() {
                let _ = q.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
    }
    {
        let weak = ui.as_weak();
        let m = manager.as_weak();
        let state = state.clone();
        let tools = tools.clone();
        let tx = tx.clone();
        ui.on_manage_collection(move || {
            let (Some(ui), Some(manager)) = (weak.upgrade(), m.upgrade()) else {
                return;
            };
            manager.set_delete_confirmed(false);
            let _ = window_chrome::show_centered(&manager, ui.window());
            request_manager(
                &ui,
                &manager,
                &mut state.borrow_mut(),
                &mut tools.borrow_mut(),
                &tx,
                None,
            );
        });
    }
    for kind in 0..4 {
        let weak = ui.as_weak();
        let m = manager.as_weak();
        let state = state.clone();
        let tools = tools.clone();
        let tx = tx.clone();
        let action = move || {
            let (Some(ui), Some(manager)) = (weak.upgrade(), m.upgrade()) else {
                return;
            };
            let operation = if kind == 3 {
                None
            } else {
                match manager_operation(&manager, &tools.borrow(), kind) {
                    Ok(operation) => Some(operation),
                    Err(error) => {
                        manager.set_message(error.into());
                        return;
                    }
                }
            };
            request_manager(
                &ui,
                &manager,
                &mut state.borrow_mut(),
                &mut tools.borrow_mut(),
                &tx,
                operation,
            );
        };
        match kind {
            0 => manager.on_create_collection(action),
            1 => manager.on_save_collection(action),
            2 => manager.on_delete_collection(action),
            _ => manager.on_refresh_list(action),
        }
    }
    {
        let m = manager.as_weak();
        let state = state.clone();
        manager.on_open_web_manager(move || {
            let Some(manager) = m.upgrade() else { return };
            let st = state.borrow();
            let Some(client) = st.web.client.clone().or_else(|| st.web.pending.clone()) else {
                manager.set_message("请先在账号设置打开应用内网页登录并完成登录".into());
                return;
            };
            let owner = st.web.username.clone();
            if account::validate_username(&owner).is_err() {
                manager.set_message("尚未确认网页登录账号，请先完成登录".into());
                return;
            }
            std::thread::spawn(move || {
                let _ = client.show_collection_management(&owner);
            });
            manager.set_message("正在同一应用内会话打开收藏管理页；修改后点击本窗口刷新".into());
        });
    }
    {
        let m = manager.as_weak();
        let tools = tools.clone();
        manager.on_choose(move |index| {
            if let Some(manager) = m.upgrade() {
                select_manager(&manager, &tools.borrow(), index);
            }
        });
        let m = manager.as_weak();
        manager.on_close_dialog(move || {
            if let Some(manager) = m.upgrade() {
                let _ = manager.hide();
            }
        });
        let m = manager.as_weak();
        manager.window().on_close_requested(move || {
            if let Some(manager) = m.upgrade() {
                let _ = manager.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
    }
    let timer = slint::Timer::default();
    {
        let weak = ui.as_weak();
        let q = quick.as_weak();
        let m = manager.as_weak();
        let state = state.clone();
        let tools = tools.clone();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(40),
            move || {
                let (Some(ui), Some(quick), Some(manager)) =
                    (weak.upgrade(), q.upgrade(), m.upgrade())
                else {
                    return;
                };
                {
                    let st = state.borrow();
                    let mut tools = tools.borrow_mut();
                    if tools.quick_session != st.web.generation {
                        quick.set_ready(false);
                        quick.set_targets(Default::default());
                    }
                    if tools.manager_session != st.web.generation {
                        tools.manager_rows.clear();
                        manager.set_collection_names(Default::default());
                        manager.set_editable(false);
                        manager.set_delete_confirmed(false);
                    }
                }
                for event in rx.try_iter() {
                    let mut st = state.borrow_mut();
                    let mut tools = tools.borrow_mut();
                    let (session, request) = match &event {
                        ResultEvent::Favorite {
                            session, request, ..
                        }
                        | ResultEvent::Manager {
                            session, request, ..
                        } => (*session, *request),
                    };
                    if request != tools.request {
                        continue;
                    }
                    tools.busy = false;
                    quick.set_busy(false);
                    manager.set_busy(false);
                    if session != st.web.generation {
                        quick.set_ready(false);
                        quick.set_message("账号会话已变化，请刷新".into());
                        manager.set_editable(false);
                        manager.set_message("账号会话已变化，请刷新".into());
                        continue;
                    }
                    st.web.busy = false;
                    website_controller::sync(&ui, None, &st);
                    match event {
                        ResultEvent::Favorite { id, result, .. } => match result {
                            Ok(result) => {
                                st.web.favorites.insert(id.clone(), result.is_favorite);
                                tools.quick_session = session;
                                quick.set_favorited(result.is_favorite);
                                quick.set_targets(strings(
                                    result.targets.into_iter().map(|t| t.label).collect(),
                                ));
                                quick.set_target_index(0);
                                quick.set_ready(true);
                                quick.set_message(
                                    if result.is_favorite {
                                        "已核对：当前壁纸已收藏，可在此取消"
                                    } else {
                                        "尚未收藏，选择收藏夹后确认"
                                    }
                                    .into(),
                                );
                                if st.detail.as_ref().is_some_and(|w| w.id == id) {
                                    ui.set_favorite_known(true);
                                    ui.set_is_favorite(result.is_favorite);
                                }
                                render(&ui, &mut st, &loader, &tx_events);
                            }
                            Err(error) => {
                                if error == website::SiteError::NotAuthenticated {
                                    website_controller::clear_session(
                                        &ui,
                                        None,
                                        &mut st,
                                        &loader,
                                        "网站会话已过期，请重新登录",
                                    );
                                    render(&ui, &mut st, &loader, &tx_events);
                                }
                                quick.set_ready(false);
                                quick.set_message(
                                    format!("收藏操作未确认：{error}；未自动重试写入").into(),
                                );
                            }
                        },
                        ResultEvent::Manager {
                            owner,
                            message,
                            result,
                            ..
                        } => match result {
                            Ok(response) => {
                                tools.manager_owner = owner.clone();
                                tools.manager_session = session;
                                tools.manager_rows = response.data.clone();
                                manager.set_collection_names(strings(
                                    response
                                        .data
                                        .iter()
                                        .map(|c| {
                                            format!(
                                                "{}  ({})",
                                                c.label.as_deref().unwrap_or("未命名"),
                                                c.count
                                                    .map(|n| n.to_string())
                                                    .unwrap_or_else(|| "?".into())
                                            )
                                        })
                                        .collect(),
                                ));
                                manager.set_selected_index(0);
                                select_manager(&manager, &tools, 0);
                                manager.set_message(
                                    format!("{message}；共 {} 个", response.data.len()).into(),
                                );
                                st.collections_authenticated = true;
                                st.collections_website = true;
                                account_controller::receive_collections(
                                    &ui,
                                    &mut st,
                                    owner,
                                    Ok(response),
                                );
                            }
                            Err(error) => {
                                manager.set_message(
                                    format!("管理未完成：{error}；没有自动重试写入").into(),
                                );
                            }
                        },
                    }
                }
            },
        );
    }
    Ok(CollectionTools {
        _quick: quick,
        _manager: manager,
        _timer: timer,
    })
}

#[cfg(test)]
pub(super) fn assert_manager_guards() {
    let manager = CollectionManagerWindow::new().unwrap();
    let tools = ToolsState {
        manager_rows: vec![Collection {
            id: Some(12),
            label: Some("Existing".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(
        manager_operation(&manager, &tools, 2).is_err(),
        "delete submitted without confirmation"
    );
    manager.set_delete_confirmed(true);
    assert!(matches!(
        manager_operation(&manager, &tools, 2),
        Ok(ManageOperation::Delete { id: 12 })
    ));
    manager.set_label_input("".into());
    assert!(manager_operation(&manager, &tools, 0).is_err());
    manager.set_label_input("Mountain 收藏".into());
    assert!(manager_operation(&manager, &tools, 0).is_ok());
    select_manager(&manager, &tools, 0);
    assert!(!manager.get_delete_confirmed());
    assert!(
        manager_operation(&manager, &tools, 1).is_err(),
        "unknown visibility was silently overwritten"
    );
    manager.set_visibility_confirmed(true);
    assert!(manager_operation(&manager, &tools, 1).is_ok());
    assert!(
        manager.get_visibility_unknown(),
        "confirmation control vanished once checked"
    );
    manager.set_visibility_confirmed(false);
    assert!(manager_operation(&manager, &tools, 1).is_err());
    manager.set_visibility_confirmed(true);
    assert!(
        manager_operation(&manager, &tools, 0).is_err(),
        "duplicate name was allowed"
    );
}
