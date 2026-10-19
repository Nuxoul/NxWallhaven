//! Credential-free, bounded public uploader reads. Never uses the website session.
use scraper::{ElementRef, Html, Selector};
use std::io::Read;
use std::time::Duration;
use wh_core::{CollectionsResponse, SearchQuery, SearchResponse, WallhavenClient, Wallpaper};

#[derive(Debug)]
pub(super) struct PublicProfile {
    pub username: String,
    pub group: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub fields: Vec<(String, String)>,
}
#[derive(Clone)]
pub(super) struct ProfileClient {
    api: WallhavenClient,
    http: reqwest::blocking::Client,
}
impl ProfileClient {
    pub(super) fn new(mut api: WallhavenClient) -> Result<Self, String> {
        api.set_api_key(None);
        let http = reqwest::blocking::Client::builder()
            .user_agent("WallHavenStudio/0.1 (third-party desktop client)")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() > 3 || !same_origin(attempt.url()) {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|_| "无法创建公开资料读取通道".to_string())?;
        Ok(Self { api, http })
    }
    pub(super) fn profile(&self, username: &str) -> Result<PublicProfile, String> {
        wh_core::validate_username(username)?;
        self.api.bucket().acquire();
        let response = self
            .http
            .get(format!("https://wallhaven.cc/user/{username}"))
            .send()
            .map_err(|_| "公开资料网络连接失败，请稍后刷新".to_string())?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(format!(
                "公开资料读取失败（HTTP {}），可能需要稍后重试或网站验证",
                response.status().as_u16()
            ));
        }
        if !same_origin(response.url()) {
            return Err("公开资料跳转到不支持的地址".into());
        }
        let mut bytes = Vec::new();
        response
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "公开资料响应读取失败".to_string())?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err("公开资料超过大小上限".into());
        }
        let html = String::from_utf8(bytes).map_err(|_| "公开资料编码不受支持".to_string())?;
        parse_profile(&html, username)
    }
    pub(super) fn uploads(
        &self,
        username: &str,
        page: u32,
        purity: &str,
        key: Option<String>,
    ) -> Result<SearchResponse, String> {
        validate_page(username, page)?;
        let purity = validate_purity(purity)?;
        self.content_api(purity, key)
            .search(
                &SearchQuery {
                    q: format!("@{username}"),
                    purity: purity.into(),
                    ..Default::default()
                },
                page,
            )
            .map_err(|e| e.to_string())
    }
    pub(super) fn collections(&self, username: &str) -> Result<CollectionsResponse, String> {
        wh_core::validate_username(username)?;
        self.api
            .collections(Some(username))
            .map_err(|e| e.to_string())
    }
    pub(super) fn collection_page(
        &self,
        username: &str,
        id: u32,
        page: u32,
        purity: &str,
        key: Option<String>,
    ) -> Result<SearchResponse, String> {
        validate_page(username, page)?;
        let purity = validate_purity(purity)?;
        self.content_api(purity, key)
            .collection_wallpapers(username, id, purity, page)
            .map_err(|e| e.to_string())
    }
    fn content_api(&self, purity: &str, key: Option<String>) -> WallhavenClient {
        let mut api = self.api.clone();
        // This clone is key-free; only explicit NSFW content can gain a key.
        if purity.as_bytes()[2] == b'1' {
            api.set_api_key(key);
        }
        api
    }
}
/// Key selection is made from current settings for each content request.
pub(super) fn content_key(settings: &crate::config::Settings, purity: &str) -> Option<String> {
    if !settings.nsfw_enabled || !settings.has_api_key() || purity.as_bytes().get(2) != Some(&b'1')
    {
        return None;
    }
    settings
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}
fn validate_purity(value: &str) -> Result<&str, String> {
    if value.len() == 3 && value.bytes().all(|b| b == b'0' || b == b'1') && value != "000" {
        Ok(value)
    } else {
        Err("不支持的内容分级".into())
    }
}
fn validate_page(username: &str, page: u32) -> Result<(), String> {
    wh_core::validate_username(username)?;
    if page == 0 {
        Err("页码必须大于零".into())
    } else {
        Ok(())
    }
}
fn same_origin(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("wallhaven.cc")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}
pub(super) fn uploader_username(wall: &Wallpaper) -> Option<String> {
    let name = wall.uploader.as_ref()?.username.as_deref()?.trim();
    wh_core::validate_username(name).ok()?;
    Some(name.to_owned())
}
fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static profile selector")
}
fn text(element: ElementRef<'_>, limit: usize) -> Option<String> {
    let pieces = element
        .descendants()
        .filter_map(|node| {
            let value = node.value().as_text()?;
            if node.ancestors().filter_map(ElementRef::wrap).any(|e| {
                matches!(
                    e.value().name(),
                    "script" | "style" | "noscript" | "textarea"
                )
            }) {
                return None;
            }
            Some(value.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ");
    let value = pieces
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect::<String>();
    (!value.is_empty()).then_some(value)
}
fn parse_profile(html: &str, username: &str) -> Result<PublicProfile, String> {
    wh_core::validate_username(username)?;
    let document = Html::parse_document(html);
    let identity = document
        .select(&selector("#profile-header #user h1.username a"))
        .next()
        .ok_or("未识别到公开用户主页，网站可能正在验证或页面结构已变化")?;
    let name = text(identity, 64).ok_or("主页用户名缺失")?;
    let href = identity.value().attr("href").ok_or("主页身份地址缺失")?;
    let url = reqwest::Url::parse("https://wallhaven.cc")
        .unwrap()
        .join(href)
        .map_err(|_| "主页身份地址无效")?;
    if !name.eq_ignore_ascii_case(username)
        || !same_origin(&url)
        || !url
            .path()
            .eq_ignore_ascii_case(&format!("/user/{username}"))
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("主页身份与当前上传者不一致，未显示返回内容".into());
    }
    let group = document
        .select(&selector("#profile-header #user .groupname"))
        .next()
        .and_then(|e| text(e, 100));
    let bio = document
        .select(&selector("#profile-content #profile-bio"))
        .next()
        .and_then(|e| text(e, 8000));
    let avatar_url = document
        .select(&selector("#profile-header a.avatar img"))
        .next()
        .and_then(|e| e.value().attr("src"))
        .and_then(avatar_url);
    let mut fields = Vec::new();
    for list in document.select(&selector("#profile-content .user-info-right dl.datalist")) {
        let mut label = None;
        for element in list.children().filter_map(ElementRef::wrap) {
            if element.value().name() == "dt" {
                label = text(element, 80);
            } else if element.value().name() == "dd" {
                let Some(key) = label.take() else { continue };
                let localized = match key.as_str() {
                    "Last Active" => "最后活跃",
                    "Joined" => "加入时间",
                    "Uploads" => "上传作品",
                    "Favorites" => "收藏图片",
                    "Subscribers" => "订阅者",
                    "Profile Views" => "主页浏览",
                    "Profile Comments" => "主页评论",
                    "Forum Posts" => "论坛帖子",
                    "Wallpapers Tagged" => "标注壁纸",
                    "Wallpapers Flagged" => "报告壁纸",
                    _ => continue,
                };
                let value = element
                    .select(&selector("time"))
                    .next()
                    .and_then(|e| {
                        e.value()
                            .attr("datetime")
                            .or_else(|| e.value().attr("title"))
                    })
                    .map(|v| v.chars().take(100).collect::<String>())
                    .filter(|v| !v.trim().is_empty())
                    .or_else(|| text(element, 200));
                if let Some(value) = value {
                    if !fields.iter().any(|(k, _)| k == localized) {
                        fields.push((localized.into(), value));
                    }
                }
            }
        }
    }
    Ok(PublicProfile {
        username: name,
        group,
        bio,
        avatar_url,
        fields,
    })
}
fn avatar_url(value: &str) -> Option<String> {
    let value = if value.starts_with("//") {
        format!("https:{value}")
    } else {
        value.to_owned()
    };
    if value.contains("..") || value.contains('%') || value.contains('\\') {
        return None;
    }
    let url = reqwest::Url::parse(&value).ok()?;
    (same_origin(&url)
        && url.path().starts_with("/images/user/avatar/")
        && url.query().is_none()
        && url.fragment().is_none())
    .then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE: &str = r#"<header id="profile-header"><a class="avatar"><img src="//wallhaven.cc/images/user/avatar/200/example.jpg"></a><div id="user"><h1 class="username"><a href="https://wallhaven.cc/user/whimsicalmango">whimsicalmango</a></h1><h4 class="groupname">User</h4></div></header><section id="profile-content"><article id="profile-bio"><p>Hello &amp; welcome</p><script>do not show</script></article><div class="comments"><dl class="datalist"><dt>Uploads</dt><dd>99999</dd></dl>not biography</div><div class="user-info-right"><h2>User Information</h2><dl class="datalist"><dt>Last Active</dt><dd><time datetime="2026-05-14T11:57:53+00:00">5 months ago</time></dd><dt>Joined</dt><dd><time datetime="2026-04-22T07:00:37+00:00">5 months ago</time></dd><dt>Uploads</dt><dd>138</dd><dt>Favorites</dt><dd>0</dd><dt>Profile Views</dt><dd>2,771</dd></dl></div></section>"#;
    #[test]
    fn parses_real_profile_structure_without_comments_or_scripts() {
        let p = parse_profile(FIXTURE, "whimsicalmango").unwrap();
        assert_eq!(p.username, "whimsicalmango");
        assert_eq!(p.group.as_deref(), Some("User"));
        assert_eq!(p.bio.as_deref(), Some("Hello & welcome"));
        assert!(p.avatar_url.is_some());
        assert!(p.fields.contains(&("上传作品".into(), "138".into())));
        assert!(p.fields.contains(&("收藏图片".into(), "0".into())));
        assert!(p
            .fields
            .iter()
            .any(|(_, v)| v == "2026-04-22T07:00:37+00:00"));
        assert!(!p.fields.iter().any(|(_, v)| v == "99999"));
    }
    #[test]
    fn rejects_wrong_user_and_security_pages() {
        assert!(parse_profile(FIXTURE, "other").is_err());
        for html in [
            "<title>Just a moment...</title>",
            "<form action='/login'></form>",
            "<html>unknown</html>",
        ] {
            assert!(parse_profile(html, "user").is_err());
        }
    }
    #[test]
    fn missing_fields_remain_absent() {
        let p=parse_profile(r#"<header id="profile-header"><div id="user"><h1 class="username"><a href="/user/test">test</a></h1></div></header><section id="profile-content"></section>"#,"test").unwrap();
        assert!(p.bio.is_none());
        assert!(p.group.is_none());
        assert!(p.avatar_url.is_none());
        assert!(p.fields.is_empty());
    }
    #[test]
    fn avatar_and_input_validation_are_strict() {
        assert!(avatar_url("//wallhaven.cc/images/user/avatar/200/a.jpg").is_some());
        for value in [
            "https://evil.test/a.jpg",
            "http://wallhaven.cc/images/user/avatar/a.jpg",
            "https://wallhaven.cc@evil.test/images/user/avatar/a.jpg",
            "https://wallhaven.cc/images/user/avatar/a.jpg?x=1",
            "https://wallhaven.cc/images/user/avatar/../other.jpg",
        ] {
            assert!(avatar_url(value).is_none(), "{value}");
        }
        assert!(validate_page("../other", 1).is_err());
        assert!(validate_page("test", 0).is_err());
        assert!(uploader_username(&Wallpaper::default()).is_none());
    }
    #[test]
    fn profile_content_key_only_for_explicit_permitted_nsfw() {
        let mut settings = crate::config::Settings::default();
        settings.api_key = Some("fixture-key".into());
        assert_eq!(content_key(&settings, "001"), None);
        settings.nsfw_enabled = true;
        for purity in ["100", "010", "110", "000"] {
            assert_eq!(content_key(&settings, purity), None);
        }
        assert_eq!(
            content_key(&settings, "001").as_deref(),
            Some("fixture-key")
        );
        assert_eq!(
            content_key(&settings, "111").as_deref(),
            Some("fixture-key")
        );
        settings.api_key = None;
        assert_eq!(content_key(&settings, "001"), None);
        assert!(validate_purity("000").is_err());
        assert!(validate_purity("010").is_ok());
    }
    #[test]
    #[ignore = "read-only live public Wallhaven requests"]
    fn live_public_profile_uploads_and_collections() {
        let client = ProfileClient::new(
            WallhavenClient::new(Some("invalid-public-test-key".into())).unwrap(),
        )
        .unwrap();
        let p = client.profile("whimsicalmango").unwrap();
        let uploads = client.uploads(&p.username, 1, "100", None).unwrap();
        let collections = client.collections(&p.username).unwrap();
        let second = client.uploads(&p.username, 2, "100", None).unwrap();
        assert!(!second.data.is_empty());
        assert_ne!(
            uploads.data.first().map(|w| &w.id),
            second.data.first().map(|w| &w.id)
        );
        let id = collections
            .data
            .iter()
            .find_map(|c| c.id)
            .expect("public collection fixture");
        let contents = client
            .collection_page(&p.username, id, 1, "100", None)
            .unwrap();
        assert!(contents
            .data
            .iter()
            .all(|w| w.purity.as_deref() == Some("sfw")));
        println!(
            "public uploads_page2={} collection_sfw={}",
            second.data.len(),
            contents.data.len()
        );
        println!(
            "public profile={} fields={} uploads={} collections={}",
            p.username,
            p.fields.len(),
            uploads.data.len(),
            collections.data.len()
        );
        assert!(uploads
            .data
            .iter()
            .all(|w| w.purity.as_deref() == Some("sfw")));
    }
}
