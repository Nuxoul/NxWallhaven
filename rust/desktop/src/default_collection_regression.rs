//! Read-only fixture distinguishes public profile collections from the owner's list.
use super::*;
use crate::browser_session::BrowserResponse;
use crate::website::SessionTransport;
use std::sync::Mutex;
struct OwnCollectionsFixture {
    paths: Mutex<Vec<String>>,
}
impl SessionTransport for OwnCollectionsFixture {
    fn show_login(&self) -> Result<(), String> {
        Ok(())
    }
    fn hide(&self) -> Result<(), String> {
        Ok(())
    }
    fn was_closed(&self) -> bool {
        false
    }
    fn current_html(&self) -> Result<String, String> {
        Ok(panel().into())
    }
    fn post(&self, _: &str, _: &str) -> Result<BrowserResponse, String> {
        panic!("read-only collection fixture must never submit a write")
    }
    fn get(&self, path: &str) -> Result<BrowserResponse, String> {
        self.paths.lock().unwrap().push(path.into());
        let body = if path == "/" {
            panel().to_string()
        } else if path == "/favorites" {
            format!(
                r#"{}<ul id="collections"><li class="collection private active" data-collection-id="1610304"><span class="tools"><a class="collection-rename" href="/collection/rename/1610304"><i class="fa-pencil"></i></a><a class="collection-toggle-private" href="/collection/privacy/1610304"><i class="fa-eye"></i></a><a class="collection-rm" href="/collection/remove/1610304">Remove</a></span><a class="label" href="/favorites/1610304"><i class="fa-star"></i><i class="fa-eye-slash"></i><small>4</small>Default</a></li><li class="collection public" data-collection-id="2287709"><span class="tools"><a href="/collection/privacy/2287709"><i class="fa-eye-slash"></i></a></span><a class="label" href="/favorites/2287709"><small>9</small>Custom</a></li><li class="collection"><a href="/favorites/trash">Trash</a></li></ul>"#,
                panel()
            )
        } else if path == "/user/Test/favorites" {
            format!(
                r#"{}<ul id="collections"><li class="collection"><a href="/user/Test/favorites/2287709"><span class="collection-label">Custom</span><small>9</small></a></li></ul>"#,
                panel()
            )
        } else if path.starts_with("/favorites/1610304?") {
            format!(
                r#"{}<div id="thumbs"><figure class="thumb-sfw" data-wallpaper-id="abc123"><img src="https://th.wallhaven.cc/small/ab/abc123.jpg"><span class="wall-res">1920 x 1080</span></figure></div><ul data-pagination='{{"total":2,"current":1}}'></ul>"#,
                panel()
            )
        } else {
            return Err("fixture rejected wrong collection route".into());
        };
        Ok(BrowserResponse {
            status: 200,
            url: format!("https://wallhaven.cc{path}"),
            body,
        })
    }
}
fn panel() -> &'static str {
    r#"<div id="userpanel"><a href="/user/Test">Test</a><form action="/logout"></form></div>"#
}
#[test]
fn private_default_is_listed_and_read_from_authenticated_owner_routes() {
    let fixture = Arc::new(OwnCollectionsFixture {
        paths: Default::default(),
    });
    let site = website::SiteClient::from_test_transport(fixture.clone());
    let list = site.read_collections("Test").unwrap();
    let default = list
        .data
        .iter()
        .find(|row| row.label.as_deref() == Some("Default"))
        .expect("private Default was omitted by reading public profile collections");
    assert_eq!(default.id, Some(1610304));
    assert_eq!(default.count, Some(4));
    assert_eq!(default.public, Some(false));
    assert_eq!(list.data.len(), 2, "trash must not become a collection");
    assert_eq!(
        list.data
            .iter()
            .find(|row| row.id == Some(2287709))
            .unwrap()
            .public,
        Some(true),
        "privacy toggle icon was mistaken for current visibility"
    );
    let page = site
        .read_collection_wallpapers("Test", 1610304, "100", 1)
        .unwrap();
    assert_eq!(page.data.len(), 1);
    assert_eq!(page.meta.last_page, Some(2));
    site.read_collection_wallpapers("Test", 1610304, "100", 2)
        .unwrap();
    let paths = fixture.paths.lock().unwrap();
    assert!(paths.iter().any(|p| p == "/favorites"));
    assert!(
        !paths.iter().any(|p| p.starts_with("/user/Test/favorites")),
        "authenticated own favorites must not use public-profile routes"
    );
    assert!(paths
        .iter()
        .any(|p| p == "/favorites/1610304?page=2&purity=100"));
    drop(paths);
    assert!(
        site.read_collections("OtherUser").is_err(),
        "owner mismatch was allowed"
    );
}
