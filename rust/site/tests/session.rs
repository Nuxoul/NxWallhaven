//! Local browser-session fixtures only: these tests never contact a real server.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use wh_site::{ManageOperation, Request, Response, SiteClient, SiteError, SiteTransport};

#[derive(Clone, Copy, PartialEq)]
enum Outcome {
    Confirmed,
    LostUnapplied,
    LostApplied,
    Malformed,
}
struct State {
    owner: String,
    wallpaper_owner: Option<String>,
    rows: BTreeMap<u32, (String, bool)>,
    favorite: bool,
    token: usize,
    requests: Vec<Request>,
    outcome: Outcome,
}
struct Browser(Mutex<State>);
impl Browser {
    fn new(outcome: Outcome) -> Arc<Self> {
        Arc::new(Self(Mutex::new(State {
            owner: "Test_1".into(),
            wallpaper_owner: None,
            rows: BTreeMap::from([(12, ("Default".into(), false))]),
            favorite: false,
            token: 0,
            requests: Vec::new(),
            outcome,
        })))
    }
    fn posts(&self) -> usize {
        self.0
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.method == "POST")
            .count()
    }
}
fn panel(owner: &str) -> String {
    format!(
        r#"<div id="userpanel"><a href="/user/{owner}">{owner}</a><form action="/logout"></form></div>"#
    )
}
fn form(action: &str, token: usize, method: &str, fields: bool) -> String {
    format!(
        r#"<form method="post" action="{action}"><input type="hidden" name="_token" value="fresh-{token}"><input type="hidden" name="_method" value="{method}">{}</form>"#,
        if fields {
            r#"<input name="label"><input name="public" type="checkbox">"#
        } else {
            ""
        }
    )
}
impl SiteTransport for Browser {
    fn request(&self, request: Request) -> Result<Response, String> {
        let mut s = self.0.lock().unwrap();
        let url = url::Url::parse(&request.url).unwrap();
        assert_eq!(url.origin().ascii_serialization(), "https://wallhaven.cc");
        s.requests.push(request.clone());
        let path = url.path();
        let body = if request.method == "GET" {
            assert!(request.fields.is_empty());
            assert!(request.csrf.is_none());
            if path == "/" {
                panel(&s.owner)
            } else if path.starts_with("/w/") {
                s.token += 1;
                let owner = s.wallpaper_owner.as_ref().unwrap_or(&s.owner);
                let button = if s.favorite {
                    r#"<a id="fav-button" href="/favorite/remove/abc123">Remove</a>"#
                } else {
                    r#"<div id="fav-button" class="add-button"><a class="add-fav" href="/favorite/add/abc123">Default</a></div>"#
                };
                format!(
                    r#"{}<meta name="csrf-token" content="fresh-{}">{button}"#,
                    panel(owner),
                    s.token
                )
            } else if path == "/favorites" {
                s.token += 1;
                let rows = s.rows.iter().map(|(id, (label, public))| format!(r#"<li class="collection" data-public="{public}"><a href="/favorites/{id}"><span class="collection-label">{label}</span><small>4</small></a></li>"#)).collect::<String>();
                format!(
                    r#"{}<ul class="collections-list">{rows}</ul>{}<ul class="pagination"><a rel="next" href="/favorites/12?page=2">Images, not collection list</a></ul>"#,
                    panel(&s.owner),
                    form("/collections", s.token, "POST", true)
                )
            } else if let Some(id) = path.strip_prefix("/favorites/") {
                assert!(s.rows.contains_key(&id.parse::<u32>().unwrap()));
                s.token += 1;
                format!(
                    "{}{}{}",
                    panel(&s.owner),
                    form(&format!("/collections/{id}"), s.token, "PATCH", true),
                    form(&format!("/collections/{id}"), s.token, "DELETE", false)
                )
            } else {
                panic!("unexpected GET {path}")
            }
        } else {
            assert_eq!(request.method, "POST");
            assert_eq!(
                request.csrf,
                Some(format!("fresh-{}", s.token)),
                "write must use freshly discovered CSRF"
            );
            let fields = request.fields.iter().cloned().collect::<BTreeMap<_, _>>();
            if path.starts_with("/collections") {
                assert_eq!(fields.get("_token"), request.csrf.as_ref());
                if s.outcome != Outcome::LostUnapplied && s.outcome != Outcome::Malformed {
                    if path == "/collections" {
                        s.rows
                            .insert(13, (fields["label"].clone(), fields["public"] == "1"));
                    } else {
                        let id = path.rsplit('/').next().unwrap().parse::<u32>().unwrap();
                        match fields["_method"].as_str() {
                            "DELETE" => {
                                s.rows.remove(&id);
                            }
                            "PATCH" => {
                                s.rows
                                    .insert(id, (fields["label"].clone(), fields["public"] == "1"));
                            }
                            _ => panic!("unexpected override"),
                        }
                    }
                }
            } else {
                assert!(request.fields.is_empty());
                assert!(path.starts_with("/favorite/"));
                if s.outcome != Outcome::LostUnapplied && s.outcome != Outcome::Malformed {
                    s.favorite = path.contains("/add/");
                }
            }
            match s.outcome {
                Outcome::LostApplied | Outcome::LostUnapplied => {
                    return Err("SECRET cookie=xyz csrf=xyz <html>".into())
                }
                Outcome::Malformed => "<html>unrecognized result SECRET</html>".into(),
                Outcome::Confirmed => r#"{"status":true}"#.into(),
            }
        };
        Ok(Response {
            status: 200,
            url: request.url,
            body,
        })
    }
}
#[test]
fn create_update_delete_use_fresh_forms_and_return_refreshed_wire_list() {
    let browser = Browser::new(Outcome::Confirmed);
    let client = SiteClient::with_transport(browser.clone());
    let initial = client.read_collections("test_1").unwrap();
    assert_eq!(initial.data[0].label.as_deref(), Some("Default"));
    assert_eq!(initial.data[0].public, Some(false));
    let created = client
        .manage_collection(
            "Test_1",
            ManageOperation::Create {
                label: "New".into(),
                public: true,
            },
        )
        .unwrap();
    assert_eq!(created.data.len(), 2);
    assert_eq!(created.data[1].id, Some(13));
    assert_eq!(created.data[1].public, Some(true));
    let updated = client
        .manage_collection(
            "Test_1",
            ManageOperation::Update {
                id: 13,
                label: "Renamed".into(),
                public: false,
            },
        )
        .unwrap();
    assert_eq!(updated.data[1].label.as_deref(), Some("Renamed"));
    assert_eq!(updated.data[1].public, Some(false));
    let deleted = client
        .manage_collection("Test_1", ManageOperation::Delete { id: 13 })
        .unwrap();
    assert_eq!(deleted.data.len(), 1);
    assert_eq!(browser.posts(), 3);
    let wire = serde_json::to_value(deleted).unwrap();
    assert_eq!(wire["data"][0]["public"], false);
    assert!(wire["data"][0]["views"].is_null());
    assert!(!browser
        .0
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|r| r.url.contains("apikey") || r.url.contains("/user/")));
}
#[test]
fn human_session_and_favorite_share_exact_transport_and_noop_does_not_repost() {
    let browser = Browser::new(Outcome::Confirmed);
    let client = SiteClient::with_transport(browser.clone());
    assert_eq!(client.verify().unwrap(), "Test_1");
    let state = client.favorite_state("abc123").unwrap();
    assert!(!state.is_favorite);
    assert_eq!(
        serde_json::to_value(&state).unwrap(),
        serde_json::json!({"is_favorite":false,"targets":[{"label":"Default"}]})
    );
    assert!(
        client
            .set_favorite("Test_1", "abc123", true, 0)
            .unwrap()
            .is_favorite
    );
    assert!(
        client
            .set_favorite("Test_1", "abc123", true, 0)
            .unwrap()
            .is_favorite
    );
    assert_eq!(browser.posts(), 1, "already desired state must not repost");
    assert!(
        !client
            .set_favorite("Test_1", "abc123", false, 0)
            .unwrap()
            .is_favorite
    );
    assert_eq!(browser.posts(), 2);
    let s = browser.0.lock().unwrap();
    for (index, _) in s
        .requests
        .iter()
        .enumerate()
        .filter(|(_, r)| r.method == "POST")
    {
        assert!(s.requests[..index]
            .iter()
            .rev()
            .any(|r| r.method == "GET" && r.url == "https://wallhaven.cc/"));
    }
}
#[test]
fn owner_mismatch_and_changed_wallpaper_identity_never_post() {
    let browser = Browser::new(Outcome::Confirmed);
    let client = SiteClient::with_transport(browser.clone());
    assert_eq!(
        client.set_favorite("Other", "abc123", true, 0).unwrap_err(),
        SiteError::OwnerMismatch
    );
    assert!(client
        .manage_collection("Other", ManageOperation::Delete { id: 12 })
        .is_err());
    assert!(client.read_collections("Other").is_err());
    browser.0.lock().unwrap().wallpaper_owner = Some("Other".into());
    assert_eq!(
        client
            .set_favorite("Test_1", "abc123", true, 0)
            .unwrap_err(),
        SiteError::OwnerMismatch
    );
    assert_eq!(browser.posts(), 0);
}
#[test]
fn unknown_favorite_outcomes_never_retry_and_do_not_leak_callback_text() {
    for outcome in [
        Outcome::LostUnapplied,
        Outcome::LostApplied,
        Outcome::Malformed,
    ] {
        let browser = Browser::new(outcome);
        let client = SiteClient::with_transport(browser.clone());
        let error = client
            .set_favorite("Test_1", "abc123", true, 0)
            .unwrap_err();
        assert!(matches!(
            error,
            SiteError::UnknownOutcome | SiteError::WriteRejected
        ));
        assert!(!error.to_string().contains("SECRET"));
        assert_eq!(browser.posts(), 1);
    }
}
#[test]
fn unknown_management_results_are_reconciled_only_by_reads_never_reposted() {
    for outcome in [
        Outcome::LostUnapplied,
        Outcome::LostApplied,
        Outcome::Malformed,
    ] {
        for operation in [
            ManageOperation::Create {
                label: "New".into(),
                public: true,
            },
            ManageOperation::Update {
                id: 12,
                label: "Renamed".into(),
                public: true,
            },
            ManageOperation::Delete { id: 12 },
        ] {
            let browser = Browser::new(outcome);
            let client = SiteClient::with_transport(browser.clone());
            let result = client.manage_collection("Test_1", operation);
            if outcome == Outcome::LostApplied {
                assert!(result.is_ok(), "readback can confirm an applied write");
            } else {
                let error = result.unwrap_err();
                assert!(error.contains("未自动重试"));
                assert!(!error.contains("SECRET"));
            }
            assert_eq!(browser.posts(), 1);
        }
    }
}
#[test]
fn invalid_target_never_posts() {
    let browser = Browser::new(Outcome::Confirmed);
    let client = SiteClient::with_transport(browser.clone());
    assert_eq!(
        client
            .set_favorite("Test_1", "abc123", true, 99)
            .unwrap_err(),
        SiteError::InvalidTargetIndex
    );
    assert!(client
        .manage_collection("Test_1", ManageOperation::Delete { id: 99 })
        .is_err());
    assert_eq!(browser.posts(), 0);
}
