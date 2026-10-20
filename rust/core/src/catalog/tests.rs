use super::*;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use std::fs;

fn picture(path: &Path, format: ImageFormat, value: u8) {
    DynamicImage::ImageRgb8(RgbImage::from_pixel(3, 2, Rgb([value, 20, 30])))
        .save_with_format(path, format)
        .unwrap();
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

#[test]
fn unicode_tags_are_trimmed_collapsed_sorted_and_empty_removed() {
    assert_eq!(
        normalize_tags(&strings(&[
            "  BLUE\tSky ",
            "blue sky",
            "",
            " \n",
            "ÄPFEL",
            "äpfel",
            "東京\u{3000}空"
        ])),
        strings(&["blue sky", "äpfel", "東京 空"])
    );
}

#[test]
fn persistence_duplicates_tags_export_and_record_only_removal() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let a = source.path().join("a.png");
    let b = source.path().join("different.name");
    picture(&a, ImageFormat::Png, 10);
    fs::copy(&a, &b).unwrap();
    fs::write(source.path().join("keep.txt"), b"unrelated unchanged").unwrap();
    let original = fs::read(&a).unwrap();
    let mut preview = Catalog::scan(source.path(), false).unwrap();
    assert_eq!(preview.files.len(), 2);
    assert_eq!(preview.skipped, 1);
    preview.files[0].tags = strings(&[" BLUE  Sky", "", "blue sky"]);
    let db = Catalog::open(data.path()).unwrap();
    assert_eq!(db.import(&preview).unwrap(), 1);
    assert_eq!(db.import(&preview).unwrap(), 0);
    drop(db);
    let db = Catalog::open(data.path()).unwrap();
    let items = db.items().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.id.len(), 64);
    assert_eq!(item.paths.len(), 2);
    assert_eq!(item.tags, strings(&["blue sky"]));
    assert_eq!(
        (item.width, item.height, item.mime.as_str()),
        (3, 2, "image/png")
    );
    assert!(item.available);
    assert_eq!(item.bytes, original.len() as u64);
    assert_eq!(db.duplicate_groups().unwrap(), vec![item.paths.clone()]);
    db.set_tags(&item.id, strings(&[" X ", "x", "\t"])).unwrap();
    assert_eq!(db.items().unwrap()[0].tags, strings(&["x"]));
    db.set_tags(&item.id, strings(&["", " \t"])).unwrap();
    assert!(db.items().unwrap()[0].tags.is_empty());
    let exported: serde_json::Value = serde_json::from_str(&db.export_json().unwrap()).unwrap();
    assert_eq!(exported.as_array().unwrap().len(), 1);
    assert_eq!(exported[0]["id"], item.id);
    db.remove(&item.id).unwrap();
    assert!(db.items().unwrap().is_empty());
    assert_eq!(fs::read(&a).unwrap(), original);
    assert_eq!(fs::read(&b).unwrap(), original);
    assert_eq!(
        fs::read(source.path().join("keep.txt")).unwrap(),
        b"unrelated unchanged"
    );
}

#[test]
fn separate_imports_merge_paths_and_tags_without_overwrite() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let a = source.path().join("one");
    let b = source.path().join("two");
    picture(&a, ImageFormat::Png, 2);
    fs::copy(&a, &b).unwrap();
    let db = Catalog::open(data.path()).unwrap();
    let mut first = Catalog::scan(&a, false).unwrap();
    first.files[0].tags = strings(&["First"]);
    assert_eq!(db.import(&first).unwrap(), 1);
    let created = db.items().unwrap()[0].created;
    let mut second = Catalog::scan(&b, false).unwrap();
    second.files[0].tags = strings(&["Second"]);
    assert_eq!(db.import(&second).unwrap(), 0);
    let item = &db.items().unwrap()[0];
    assert_eq!(item.paths.len(), 2);
    assert_eq!(item.tags, strings(&["first", "second"]));
    assert_eq!(item.created, created);
}

#[test]
fn changed_or_forged_preview_rejects_whole_import() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let a = source.path().join("a.png");
    let b = source.path().join("b.png");
    picture(&a, ImageFormat::Png, 1);
    picture(&b, ImageFormat::Png, 2);
    let preview = Catalog::scan(source.path(), false).unwrap();
    let db = Catalog::open(data.path()).unwrap();
    picture(&b, ImageFormat::Png, 3);
    assert!(db.import(&preview).is_err());
    assert!(db.items().unwrap().is_empty());
    let mut forged = Catalog::scan(source.path(), false).unwrap();
    forged.files[1].width += 1;
    assert!(db.import(&forged).is_err());
    assert!(db.items().unwrap().is_empty());
    fs::remove_file(&b).unwrap();
    assert!(db.import(&preview).is_err());
    assert!(db.items().unwrap().is_empty());
}

#[test]
fn availability_uses_a_surviving_matching_source() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let a = source.path().join("a.png");
    let b = source.path().join("b.png");
    picture(&a, ImageFormat::Png, 1);
    fs::copy(&a, &b).unwrap();
    let db = Catalog::open(data.path()).unwrap();
    db.import(&Catalog::scan(source.path(), false).unwrap())
        .unwrap();
    fs::remove_file(&a).unwrap();
    let item = &db.items().unwrap()[0];
    assert!(item.available);
    assert_eq!(item.path, fs::canonicalize(&b).unwrap().to_str().unwrap());
    picture(&b, ImageFormat::Png, 9);
    assert!(!db.items().unwrap()[0].available);
}

#[test]
fn formats_corruption_readonly_and_opt_in_recursion() {
    let source = tempfile::tempdir().unwrap();
    for (name, format) in [
        ("jpeg", ImageFormat::Jpeg),
        ("png", ImageFormat::Png),
        ("gif", ImageFormat::Gif),
        ("webp", ImageFormat::WebP),
    ] {
        picture(&source.path().join(name), format, 7);
    }
    fs::write(
        source.path().join("corrupt.png"),
        b"\x89PNG\r\n\x1a\nnot a png",
    )
    .unwrap();
    fs::write(source.path().join("corrupt.jpg"), [0xff, 0xd8, 0xff, 0xd9]).unwrap();
    fs::write(source.path().join("corrupt.gif"), b"GIF89a").unwrap();
    fs::write(
        source.path().join("corrupt.webp"),
        b"RIFF\xff\xff\xff\xffWEBP",
    )
    .unwrap();
    let sub = source.path().join("nested");
    fs::create_dir(&sub).unwrap();
    picture(&sub.join("nested.png"), ImageFormat::Png, 8);
    let flat = Catalog::scan(source.path(), false).unwrap();
    assert_eq!(flat.files.len(), 4);
    assert_eq!(flat.skipped, 5);
    assert_eq!(Catalog::scan(source.path(), true).unwrap().files.len(), 5);
    assert!(!source.path().join("library-v1.sqlite3").exists());
    assert_eq!(
        fs::read(source.path().join("corrupt.gif")).unwrap(),
        b"GIF89a"
    );
}

#[test]
fn coexists_with_library_v1_tasks_and_rejects_future_schema() {
    let data = tempfile::tempdir().unwrap();
    let library = crate::library::Library::open(data.path()).unwrap();
    library.enqueue(serde_json::json!({"id":"abc123"})).unwrap();
    let db = Catalog::open(data.path()).unwrap();
    assert!(db.items().unwrap().is_empty());
    assert_eq!(library.get("abc123").unwrap().state, "queued");
    let conn = Connection::open(data.path().join("library-v1.sqlite3")).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        1
    );
    conn.pragma_update(None, "user_version", 2).unwrap();
    assert!(Catalog::open(data.path()).is_err());
}

#[test]
fn catalog_first_is_compatible_with_library_open() {
    let data = tempfile::tempdir().unwrap();
    let db = Catalog::open(data.path()).unwrap();
    let library = crate::library::Library::open(data.path()).unwrap();
    library.enqueue(serde_json::json!({"id":"abc123"})).unwrap();
    assert!(db.items().unwrap().is_empty());
    assert_eq!(library.tasks().unwrap().len(), 1);
}

#[test]
fn traversal_and_preview_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let mut path = root.path().to_path_buf();
    for _ in 0..MAX_DEPTH + 1 {
        path = path.join("d");
        fs::create_dir(&path).unwrap();
    }
    assert!(Catalog::scan(root.path(), true).is_err());
    assert!(Catalog::scan(root.path(), false).unwrap().files.is_empty());
    let data = tempfile::tempdir().unwrap();
    let db = Catalog::open(data.path()).unwrap();
    picture(&root.path().join("one.png"), ImageFormat::Png, 1);
    let candidate = Catalog::scan(&root.path().join("one.png"), false)
        .unwrap()
        .files
        .remove(0);
    assert!(db
        .import(&ImportPreview {
            files: vec![candidate; MAX_ENTRIES + 1],
            skipped: 0
        })
        .is_err());
}

#[cfg(any(unix, windows))]
#[test]
fn links_are_not_followed_in_scan_or_import() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let external = outside.path().join("external.png");
    picture(&external, ImageFormat::Png, 1);
    let local = root.path().join("local.png");
    picture(&local, ImageFormat::Png, 2);
    let preview = Catalog::scan(&local, false).unwrap();
    let file_link = root.path().join("file-link.png");
    let dir_link = root.path().join("dir-link");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&external, &file_link).unwrap();
        std::os::unix::fs::symlink(outside.path(), &dir_link).unwrap();
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(&external, &file_link).unwrap();
        std::os::windows::fs::symlink_dir(outside.path(), &dir_link).unwrap();
    }
    assert!(Catalog::scan(&file_link, false).is_err());
    assert!(Catalog::scan(&dir_link, true).is_err());
    assert!(Catalog::scan(&dir_link.join("external.png"), false).is_err());
    let scanned = Catalog::scan(root.path(), true).unwrap();
    assert_eq!(scanned.files.len(), 1);
    assert_eq!(scanned.skipped, 2);
    fs::remove_file(&local).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &local).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&external, &local).unwrap();
    let db = Catalog::open(data.path()).unwrap();
    assert!(db.import(&preview).is_err());
    assert!(db.items().unwrap().is_empty());
}

#[test]
fn database_error_rolls_back_earlier_rows_and_sources() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    picture(&source.path().join("a.png"), ImageFormat::Png, 1);
    picture(&source.path().join("b.png"), ImageFormat::Png, 2);
    let preview = Catalog::scan(source.path(), false).unwrap();
    let db = Catalog::open(data.path()).unwrap();
    let conn = Connection::open(data.path().join("library-v1.sqlite3")).unwrap();
    conn.execute_batch(&format!("CREATE TRIGGER reject_second BEFORE INSERT ON catalog_items WHEN NEW.id='{}' BEGIN SELECT RAISE(ABORT,'test failure'); END;", preview.files[1].sha256)).unwrap();
    assert!(db.import(&preview).is_err());
    assert!(db.items().unwrap().is_empty());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM catalog_sources", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
}

#[test]
fn changing_an_existing_source_does_not_overwrite_old_content_record() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let file = source.path().join("image.png");
    picture(&file, ImageFormat::Png, 1);
    let db = Catalog::open(data.path()).unwrap();
    let original = Catalog::scan(&file, false).unwrap();
    db.import(&original).unwrap();
    picture(&file, ImageFormat::Png, 2);
    assert!(db.import(&original).is_err());
    assert_eq!(db.import(&Catalog::scan(&file, false).unwrap()).unwrap(), 1);
    let items = db.items().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items
        .iter()
        .find(|i| i.id == original.files[0].sha256)
        .is_some_and(|i| !i.available));
    assert_eq!(items.iter().filter(|i| i.available).count(), 1);
    assert!(db.duplicate_groups().unwrap().is_empty());
}

#[cfg(windows)]
#[test]
fn junctions_and_windows_device_paths_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    picture(&outside.path().join("image.png"), ImageFormat::Png, 1);
    let junction = root.path().join("junction");
    assert!(std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(outside.path())
        .output()
        .unwrap()
        .status
        .success());
    assert!(Catalog::scan(&junction, true).is_err());
    assert!(Catalog::scan(&junction.join("image.png"), false).is_err());
    let preview = Catalog::scan(root.path(), true).unwrap();
    assert!(preview.files.is_empty());
    assert_eq!(preview.skipped, 1);
    assert!(Catalog::scan(Path::new(r"\\.\NUL"), false).is_err());
    fs::remove_dir(junction).unwrap();
}

#[test]
fn truncated_real_images_are_skipped_and_ids_are_full_content_hashes() {
    let root = tempfile::tempdir().unwrap();
    for (name, format) in [
        ("jpeg", ImageFormat::Jpeg),
        ("png", ImageFormat::Png),
        ("gif", ImageFormat::Gif),
        ("webp", ImageFormat::WebP),
    ] {
        let file = root.path().join(name);
        picture(&file, format, 7);
        let bytes = fs::read(&file).unwrap();
        let preview = Catalog::scan(&file, false).unwrap();
        assert_eq!(
            preview.files[0].sha256,
            format!("{:x}", Sha256::digest(&bytes))
        );
        fs::write(&file, &bytes[..bytes.len() / 2]).unwrap();
        let preview = Catalog::scan(&file, false).unwrap();
        assert!(preview.files.is_empty(), "{name}");
        assert_eq!(preview.skipped, 1);
    }
}

#[test]
fn jpeg_trailing_bytes_and_large_image_edges_have_no_artificial_cap() {
    let root = tempfile::tempdir().unwrap();
    let jpeg = root.path().join("trailing.jpg");
    picture(&jpeg, ImageFormat::Jpeg, 7);
    use std::io::Write;
    OpenOptions::new()
        .append(true)
        .open(&jpeg)
        .unwrap()
        .write_all(b"original trailing metadata")
        .unwrap();
    assert_eq!(Catalog::scan(&jpeg, false).unwrap().files.len(), 1);
    let wide = root.path().join("wide.png");
    DynamicImage::ImageRgb8(RgbImage::from_pixel(120_001, 1, Rgb([1, 2, 3])))
        .save_with_format(&wide, ImageFormat::Png)
        .unwrap();
    let candidate = Catalog::scan(&wide, false).unwrap().files.remove(0);
    assert_eq!((candidate.width, candidate.height), (120_001, 1));
}
