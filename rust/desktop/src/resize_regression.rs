use super::*;
use slint::Model;

fn sample_cards() -> Vec<CardModel> {
    (0..60)
        .map(|i| CardModel {
            id: format!("card{i}"),
            aspect: [0.56, 1.0, 1.78, 2.4, 1.33][i % 5],
            ..Default::default()
        })
        .collect()
}
fn rows(cards: &[CardModel]) -> Vec<Vec<String>> {
    let mut rows = Vec::<Vec<String>>::new();
    let mut y = -1.0;
    for card in cards {
        if (card.y - y).abs() > 0.1 {
            rows.push(Vec::new());
            y = card.y;
        }
        rows.last_mut().unwrap().push(card.id.clone());
    }
    rows
}

#[test]
fn resizing_keeps_row_members_and_scales_cells_to_fill_the_width() {
    for mode in 0..=1 {
        let mut cards = sample_cards();
        relayout(&mut cards, 1264.0, mode);
        let original = cards.clone();
        let members = rows(&cards);
        for width in [2544.0, 900.0, 1904.0, 1264.0] {
            relayout(&mut cards, width, mode);
            assert_eq!(
                rows(&cards),
                members,
                "resize changed row members in mode {mode}"
            );
            let scale = width / 1264.0;
            for (card, before) in cards.iter().zip(&original) {
                assert!((card.w - before.w * scale).abs() < 0.1);
                assert!((card.h - before.h * scale).abs() < 0.1);
                assert!(card.x + card.w <= width + 0.1);
            }
            let first_row_y = cards[0].y;
            let right = cards
                .iter()
                .take_while(|c| (c.y - first_row_y).abs() < 0.1)
                .map(|c| c.x + c.w)
                .fold(0.0, f32::max);
            assert!(
                (right - width).abs() < 0.1,
                "row left an unfilled right side: {right}/{width}"
            );
        }
    }
}

#[test]
fn repeated_gallery_render_preserves_the_card_model_and_loaded_images() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let mut state = test_state();
    state.cards = sample_cards();
    state.walls = state
        .cards
        .iter()
        .map(|c| Wallpaper {
            id: c.id.clone(),
            ..Default::default()
        })
        .collect();
    relayout(&mut state.cards, state.width, 0);
    let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(4, 4);
    state
        .images
        .insert("card0".into(), Image::from_rgba8(pixels));
    let loader = ImageLoader::new(1);
    let (tx, _rx) = mpsc::channel();
    render(&ui, &mut state, &loader, &tx);
    let original = ui.get_cards();
    assert!(original.row_data(0).unwrap().loaded);
    // Active delegates must not flash back to placeholders after LRU eviction.
    state.images.store.borrow_mut().map.clear();
    state.images.store.borrow_mut().order.clear();
    state.images.store.borrow_mut().bytes = 0;
    render(&ui, &mut state, &loader, &tx);
    assert!(
        original == ui.get_cards(),
        "render replaced every card delegate instead of updating existing rows"
    );
    assert!(ui.get_cards().row_data(0).unwrap().loaded);
}

#[test]
fn actual_viewport_resize_tracks_full_width_and_preserves_scaled_scroll() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
    ui.show().unwrap();
    let mut state = test_state();
    state.cards = sample_cards();
    state.walls = state
        .cards
        .iter()
        .map(|c| Wallpaper {
            id: c.id.clone(),
            ..Default::default()
        })
        .collect();
    let loader = ImageLoader::new(1);
    let (tx, _rx) = mpsc::channel();
    slint::platform::update_timers_and_animations();
    assert!(sync_gallery_geometry(&ui, &mut state));
    render(&ui, &mut state, &loader, &tx);
    let members = rows(&state.cards);
    let original_width = state.width;
    ui.set_browse_position(-400.0);
    slint::platform::update_timers_and_animations();
    sync_gallery_geometry(&ui, &mut state);
    let original_offset = state.offset;
    assert!(original_offset > 300.0);
    ui.window()
        .set_size(slint::LogicalSize::new(2560.0, 1440.0));
    slint::platform::update_timers_and_animations();
    assert!(sync_gallery_geometry(&ui, &mut state));
    render(&ui, &mut state, &loader, &tx);
    assert!(
        state.width > 2500.0,
        "maximized viewport kept stale width: {}",
        state.width
    );
    assert_eq!(rows(&state.cards), members);
    assert!((state.offset - original_offset * state.width / original_width).abs() < 1.0);
    assert!(
        ui.get_viewport_height() > 1000.0,
        "height-only viewport binding was stale"
    );
    assert!(
        !sync_gallery_geometry(&ui, &mut state),
        "stable viewport oscillated"
    );
    ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
    slint::platform::update_timers_and_animations();
    assert!(sync_gallery_geometry(&ui, &mut state));
    assert!((state.offset - original_offset).abs() < 1.0);
    assert_eq!(rows(&state.cards), members);
    ui.hide().unwrap();
}

#[test]
fn in_place_model_sync_preserves_overlap_when_scrolling_and_appending() {
    let make = |ids: &[&str]| {
        ids.iter()
            .map(|id| Card {
                id: (*id).into(),
                ..Default::default()
            })
            .collect::<Vec<_>>()
    };
    let original = slint::ModelRc::new(VecModel::from(make(&["a", "b", "c", "d"])));
    for ids in [
        &["b", "c", "d", "e"][..],
        &["a", "b", "c"][..],
        &[][..],
        &["x", "y"][..],
    ] {
        let updated =
            gallery_layout::sync_model(original.clone(), make(ids), |card| card.id.as_str());
        assert!(updated == original);
        assert_eq!(
            updated.iter().map(|c| c.id.to_string()).collect::<Vec<_>>(),
            ids
        );
    }
}
