//! wallhaven API v1 客户端（防腐层）
//!
//! 职责：
//! - 统一 base URL、UA、超时
//! - 把 HTTP 层错误转成类型化的 ApiError
//! - 429 时自动惩罚限流器并退避重试
//! - 绝不把原始 JSON 泄漏给 UI

use crate::models::{
    CollectionsResponse, DetailResponse, SearchQuery, SearchResponse, UserSettingsResponse,
};
use crate::ratelimit::TokenBucket;
use std::sync::Arc;
use std::time::Duration;

const BASE: &str = "https://wallhaven.cc/api/v1";
const UA: &str = "WallHavenStudio/0.1 (third-party desktop client)";

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized: 401 (API key invalid or NSFW without key)")]
    Unauthorized,
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("rate limited: 429")]
    RateLimited,
    #[error("not found: 404")]
    NotFound,
    #[error("http {0}")]
    Http(u16),
    #[error("network: {0}")]
    Network(String),
    #[error("schema: {0}")]
    Schema(String),
}

#[derive(Clone)]
pub struct WallhavenClient {
    http: reqwest::blocking::Client,
    bucket: Arc<TokenBucket>,
    api_key: Option<String>,
    #[cfg(test)]
    base: String,
}

impl WallhavenClient {
    pub fn new(api_key: Option<String>) -> Result<Self, ApiError> {
        Self::with_bucket(api_key, Arc::new(TokenBucket::wallhaven()))
    }

    pub fn with_bucket(
        api_key: Option<String>,
        bucket: Arc<TokenBucket>,
    ) -> Result<Self, ApiError> {
        let http = reqwest::blocking::Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        Ok(Self {
            http,
            bucket,
            api_key,
            #[cfg(test)]
            base: BASE.to_string(),
        })
    }

    pub fn set_api_key(&mut self, key: Option<String>) {
        self.api_key = key;
    }

    pub fn bucket(&self) -> Arc<TokenBucket> {
        self.bucket.clone()
    }

    fn active_api_key(&self) -> Option<&str> {
        self.api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
    }

    fn require_api_key(&self) -> Result<(), ApiError> {
        self.active_api_key()
            .map(|_| ())
            .ok_or(ApiError::Unauthorized)
    }

    #[cfg(not(test))]
    fn base(&self) -> &str {
        BASE
    }

    #[cfg(test)]
    fn base(&self) -> &str {
        &self.base
    }

    #[cfg(test)]
    fn set_test_base(&mut self, base: String) {
        self.base = base;
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, ApiError> {
        let api_key = self
            .active_api_key()
            .map(|key| {
                reqwest::header::HeaderValue::from_str(key)
                    .map_err(|_| ApiError::InvalidInput("invalid API key header value".to_string()))
            })
            .transpose()?;

        // 最多重试 3 次（应对 429）
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.bucket.acquire();

            let mut req = self.http.get(url);
            if let Some(key) = &api_key {
                req = req.header("X-API-Key", key);
            }

            match req.send() {
                Ok(resp) => {
                    let code = resp.status().as_u16();
                    match code {
                        200..=299 => {
                            let text = resp.text().map_err(|e| ApiError::Network(e.to_string()))?;
                            return serde_json::from_str::<T>(&text)
                                .map_err(|e| ApiError::Schema(e.to_string()));
                        }
                        401 => return Err(ApiError::Unauthorized),
                        404 => return Err(ApiError::NotFound),
                        429 => {
                            self.bucket
                                .penalize(Duration::from_secs(5 * attempt as u64));
                            if attempt >= 3 {
                                return Err(ApiError::RateLimited);
                            }
                            std::thread::sleep(Duration::from_secs(2 * attempt as u64));
                            continue;
                        }
                        other => return Err(ApiError::Http(other)),
                    }
                }
                Err(e) => {
                    if attempt >= 3 {
                        return Err(ApiError::Network(e.to_string()));
                    }
                    std::thread::sleep(Duration::from_millis(500 * attempt as u64));
                }
            }
        }
    }

    /// 搜索/列表
    pub fn search(&self, query: &SearchQuery, page: u32) -> Result<SearchResponse, ApiError> {
        let params = query.to_params(page);
        let qs: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let url = format!("{}/search?{qs}", self.base());
        self.get_json(&url)
    }

    /// 单图详情（含 tags / uploader）
    pub fn wallpaper(&self, id: &str) -> Result<DetailResponse, ApiError> {
        let url = format!("{}/w/{}", self.base(), urlencode(id));
        self.get_json(&url)
    }

    /// Authenticated account settings.
    pub fn user_settings(&self) -> Result<UserSettingsResponse, ApiError> {
        self.require_api_key()?;
        self.get_json(&format!("{}/settings", self.base()))
    }

    /// Collection summaries for the authenticated account or a public user.
    pub fn collections(&self, username: Option<&str>) -> Result<CollectionsResponse, ApiError> {
        let url = match username {
            Some(username) => {
                let username = username.trim();
                validate_username(username).map_err(ApiError::InvalidInput)?;
                format!("{}/collections/{}", self.base(), urlencode(username))
            }
            None => {
                self.require_api_key()?;
                format!("{}/collections", self.base())
            }
        };
        self.get_json(&url)
    }

    /// Wallpapers in a public collection. Wallhaven supports only the purity filter here.
    pub fn collection_wallpapers(
        &self,
        username: &str,
        id: u32,
        purity: &str,
        page: u32,
    ) -> Result<SearchResponse, ApiError> {
        let username = username.trim();
        validate_username(username).map_err(ApiError::InvalidInput)?;
        if id == 0 {
            return Err(ApiError::InvalidInput(
                "collection id must be nonzero".to_string(),
            ));
        }
        if page == 0 {
            return Err(ApiError::InvalidInput("page must be nonzero".to_string()));
        }
        validate_purity(purity).map_err(ApiError::InvalidInput)?;

        let url = format!(
            "{}/collections/{}/{}?page={}&purity={}",
            self.base(),
            urlencode(username),
            id,
            page,
            purity
        );
        self.get_json(&url)
    }

    /// 下载原图字节（不占 API 配额，不经过令牌桶）
    pub fn download(&self, url: &str) -> Result<Vec<u8>, ApiError> {
        let resp = self
            .http
            .get(url)
            .send()
            .map_err(|e| ApiError::Network(e.to_string()))?;

        let code = resp.status().as_u16();
        if !(200..=299).contains(&code) {
            return Err(ApiError::Http(code));
        }
        resp.bytes()
            .map(|b| b.to_vec())
            .map_err(|e| ApiError::Network(e.to_string()))
    }
}

/// Validate a Wallhaven username before placing it in a URL path.
pub fn validate_username(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 64 {
        return Err("username must be 1 to 64 ASCII characters".to_string());
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(
            "username may contain only ASCII letters, digits, underscores, and hyphens".to_string(),
        );
    }
    Ok(())
}

fn validate_purity(value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.len() != 3 || !bytes.iter().all(|b| matches!(b, b'0' | b'1')) || value == "000" {
        return Err("purity must be exactly three bits and cannot be 000".to_string());
    }
    Ok(())
}

/// 最小 URL 编码（避免额外依赖）
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            b'+' => out.push_str("%2B"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_handles_specials() {
        assert_eq!(urlencode("16x9"), "16x9");
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("+tag"), "%2Btag");
        assert_eq!(urlencode("-tag"), "-tag");
        assert_eq!(urlencode("a&b"), "a%26b");
        assert_eq!(urlencode("#66cccc"), "%2366cccc"); // 颜色参数
    }

    #[test]
    fn client_constructs() {
        let c = WallhavenClient::new(None);
        assert!(c.is_ok());
    }

    #[test]
    fn username_validation_rejects_path_and_query_injection() {
        for valid in ["a", "User_123", "user-name", &"x".repeat(64)] {
            assert!(validate_username(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "",
            " user",
            "user ",
            "user/name",
            "user?key=value",
            "user#fragment",
            "user.name",
            "控制",
            "line\nbreak",
            &"x".repeat(65),
        ] {
            assert!(validate_username(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn collection_urls_are_exact_and_inputs_are_validated_before_network() {
        let mut client = WallhavenClient::new(Some("key".to_string())).unwrap();
        client.set_test_base("http://127.0.0.1:1/api/v1".to_string());

        for (id, purity, page) in [
            (0, "100", 1),
            (15, "000", 1),
            (15, "10", 1),
            (15, "10x", 1),
            (15, "100", 0),
        ] {
            assert!(matches!(
                client.collection_wallpapers("valid-user", id, purity, page),
                Err(ApiError::InvalidInput(_))
            ));
        }
        assert!(matches!(
            client.collection_wallpapers("../escape", 15, "100", 1),
            Err(ApiError::InvalidInput(_))
        ));
        assert!(matches!(
            client.collections(Some("user/name")),
            Err(ApiError::InvalidInput(_))
        ));
    }

    #[test]
    fn missing_or_blank_key_is_explicitly_unauthorized() {
        for key in [None, Some(String::new()), Some("   ".to_string())] {
            let client = WallhavenClient::new(key).unwrap();
            assert!(matches!(
                client.user_settings(),
                Err(ApiError::Unauthorized)
            ));
            assert!(matches!(
                client.collections(None),
                Err(ApiError::Unauthorized)
            ));
        }
    }

    #[test]
    fn invalid_api_key_header_error_never_contains_the_key() {
        let secret = "secret\r\nInjected: value";
        let client = WallhavenClient::new(Some(secret.to_string())).unwrap();
        let error = client.user_settings().unwrap_err().to_string();
        assert!(!error.contains(secret));
        assert_eq!(error, "invalid input: invalid API key header value");
    }

    #[test]
    fn account_requests_use_expected_paths_queries_and_api_key_header() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        fn serve_once(body: &'static str) -> (String, std::thread::JoinHandle<String>) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let handle = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
                String::from_utf8(request).unwrap()
            });
            (format!("http://{address}/api/v1"), handle)
        }

        let (base, handle) = serve_once(r#"{"data":[]}"#);
        let mut client = WallhavenClient::new(None).unwrap();
        client.set_test_base(base);
        client.collections(Some(" Public_User ")).unwrap();
        let request = handle.join().unwrap();
        assert!(request.starts_with("GET /api/v1/collections/Public_User HTTP/1.1\r\n"));
        assert!(!request.to_ascii_lowercase().contains("x-api-key:"));

        let (base, handle) = serve_once(r#"{"data":[]}"#);
        client.set_test_base(base);
        client.set_api_key(Some("test-key".to_string()));
        client.collections(None).unwrap();
        let request = handle.join().unwrap();
        assert!(request.starts_with("GET /api/v1/collections HTTP/1.1\r\n"));
        assert!(request
            .to_ascii_lowercase()
            .contains("x-api-key: test-key\r\n"));

        let (base, handle) = serve_once(r#"{"data":[],"meta":{}}"#);
        client.set_test_base(base);
        client
            .collection_wallpapers("Public_User", 15, "101", 2)
            .unwrap();
        let request = handle.join().unwrap();
        assert!(request
            .starts_with("GET /api/v1/collections/Public_User/15?page=2&purity=101 HTTP/1.1\r\n"));

        let (base, handle) = serve_once(r#"{"data":{"per_page":"24"}}"#);
        client.set_test_base(base);
        assert_eq!(
            client.user_settings().unwrap().data.unwrap().per_page,
            Some(24)
        );
        let request = handle.join().unwrap();
        assert!(request.starts_with("GET /api/v1/settings HTTP/1.1\r\n"));
    }

    // 需要联网的集成测试，默认忽略
    #[test]
    #[ignore]
    fn live_search_works() {
        let c = WallhavenClient::new(None).unwrap();
        let q = SearchQuery::default();
        let r = c.search(&q, 1).unwrap();
        assert_eq!(r.data.len(), 24);
    }
}
