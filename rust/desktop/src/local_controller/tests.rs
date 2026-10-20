use super::*;
use std::time::Instant;
fn spin(inner: &Inner, condition: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(12);
    while !condition() {
        inner.pump();
        slint::platform::update_timers_and_animations();
        assert!(Instant::now() < end, "native local workflow timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
    inner.pump();
}
#[test]
fn native_import_preview_confirm_tags_duplicates_and_original_are_safe() {
    i_slint_backend_testing::init_no_event_loop();
    let root = std::env::temp_dir().join(format!(
        "wh-local-ui-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let source = root.join("source");
    std::fs::create_dir_all(&source).unwrap();
    let image = source.join("one.png");
    image::RgbaImage::from_pixel(19, 13, image::Rgba([70, 120, 200, 255]))
        .save(&image)
        .unwrap();
    let twin = source.join("same.png");
    std::fs::copy(&image, &twin).unwrap();
    let expected = std::fs::read(&image).unwrap();
    let ui = AppWindow::new().unwrap();
    let state = Rc::new(RefCell::new(crate::test_state()));
    let tools = bind_at(&ui, state, Arc::new(ImageLoader::new(1)), root.join("data")).unwrap();
    let inner = &tools.0;
    inner.window.show().unwrap();
    inner.window.set_tab(2);
    inner
        .window
        .set_import_path(source.to_string_lossy().into_owned().into());
    inner.window.invoke_scan();
    spin(inner, || !inner.window.get_busy());
    assert!(inner.window.get_import_ready());
    assert_eq!(
        Catalog::open(&root.join("data"))
            .unwrap()
            .items()
            .unwrap()
            .len(),
        0,
        "scan must not commit"
    );
    inner.window.invoke_commit_import();
    assert!(!inner.window.get_confirmation().is_empty());
    assert_eq!(
        Catalog::open(&root.join("data"))
            .unwrap()
            .items()
            .unwrap()
            .len(),
        0,
        "opening confirmation must not commit"
    );
    inner.window.invoke_confirm();
    spin(inner, || !inner.window.get_busy());
    spin(inner, || !inner.view.borrow().refreshing);
    assert_eq!(
        Catalog::open(&root.join("data"))
            .unwrap()
            .items()
            .unwrap()
            .len(),
        1
    );
    inner.window.invoke_change_tab(1);
    spin(inner, || inner.window.get_rows().row_count() == 1);
    let key = inner.window.get_rows().row_data(0).unwrap().key;
    inner.window.invoke_select_row(key.clone());
    inner
        .window
        .set_tags_input("  Nature , nature,  BLUE   SKY ".into());
    inner.window.invoke_save_tags();
    assert!(inner.window.get_confirmation().contains("blue sky"));
    inner.window.invoke_confirm();
    spin(inner, || !inner.window.get_busy());
    assert_eq!(
        // Saving must replace the edit field with normalized persisted tags.
        Catalog::open(&root.join("data")).unwrap().items().unwrap()[0].tags,
        vec!["blue sky", "nature"]
    );
    spin(inner, || !inner.view.borrow().refreshing);
    assert_eq!(inner.window.get_tags_input().as_str(), "blue sky, nature");
    inner.window.invoke_open_selected();
    spin(inner, || {
        inner
            .view
            .borrow()
            .viewers
            .last()
            .is_some_and(|v| v.get_loaded())
    });
    assert!(inner
        .view
        .borrow()
        .viewers
        .last()
        .unwrap()
        .get_message()
        .contains("19 × 13"));
    inner.window.invoke_duplicates();
    spin(inner, || !inner.window.get_busy());
    assert!(inner.window.get_import_summary().contains("1 组"));
    inner.window.invoke_remove_selected(false);
    assert!(!inner.window.get_confirmation().is_empty());
    inner.window.invoke_confirm();
    spin(inner, || !inner.window.get_busy());
    assert!(Catalog::open(&root.join("data"))
        .unwrap()
        .items()
        .unwrap()
        .is_empty());
    spin(inner, || !inner.view.borrow().refreshing);
    assert_eq!(
        inner.window.get_rows().row_count(),
        0,
        "records-only removal must disappear from UI"
    );
    assert_eq!(std::fs::read(&image).unwrap(), expected);
    assert_eq!(std::fs::read(&twin).unwrap(), expected);
    tools.hide_all();
    tools.manager().shutdown();
    drop(tools);
    drop(ui);
    let _ = std::fs::remove_dir_all(root);
}
#[test]
fn local_original_is_not_downsampled_and_json_export_never_overwrites() {
    let root = std::env::temp_dir().join(format!("wh-local-file-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("wide.png");
    image::RgbaImage::from_pixel(20001, 1, image::Rgba([1, 2, 3, 255]))
        .save(&path)
        .unwrap();
    let original = decode_file(&path, false).unwrap();
    assert_eq!((original.w, original.h), (20001, 1));
    let export = root.join("data.json");
    write_new(&export, b"first").unwrap();
    assert!(write_new(&export, b"second").is_err());
    assert_eq!(std::fs::read(export).unwrap(), b"first");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn permission_revoke_clears_unknown_purity_and_discards_stale_async_results() {
    i_slint_backend_testing::init_no_event_loop();
    let root = std::env::temp_dir().join(format!("wh-local-policy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ui = AppWindow::new().unwrap();
    let state = Rc::new(RefCell::new(crate::test_state()));
    state.borrow_mut().settings.api_key = Some("first-key".into());
    state.borrow_mut().settings.nsfw_enabled = true;
    let tools = bind_at(
        &ui,
        state.clone(),
        Arc::new(ImageLoader::new(1)),
        root.clone(),
    )
    .unwrap();
    let inner = &tools.0;
    spin(inner, || !inner.view.borrow().refreshing);
    let db = wh_core::library::Library::open(&root).unwrap();
    let task = db.enqueue(serde_json::json!({"id":"abc123","path":"https://w.wallhaven.cc/full/ab/wallhaven-abc123.png"})).unwrap();
    db.set_state("abc123", "paused").unwrap();
    inner.view.borrow_mut().records = vec![task_record(task.clone())];
    inner.view.borrow_mut().selected = Some("download:abc123".into());
    inner.view.borrow_mut().pending = Some(Pending::Tags("abc123".into(), vec!["secret".into()]));
    inner.window.set_confirmation("restricted detail".into());
    inner.render();
    let generation = inner.view.borrow().generation;
    state.borrow_mut().settings.nsfw_enabled = false;
    inner
        .tx
        .send(ResultEvent::Snapshot(
            generation,
            Ok(vec![task_record(task.clone())]),
        ))
        .unwrap();
    inner
        .tx
        .send(ResultEvent::Thumbnail(
            generation,
            "download:abc123".into(),
            Err("stale".into()),
        ))
        .unwrap();
    inner.pump();
    assert!(inner.view.borrow().records.is_empty());
    assert!(inner.view.borrow().thumbs.is_empty());
    assert!(inner.view.borrow().failed_thumbs.is_empty());
    assert!(inner.view.borrow().pending.is_none());
    assert!(inner.window.get_confirmation().is_empty());
    assert_eq!(inner.window.get_rows().row_count(), 0);
    spin(inner, || !inner.view.borrow().refreshing);
    assert!(
        inner.view.borrow().records.is_empty(),
        "fresh default-deny snapshot leaked unknown purity"
    );
    // Replacing a present key invalidates results even when allow remains true.
    state.borrow_mut().settings.nsfw_enabled = true;
    inner.pump();
    spin(inner, || !inner.view.borrow().refreshing);
    let generation = inner.view.borrow().generation;
    state.borrow_mut().settings.api_key = Some("second-key".into());
    inner
        .tx
        .send(ResultEvent::Snapshot(
            generation,
            Ok(vec![task_record(task)]),
        ))
        .unwrap();
    inner.pump();
    assert!(inner.view.borrow().records.is_empty());
    tools.hide_all();
    tools.manager().shutdown();
    drop(tools);
    drop(ui);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn downloaded_records_only_remove_disappears_despite_preoperation_snapshot() {
    i_slint_backend_testing::init_no_event_loop();
    let root = std::env::temp_dir().join(format!("wh-local-remove-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let image = root.join("original.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
        .save(&image)
        .unwrap();
    let ui = AppWindow::new().unwrap();
    let tools = bind_at(
        &ui,
        Rc::new(RefCell::new(crate::test_state())),
        Arc::new(ImageLoader::new(1)),
        root.join("data"),
    )
    .unwrap();
    let inner = &tools.0;
    spin(inner, || !inner.view.borrow().refreshing);
    let db = wh_core::library::Library::open(&root.join("data")).unwrap();
    db.enqueue(serde_json::json!({"id":"abc123","purity":"SFW","path":"https://w.wallhaven.cc/full/ab/wallhaven-abc123.png"})).unwrap();
    db.complete(
        "abc123",
        reqwest::Url::from_file_path(&image).unwrap().as_str(),
    )
    .unwrap();
    let record = task_record(db.get("abc123").unwrap());
    inner.view.borrow_mut().records = vec![record.clone()];
    inner.window.set_tab(1);
    inner.view.borrow_mut().selected = Some(record.key.clone());
    inner.render();
    let generation = inner.view.borrow().generation;
    // Simulate a snapshot already running before the confirmed mutation completes.
    inner.view.borrow_mut().refreshing = true;
    db.remove("abc123", true).unwrap();
    inner
        .tx
        .send(ResultEvent::Message(Ok("removed".into())))
        .unwrap();
    inner
        .tx
        .send(ResultEvent::Snapshot(generation, Ok(vec![record])))
        .unwrap();
    inner.pump();
    spin(inner, || !inner.view.borrow().refreshing);
    assert_eq!(inner.window.get_rows().row_count(), 0);
    assert!(inner.view.borrow().selected.is_none());
    assert!(inner.window.get_tags_input().is_empty());
    assert!(image.exists());
    tools.hide_all();
    tools.manager().shutdown();
    drop(tools);
    drop(ui);
    let _ = std::fs::remove_dir_all(root);
}
