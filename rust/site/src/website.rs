//! Authenticated wallhaven.cc website session support.
//! Parsers and their regression tests ported from rust/desktop/src/website.rs.
//! The desktop browser lifecycle is intentionally not part of this crate.
use http::StatusCode;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
};
use url::Url;

const ORIGIN: &str = "https://wallhaven.cc";
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Browser-session request. Transport implementations must not retry writes or
/// follow a redirect off https://wallhaven.cc (including before returning it).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub fields: Vec<(String, String)>,
    pub csrf: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Response {
    pub status: u16,
    pub url: String,
    pub body: String,
}
/// Implement this with the user's existing browser session, not an API client
/// or an exported cookie jar. Errors are deliberately discarded at this boundary.
pub trait SiteTransport: Send + Sync {
    fn request(&self, request: Request) -> Result<Response, String>;
}

/// Sanitized failures; never contain HTML, credentials, cookies or CSRF values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SiteError {
    Network,
    ResponseTooLarge,
    HumanVerificationRequired,
    NotAuthenticated,
    UnsupportedPage,
    InvalidOwner,
    OwnerMismatch,
    InvalidWallpaperId,
    InvalidTargetIndex,
    WriteRejected,
    UnknownOutcome,
}
impl fmt::Display for SiteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Network => "应用内浏览器请求失败，请重新打开登录窗口检查会话",
            Self::ResponseTooLarge => "网站响应超过大小限制",
            Self::HumanVerificationRequired => "请在应用内网页登录窗口完成人机验证，再继续登录",
            Self::NotAuthenticated => "网站会话未登录或已过期，请在应用内窗口登录",
            Self::UnsupportedPage => "网站页面结构或操作未识别，未提交猜测请求",
            Self::InvalidOwner => "网站用户名无效",
            Self::OwnerMismatch => "当前网页登录账号已变化，未提交请求；请核对收藏夹用户名",
            Self::InvalidWallpaperId => "壁纸 ID 无效",
            Self::InvalidTargetIndex => "收藏夹选择已失效，请刷新收藏状态",
            Self::WriteRejected => "网站未确认收藏变更，请刷新状态核对",
            Self::UnknownOutcome => "收藏变更结果尚未确认，请刷新核对；未自动重试",
        })
    }
}
impl std::error::Error for SiteError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FavoriteState {
    pub is_favorite: bool,
    pub targets: Vec<FavoriteTarget>,
}
/// Only the label is public; discovered actions remain private.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FavoriteTarget {
    pub label: String,
}

type FavoriteChoices = BTreeMap<String, (String, Vec<String>)>;
#[derive(Clone)]
pub struct SiteClient {
    transport: Arc<dyn SiteTransport>,
    choices: Arc<Mutex<FavoriteChoices>>,
}
impl SiteClient {
    pub fn with_transport(transport: Arc<dyn SiteTransport>) -> Self {
        Self {
            transport,
            choices: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
    pub fn verify(&self) -> Result<String, SiteError> {
        let document = self.get_document(origin_url("/")?, false)?;
        parse_authenticated_user(&document.body, &document.final_url)
    }
    pub(crate) fn verify_owner(&self, owner: &str) -> Result<(), SiteError> {
        wh_core::validate_username(owner).map_err(|_| SiteError::InvalidOwner)?;
        if !self.verify()?.eq_ignore_ascii_case(owner) {
            return Err(SiteError::OwnerMismatch);
        }
        Ok(())
    }
    pub(super) fn read_html(&self, path: &str) -> Result<String, SiteError> {
        if !path.starts_with('/') || path.starts_with("//") {
            return Err(SiteError::UnsupportedPage);
        }
        Ok(self.get_document(origin_url(path)?, false)?.body)
    }
    /// Identity and content come from the same size/origin/challenge-validated response.
    pub(super) fn read_authenticated_html(
        &self,
        path: &str,
        owner: Option<&str>,
    ) -> Result<(String, String), SiteError> {
        if !path.starts_with('/') || path.starts_with("//") {
            return Err(SiteError::UnsupportedPage);
        }
        if let Some(owner) = owner {
            wh_core::validate_username(owner).map_err(|_| SiteError::InvalidOwner)?;
        }
        let document = self.get_document(origin_url(path)?, false)?;
        let observed = parse_authenticated_user(&document.body, &document.final_url)?;
        if owner.is_some_and(|owner| !observed.eq_ignore_ascii_case(owner)) {
            return Err(SiteError::OwnerMismatch);
        }
        Ok((observed, document.body))
    }
    fn request(&self, request: Request) -> Result<Received, SiteError> {
        let url = Url::parse(&request.url).map_err(|_| SiteError::UnsupportedPage)?;
        if !is_wallhaven_url(&url) || !matches!(request.method.as_str(), "GET" | "POST") {
            return Err(SiteError::UnsupportedPage);
        }
        let writing = request.method == "POST";
        let response = self.transport.request(request).map_err(|_| {
            if writing {
                SiteError::UnknownOutcome
            } else {
                SiteError::Network
            }
        })?;
        receive_browser(response)
    }
    pub(super) fn submit_form(
        &self,
        action: &str,
        fields: &[(String, String)],
        csrf: &str,
    ) -> Result<String, SiteError> {
        let response = self.request(Request {
            method: "POST".into(),
            url: action.into(),
            fields: fields.to_vec(),
            csrf: Some(csrf.into()),
        })?;
        validate_document(&response, false)?;
        if let Ok(value) = serde_json::from_str::<Value>(&response.body) {
            if value.get("status").and_then(Value::as_bool) == Some(false) {
                return Err(SiteError::WriteRejected);
            }
        }
        Ok(response.body)
    }
    pub fn favorite_state(&self, id: &str) -> Result<FavoriteState, SiteError> {
        let (owner, page) = self.fetch_owned_favorite_page(id, None)?;
        self.remember_choices(id, &owner, &page.state);
        Ok(page.state)
    }
    pub fn favorite_snapshot(&self, id: &str) -> Result<Value, SiteError> {
        let (owner, page) = self.fetch_owned_favorite_page(id, None)?;
        self.remember_choices(id, &owner, &page.state);
        Ok(
            serde_json::json!({"owner":owner,"is_favorite":page.state.is_favorite,"targets":page.state.targets}),
        )
    }
    fn remember_choices(&self, id: &str, owner: &str, state: &FavoriteState) {
        let mut choices = self.choices.lock().unwrap();
        if choices.len() >= 32 && !choices.contains_key(id) {
            choices.clear();
        }
        choices.insert(
            id.into(),
            (
                owner.into(),
                state.targets.iter().map(|t| t.label.clone()).collect(),
            ),
        );
    }
    /// The mobile caller supplies its selected label, never an index into a stale form.
    pub fn set_favorite_target(
        &self,
        owner: &str,
        id: &str,
        desired: bool,
        label: &str,
    ) -> Result<FavoriteState, SiteError> {
        self.write_favorite(owner, id, desired, Some(label), 0)
    }
    pub fn set_favorite(
        &self,
        owner: &str,
        id: &str,
        desired: bool,
        target_index: usize,
    ) -> Result<FavoriteState, SiteError> {
        let selected = self.choices.lock().unwrap().get(id).cloned();
        let label = if desired {
            match selected {
                Some((observed, labels)) => {
                    if !observed.eq_ignore_ascii_case(owner) {
                        return Err(SiteError::OwnerMismatch);
                    }
                    Some(
                        labels
                            .get(target_index)
                            .cloned()
                            .ok_or(SiteError::InvalidTargetIndex)?,
                    )
                }
                None => None, // No remembered choice is safe only for a sole fresh target.
            }
        } else {
            None
        };
        self.write_favorite(owner, id, desired, label.as_deref(), target_index)
    }
    fn write_favorite(
        &self,
        owner: &str,
        id: &str,
        desired: bool,
        label: Option<&str>,
        target_index: usize,
    ) -> Result<FavoriteState, SiteError> {
        wh_core::validate_username(owner).map_err(|_| SiteError::InvalidOwner)?;
        let page = self.fetch_favorite_page(id, Some(owner))?;
        if page.state.is_favorite == desired {
            return Ok(page.state);
        }
        let target_index = if desired {
            if let Some(label) = label {
                let mut indices = page
                    .state
                    .targets
                    .iter()
                    .enumerate()
                    .filter(|(_, target)| target.label == label)
                    .map(|(index, _)| index);
                let index = indices.next().ok_or(SiteError::InvalidTargetIndex)?;
                if indices.next().is_some() {
                    return Err(SiteError::InvalidTargetIndex);
                }
                index
            } else if target_index == 0 && page.state.targets.len() == 1 {
                0
            } else {
                return Err(SiteError::InvalidTargetIndex);
            }
        } else {
            0
        };
        let csrf = page.csrf.ok_or(SiteError::UnsupportedPage)?;
        let action = page
            .actions
            .get(if desired { target_index } else { 0 })
            .ok_or(SiteError::InvalidTargetIndex)?;
        let document = self
            .request(Request {
                method: "POST".into(),
                url: action.as_str().into(),
                fields: Vec::new(),
                csrf: Some(csrf),
            })
            .map_err(|_| SiteError::UnknownOutcome)?;
        validate_write_response(&document)?;
        let observed = self
            .fetch_favorite_page(id, Some(owner))
            .map_err(|_| SiteError::UnknownOutcome)?
            .state;
        if observed.is_favorite == desired {
            Ok(observed)
        } else {
            Err(SiteError::UnknownOutcome)
        }
    }
    fn get_document(&self, url: Url, allow_login_page: bool) -> Result<Received, SiteError> {
        let document = self.request(Request {
            method: "GET".into(),
            url: url.into(),
            fields: Vec::new(),
            csrf: None,
        })?;
        validate_document(&document, allow_login_page)?;
        Ok(document)
    }
    fn fetch_favorite_page(
        &self,
        id: &str,
        owner: Option<&str>,
    ) -> Result<FavoritePage, SiteError> {
        Ok(self.fetch_owned_favorite_page(id, owner)?.1)
    }
    fn fetch_owned_favorite_page(
        &self,
        id: &str,
        owner: Option<&str>,
    ) -> Result<(String, FavoritePage), SiteError> {
        validate_wallpaper_id(id)?;
        let document = self.get_document(origin_url(&format!("/w/{id}"))?, false)?;
        let observed = parse_authenticated_user(&document.body, &document.final_url)?;
        if owner.is_some_and(|owner| !observed.eq_ignore_ascii_case(owner)) {
            return Err(SiteError::OwnerMismatch);
        }
        Ok((
            observed,
            parse_favorite_page(&document.body, &document.final_url, false)?,
        ))
    }
}
struct Received {
    status: StatusCode,
    final_url: Url,
    location: Option<String>,
    challenge_header: bool,
    body: String,
}
fn receive_browser(response: Response) -> Result<Received, SiteError> {
    if response.body.len() > MAX_BODY_BYTES {
        return Err(SiteError::ResponseTooLarge);
    }
    let url = Url::parse(&response.url).map_err(|_| SiteError::UnsupportedPage)?;
    if !is_wallhaven_url(&url) || !(100..=599).contains(&response.status) {
        return Err(SiteError::UnsupportedPage);
    }
    Ok(Received {
        status: StatusCode::from_u16(response.status).map_err(|_| SiteError::UnsupportedPage)?,
        final_url: url,
        location: None,
        challenge_header: false,
        body: response.body,
    })
}
fn validate_document(document: &Received, allow_login_page: bool) -> Result<(), SiteError> {
    if !is_wallhaven_url(&document.final_url) {
        return Err(SiteError::UnsupportedPage);
    }
    if document.challenge_header
        || matches!(
            document.status,
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
        )
        || detects_human_verification(&document.body)
    {
        return Err(SiteError::HumanVerificationRequired);
    }
    if document.status == StatusCode::UNAUTHORIZED {
        return Err(SiteError::NotAuthenticated);
    }
    if document.status.is_redirection() {
        let location = document
            .location
            .as_deref()
            .ok_or(SiteError::UnsupportedPage)?;
        let target = document
            .final_url
            .join(location)
            .map_err(|_| SiteError::UnsupportedPage)?;
        if !is_wallhaven_url(&target) {
            return Err(SiteError::UnsupportedPage);
        }
        if is_login_path(&target) {
            return Err(SiteError::NotAuthenticated);
        }
        return Err(SiteError::UnsupportedPage);
    }
    if !document.status.is_success() {
        return Err(SiteError::UnsupportedPage);
    }
    if !allow_login_page && is_login_path(&document.final_url) {
        return Err(SiteError::NotAuthenticated);
    }
    Ok(())
}
fn validate_write_response(document: &Received) -> Result<(), SiteError> {
    validate_document(document, false).map_err(|error| match error {
        SiteError::UnsupportedPage => SiteError::WriteRejected,
        other => other,
    })?;
    let value: Value =
        serde_json::from_str(&document.body).map_err(|_| SiteError::WriteRejected)?;
    if value.get("status").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(SiteError::WriteRejected)
    }
}
fn origin_url(path: &str) -> Result<Url, SiteError> {
    Url::parse(&format!("{ORIGIN}{path}")).map_err(|_| SiteError::UnsupportedPage)
}
fn is_wallhaven_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("wallhaven.cc")
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
}
fn is_login_path(url: &Url) -> bool {
    let path = url.path().trim_end_matches('/');
    path == "/login" || path.starts_with("/login/")
}
fn validate_wallpaper_id(id: &str) -> Result<(), SiteError> {
    if id.is_empty() || id.len() > 16 || !id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(SiteError::InvalidWallpaperId);
    }
    Ok(())
}

// The login parser is retained only for the original parser regression tests;
// this crate never submits credentials or creates a login window.
#[cfg(test)]
struct LoginForm {
    action: Url,
    hidden: Vec<(String, String)>,
    username_field: String,
    password_field: String,
}
#[cfg(test)]
fn parse_login_form(html: &str, page_url: &Url) -> Result<LoginForm, SiteError> {
    if detects_human_verification(html) {
        return Err(SiteError::HumanVerificationRequired);
    }
    let document = Html::parse_document(html);
    let form_selector = selector("form");
    let input_selector = selector("input");
    for form in document.select(&form_selector) {
        let mut password_field = None;
        let mut username_candidates = Vec::new();
        let mut hidden = Vec::new();
        for input in form.select(&input_selector) {
            if input.value().attr("disabled").is_some() {
                continue;
            }
            let Some(name) = input.value().attr("name").filter(|name| !name.is_empty()) else {
                continue;
            };
            let input_type = input
                .value()
                .attr("type")
                .unwrap_or("text")
                .to_ascii_lowercase();
            match input_type.as_str() {
                "password" => {
                    if password_field.is_none() {
                        password_field = Some(name.to_owned());
                    }
                }
                "hidden" => hidden.push((
                    name.to_owned(),
                    input.value().attr("value").unwrap_or("").to_owned(),
                )),
                "text" | "email" => {
                    let lower_name = name.to_ascii_lowercase();
                    let autocomplete = input
                        .value()
                        .attr("autocomplete")
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let rank = if autocomplete == "username" {
                        0
                    } else if input_type == "email" {
                        1
                    } else if lower_name.contains("user") || lower_name.contains("email") {
                        2
                    } else {
                        3
                    };
                    username_candidates.push((rank, name.to_owned()));
                }
                _ => {}
            }
        }
        let Some(password_field) = password_field else {
            continue;
        };
        username_candidates.sort_by_key(|(rank, _)| *rank);
        let Some((_, username_field)) = username_candidates.into_iter().next() else {
            continue;
        };
        let action_text = form.value().attr("action").unwrap_or("").trim();
        let action = if action_text.is_empty() {
            page_url.clone()
        } else {
            match page_url.join(action_text) {
                Ok(action) => action,
                Err(_) => continue,
            }
        };
        if !is_wallhaven_url(&action) {
            continue;
        }
        if !hidden
            .iter()
            .any(|(name, value)| name == "_token" && !value.is_empty())
        {
            return Err(SiteError::UnsupportedPage);
        }
        return Ok(LoginForm {
            action,
            hidden,
            username_field,
            password_field,
        });
    }
    Err(SiteError::UnsupportedPage)
}
fn parse_authenticated_user(html: &str, page_url: &Url) -> Result<String, SiteError> {
    if detects_human_verification(html) {
        return Err(SiteError::HumanVerificationRequired);
    }
    let document = Html::parse_document(html);
    let panel_selector = selector("#userpanel");
    let link_selector = selector("a[href]");
    let form_selector = selector("form[action]");
    let mut panels = document.select(&panel_selector);
    let panel = panels.next().ok_or(SiteError::UnsupportedPage)?;
    if panels.next().is_some() {
        return Err(SiteError::UnsupportedPage);
    }
    let mut observed_user = None;
    let mut has_logout = false;
    let mut has_login = false;
    for link in panel.select(&link_selector) {
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let Ok(url) = page_url.join(href) else {
            continue;
        };
        if !is_wallhaven_url(&url) {
            continue;
        }
        if let Some(username) = username_from_profile_url(&url) {
            observed_user = Some(username);
        }
        if path_has_segment(&url, "logout") {
            has_logout = true;
        }
        if path_has_segment(&url, "login") || path_has_segment(&url, "register") {
            has_login = true;
        }
    }
    for form in panel.select(&form_selector) {
        let Some(action) = form.value().attr("action") else {
            continue;
        };
        let Ok(url) = page_url.join(action) else {
            continue;
        };
        if is_wallhaven_url(&url) && path_has_segment(&url, "logout") {
            has_logout = true;
        }
    }
    match (observed_user, has_logout) {
        (Some(username), true) => Ok(username),
        _ if has_login => Err(SiteError::NotAuthenticated),
        _ => Err(SiteError::UnsupportedPage),
    }
}
fn username_from_profile_url(url: &Url) -> Option<String> {
    let mut segments = url.path_segments()?;
    if segments.next()? != "user" {
        return None;
    }
    let username = segments.next()?;
    if username.is_empty()
        || username.len() > 64
        || !username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(username.to_owned())
}
fn path_has_segment(url: &Url, expected: &str) -> bool {
    url.path_segments()
        .is_some_and(|segments| segments.into_iter().any(|segment| segment == expected))
}
struct FavoritePage {
    state: FavoriteState,
    actions: Vec<Url>,
    csrf: Option<String>,
}
fn parse_favorite_page(
    html: &str,
    page_url: &Url,
    require_csrf: bool,
) -> Result<FavoritePage, SiteError> {
    parse_authenticated_user(html, page_url)?;
    let document = Html::parse_document(html);
    let favorite_selector = selector("#fav-button");
    let mut favorites = document.select(&favorite_selector);
    let favorite = favorites.next().ok_or(SiteError::UnsupportedPage)?;
    if favorites.next().is_some() {
        return Err(SiteError::UnsupportedPage);
    }
    let csrf = parse_csrf(&document)?;
    if require_csrf && csrf.is_none() {
        return Err(SiteError::UnsupportedPage);
    }
    let is_add = favorite.value().attr("class").is_some_and(|classes| {
        classes
            .split_whitespace()
            .any(|class| class == "add-button")
    });
    if is_add {
        parse_add_favorite(favorite, page_url, csrf)
    } else {
        parse_remove_favorite(favorite, page_url, csrf)
    }
}
fn parse_csrf(document: &Html) -> Result<Option<String>, SiteError> {
    let csrf_selector = selector("meta[name=\"csrf-token\"]");
    let mut tokens = document
        .select(&csrf_selector)
        .filter_map(|meta| meta.value().attr("content"))
        .filter(|content| !content.is_empty());
    let token = tokens.next().map(str::to_owned);
    if tokens.next().is_some() || token.as_ref().is_some_and(|token| token.len() > 8192) {
        return Err(SiteError::UnsupportedPage);
    }
    Ok(token)
}
fn parse_add_favorite(
    favorite: ElementRef<'_>,
    page_url: &Url,
    csrf: Option<String>,
) -> Result<FavoritePage, SiteError> {
    let add_selector = selector("a.add-fav");
    let mut targets = Vec::new();
    let mut actions = Vec::new();
    for link in favorite.select(&add_selector) {
        let href = link
            .value()
            .attr("href")
            .ok_or(SiteError::UnsupportedPage)?;
        let action = page_url
            .join(href)
            .map_err(|_| SiteError::UnsupportedPage)?;
        if !is_wallhaven_url(&action) {
            return Err(SiteError::UnsupportedPage);
        }
        let label = normalized_label(link).ok_or(SiteError::UnsupportedPage)?;
        targets.push(FavoriteTarget { label });
        actions.push(action);
    }
    if targets.is_empty() {
        return Err(SiteError::UnsupportedPage);
    }
    Ok(FavoritePage {
        state: FavoriteState {
            is_favorite: false,
            targets,
        },
        actions,
        csrf,
    })
}
fn parse_remove_favorite(
    favorite: ElementRef<'_>,
    page_url: &Url,
    csrf: Option<String>,
) -> Result<FavoritePage, SiteError> {
    if favorite.value().name() != "a" {
        return Err(SiteError::UnsupportedPage);
    }
    let href = favorite
        .value()
        .attr("href")
        .ok_or(SiteError::UnsupportedPage)?;
    let action = page_url
        .join(href)
        .map_err(|_| SiteError::UnsupportedPage)?;
    if !is_wallhaven_url(&action) {
        return Err(SiteError::UnsupportedPage);
    }
    Ok(FavoritePage {
        state: FavoriteState {
            is_favorite: true,
            targets: Vec::new(),
        },
        actions: vec![action],
        csrf,
    })
}
fn normalized_label(element: ElementRef<'_>) -> Option<String> {
    let text = element.text().collect::<Vec<_>>().join(" ");
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !normalized.is_empty() {
        return Some(normalized);
    }
    for attribute in ["aria-label", "title"] {
        if let Some(value) = element.value().attr(attribute) {
            let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
            if !normalized.is_empty() {
                return Some(normalized);
            }
        }
    }
    None
}
fn detects_human_verification(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let document = Html::parse_document(html);
    if document.select(&selector("script[src]")).any(|script| {
        let src = script.value().attr("src").unwrap_or_default();
        src.contains("challenge-platform") && !src.ends_with("/scripts/precursor/main.js")
    }) {
        return true;
    }
    const CHALLENGE_MARKERS: &[&str] = &[
        "cf-chl-",
        "__cf_chl",
        "cf-turnstile",
        "g-recaptcha",
        "h-captcha",
        "hcaptcha",
        "cloudflare ray id",
        "attention required! | cloudflare",
        "just a moment...",
        "name=\"captcha\"",
        "name='captcha'",
    ];
    if CHALLENGE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return true;
    }
    const SECOND_FACTOR_MARKERS: &[&str] = &[
        "autocomplete=\"one-time-code\"",
        "autocomplete='one-time-code'",
        "name=\"otp\"",
        "name='otp'",
        "name=\"totp\"",
        "name='totp'",
        "action=\"/two-factor",
        "action='/two-factor",
        "action=\"/2fa",
        "action='/2fa",
        "enter your two-factor",
        "two-factor authentication code",
    ];
    SECOND_FACTOR_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}
fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static selector is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn page_url(path: &str) -> Url {
        Url::parse(&format!("https://wallhaven.cc{path}")).unwrap()
    }
    fn authenticated_panel() -> &'static str {
        r#"<div id="userpanel"><a href="/user/Alice_1">Alice</a><form action="/logout" method="post"></form></div>"#
    }
    fn received(status: u16, path: &str, body: &str) -> Received {
        Received {
            status: StatusCode::from_u16(status).unwrap(),
            final_url: page_url(path),
            location: None,
            challenge_header: false,
            body: body.to_owned(),
        }
    }
    #[test]
    fn parses_dynamic_login_action_and_fields() {
        let html = r#"<form action="/session/start?from=login" method="post">
            <input type="hidden" name="_token" value="fresh-token"><input type="hidden" name="remember" value="1">
            <input type="email" name="identity" autocomplete="username"><input type="password" name="secret" autocomplete="current-password"></form>"#;
        let form = parse_login_form(html, &page_url("/login")).unwrap();
        assert_eq!(
            form.action.as_str(),
            "https://wallhaven.cc/session/start?from=login"
        );
        assert_eq!(form.username_field, "identity");
        assert_eq!(form.password_field, "secret");
        assert_eq!(
            form.hidden,
            vec![
                ("_token".to_owned(), "fresh-token".to_owned()),
                ("remember".to_owned(), "1".to_owned())
            ]
        );
    }
    #[test]
    fn rejects_cross_origin_login_action() {
        let html = r#"<form action="https://example.com/collect"><input name="username"><input type="password" name="password"></form>"#;
        assert_eq!(
            parse_login_form(html, &page_url("/login")).err(),
            Some(SiteError::UnsupportedPage)
        );
    }
    #[test]
    fn detects_cloudflare_captcha_and_second_factor() {
        for html in [
            r#"<script src="/cdn-cgi/challenge-platform/x"></script>"#,
            r#"<div class="g-recaptcha"></div>"#,
            r#"<form action="/two-factor"><input autocomplete="one-time-code"></form>"#,
        ] {
            assert_eq!(
                parse_login_form(html, &page_url("/login")).err(),
                Some(SiteError::HumanVerificationRequired)
            );
        }
    }
    #[test]
    fn parses_authenticated_user_only_with_profile_and_logout_evidence() {
        assert_eq!(
            parse_authenticated_user(
                &format!("<html>{}</html>", authenticated_panel()),
                &page_url("/")
            ),
            Ok("Alice_1".to_owned())
        );
        let anonymous = r#"<div id="userpanel"><a href="/login">Login</a><a href="/register">Register</a></div>"#;
        assert_eq!(
            parse_authenticated_user(anonymous, &page_url("/")),
            Err(SiteError::NotAuthenticated)
        );
    }
    #[test]
    fn csrf_is_required_for_a_write_parse() {
        let html = format!(
            r#"{}<div id="fav-button" class="add-button"><a class="add-fav" href="/favorites/add/1">Default</a></div>"#,
            authenticated_panel()
        );
        assert_eq!(
            parse_favorite_page(&html, &page_url("/w/xekmro"), true).err(),
            Some(SiteError::UnsupportedPage)
        );
        assert!(parse_favorite_page(&html, &page_url("/w/xekmro"), false).is_ok());
    }
    #[test]
    fn parses_favorite_add_menu_in_document_order() {
        let html = format!(
            r#"<meta name="csrf-token" content="csrf-value">{}
            <div id="fav-button" class="button add-button"><a class="add-fav" href="/favorites/add/default"> Default </a>
            <span><a class="add-fav" href="favorites/add/collection">My   Collection</a></span></div>"#,
            authenticated_panel()
        );
        let page = parse_favorite_page(&html, &page_url("/w/xekmro"), true).unwrap();
        assert!(!page.state.is_favorite);
        assert_eq!(
            page.state.targets,
            vec![
                FavoriteTarget {
                    label: "Default".to_owned()
                },
                FavoriteTarget {
                    label: "My Collection".to_owned()
                }
            ]
        );
        assert_eq!(
            page.actions[0].as_str(),
            "https://wallhaven.cc/favorites/add/default"
        );
        assert_eq!(
            page.actions[1].as_str(),
            "https://wallhaven.cc/w/favorites/add/collection"
        );
    }
    #[test]
    fn parses_clickable_favorite_removal_button() {
        let html = format!(
            r#"<meta name="csrf-token" content="csrf-value">{}<a id="fav-button" class="button" href="/favorites/remove/42">Remove</a>"#,
            authenticated_panel()
        );
        let page = parse_favorite_page(&html, &page_url("/w/xekmro"), true).unwrap();
        assert!(page.state.is_favorite);
        assert!(page.state.targets.is_empty());
        assert_eq!(page.actions.len(), 1);
        assert_eq!(
            page.actions[0].as_str(),
            "https://wallhaven.cc/favorites/remove/42"
        );
    }
    #[test]
    fn rejects_invalid_or_cross_origin_favorite_links() {
        for link in [
            r#"<a class="add-fav">Missing</a>"#,
            r#"<a class="add-fav" href="javascript:alert(1)">Bad</a>"#,
            r#"<a class="add-fav" href="https://example.com/favorite">External</a>"#,
        ] {
            let html = format!(
                r#"<meta name="csrf-token" content="csrf-value">{}<div id="fav-button" class="add-button">{link}</div>"#,
                authenticated_panel()
            );
            assert_eq!(
                parse_favorite_page(&html, &page_url("/w/xekmro"), true).err(),
                Some(SiteError::UnsupportedPage)
            );
        }
    }
    #[test]
    fn validates_write_json_status_and_http_auth_status() {
        assert_eq!(
            validate_write_response(&received(
                200,
                "/favorites/add/1",
                r#"{"status":true,"view":"<a>ok</a>"}"#
            )),
            Ok(())
        );
        for body in [r#"{"status":false}"#, r#"{"status":"true"}"#, "not-json"] {
            assert_eq!(
                validate_write_response(&received(200, "/favorites/add/1", body)),
                Err(SiteError::WriteRejected)
            );
        }
        assert_eq!(
            validate_write_response(&received(401, "/favorites/add/1", "")),
            Err(SiteError::NotAuthenticated)
        );
    }
    #[test]
    fn validates_wallpaper_ids_before_building_a_url() {
        for invalid in ["", "abc/def", "abc-def", "é", "12345678901234567"] {
            assert_eq!(
                validate_wallpaper_id(invalid),
                Err(SiteError::InvalidWallpaperId)
            );
        }
        for valid in ["xekmro", "ABC123", "1234567890123456"] {
            assert_eq!(validate_wallpaper_id(valid), Ok(()));
        }
    }
    #[test]
    fn stopped_login_and_external_redirects_are_not_accepted() {
        let mut login = received(302, "/w/xekmro", "");
        login.location = Some("/login".to_owned());
        assert_eq!(
            validate_document(&login, false),
            Err(SiteError::NotAuthenticated)
        );
        let mut external = received(302, "/w/xekmro", "");
        external.location = Some("https://example.com/collect".to_owned());
        assert_eq!(
            validate_document(&external, false),
            Err(SiteError::UnsupportedPage)
        );
    }
    #[test]
    fn ordinary_cloudflare_telemetry_script_is_not_a_challenge() {
        assert!(!detects_human_verification(
            r#"<div id="collections"></div><script src="/cdn-cgi/challenge-platform/scripts/precursor/main.js"></script>"#
        ));
    }
    #[test]
    fn login_form_without_token_is_not_submitted() {
        let html = r#"<form action="/login"><input name="username"><input type="password" name="password"></form>"#;
        assert!(matches!(
            parse_login_form(html, &origin_url("/login").unwrap()),
            Err(SiteError::UnsupportedPage)
        ));
    }
    #[test]
    fn browser_result_cannot_return_external_or_oversized_document() {
        for url in [
            "http://wallhaven.cc/",
            "https://evil.test/",
            "https://wallhaven.cc.evil.test/",
            "https://user@wallhaven.cc/",
            "https://wallhaven.cc:444/",
        ] {
            assert!(matches!(
                receive_browser(Response {
                    status: 200,
                    url: url.into(),
                    body: String::new()
                }),
                Err(SiteError::UnsupportedPage)
            ));
        }
        assert!(matches!(
            receive_browser(Response {
                status: 200,
                url: "https://wallhaven.cc/".into(),
                body: "x".repeat(MAX_BODY_BYTES + 1)
            }),
            Err(SiteError::ResponseTooLarge)
        ));
    }
    #[test]
    fn verification_error_gives_actionable_chinese_instruction() {
        let message = SiteError::HumanVerificationRequired.to_string();
        assert!(message.contains("应用内"));
        assert!(message.contains("验证"));
        assert!(!message.contains("human verification is required"));
    }
}
