//! Public profile reads use a separate cookie-free, key-free HTTP client.
use crate::{urls, MobileError};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use std::{io::Read, sync::Arc, time::Duration};
use wh_core::TokenBucket;

pub(crate) struct ProfileClient {
    http: reqwest::blocking::Client,
    bucket: Arc<TokenBucket>,
}
#[derive(Serialize)]
pub(crate) struct Profile {
    username: String,
    group: Option<String>,
    biography: Option<String>,
    avatar: Option<String>,
    fields: Vec<Field>,
}
#[derive(Serialize)]
struct Field {
    label: String,
    value: String,
}

impl ProfileClient {
    pub(crate) fn new(bucket: Arc<TokenBucket>) -> Result<Self, MobileError> {
        let http = reqwest::blocking::Client::builder()
            .user_agent("NxWallhaven/0.1 (public mobile profile reader)")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| MobileError::Network)?;
        Ok(Self { http, bucket })
    }
    pub(crate) fn get(&self, username: &str) -> Result<Profile, MobileError> {
        crate::username(username)?;
        self.bucket.acquire();
        let response = self
            .http
            .get(format!("https://wallhaven.cc/user/{username}"))
            .send()
            .map_err(|_| MobileError::Network)?;
        if response.status().as_u16() != 200 {
            return Err(MobileError::Http {
                status: response.status().as_u16(),
            });
        }
        if !urls::same_origin(response.url())
            || response.url().path() != format!("/user/{username}")
        {
            return Err(MobileError::Response);
        }
        let mut bytes = Vec::new();
        response
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| MobileError::Network)?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(MobileError::Response);
        }
        parse(
            std::str::from_utf8(&bytes).map_err(|_| MobileError::Response)?,
            username,
        )
    }
}
fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static selector")
}
fn text(element: ElementRef<'_>, limit: usize) -> Option<String> {
    let pieces = element
        .descendants()
        .filter_map(|node| {
            let text = node.value().as_text()?;
            if node.ancestors().filter_map(ElementRef::wrap).any(|e| {
                matches!(
                    e.value().name(),
                    "script" | "style" | "noscript" | "textarea"
                )
            }) {
                return None;
            }
            Some(text.to_string())
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
fn parse(html: &str, username: &str) -> Result<Profile, MobileError> {
    crate::username(username)?;
    let doc = Html::parse_document(html);
    let identity = doc
        .select(&selector("#profile-header #user h1.username a"))
        .next()
        .ok_or(MobileError::Response)?;
    let name = text(identity, 65).ok_or(MobileError::Response)?;
    let href = identity.value().attr("href").ok_or(MobileError::Response)?;
    if href.contains(['%', '\\']) {
        return Err(MobileError::Response);
    }
    let url = reqwest::Url::parse("https://wallhaven.cc")
        .unwrap()
        .join(href)
        .map_err(|_| MobileError::Response)?;
    // Case is intentional: the requested identity must match exactly, not just a nearby link.
    if name != username
        || !urls::same_origin(&url)
        || url.path() != format!("/user/{username}")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(MobileError::Response);
    }
    let group = doc
        .select(&selector("#profile-header #user .groupname"))
        .next()
        .and_then(|e| text(e, 100));
    let biography = doc
        .select(&selector("#profile-content #profile-bio"))
        .next()
        .and_then(|e| text(e, 8000));
    let avatar = doc
        .select(&selector("#profile-header a.avatar img"))
        .next()
        .and_then(|e| e.value().attr("src"))
        .and_then(urls::avatar);
    let mut fields: Vec<Field> = Vec::new();
    for list in doc.select(&selector("#profile-content .user-info-right dl.datalist")) {
        let mut label = None;
        for e in list.children().filter_map(ElementRef::wrap) {
            if e.value().name() == "dt" {
                label = text(e, 80);
            } else if e.value().name() == "dd" {
                let Some(label) = label.take() else { continue };
                if ![
                    "Last Active",
                    "Joined",
                    "Uploads",
                    "Favorites",
                    "Subscribers",
                    "Profile Views",
                    "Profile Comments",
                    "Forum Posts",
                    "Wallpapers Tagged",
                    "Wallpapers Flagged",
                ]
                .contains(&label.as_str())
                {
                    continue;
                }
                let value = e
                    .select(&selector("time"))
                    .next()
                    .and_then(|e| {
                        e.value()
                            .attr("datetime")
                            .or_else(|| e.value().attr("title"))
                    })
                    .map(|s| {
                        s.chars()
                            .filter(|c| !c.is_control())
                            .take(100)
                            .collect::<String>()
                    })
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| text(e, 200));
                if let Some(value) = value {
                    if !fields.iter().any(|f| f.label == label) {
                        fields.push(Field { label, value });
                    }
                }
            }
        }
    }
    Ok(Profile {
        username: name,
        group,
        biography,
        avatar,
        fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const HTML: &str = r#"<header id="profile-header"><a class="avatar"><img src="//wallhaven.cc/images/user/avatar/200/test.jpg"></a><div id="user"><h1 class="username"><a href="/user/test">test</a></h1><h4 class="groupname">User</h4></div></header><section id="profile-content"><article id="profile-bio"><p>Hello &amp; welcome</p><script>secret</script></article><div class="comments"><dl class="datalist"><dt>Uploads</dt><dd>999</dd></dl></div><div class="user-info-right"><dl class="datalist"><dt>Joined</dt><dd><time datetime="2026-01-01">ago</time></dd><dt>Uploads</dt><dd>12</dd></dl></div></section>"#;
    #[test]
    fn native_fields_are_plain_text_and_scoped() {
        let p = parse(HTML, "test").unwrap();
        assert_eq!(p.biography.as_deref(), Some("Hello & welcome"));
        assert_eq!(
            p.avatar.as_deref(),
            Some("https://wallhaven.cc/images/user/avatar/200/test.jpg")
        );
        assert_eq!(p.fields.len(), 2);
        assert_eq!(p.fields[0].value, "2026-01-01");
        assert_eq!(p.fields[1].value, "12");
        assert!(!serde_json::to_string(&p).unwrap().contains("secret"));
    }
    #[test]
    fn wrong_identity_and_untrusted_links_fail() {
        assert!(parse(HTML, "other").is_err());
        assert!(parse(HTML, "Test").is_err());
        assert!(parse("<title>Just a moment</title>", "test").is_err());
        for href in [
            "https://evil.test/user/test",
            "https://wallhaven.cc@evil.test/user/test",
            "/user/test?apikey=secret",
            "/user/test#x",
            "/user/%74est",
        ] {
            assert!(parse(
                &HTML.replace("href=\"/user/test\"", &format!("href=\"{href}\"")),
                "test"
            )
            .is_err());
        }
    }
}
