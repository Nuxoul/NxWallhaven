//! Conservative website-form management ported from rust/desktop/src/collection_management.rs.
//! Includes the original parsing tests. Never guesses write routes or retries a POST.
use crate::website::{SiteClient, SiteError};
use scraper::{Html, Selector};
use serde::Deserialize;
use wh_core::CollectionsResponse;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ManageOperation {
    Create {
        label: String,
        public: bool,
    },
    Update {
        id: u32,
        label: String,
        public: bool,
    },
    Delete {
        id: u32,
    },
}
struct FormAction {
    action: String,
    fields: Vec<(String, String)>,
    csrf: String,
}
fn selector(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}
fn same_site_path(href: &str, base: &str) -> Option<url::Url> {
    let base = url::Url::parse(base).ok()?;
    let url = base.join(href.trim()).ok()?;
    (url.scheme() == "https"
        && url.host_str() == Some("wallhaven.cc")
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}
fn collection_route(url: &url::Url) -> bool {
    url.path_segments().is_some_and(|parts| {
        parts
            .into_iter()
            .any(|part| matches!(part, "collection" | "collections" | "favorites" | "fav"))
    })
}
fn target_id(operation: &ManageOperation) -> Option<u32> {
    match operation {
        ManageOperation::Create { .. } => None,
        ManageOperation::Update { id, .. } | ManageOperation::Delete { id } => Some(*id),
    }
}
fn validate(operation: &ManageOperation) -> Result<(), String> {
    if target_id(operation) == Some(0) {
        return Err("收藏夹 ID 无效".into());
    }
    if let ManageOperation::Create { label, .. } | ManageOperation::Update { label, .. } = operation
    {
        if label.trim().is_empty()
            || label.chars().count() > 100
            || label.chars().any(char::is_control)
        {
            return Err("收藏夹名称需为 1–100 个可显示字符".into());
        }
    }
    Ok(())
}
fn parse_form(
    html: &str,
    page: &str,
    operation: &ManageOperation,
) -> Result<Option<FormAction>, String> {
    let doc = Html::parse_document(html);
    let meta = doc
        .select(&selector("meta[name='csrf-token']"))
        .next()
        .and_then(|e| e.value().attr("content"));
    for form in doc.select(&selector("form[action]")) {
        let Some(action) = same_site_path(form.value().attr("action").unwrap_or_default(), page)
        else {
            continue;
        };
        if !collection_route(&action) {
            continue;
        }
        let inputs = form
            .select(&selector("input[name],select[name],textarea[name]"))
            .collect::<Vec<_>>();
        if inputs
            .iter()
            .any(|e| e.value().attr("type") == Some("password"))
        {
            continue;
        }
        if let Some(id) = target_id(operation) {
            let id_text = id.to_string();
            let scoped = action
                .path_segments()
                .is_some_and(|parts| parts.into_iter().any(|p| p == id_text))
                || inputs.iter().any(|e| {
                    matches!(e.value().attr("name"), Some("id" | "collection_id"))
                        && e.value().attr("value") == Some(id_text.as_str())
                });
            if !scoped {
                continue;
            }
        }
        let method = form
            .value()
            .attr("method")
            .unwrap_or("get")
            .to_ascii_lowercase();
        let override_method = inputs
            .iter()
            .find(|e| e.value().attr("name") == Some("_method"))
            .and_then(|e| e.value().attr("value"))
            .unwrap_or("")
            .to_ascii_lowercase();
        let hints = format!(
            "{} {} {}",
            form.value().attr("id").unwrap_or(""),
            form.value().attr("class").unwrap_or(""),
            action.path()
        )
        .to_ascii_lowercase();
        let deletion = override_method == "delete"
            || method == "delete"
            || hints.contains("delete")
            || hints.contains("destroy");
        let label_field = inputs.iter().find(|e| {
            matches!(
                e.value().attr("name"),
                Some("label" | "name" | "collection_name" | "collection_label")
            )
        });
        match operation {
            ManageOperation::Delete { .. } => {
                if !deletion {
                    continue;
                }
            }
            ManageOperation::Create { .. } => {
                if deletion
                    || action
                        .path_segments()
                        .is_some_and(|parts| parts.into_iter().any(|p| p.parse::<u32>().is_ok()))
                    || label_field.is_none()
                    || override_method == "put"
                    || method == "put"
                    || method == "patch"
                    || inputs
                        .iter()
                        .any(|e| matches!(e.value().attr("name"), Some("id" | "collection_id")))
                    || override_method == "patch"
                    || hints.contains("update")
                    || hints.contains("edit")
                {
                    continue;
                }
            }
            ManageOperation::Update { .. } => {
                if deletion || label_field.is_none() {
                    continue;
                }
            }
        }
        if !matches!(method.as_str(), "post" | "put" | "patch" | "delete") {
            continue;
        }
        let mut fields = Vec::new();
        for input in &inputs {
            if input.value().attr("type") == Some("hidden")
                && input.value().attr("disabled").is_none()
            {
                fields.push((
                    input.value().attr("name").unwrap_or_default().into(),
                    input.value().attr("value").unwrap_or_default().into(),
                ));
            }
        }
        if method != "post" && !fields.iter().any(|(k, _)| k == "_method") {
            fields.push(("_method".into(), method.to_ascii_uppercase()));
        }
        let csrf = fields
            .iter()
            .find(|(k, v)| k == "_token" && !v.is_empty())
            .map(|(_, v)| v.clone())
            .or_else(|| meta.map(str::to_owned))
            .filter(|value| !value.is_empty() && value.len() <= 8192)
            .ok_or("网站管理表单缺少 CSRF，未提交")?;
        if !fields.iter().any(|(k, _)| k == "_token") {
            fields.push(("_token".into(), csrf.clone()));
        }
        if let ManageOperation::Create { label, public }
        | ManageOperation::Update { label, public, .. } = operation
        {
            let label_key = label_field
                .unwrap()
                .value()
                .attr("name")
                .unwrap()
                .to_string();
            fields.retain(|(k, _)| k != &label_key);
            fields.push((label_key, label.trim().into()));
            let visibility = inputs
                .iter()
                .find(|e| {
                    matches!(
                        e.value().attr("name"),
                        Some("public" | "is_public" | "visibility" | "collection_public")
                    )
                })
                .ok_or("未识别收藏夹可见性字段，未提交猜测表单")?;
            let key = visibility.value().attr("name").unwrap().to_string();
            let value = if key == "visibility" {
                if *public {
                    "public"
                } else {
                    "private"
                }
            } else if *public {
                "1"
            } else {
                "0"
            };
            fields.retain(|(k, _)| k != &key);
            fields.push((key, value.into()));
        }
        return Ok(Some(FormAction {
            action: action.into(),
            fields,
            csrf,
        }));
    }
    Ok(None)
}
fn management_links(html: &str, page: &str, operation: &ManageOperation) -> Vec<String> {
    let doc = Html::parse_document(html);
    let id = target_id(operation).map(|id| id.to_string());
    let mut links = Vec::new();
    for link in doc.select(&selector("a[href],a[data-href],button[data-href]")) {
        let Some(href) = link
            .value()
            .attr("data-href")
            .or_else(|| link.value().attr("href"))
        else {
            continue;
        };
        let Some(url) = same_site_path(href, page) else {
            continue;
        };
        if !collection_route(&url) {
            continue;
        }
        let hints = format!(
            "{} {} {}",
            link.value().attr("class").unwrap_or(""),
            link.value().attr("id").unwrap_or(""),
            url.path()
        )
        .to_ascii_lowercase();
        let relevant = match operation {
            ManageOperation::Create { .. } => {
                hints.contains("create") || hints.contains("new") || hints.contains("add")
            }
            ManageOperation::Update { .. } => hints.contains("edit") || hints.contains("manage"),
            ManageOperation::Delete { .. } => {
                hints.contains("delete") || hints.contains("edit") || hints.contains("manage")
            }
        };
        if !relevant || hints.contains("delete") || hints.contains("destroy") {
            continue;
        }
        if let Some(id) = &id {
            if !url
                .path_segments()
                .is_some_and(|p| p.into_iter().any(|v| v == id))
                && !hints.contains("manage")
            {
                continue;
            }
        }
        let path = format!(
            "{}{}",
            url.path(),
            url.query().map(|q| format!("?{q}")).unwrap_or_default()
        );
        if !links.contains(&path) {
            links.push(path);
        }
    }
    links
}
impl SiteClient {
    /// Returns the refreshed, verified collection list. A lost response is only
    /// followed by reads to reconcile; it is never followed by another POST.
    pub fn manage_collection(
        &self,
        owner: &str,
        operation: ManageOperation,
    ) -> Result<CollectionsResponse, String> {
        validate(&operation)?;
        self.verify_owner(owner).map_err(|e| e.to_string())?;
        let before = self.read_collections(owner)?;
        if let Some(id) = target_id(&operation) {
            if !before.data.iter().any(|row| row.id == Some(id)) {
                return Err("选中收藏夹已不存在，请刷新".into());
            }
        }
        let path = match target_id(&operation) {
            Some(id) => format!("/favorites/{id}"),
            None => "/favorites".into(),
        };
        let html = self.read_html(&path).map_err(|e| e.to_string())?;
        let page = format!("https://wallhaven.cc{path}");
        let mut form = parse_form(&html, &page, &operation)?;
        if form.is_none() {
            for link in management_links(&html, &page, &operation)
                .into_iter()
                .take(8)
            {
                let html = self.read_html(&link).map_err(|e| e.to_string())?;
                // Some AJAX links return {view: html} rather than a full page.
                let view = serde_json::from_str::<serde_json::Value>(&html)
                    .ok()
                    .and_then(|v| v.get("view").and_then(|x| x.as_str()).map(str::to_owned))
                    .unwrap_or(html);
                form = parse_form(&view, &format!("https://wallhaven.cc{link}"), &operation)?;
                if form.is_some() {
                    break;
                }
            }
        }
        let form = form.ok_or(
            "当前网站页面未提供可识别的管理表单，未发送猜测请求；请在应用内网页登录窗口管理",
        )?;
        self.verify_owner(owner).map_err(|e| e.to_string())?;
        let submitted = self.submit_form(&form.action, &form.fields, &form.csrf);
        if matches!(
            submitted,
            Err(SiteError::NotAuthenticated | SiteError::HumanVerificationRequired)
        ) {
            return Err(submitted.err().unwrap().to_string());
        }
        let after = self
            .read_collections(owner)
            .map_err(|_| "管理请求已发送，但读取核对失败；请刷新，未自动重试".to_string())?;
        let verified = match &operation {
            ManageOperation::Create { label, public } => after.data.iter().any(|row| {
                row.label.as_deref() == Some(label.trim())
                    && row.public == Some(*public)
                    && !before.data.iter().any(|old| old.id == row.id)
            }),
            ManageOperation::Update { id, label, public } => after.data.iter().any(|row| {
                row.id == Some(*id)
                    && row.label.as_deref() == Some(label.trim())
                    && row.public == Some(*public)
            }),
            ManageOperation::Delete { id } => !after.data.iter().any(|row| row.id == Some(*id)),
        };
        if verified {
            Ok(after)
        } else {
            Err("网站管理请求已发送，但最终名称/可见性/删除状态未确认；请刷新，未自动重试".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_fields_and_preserves_csrf() {
        let h = r#"<form action="/collections/12" method="post"><input type="hidden" name="_token" value="fixture"><input type="hidden" name="_method" value="PATCH"><input name="label"><input type="checkbox" name="public"></form>"#;
        let f = parse_form(
            h,
            "https://wallhaven.cc/",
            &ManageOperation::Update {
                id: 12,
                label: "中文 收藏".into(),
                public: false,
            },
        )
        .unwrap()
        .unwrap();
        assert!(f.fields.contains(&("label".into(), "中文 收藏".into())));
        assert!(f.fields.contains(&("public".into(), "0".into())));
        assert_eq!(f.csrf, "fixture");
    }
    #[test]
    fn deletion_requires_scoped_form() {
        let h = r#"<form id="collection-delete" action="/collections/12" method="post"><input type="hidden" name="_token" value="fixture"><input type="hidden" name="_method" value="DELETE"></form>"#;
        assert!(parse_form(
            h,
            "https://wallhaven.cc/",
            &ManageOperation::Delete { id: 12 }
        )
        .unwrap()
        .is_some());
        assert!(parse_form(
            h,
            "https://wallhaven.cc/",
            &ManageOperation::Delete { id: 13 }
        )
        .unwrap()
        .is_none());
    }
    #[test]
    fn external_or_missing_csrf_never_submits() {
        let h = r#"<form action="https://evil.test/collections" method="post"><input name="label"><input name="public"></form>"#;
        let op = ManageOperation::Create {
            label: "new".into(),
            public: true,
        };
        assert!(parse_form(h, "https://wallhaven.cc/", &op)
            .unwrap()
            .is_none());
        assert!(parse_form(
            &h.replace("https://evil.test", "https://wallhaven.cc"),
            "https://wallhaven.cc/",
            &op
        )
        .is_err());
    }
    #[test]
    fn create_cannot_select_update_form() {
        let h = r#"<form action="/collections/12/edit" method="post"><input type="hidden" name="_token" value="fixture"><input name="label"><input name="public"></form>"#;
        assert!(parse_form(
            h,
            "https://wallhaven.cc/",
            &ManageOperation::Create {
                label: "new".into(),
                public: true
            }
        )
        .unwrap()
        .is_none());
        let numeric = h.replace("/collections/12/edit", "/collections/12");
        assert!(parse_form(
            &numeric,
            "https://wallhaven.cc/",
            &ManageOperation::Create {
                label: "new".into(),
                public: false
            }
        )
        .unwrap()
        .is_none());
    }
    #[test]
    fn invalid_operations_rejected() {
        assert!(validate(&ManageOperation::Delete { id: 0 }).is_err());
        assert!(validate(&ManageOperation::Create {
            label: "".into(),
            public: false
        })
        .is_err());
        assert!(validate(&ManageOperation::Update {
            id: 1,
            label: "bad\nname".into(),
            public: true
        })
        .is_err());
    }
    #[test]
    fn management_links_stay_same_site_and_target() {
        let h = r#"<a class="edit" href="/collections/12/edit">Edit</a><a class="edit" href="https://evil.test/collections/12/edit">Bad</a><a href="/collections/13/edit">Other</a>"#;
        let links = management_links(
            h,
            "https://wallhaven.cc/",
            &ManageOperation::Update {
                id: 12,
                label: "x".into(),
                public: true,
            },
        );
        assert_eq!(links, vec!["/collections/12/edit"]);
    }
}
