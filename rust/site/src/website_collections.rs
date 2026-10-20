//! Ported from rust/desktop/src/website_collections.rs, including parser regression tests.
//! Read website HTML without confusing verification pages with empty results.
use scraper::{Html, Selector};
use wh_core::models::{Meta, Thumbs};
use wh_core::{Collection, CollectionsResponse, SearchResponse, Wallpaper};

impl super::website::SiteClient {
    pub fn read_collections(&self, owner: &str) -> Result<CollectionsResponse, String> {
        wh_core::validate_username(owner)?;
        Ok(self.collect_snapshot(Some(owner))?.1)
    }
    pub fn collection_snapshot(&self) -> Result<serde_json::Value, String> {
        let (owner, response) = self.collect_snapshot(None)?;
        Ok(serde_json::json!({"owner":owner,"data":response.data}))
    }
    fn collect_snapshot(
        &self,
        expected: Option<&str>,
    ) -> Result<(String, CollectionsResponse), String> {
        let mut owner = expected.map(str::to_owned);
        let mut path = "/favorites".to_string();
        let mut pages = std::collections::HashSet::new();
        let mut merged = std::collections::BTreeMap::new();
        loop {
            if pages.len() >= 100 || !pages.insert(path.clone()) {
                return Err("收藏夹列表分页异常，未将部分列表冒充完整结果".into());
            }
            let (observed, html) = self
                .read_authenticated_html(&path, owner.as_deref())
                .map_err(|e| e.to_string())?;
            if owner.is_none() {
                owner = Some(observed);
            }
            let response = parse_own_collections(&html)?;
            for collection in response.data {
                if let Some(id) = collection.id {
                    merged.entry(id).or_insert(collection);
                }
            }
            match next_collection_page_for_path(&html, "/favorites")? {
                Some(next) => path = next,
                None => break,
            }
        }
        Ok((
            owner.ok_or("网站账号未确认")?,
            CollectionsResponse {
                data: merged.into_values().collect(),
            },
        ))
    }
    /// Caller controls NSFW policy; the mobile bridge requires both explicit
    /// consent and a currently present key before passing a third purity bit.
    pub fn read_collection_wallpapers(
        &self,
        owner: &str,
        id: u32,
        purity: &str,
        page: u32,
    ) -> Result<SearchResponse, String> {
        wh_core::validate_username(owner)?;
        if id == 0 || page == 0 || !valid_purity(purity) {
            return Err("收藏分页参数无效".into());
        }
        let (_, html) = self
            .read_authenticated_html(
                &format!("/favorites/{id}?page={page}&purity={purity}"),
                Some(owner),
            )
            .map_err(|e| e.to_string())?;
        parse_wallpapers(&html, purity, page)
    }
}
fn valid_purity(purity: &str) -> bool {
    purity.len() == 3 && purity != "000" && purity.bytes().all(|b| b == b'0' || b == b'1')
}
fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static selector")
}
fn guard(html: &str) -> Result<Html, String> {
    let lower = html.to_ascii_lowercase();
    if lower.contains("cf-chl-")
        || lower.contains("challenges.cloudflare.com")
        || lower.contains("just a moment")
    {
        return Err("网站要求本人安全验证，未读取收藏内容".into());
    }
    Ok(Html::parse_document(html))
}
/// Authenticated owner sidebar; never substitute the public profile list.
fn parse_own_collections(html: &str) -> Result<CollectionsResponse, String> {
    parse_collection_list(html, None)
}
#[cfg(test)]
fn parse_collections(html: &str, owner: &str) -> Result<CollectionsResponse, String> {
    wh_core::validate_username(owner)?;
    parse_collection_list(html, Some(owner))
}
fn parse_collection_list(html: &str, owner: Option<&str>) -> Result<CollectionsResponse, String> {
    let document = guard(html)?;
    if document
        .select(&selector("#collections, .collections-list"))
        .next()
        .is_none()
    {
        return Err("网站收藏夹页面结构未识别，未当作空收藏处理".into());
    }
    let mut data = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in document.select(&selector(
        "#collections .collection, .collections-list .collection",
    )) {
        let route_id = |href: &str| match owner {
            Some(owner) => collection_id(href, owner),
            None => own_collection_id(href),
        };
        let link = if item.value().name() == "a"
            && item.value().attr("href").and_then(route_id).is_some()
        {
            Some(item)
        } else {
            item.select(&selector("a[href]"))
                .find(|link| link.value().attr("href").and_then(route_id).is_some())
        };
        let id = link
            .and_then(|link| link.value().attr("href"))
            .and_then(route_id);
        let Some(id) = id else { continue };
        if !seen.insert(id) {
            continue;
        }
        let label = item
            .select(&selector(".collection-label, .collection-name"))
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                link.and_then(|link| {
                    let label = link
                        .descendants()
                        .filter_map(|node| {
                            let text = node.value().as_text()?;
                            let decoration = node
                                .ancestors()
                                .filter_map(scraper::ElementRef::wrap)
                                .any(|e| {
                                    matches!(e.value().name(), "i" | "small")
                                        || e.value().classes().any(|c| {
                                            matches!(c, "collection-count" | "count" | "badge")
                                        })
                                });
                            (!decoration).then_some(text.to_string())
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim()
                        .to_string();
                    (!label.is_empty()).then_some(label)
                })
            });
        let stats = item
            .select(&selector(".collection-stats"))
            .next()
            .map(|e| e.text().collect::<String>());
        let count = stats
            .and_then(|stats| stats.split_whitespace().next().map(|s| s.replace(',', "")))
            .and_then(|s| s.parse::<u32>().ok())
            .or_else(|| {
                item.select(&selector("small, .collection-count, .count"))
                    .next()
                    .and_then(|e| {
                        e.text()
                            .collect::<String>()
                            .replace(',', "")
                            .trim()
                            .parse::<u32>()
                            .ok()
                    })
            });
        let class_visibility = if item
            .value()
            .has_class("private", scraper::CaseSensitivity::AsciiCaseInsensitive)
        {
            Some(false)
        } else if item
            .value()
            .has_class("public", scraper::CaseSensitivity::AsciiCaseInsensitive)
        {
            Some(true)
        } else {
            None
        };
        let visibility = item
            .value()
            .attr("data-public")
            .and_then(|v| match v {
                "1" | "true" => Some(true),
                "0" | "false" => Some(false),
                _ => None,
            })
            .or(class_visibility)
            .or_else(|| {
                link.and_then(|link| {
                    if link
                        .select(&selector(
                            ".fa-lock, .fa-eye-slash, .private, [title='Private']",
                        ))
                        .next()
                        .is_some()
                    {
                        Some(false)
                    } else if link
                        .select(&selector(".fa-globe, .fa-eye, .public, [title='Public']"))
                        .next()
                        .is_some()
                    {
                        Some(true)
                    } else {
                        None
                    }
                })
            })
            .or_else(|| {
                owner.and_then(|_| {
                    // Public profile cards can keep visibility beside the content link.
                    if item
                        .select(&selector(".fa-lock, .private, [title='Private']"))
                        .next()
                        .is_some()
                    {
                        Some(false)
                    } else {
                        None
                    }
                })
            });
        data.push(Collection {
            id: Some(id),
            label,
            count,
            public: visibility,
            ..Default::default()
        });
    }
    if data.is_empty()
        && document
            .select(&selector(
                "#collections .collection, .collections-list .collection",
            ))
            .next()
            .is_some()
    {
        return Err("发现收藏夹条目但无法识别地址，未当作空列表处理".into());
    }
    Ok(CollectionsResponse { data })
}
fn own_collection_id(href: &str) -> Option<u32> {
    let base = url::Url::parse("https://wallhaven.cc/").ok()?;
    let url = base.join(href.trim()).ok()?;
    if url.origin() != base.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts = url.path_segments()?.collect::<Vec<_>>();
    if parts.len() != 2 || parts[0] != "favorites" {
        return None;
    }
    parts[1].parse::<u32>().ok().filter(|id| *id > 0)
}
fn collection_id(href: &str, owner: &str) -> Option<u32> {
    let base = url::Url::parse("https://wallhaven.cc/").ok()?;
    let url = base.join(href.trim()).ok()?;
    if url.origin() != base.origin() || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let parts = url.path_segments()?.collect::<Vec<_>>();
    if parts.len() != 4
        || parts[0] != "user"
        || !parts[1].eq_ignore_ascii_case(owner)
        || parts[2] != "favorites"
    {
        return None;
    }
    parts[3].parse::<u32>().ok().filter(|id| *id > 0)
}
#[cfg(test)]
fn next_collection_page(html: &str, owner: &str) -> Result<Option<String>, String> {
    next_collection_page_for_path(html, &format!("/user/{owner}/favorites"))
}
fn next_collection_page_for_path(html: &str, expected: &str) -> Result<Option<String>, String> {
    let document = guard(html)?;
    let base = url::Url::parse("https://wallhaven.cc/").unwrap();
    let next_selector = if expected == "/favorites" {
        // Wallpaper pagination is not collection-list pagination.
        ".collections-list .pagination a[rel='next'], #collections .pagination a[rel='next'], .collections-pagination a[rel='next']"
    } else {
        ".pagination a[rel='next'], .pagination a.next, a[rel='next']"
    };
    for link in document.select(&selector(next_selector)) {
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let url = base.join(href.trim()).map_err(|_| "收藏夹分页地址无效")?;
        if url.origin() != base.origin()
            || !url.path().eq_ignore_ascii_case(expected)
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("拒绝非当前账号的收藏夹分页地址".into());
        }
        return Ok(Some(format!(
            "{}{}",
            url.path(),
            url.query().map(|q| format!("?{q}")).unwrap_or_default()
        )));
    }
    Ok(None)
}
fn parse_wallpapers(html: &str, purity: &str, page: u32) -> Result<SearchResponse, String> {
    if page == 0 || !valid_purity(purity) {
        return Err("收藏分页参数无效".into());
    }
    let document = guard(html)?;
    if document.select(&selector("#thumbs")).next().is_none() {
        return Err("网站收藏内容结构未识别".into());
    }
    let mut data = Vec::new();
    for item in document.select(&selector("#thumbs figure[data-wallpaper-id]")) {
        let Some(id) = item.value().attr("data-wallpaper-id") else {
            continue;
        };
        if id.is_empty() || id.len() > 16 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            continue;
        }
        let classes = item.value().classes().collect::<Vec<_>>();
        let (kind, index) = if classes.contains(&"thumb-sfw") {
            ("sfw", 0)
        } else if classes.contains(&"thumb-sketchy") {
            ("sketchy", 1)
        } else if classes.contains(&"thumb-nsfw") {
            ("nsfw", 2)
        } else {
            continue;
        };
        if purity.as_bytes()[index] != b'1' {
            continue;
        }
        let dimensions = item
            .select(&selector(".wall-res"))
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default();
        let (width, height) = dimensions
            .split_once('x')
            .and_then(|(w, h)| Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?)))
            .unwrap_or((0, 0));
        let image = item
            .select(&selector("img"))
            .next()
            .and_then(|e| e.value().attr("data-src").or_else(|| e.value().attr("src")))
            .filter(|url| safe_thumb(url))
            .map(str::to_owned);
        let category = ["general", "anime", "people"]
            .into_iter()
            .find(|category| classes.contains(&format!("thumb-{category}").as_str()));
        data.push(Wallpaper {
            id: id.into(),
            url: Some(format!("https://wallhaven.cc/w/{id}")),
            dimension_x: (width > 0).then_some(width),
            dimension_y: (height > 0).then_some(height),
            resolution: (width > 0 && height > 0).then(|| format!("{width}x{height}")),
            purity: Some(kind.into()),
            category: category.map(str::to_owned),
            thumbs: image.map(|small| Thumbs {
                original: uncropped_thumbnail(&small, id),
                small: Some(small),
                ..Default::default()
            }),
            ..Default::default()
        });
    }
    let last_page = document
        .select(&selector("[data-pagination]"))
        .next()
        .and_then(|e| e.value().attr("data-pagination"))
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .and_then(|value| {
            value
                .get("total")
                .and_then(|n| n.as_u64())
                .and_then(|n| u32::try_from(n).ok())
        })
        .unwrap_or(page);
    Ok(SearchResponse {
        data,
        meta: Meta {
            current_page: Some(page),
            last_page: Some(last_page.max(page)),
            ..Default::default()
        },
    })
}
// Rewrite only canonical CDN paths for the current wallpaper, preserving aspect ratio.
fn uncropped_thumbnail(value: &str, id: &str) -> Option<String> {
    if !safe_thumb(value) || id.len() < 2 {
        return None;
    }
    let mut url = url::Url::parse(value).ok()?;
    let prefix = id.get(..2)?;
    if !["small", "lg", "orig"]
        .iter()
        .any(|kind| url.path() == format!("/{kind}/{prefix}/{id}.jpg"))
    {
        return None;
    }
    url.set_path(&format!("/orig/{prefix}/{id}.jpg"));
    Some(url.into())
}
fn safe_thumb(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("th.wallhaven.cc")
            && url.username().is_empty()
            && url.password().is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uncropped_thumbnails_only_rewrite_matching_cdn_paths() {
        for kind in ["small", "lg", "orig"] {
            assert_eq!(
                uncropped_thumbnail(
                    &format!("https://th.wallhaven.cc/{kind}/6l/6lw16w.jpg"),
                    "6lw16w"
                )
                .as_deref(),
                Some("https://th.wallhaven.cc/orig/6l/6lw16w.jpg")
            );
        }
        for url in [
            "https://evil.test/small/6l/6lw16w.jpg",
            "https://th.wallhaven.cc/small/ab/abc123.jpg",
            "https://th.wallhaven.cc/unrecognized.jpg",
        ] {
            assert!(uncropped_thumbnail(url, "6lw16w").is_none());
        }
        assert!(uncropped_thumbnail("https://th.wallhaven.cc/small/6l/6lw16w.jpg", "中").is_none());
    }
    #[test]
    fn parses_collection_links_and_counts_without_external_routes() {
        let response = parse_collections(r#"<ul id="collections"><li class="collection"><a class="collection-inner" href=" https://wallhaven.cc/user/Test/favorites/12"><div class="collection-label">Mountains</div><span class="collection-stats">1,066 7,720</span></a></li><li class="collection"><a href="https://evil.test/user/Test/favorites/13">bad</a></li></ul>"#, "Test").unwrap();
        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].id, Some(12));
        assert_eq!(response.data[0].count, Some(1066));
    }
    #[test]
    fn empty_collections_and_security_page_are_distinct() {
        assert!(parse_collections("<ul id='collections'></ul>", "Test")
            .unwrap()
            .data
            .is_empty());
        assert!(parse_collections("<div>Just a moment</div>", "Test").is_err());
        assert!(parse_collections("<form id='login'></form>", "Test").is_err());
    }
    #[test]
    fn preserves_aspect_and_filters_purity_and_thumbnail_hosts() {
        let html = r#"<div id="thumbs"><figure class="thumb-sfw thumb-general" data-wallpaper-id="abc123"><img data-src="https://th.wallhaven.cc/small/ab/abc123.jpg"><span class="wall-res">1920 x 1080</span></figure><figure class="thumb-nsfw" data-wallpaper-id="def456"><img data-src="https://evil.test/image"><span class="wall-res">1000 x 1600</span></figure></div><ul data-pagination='{"total":5,"current":1}'></ul>"#;
        let response = parse_wallpapers(html, "100", 1).unwrap();
        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].dimension_x, Some(1920));
        assert_eq!(response.meta.last_page, Some(5));
        assert_eq!(
            response.data[0].thumb_url(),
            Some("https://th.wallhaven.cc/orig/ab/abc123.jpg"),
            "website collection cards must use uncropped thumbnail too"
        );
        assert!(parse_wallpapers(html, "111", 1).unwrap().data[1]
            .thumbs
            .is_none());
        assert!(parse_wallpapers(html, "000", 1).is_err());
    }
    #[test]
    fn anchor_items_do_not_disappear_and_names_are_preserved() {
        let html = r#"<ul class="collections-list"><a class="collection" data-collection-id="12" href="/user/Test/favorites/12"><span class="collection-label">中文 very long collection label</span><small>0</small></a><li class="collection"><a href="/user/Test/favorites/13"><span class="collection-label">Private</span><i class="fa-lock"></i><small>2</small></a></li></ul>"#;
        let result = parse_collections(html, "Test").unwrap();
        assert_eq!(result.data.len(), 2);
        assert_eq!(
            result.data[0].label.as_deref(),
            Some("中文 very long collection label")
        );
        assert_eq!(result.data[1].public, Some(false));
    }
    #[test]
    fn pagination_is_only_allowed_for_same_owner() {
        let html = r#"<ul class="pagination"><a rel="next" href="/user/Test/favorites?page=2">Next</a></ul>"#;
        assert_eq!(
            next_collection_page(html, "Test").unwrap().as_deref(),
            Some("/user/Test/favorites?page=2")
        );
        assert!(next_collection_page(html, "Other").is_err());
        assert!(next_collection_page(
            r#"<a rel="next" href="https://evil.test/user/Test/favorites?page=2">Next</a>"#,
            "Test"
        )
        .is_err());
    }
    #[test]
    fn owner_sidebar_default_uses_real_id_and_ignores_trash_and_image_pagination() {
        let html = r#"<ul class="collections-list"><li class="collection" data-collection-id="12"><a href="/favorites/12"><i class="fa-star"></i><i class="fa-eye-slash"></i><small>4</small>Default</a></li><li class="collection"><a href="/favorites/trash">Trash</a></li></ul><ul class="pagination"><a rel="next" href="/favorites/12?page=2">Next images</a></ul>"#;
        let list = parse_own_collections(html).unwrap();
        assert_eq!(list.data.len(), 1);
        assert_eq!(list.data[0].label.as_deref(), Some("Default"));
        assert_eq!(list.data[0].id, Some(12));
        assert_eq!(list.data[0].count, Some(4));
        assert_eq!(list.data[0].public, Some(false));
        assert!(next_collection_page_for_path(html, "/favorites")
            .unwrap()
            .is_none());
        for href in [
            "https://evil.test/favorites/12",
            "/favorites/trash",
            "/favorites/0",
            "/user/Other/favorites/12",
            "/favorites/12?delete=1",
        ] {
            assert!(own_collection_id(href).is_none());
        }
    }
    #[test]
    fn missing_and_unselected_purity_are_stripped() {
        let html = r#"<div id="thumbs"><figure data-wallpaper-id="abc123"></figure><figure class="thumb-sketchy" data-wallpaper-id="def456"></figure><figure class="thumb-sfw" data-wallpaper-id="xyz789"></figure></div>"#;
        let response = parse_wallpapers(html, "100", 1).unwrap();
        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].id, "xyz789");
    }
    #[test]
    fn owner_collection_pagination_rejects_external_and_wallpaper_routes() {
        for href in [
            "https://evil.test/favorites?page=2",
            "/favorites/12?page=2",
            "/user/Other/favorites?page=2",
        ] {
            let html = format!(
                r#"<div class="collections-pagination"><a rel="next" href="{href}">Next</a></div>"#
            );
            assert!(next_collection_page_for_path(&html, "/favorites").is_err());
        }
    }
}
