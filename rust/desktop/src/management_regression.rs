//! Complete in-memory website fixture: no real account or network mutation.
use super::*;
use crate::browser_session::BrowserResponse;
use crate::website::SessionTransport;
use std::sync::Mutex;
struct Fixture {
    rows: Mutex<Vec<(u32, String, bool)>>,
    posts: std::sync::atomic::AtomicUsize,
}
impl Fixture {
    fn response(&self, path: &str) -> BrowserResponse {
        let rows = self.rows.lock().unwrap();
        let listing = if path.contains("page=2") {
            rows.iter().skip(1).collect::<Vec<_>>()
        } else {
            rows.iter().take(1).collect::<Vec<_>>()
        };
        let mut html = String::from(
            r#"<div id="userpanel"><a href="/user/Test">Test</a><form action="/logout"></form></div><meta name="csrf-token" content="fixture"><div id="collections">"#,
        );
        for (id, label, public) in listing {
            html.push_str(&format!(r#"<a class="collection" data-public="{}" href="/favorites/{}"><span class="collection-label">{}</span><small>2</small></a>"#,u8::from(*public),id,label));
        }
        html.push_str("</div>");
        if path == "/favorites" {
            html.push_str(r#"<div class="collections-pagination"><a rel="next" href="/favorites?page=2">Next</a></div><form action="/collections/create" method="post"><input type="hidden" name="_token" value="fixture"><input name="label"><input name="public"></form>"#);
        }
        if let Some(id) = path
            .strip_prefix("/favorites/")
            .and_then(|s| s.parse::<u32>().ok())
        {
            html.push_str(&format!(r#"<form action="/collections/{id}" method="post"><input type="hidden" name="_token" value="fixture"><input type="hidden" name="_method" value="PATCH"><input name="label"><input name="public"></form><form action="/collections/{id}/delete" method="post"><input type="hidden" name="_token" value="fixture"><input type="hidden" name="_method" value="DELETE"></form>"#));
        }
        BrowserResponse {
            status: 200,
            url: format!("https://wallhaven.cc{path}"),
            body: html,
        }
    }
}
impl SessionTransport for Fixture {
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
        Ok(self.response("/").body)
    }
    fn get(&self, path: &str) -> Result<BrowserResponse, String> {
        assert!(
            !path.ends_with("/delete"),
            "delete route was fetched as GET"
        );
        Ok(self.response(path))
    }
    fn post(&self, _: &str, _: &str) -> Result<BrowserResponse, String> {
        Err("not used".into())
    }
    fn post_form(
        &self,
        url: &str,
        fields: &[(String, String)],
        csrf: &str,
    ) -> Result<BrowserResponse, String> {
        assert_eq!(csrf, "fixture");
        self.posts.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let field = |key: &str| {
            fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let mut rows = self.rows.lock().unwrap();
        if url.ends_with("/create") {
            rows.push((14, field("label"), field("public") == "1"));
        } else {
            let id = url.split('/').find_map(|s| s.parse::<u32>().ok()).unwrap();
            if field("_method") == "DELETE" {
                rows.retain(|row| row.0 != id);
            } else {
                let row = rows.iter_mut().find(|row| row.0 == id).unwrap();
                row.1 = field("label");
                row.2 = field("public") == "1";
            }
        }
        // The site has applied the write, but a redirect/network failure hides the response.
        Err("fixture response lost".into())
    }
}
#[test]
fn complete_collection_management_refetches_and_never_reposts_unknown_write() {
    use crate::collection_management::ManageOperation;
    let fixture = Arc::new(Fixture {
        rows: Mutex::new(vec![
            (12, "Existing".into(), false),
            (13, "第二页 收藏夹".into(), true),
        ]),
        posts: Default::default(),
    });
    let site = website::SiteClient::from_test_transport(fixture.clone());
    assert_eq!(site.read_collections("Test").unwrap().data.len(), 2);
    site.manage_collection(
        "Test",
        ManageOperation::Create {
            label: "新收藏夹".into(),
            public: true,
        },
    )
    .unwrap();
    site.manage_collection(
        "Test",
        ManageOperation::Update {
            id: 14,
            label: "重命名 收藏".into(),
            public: false,
        },
    )
    .unwrap();
    let rows = site.read_collections("Test").unwrap();
    assert_eq!(rows.data.len(), 3);
    assert_eq!(
        rows.data
            .iter()
            .find(|row| row.id == Some(14))
            .unwrap()
            .label
            .as_deref(),
        Some("重命名 收藏")
    );
    site.manage_collection("Test", ManageOperation::Delete { id: 14 })
        .unwrap();
    assert_eq!(site.read_collections("Test").unwrap().data.len(), 2);
    assert_eq!(fixture.posts.load(std::sync::atomic::Ordering::Acquire), 3);
}
