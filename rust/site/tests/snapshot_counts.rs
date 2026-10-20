//! Deterministic request-count fixtures. No network or real account writes.
use std::sync::{Arc, Mutex};
use wh_site::{Request, Response, SiteClient, SiteTransport};

#[derive(Default)]
struct State {
    requests: Vec<Request>,
    favorite: bool,
    owner: String,
    drop_write: bool,
    reverse: bool,
}
struct Browser(Mutex<State>);
impl Browser {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(State {
            owner: "Alice".into(),
            ..State::default()
        })))
    }
    fn calls(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .requests
            .iter()
            .map(|r| {
                format!(
                    "{} {}",
                    r.method,
                    r.url.strip_prefix("https://wallhaven.cc").unwrap()
                )
            })
            .collect()
    }
    fn clear(&self) {
        self.0.lock().unwrap().requests.clear();
    }
}
impl SiteTransport for Browser {
    fn request(&self, r: Request) -> Result<Response, String> {
        let mut s = self.0.lock().unwrap();
        s.requests.push(r.clone());
        let panel = format!(
            r#"<div id="userpanel"><a href="/user/{0}">{0}</a><form action="/logout"></form></div>"#,
            s.owner
        );
        let body = if r.method == "POST" {
            assert_eq!(r.csrf.as_deref(), Some("fresh"));
            if s.drop_write {
                return Err("lost response".into());
            }
            s.favorite = true;
            r#"{"status":true}"#.into()
        } else if r.url.ends_with("/favorites") {
            format!(
                r#"{panel}<ul class="collections-list"><li class="collection"><a href="/favorites/12"><span class="collection-label">Default</span></a></li></ul>"#
            )
        } else if r.url.contains("/w/") {
            let button = if s.favorite {
                r#"<a id="fav-button" href="/favorite/remove/abc123">Remove</a>"#.into()
            } else {
                let a = r#"<a class="add-fav" href="/favorite/add/abc123/12">Default</a>"#;
                let b = r#"<a class="add-fav" href="/favorite/add/abc123/13">Other</a>"#;
                format!(
                    r#"<div id="fav-button" class="add-button">{}</div>"#,
                    if s.reverse {
                        format!("{b}{a}")
                    } else {
                        format!("{a}{b}")
                    }
                )
            };
            format!(r#"{panel}<meta name="csrf-token" content="fresh">{button}"#)
        } else {
            panel
        };
        Ok(Response {
            status: 200,
            url: r.url,
            body,
        })
    }
}
#[test]
fn known_owner_collection_read_is_one_get_without_homepage() {
    let b = Browser::new();
    let c = SiteClient::with_transport(b.clone());
    assert_eq!(c.read_collections("Alice").unwrap().data.len(), 1);
    assert_eq!(b.calls(), ["GET /favorites"]);
}
#[test]
fn favorite_write_uses_fresh_page_and_readback_not_homepage() {
    let b = Browser::new();
    let c = SiteClient::with_transport(b.clone());
    c.favorite_state("abc123").unwrap();
    b.clear();
    c.set_favorite("Alice", "abc123", true, 0).unwrap();
    assert_eq!(
        b.calls(),
        [
            "GET /w/abc123",
            "POST /favorite/add/abc123/12",
            "GET /w/abc123"
        ]
    );
}
#[test]
fn selected_target_survives_fresh_form_reordering() {
    let b = Browser::new();
    let c = SiteClient::with_transport(b.clone());
    c.favorite_state("abc123").unwrap();
    b.clear();
    b.0.lock().unwrap().reverse = true;
    c.set_favorite("Alice", "abc123", true, 0).unwrap();
    assert!(
        b.calls().contains(&"POST /favorite/add/abc123/12".into()),
        "must reconcile Default, not reuse index 0"
    );
}

#[test]
fn snapshots_report_owner_from_only_the_requested_page() {
    let b = Browser::new();
    let c = SiteClient::with_transport(b.clone());
    let collections = c.collection_snapshot().unwrap();
    assert_eq!(collections["owner"], "Alice");
    assert_eq!(b.calls(), ["GET /favorites"]);
    b.clear();
    b.0.lock().unwrap().owner = "Bob".into();
    let favorite = c.favorite_snapshot("abc123").unwrap();
    assert_eq!(favorite["owner"], "Bob");
    assert_eq!(b.calls(), ["GET /w/abc123"]);
    b.clear();
    assert!(c.read_collections("Alice").is_err());
    assert_eq!(b.calls(), ["GET /favorites"]);
    b.clear();
    assert_eq!(
        c.set_favorite_target("Alice", "abc123", true, "Default")
            .unwrap_err(),
        wh_site::SiteError::OwnerMismatch
    );
    assert_eq!(b.calls(), ["GET /w/abc123"]);
}

#[test]
fn lost_write_response_is_unknown_and_never_replayed() {
    let b = Browser::new();
    let c = SiteClient::with_transport(b.clone());
    b.0.lock().unwrap().drop_write = true;
    assert_eq!(
        c.set_favorite_target("Alice", "abc123", true, "Default")
            .unwrap_err(),
        wh_site::SiteError::UnknownOutcome
    );
    assert_eq!(b.calls(), ["GET /w/abc123", "POST /favorite/add/abc123/12"]);
}

#[test]
fn collection_pagination_rejects_changed_owner_on_second_page() {
    struct Pages(Mutex<Vec<Request>>);
    impl SiteTransport for Pages {
        fn request(&self, r: Request) -> Result<Response, String> {
            let mut calls = self.0.lock().unwrap();
            calls.push(r.clone());
            let owner = if calls.len() == 1 { "Alice" } else { "Bob" };
            Ok(Response {
                status: 200,
                url: r.url,
                body: format!(
                    r#"<div id="userpanel"><a href="/user/{owner}">{owner}</a><form action="/logout"></form></div><ul class="collections-list"><li class="collection"><a href="/favorites/12">Default</a></li><li class="pagination"><a rel="next" href="/favorites?page=2">Next</a></li></ul>"#
                ),
            })
        }
    }
    let b = Arc::new(Pages(Mutex::new(Vec::new())));
    let c = SiteClient::with_transport(b.clone());
    assert!(c.collection_snapshot().unwrap_err().contains("账号已变化"));
    let calls = b.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|r| r.url.contains("/favorites")));
}
