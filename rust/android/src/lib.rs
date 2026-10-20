//! Blocking, platform-neutral UniFFI bridge. Call network methods off the UI thread.
mod cache;
mod downloads;
pub use downloads::{DownloadObserver, MobileDownloads};
mod image_format;
mod profile;
mod site_bridge;
mod urls;

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use wh_core::{ApiError, SearchQuery, WallhavenClient};

uniffi::setup_scaffolding!();

#[cfg(target_os = "android")]
mod android_tls {
    use jni::{
        errors::ThrowRuntimeExAndDefault,
        jni_mangle,
        objects::{JClass, JObject},
        EnvUnowned,
    };
    #[jni_mangle("io.github.nuxoul.nxwallhaven.NxApplication")]
    pub fn initTls<'caller>(
        mut unowned_env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        context: JObject<'caller>,
    ) {
        unowned_env
            .with_env(|env| rustls_platform_verifier::android::init_with_env(env, context))
            .resolve::<ThrowRuntimeExAndDefault>();
    }
}

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum MobileError {
    #[error("invalid input: {reason}")]
    InvalidInput { reason: String },
    #[error("an API key is required for NSFW content")]
    Unauthorized,
    #[error("resource not found")]
    NotFound,
    #[error("rate limited; retry later")]
    RateLimited,
    #[error("HTTP {status}")]
    Http { status: u16 },
    #[error("network request failed")]
    Network,
    #[error("invalid or incomplete response")]
    Response,
    #[error("cache operation failed")]
    Cache,
}

impl From<ApiError> for MobileError {
    fn from(error: ApiError) -> Self {
        // Never forward reqwest URLs, headers, server bodies, or caller credentials.
        match error {
            ApiError::Unauthorized => Self::Unauthorized,
            ApiError::NotFound => Self::NotFound,
            ApiError::RateLimited => Self::RateLimited,
            ApiError::Http(status) => Self::Http { status },
            ApiError::InvalidInput(_) => invalid("invalid API request"),
            ApiError::Network(_) => Self::Network,
            ApiError::Schema(_) => Self::Response,
        }
    }
}

pub(crate) fn invalid(message: &str) -> MobileError {
    MobileError::InvalidInput {
        reason: message.into(),
    }
}
fn json(value: &impl Serialize) -> Result<String, MobileError> {
    serde_json::to_string(value).map_err(|_| MobileError::Response)
}
fn key(value: Option<String>) -> Result<Option<String>, MobileError> {
    let value = value.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    if let Some(value) = &value {
        reqwest::header::HeaderValue::from_str(value).map_err(|_| invalid("invalid API key"))?;
    }
    Ok(value)
}
fn bits(value: &str) -> bool {
    value.len() == 3 && value != "000" && value.bytes().all(|b| matches!(b, b'0' | b'1'))
}
fn purity(value: &str, key: &Option<String>) -> Result<(), MobileError> {
    if !bits(value) {
        return Err(invalid("purity must be three bits and cannot be 000"));
    }
    if value.as_bytes()[2] == b'1' && key.is_none() {
        return Err(MobileError::Unauthorized);
    }
    Ok(())
}
fn username(value: &str) -> Result<(), MobileError> {
    wh_core::validate_username(value).map_err(|_| invalid("invalid username"))
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Query {
    q: String,
    categories: String,
    purity: String,
    sorting: String,
    order: String,
    atleast: Option<String>,
    resolutions: Option<String>,
    ratios: Option<String>,
    colors: Option<String>,
    top_range: Option<String>,
    seed: Option<String>,
    page: u32,
}
impl Default for Query {
    fn default() -> Self {
        let q = SearchQuery::default();
        Self {
            q: q.q,
            categories: q.categories,
            purity: q.purity,
            sorting: q.sorting,
            order: q.order,
            atleast: q.atleast,
            resolutions: None,
            ratios: q.ratios,
            colors: q.colors,
            top_range: q.top_range,
            seed: q.seed,
            page: 1,
        }
    }
}
impl Query {
    fn parse(input: &str, key: &Option<String>) -> Result<Self, MobileError> {
        if input.len() > 64 * 1024 {
            return Err(invalid("search query too large"));
        }
        let q: Self = serde_json::from_str(input).map_err(|_| invalid("invalid search JSON"))?;
        purity(&q.purity, key)?;
        if !bits(&q.categories) || q.page == 0 {
            return Err(invalid("invalid categories or page"));
        }
        if ![
            "date_added",
            "relevance",
            "random",
            "views",
            "favorites",
            "toplist",
            "hot",
        ]
        .contains(&q.sorting.as_str())
            || !["asc", "desc"].contains(&q.order.as_str())
        {
            return Err(invalid("invalid sorting or order"));
        }
        // wh-core currently exposes `atleast`, not the API's exact-resolutions filter.
        // Explicit rejection is safer than silently broadening an exact filter.
        if q.resolutions.as_deref().is_some_and(|s| !s.is_empty()) {
            return Err(invalid(
                "exact resolutions are not supported by this wh-core version; use atleast",
            ));
        }
        Ok(q)
    }
    fn core(self) -> SearchQuery {
        SearchQuery {
            q: self.q,
            categories: self.categories,
            purity: self.purity,
            sorting: self.sorting,
            order: self.order,
            top_range: self.top_range,
            atleast: self.atleast,
            ratios: self.ratios,
            colors: self.colors,
            seed: self.seed,
        }
    }
}

#[derive(uniffi::Object)]
pub struct MobileCore {
    api: WallhavenClient,
    profile: profile::ProfileClient,
    cache: cache::ImageCache,
}

#[uniffi::export]
impl MobileCore {
    #[uniffi::constructor]
    pub fn new(cache_dir: String) -> Result<Arc<Self>, MobileError> {
        let api = WallhavenClient::new(None)?;
        Ok(Arc::new(Self {
            profile: profile::ProfileClient::new(api.bucket())?,
            api,
            cache: cache::ImageCache::new(&cache_dir)?,
        }))
    }
    pub fn ping(&self) -> String {
        "pong".into()
    }
    pub fn search(&self, query_json: String, key: Option<String>) -> Result<String, MobileError> {
        let key = key_fn(key)?;
        let query = Query::parse(&query_json, &key)?;
        let page = query.page;
        let mut api = self.api.clone();
        api.set_api_key(key);
        json(&api.search(&query.core(), page)?)
    }
    pub fn detail(&self, id: String, key: Option<String>) -> Result<String, MobileError> {
        if id.len() != 6 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(invalid("wallpaper id must be six ASCII letters or digits"));
        }
        let mut api = self.api.clone();
        api.set_api_key(key_fn(key)?);
        json(&api.wallpaper(&id)?)
    }
    pub fn public_profile(&self, username: String) -> Result<String, MobileError> {
        json(&self.profile.get(&username)?)
    }
    pub fn collections(
        &self,
        username: String,
        key: Option<String>,
    ) -> Result<String, MobileError> {
        username_fn(&username)?;
        let mut api = self.api.clone();
        api.set_api_key(key_fn(key)?);
        json(&api.collections(Some(&username))?)
    }
    pub fn collection_page(
        &self,
        username: String,
        id: u32,
        page: u32,
        purity: String,
        key: Option<String>,
    ) -> Result<String, MobileError> {
        username_fn(&username)?;
        let key = key_fn(key)?;
        purity_fn(&purity, &key)?;
        if id == 0 || page == 0 {
            return Err(invalid("collection id and page must be nonzero"));
        }
        let mut api = self.api.clone();
        api.set_api_key(key);
        json(&api.collection_wallpapers(&username, id, &purity, page)?)
    }
    pub fn acquire_image(&self, url: String) -> Result<String, MobileError> {
        self.cache.acquire(&url)
    }
    pub fn cache_usage(&self) -> Result<String, MobileError> {
        json(&self.cache.usage()?)
    }
    pub fn clear_cache(&self) -> Result<(), MobileError> {
        self.cache.clear()
    }
}
// Aliases avoid shadowing exported argument names (which form the UniFFI ABI).
use key as key_fn;
use purity as purity_fn;
use username as username_fn;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auth_and_purity_are_explicit() {
        for p in ["", "000", "11", "1111", "10x", " 100"] {
            assert!(purity(p, &None).is_err());
        }
        for p in ["100", "010", "110"] {
            assert!(purity(p, &None).is_ok());
        }
        for p in ["001", "101", "011", "111"] {
            assert!(matches!(purity(p, &None), Err(MobileError::Unauthorized)));
            assert!(purity(p, &key(Some(" explicit ".into())).unwrap()).is_ok());
        }
        assert!(key(Some("  ".into())).unwrap().is_none());
        assert!(key(Some("secret\r\nheader".into())).is_err());
        let q = Query::parse("{}", &Some("key".into())).unwrap();
        assert_eq!(q.purity, "100"); // A key never silently enables NSFW.
        assert_eq!(q.page, 1);
        assert!(Query::parse(r#"{"purity":"001"}"#, &None).is_err());
        assert!(Query::parse(r#"{"page":0}"#, &None).is_err());
        assert!(Query::parse(r#"{"apikey":"secret"}"#, &None).is_err());
    }
    #[test]
    fn bridge_validation_is_offline_and_errors_are_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let core = MobileCore::new(dir.path().to_string_lossy().into()).unwrap();
        assert_eq!(core.ping(), "pong");
        assert!(core
            .collection_page("test".into(), 1, 1, "001".into(), None)
            .is_err());
        assert!(core.collections("../bad".into(), None).is_err());
        assert!(core.detail("bad/id".into(), None).is_err());
        assert!(core.public_profile("bad/user".into()).is_err());
        let e: MobileError =
            ApiError::Network("https://x/?apikey=secret X-API-Key: secret".into()).into();
        assert!(!e.to_string().contains("secret"));
    }
}
