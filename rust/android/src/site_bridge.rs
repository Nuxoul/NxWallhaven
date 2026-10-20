//! UniFFI website-session boundary. The foreign callback must use only the
//! user's session WebView, must enforce same-origin redirects, and must never
//! automatically retry POSTs. No reqwest/cookie fallback is provided.
use crate::MobileError;
use serde::Serialize;
use std::sync::Arc;
use wh_site::{ManageOperation, Request, Response, SiteClient, SiteError, SiteTransport};

#[uniffi::export(with_foreign)]
pub trait BrowserTransport: Send + Sync {
    fn request(&self, request_json: String) -> Result<String, MobileError>;
}
struct CallbackTransport(Arc<dyn BrowserTransport>);
impl SiteTransport for CallbackTransport {
    fn request(&self, request: Request) -> Result<Response, String> {
        let request_json = serde_json::to_string(&request).map_err(|_| "请求编码失败")?;
        // Never expose the callback's arbitrary error text to callers.
        let response_json = self
            .0
            .request(request_json)
            .map_err(|_| "浏览器会话请求失败")?;
        // A 2 MiB UTF-8 body may use six JSON bytes per escaped ASCII byte.
        if response_json.len() > 12 * 1024 * 1024 + 64 * 1024 {
            return Err("浏览器响应超过大小限制".into());
        }
        // wh-site additionally validates final URL, HTTP status and decoded body <= 2 MiB.
        serde_json::from_str(&response_json).map_err(|_| "浏览器响应格式无效".into())
    }
}
fn site_error(error: SiteError) -> MobileError {
    safe_error(error.to_string())
}
fn safe_error(reason: String) -> MobileError {
    MobileError::InvalidInput { reason }
}
fn json(value: &impl Serialize) -> Result<String, MobileError> {
    serde_json::to_string(value).map_err(|_| crate::invalid("网站结果编码失败"))
}

#[derive(uniffi::Object)]
pub struct MobileSite {
    client: SiteClient,
}
#[uniffi::export]
impl MobileSite {
    #[uniffi::constructor]
    pub fn new(transport: Arc<dyn BrowserTransport>) -> Arc<Self> {
        Arc::new(Self {
            client: SiteClient::with_transport(Arc::new(CallbackTransport(transport))),
        })
    }
    /// Returns the freshly observed username, not JSON.
    pub fn verify(&self) -> Result<String, MobileError> {
        self.client.verify().map_err(site_error)
    }
    pub fn collections(&self, owner: String) -> Result<String, MobileError> {
        json(&self.client.read_collections(&owner).map_err(safe_error)?)
    }
    pub fn collection_snapshot(&self) -> Result<String, MobileError> {
        json(&self.client.collection_snapshot().map_err(safe_error)?)
    }
    pub fn collection_page(
        &self,
        owner: String,
        id: u32,
        purity: String,
        page: u32,
        allow_nsfw: bool,
        key_present: bool,
    ) -> Result<String, MobileError> {
        if !crate::bits(&purity) {
            return Err(crate::invalid(
                "purity must be three bits and cannot be 000",
            ));
        }
        if purity.as_bytes()[2] == b'1' && !(allow_nsfw && key_present) {
            return Err(crate::invalid(
                "NSFW 收藏内容需要明确允许且当前已配置 API key",
            ));
        }
        json(
            &self
                .client
                .read_collection_wallpapers(&owner, id, &purity, page)
                .map_err(safe_error)?,
        )
    }
    pub fn favorite_state(&self, id: String) -> Result<String, MobileError> {
        json(&self.client.favorite_state(&id).map_err(site_error)?)
    }
    pub fn favorite_snapshot(&self, id: String) -> Result<String, MobileError> {
        json(&self.client.favorite_snapshot(&id).map_err(site_error)?)
    }
    pub fn set_favorite_target(
        &self,
        owner: String,
        id: String,
        desired: bool,
        label: String,
    ) -> Result<String, MobileError> {
        json(
            &self
                .client
                .set_favorite_target(&owner, &id, desired, &label)
                .map_err(site_error)?,
        )
    }
    pub fn set_favorite(
        &self,
        owner: String,
        id: String,
        desired: bool,
        target_index: u32,
    ) -> Result<String, MobileError> {
        let index = usize::try_from(target_index).map_err(|_| crate::invalid("收藏夹索引无效"))?;
        json(
            &self
                .client
                .set_favorite(&owner, &id, desired, index)
                .map_err(site_error)?,
        )
    }
    pub fn manage_collection(
        &self,
        owner: String,
        operation_json: String,
    ) -> Result<String, MobileError> {
        if operation_json.len() > 16 * 1024 {
            return Err(crate::invalid("收藏夹操作过大"));
        }
        let operation: ManageOperation = serde_json::from_str(&operation_json)
            .map_err(|_| crate::invalid("收藏夹操作 JSON 无效"))?;
        json(
            &self
                .client
                .manage_collection(&owner, operation)
                .map_err(safe_error)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };
    struct Fixture {
        calls: AtomicUsize,
        response: Mutex<Result<String, MobileError>>,
    }
    impl BrowserTransport for Fixture {
        fn request(&self, request_json: String) -> Result<String, MobileError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let request: serde_json::Value = serde_json::from_str(&request_json).unwrap();
            assert_eq!(request["method"], "GET");
            assert!(request["url"]
                .as_str()
                .unwrap()
                .starts_with("https://wallhaven.cc/"));
            assert_eq!(request["fields"], serde_json::json!([]));
            assert!(request["csrf"].is_null());
            self.response.lock().unwrap().clone()
        }
    }
    fn fixture(body: &str, url: &str, status: u16) -> Arc<Fixture> {
        Arc::new(Fixture {
            calls: AtomicUsize::new(0),
            response: Mutex::new(Ok(
                serde_json::json!({"status":status,"url":url,"body":body}).to_string(),
            )),
        })
    }
    #[test]
    fn verify_returns_plain_username_and_callback_wire() {
        let transport = fixture(
            r#"<div id="userpanel"><a href="/user/Alice">Alice</a><form action="/logout"></form></div>"#,
            "https://wallhaven.cc/",
            200,
        );
        assert_eq!(MobileSite::new(transport).verify().unwrap(), "Alice");
    }
    #[test]
    fn explicit_nsfw_consent_and_current_key_are_both_required_before_callback() {
        let transport = fixture("", "https://wallhaven.cc/", 200);
        let site = MobileSite::new(transport.clone());
        for (allow, key) in [(false, false), (true, false), (false, true)] {
            assert!(matches!(
                site.collection_page("Alice".into(), 12, "111".into(), 1, allow, key),
                Err(MobileError::InvalidInput { .. })
            ));
        }
        for purity in ["000", "11", "1é", "xx1"] {
            assert!(site
                .collection_page("Alice".into(), 12, purity.into(), 1, true, true)
                .is_err());
        }
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn content_purity_is_filtered_and_nsfw_requires_both_flags() {
        let body = r#"<div id="userpanel"><a href="/user/Alice">Alice</a><form action="/logout"></form></div><div id="thumbs"><figure class="thumb-sfw" data-wallpaper-id="abc123"></figure><figure class="thumb-nsfw" data-wallpaper-id="def456"></figure><figure data-wallpaper-id="xyz789"></figure></div>"#;
        let transport = fixture(body, "https://wallhaven.cc/favorites/12", 200);
        let site = MobileSite::new(transport);
        let sfw: serde_json::Value = serde_json::from_str(
            &site
                .collection_page("Alice".into(), 12, "100".into(), 1, false, false)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(sfw["data"].as_array().unwrap().len(), 1);
        assert_eq!(sfw["data"][0]["purity"], "sfw");
        let opted_in: serde_json::Value = serde_json::from_str(
            &site
                .collection_page("Alice".into(), 12, "111".into(), 1, true, true)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(opted_in["data"].as_array().unwrap().len(), 2);
        assert_eq!(opted_in["data"][1]["purity"], "nsfw");
    }
    #[test]
    fn collection_summary_keeps_unknown_visibility_nullable_without_key() {
        let body = r#"<div id="userpanel"><a href="/user/Alice">Alice</a><form action="/logout"></form></div><ul class="collections-list"><li class="collection"><a href="/favorites/12"><span class="collection-label">Default</span></a></li></ul>"#;
        let site = MobileSite::new(fixture(body, "https://wallhaven.cc/favorites", 200));
        let value: serde_json::Value =
            serde_json::from_str(&site.collections("Alice".into()).unwrap()).unwrap();
        assert_eq!(value["data"][0]["label"], "Default");
        assert!(value["data"][0].as_object().unwrap().contains_key("public"));
        assert!(value["data"][0]["public"].is_null());
    }
    #[test]
    fn callback_errors_and_invalid_responses_are_sanitized() {
        let transport = fixture("", "https://wallhaven.cc/", 200);
        *transport.response.lock().unwrap() = Err(crate::invalid("SECRET_COOKIE_CSRF_HTML"));
        let site = MobileSite::new(transport.clone());
        let error = site.verify().unwrap_err();
        assert!(matches!(error, MobileError::InvalidInput { .. }));
        assert!(!error.to_string().contains("SECRET"));
        for (url, status, body) in [
            ("https://evil.test/", 200, "SECRET"),
            ("https://wallhaven.cc/", 600, "SECRET"),
            ("https://wallhaven.cc/", 403, "SECRET"),
        ] {
            *transport.response.lock().unwrap() =
                Ok(serde_json::json!({"url":url,"status":status,"body":body}).to_string());
            assert!(!site.verify().unwrap_err().to_string().contains("SECRET"));
        }
        *transport.response.lock().unwrap() = Ok(serde_json::json!({"url":"https://wallhaven.cc/","status":200,"body":"x".repeat(2*1024*1024+1)}).to_string());
        assert!(site.verify().unwrap_err().to_string().contains("大小限制"));
    }
    #[test]
    fn invalid_operation_json_never_calls_transport() {
        let transport = fixture("", "https://wallhaven.cc/", 200);
        let site = MobileSite::new(transport.clone());
        for operation in [
            r#"{"kind":"unknown"}"#,
            r#"{"kind":"delete","id":12,"csrf":"SECRET"}"#,
            r#"{"kind":"create","label":"a"}"#,
        ] {
            assert!(site
                .manage_collection("Alice".into(), operation.into())
                .is_err());
        }
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }
}
