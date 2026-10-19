use super::*;
use crate::profile_controller::ProfileState;

fn page(ids: &[(&str, &str)]) -> SearchResponse {
    SearchResponse {
        data: ids
            .iter()
            .map(|(id, purity)| Wallpaper {
                id: (*id).into(),
                purity: Some((*purity).into()),
                ..Default::default()
            })
            .collect(),
        meta: wh_core::models::Meta {
            current_page: Some(1),
            last_page: Some(3),
            ..Default::default()
        },
    }
}
#[test]
fn profile_pages_are_generation_guarded_default_safe_and_deduplicated() {
    let mut state = ProfileState::default();
    let old = state.open("first".into());
    let current = state.open("second".into());
    assert!(!state.receive_page(old, 1, page(&[("old", "sfw")])));
    assert!(state.walls.is_empty());
    assert_eq!(state.start_page(), Some((current, 1)));
    assert!(state.receive_page(
        current,
        1,
        page(&[
            ("safe", "sfw"),
            ("safe", "sfw"),
            ("unsafe", "nsfw"),
            ("unknown", "")
        ])
    ));
    assert_eq!(
        state
            .walls
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        ["safe"]
    );
    assert_eq!(state.page, 1);
    assert_eq!(state.last, 3);
    assert_eq!(state.username, "second");
}

#[test]
fn profile_filters_accept_selected_sketchy_and_authorized_nsfw_only() {
    let mut state = ProfileState::default();
    let original = state.open("uploader".into());
    state.set_filters(false, true, false);
    assert!(!state.receive_page(original, 1, page(&[("old", "sfw")])));
    let sketchy = state.request;
    assert_eq!(state.purity(), "010");
    assert_eq!(state.start_page(), Some((sketchy, 1)));
    assert!(state.receive_page(
        sketchy,
        1,
        page(&[("s", "sfw"), ("k", "sketchy"), ("n", "nsfw")])
    ));
    assert_eq!(
        state
            .walls
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        ["k"]
    );

    let mut settings = Settings::default();
    settings.api_key = Some("fixture-key".into());
    settings.nsfw_enabled = true;
    state.sync_permissions(&settings);
    state.set_filters(false, false, true);
    let restricted = state.request;
    let old_preview = state.preview_request;
    assert_eq!(state.purity(), "001");
    assert_eq!(state.start_page(), Some((restricted, 1)));
    assert!(state.receive_page(restricted, 1, page(&[("n", "nsfw"), ("k", "sketchy")])));
    assert_eq!(state.walls[0].id, "n");
    settings.api_key = Some("another-fixture-key".into());
    assert!(state.sync_permissions(&settings));
    assert!(state.preview_request > old_preview);
    assert!(!state.receive_page(restricted, 1, page(&[("late", "nsfw")])));
    assert!(state.walls.is_empty());
    assert_eq!(state.purity(), "100");
    state.set_filters(false, false, true);
    settings.nsfw_enabled = false;
    assert!(state.sync_permissions(&settings));
    assert_eq!(state.purity(), "100");
    assert!(state.walls.is_empty());
}

#[test]
fn profile_empty_selection_falls_back_to_sfw_and_open_resets_filters() {
    let mut state = ProfileState::default();
    state.set_filters(false, false, false);
    assert_eq!(state.purity(), "100");
    state.set_filters(false, true, false);
    state.open("another".into());
    assert_eq!(state.purity(), "100");
}

#[test]
fn native_profile_window_preserves_main_detail_and_browse_context() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1200.0, 800.0));
    ui.set_content_height(3200.0);
    ui.show().unwrap();
    slint::platform::update_timers_and_animations();
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(500.0, 400.0),
            delta_x: 0.0,
            delta_y: -600.0,
        });
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
    slint::platform::update_timers_and_animations();
    let position = ui.get_scroll_offset();
    assert!(position > 0.0);
    fill_detail(
        &ui,
        &Wallpaper {
            id: "6l2rgq".into(),
            uploader: Some(wh_core::models::Uploader {
                username: Some("whimsicalmango".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    assert!(ui.get_detail_uploader_available());
    ui.set_screen(1);
    let size = ui.window().size();
    let profile = ProfileWindow::new().unwrap();
    profile.set_username("whimsicalmango".into());
    profile.show().unwrap();
    profile
        .window()
        .set_size(slint::LogicalSize::new(760.0, 560.0));
    for tab in 0..=2 {
        profile.set_tab(tab);
        slint::platform::update_timers_and_animations();
        assert_eq!(
            profile.window().size(),
            slint::PhysicalSize::new(760, 560),
            "profile content forced window growth"
        );
    }
    profile.set_tab(1);
    profile.set_content_height(2400.0);
    slint::platform::update_timers_and_animations();
    profile
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(400.0, 340.0),
            delta_x: 0.0,
            delta_y: -400.0,
        });
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
    slint::platform::update_timers_and_animations();
    let profile_position = profile.get_gallery_offset();
    assert!(profile_position > 0.0, "profile gallery cannot scroll");
    profile.set_preview_open(true);
    slint::platform::update_timers_and_animations();
    profile.set_preview_open(false);
    slint::platform::update_timers_and_animations();
    assert!(
        (profile.get_gallery_offset() - profile_position).abs() < 1.0,
        "preview reset profile gallery"
    );
    profile.hide().unwrap();
    profile.show().unwrap();
    assert_eq!(ui.window().size(), size);
    assert_eq!(ui.get_screen(), 1);
    assert_eq!(ui.get_detail_id().as_str(), "6l2rgq");
    ui.set_screen(0);
    slint::platform::update_timers_and_animations();
    assert!(
        (ui.get_scroll_offset() - position).abs() < 1.0,
        "profile reset the main browse position"
    );
    fill_detail(&ui, &Wallpaper::default());
    assert!(
        !ui.get_detail_uploader_available(),
        "missing uploader must not be a false link"
    );
    profile.hide().unwrap();
    ui.hide().unwrap();
}

#[test]
fn profile_append_deduplicates_stops_on_empty_and_never_fetches_twice() {
    let mut state = ProfileState::default();
    state.open("author".into());
    let (generation, first) = state.start_page().expect("first page");
    assert_eq!(first, 1);
    assert!(state.start_page().is_none(), "only one page in flight");
    assert!(state.receive_page(generation, first, page(&[("a", "sfw"), ("b", "sfw")])));
    let (generation, second) = state.start_page().expect("second page");
    assert_eq!(second, 2);
    assert!(!state.receive_page(generation, first, page(&[("stale", "sfw")])));
    assert!(state.receive_page(generation, second, page(&[("b", "sfw"), ("c", "sfw")])));
    assert_eq!(
        state
            .walls
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    let (generation, third) = state.start_page().expect("third page");
    assert!(state.receive_page(generation, third, page(&[("a", "sfw"), ("c", "sfw")])));
    assert!(
        state.start_page().is_none(),
        "duplicate-only page ends pagination"
    );
    assert_eq!(state.walls.len(), 3);
    state.open("new author".into());
    assert!(!state.receive_page(generation, third, page(&[("late", "sfw")])));
    let (next, first) = state.start_page().unwrap();
    assert!(state.receive_page(next, first, page(&[])));
    assert!(state.start_page().is_none(), "empty page ends pagination");
}

#[test]
fn profile_window_append_retains_scroll_and_reports_end() {
    i_slint_backend_testing::init_no_event_loop();
    let profile = ProfileWindow::new().unwrap();
    profile.show().unwrap();
    profile.set_tab(1);
    profile.set_content_height(2200.0);
    slint::platform::update_timers_and_animations();
    profile
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(400.0, 330.0),
            delta_x: 0.0,
            delta_y: -400.0,
        });
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
    slint::platform::update_timers_and_animations();
    let before = profile.get_gallery_offset();
    assert!(before > 0.0);
    assert!(profile.get_gallery_visible_height() > 0.0);
    profile.set_loaded_count(64);
    profile.set_has_next(false);
    profile.set_content_height(4200.0);
    slint::platform::update_timers_and_animations();
    assert!((profile.get_gallery_offset() - before).abs() < 1.0);
    assert_eq!(profile.get_loaded_count(), 64);
    assert!(!profile.get_has_next());
    profile.set_load_error(true);
    assert!(profile.get_load_error());
    profile.hide().unwrap();
}

#[test]
fn profile_page_is_not_capped_at_sixty_four_items() {
    let mut state = ProfileState::default();
    state.open("author".into());
    let (token, number) = state.start_page().unwrap();
    let ids = (0..70)
        .map(|i| (format!("wall-{i}"), "sfw"))
        .collect::<Vec<_>>();
    let response = page(
        &ids.iter()
            .map(|(id, purity)| (id.as_str(), *purity))
            .collect::<Vec<_>>(),
    );
    assert!(state.receive_page(token, number, response));
    assert_eq!(state.walls.len(), 70);
    assert_eq!(state.start_page(), Some((token, 2)));
}
