//! wallhaven API v1 数据模型
//!
//! 铁律：所有字段均可选。官方原文「changes to be made without warning」。
//! 任何非空假设都会在 API 变更时导致崩溃，故一律 Option + 默认值。

use serde::{Deserialize, Serialize};

/// The authenticated API may encode counters/pagination as decimal strings.
fn optional_number<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + std::str::FromStr,
    T::Err: std::fmt::Display,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire<T> {
        Number(T),
        Text(String),
    }
    match Option::<Wire<T>>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Wire::Number(value)) => Ok(Some(value)),
        Some(Wire::Text(value)) => value
            .trim()
            .parse()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

/// Collection visibility may be encoded as a JSON boolean, 0/1, or "0"/"1".
fn optional_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Bool(bool),
        Number(u8),
        Text(String),
    }

    match Option::<Wire>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Wire::Bool(value)) => Ok(Some(value)),
        Some(Wire::Number(0)) => Ok(Some(false)),
        Some(Wire::Number(1)) => Ok(Some(true)),
        Some(Wire::Text(value)) if value.trim() == "0" => Ok(Some(false)),
        Some(Wire::Text(value)) if value.trim() == "1" => Ok(Some(true)),
        Some(Wire::Number(_) | Wire::Text(_)) => {
            Err(serde::de::Error::custom("expected a boolean or 0/1"))
        }
    }
}

/// 单张壁纸（列表与详情共用；详情多 uploader / tags）
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Wallpaper {
    pub id: String,

    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub short_url: Option<String>,

    /// 原图直链（下载用，不占 API 配额）
    #[serde(default)]
    pub path: Option<String>,

    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default, deserialize_with = "optional_number")]
    pub dimension_x: Option<u32>,
    #[serde(default, deserialize_with = "optional_number")]
    pub dimension_y: Option<u32>,
    /// "1.78" 形式
    #[serde(default)]
    pub ratio: Option<String>,

    #[serde(default, deserialize_with = "optional_number")]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub file_type: Option<String>,

    #[serde(default)]
    pub purity: Option<String>,
    #[serde(default)]
    pub category: Option<String>,

    #[serde(default)]
    pub colors: Vec<String>,

    #[serde(default)]
    pub thumbs: Option<Thumbs>,

    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default, deserialize_with = "optional_number")]
    pub views: Option<u64>,
    #[serde(default, deserialize_with = "optional_number")]
    pub favorites: Option<u64>,
    #[serde(default)]
    pub source: Option<String>,

    /// 仅详情接口返回
    #[serde(default)]
    pub uploader: Option<Uploader>,
    #[serde(default)]
    pub tags: Vec<Tag>,
}

impl Wallpaper {
    /// 宽高比（宽 / 高）。缺失时回退到 ratio 字符串，再回退到 16:9。
    pub fn aspect_ratio(&self) -> f64 {
        if let (Some(w), Some(h)) = (self.dimension_x, self.dimension_y) {
            if w > 0 && h > 0 {
                return w as f64 / h as f64;
            }
        }
        if let Some(r) = &self.ratio {
            if let Ok(v) = r.parse::<f64>() {
                if v > 0.0 {
                    return v;
                }
            }
        }
        16.0 / 9.0
    }

    /// Prefer the uncropped thumbnail: `large`/`small` are fixed-ratio crops.
    /// Card geometry follows the original wallpaper aspect ratio.
    pub fn thumb_url(&self) -> Option<&str> {
        let t = self.thumbs.as_ref()?;
        [
            t.original.as_deref(),
            t.large.as_deref(),
            t.small.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|url| !url.trim().is_empty())
    }

    /// 小缩略图（低内存场景）
    pub fn thumb_small_url(&self) -> Option<&str> {
        let t = self.thumbs.as_ref()?;
        t.small.as_deref().or(t.large.as_deref())
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Thumbs {
    #[serde(default)]
    pub large: Option<String>,
    #[serde(default)]
    pub original: Option<String>,
    #[serde(default)]
    pub small: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Uploader {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub avatar: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Tag {
    #[serde(default, deserialize_with = "optional_number")]
    pub id: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub purity: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Meta {
    #[serde(default, deserialize_with = "optional_number")]
    pub current_page: Option<u32>,
    #[serde(default, deserialize_with = "optional_number")]
    pub last_page: Option<u32>,
    #[serde(default, deserialize_with = "optional_number")]
    pub per_page: Option<u32>,
    #[serde(default, deserialize_with = "optional_number")]
    pub total: Option<u32>,
    #[serde(default)]
    pub seed: Option<String>,
    #[serde(default)]
    pub query: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub data: Vec<Wallpaper>,
    #[serde(default)]
    pub meta: Meta,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DetailResponse {
    #[serde(default)]
    pub data: Option<Wallpaper>,
}

/// Authenticated account settings returned by `/settings`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct UserSettings {
    #[serde(default)]
    pub thumb_size: Option<String>,
    #[serde(default, deserialize_with = "optional_number")]
    pub per_page: Option<u32>,
    #[serde(default)]
    pub purity: Vec<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub resolutions: Vec<String>,
    #[serde(default)]
    pub aspect_ratios: Vec<String>,
    #[serde(default)]
    pub toplist_range: Option<String>,
    #[serde(default)]
    pub tag_blacklist: Vec<String>,
    #[serde(default)]
    pub user_blacklist: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct UserSettingsResponse {
    #[serde(default)]
    pub data: Option<UserSettings>,
}

/// A Wallhaven collection summary.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Collection {
    #[serde(default, deserialize_with = "optional_number")]
    pub id: Option<u32>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, deserialize_with = "optional_number")]
    pub views: Option<u64>,
    #[serde(default, deserialize_with = "optional_bool")]
    pub public: Option<bool>,
    #[serde(default, deserialize_with = "optional_number")]
    pub count: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CollectionsResponse {
    #[serde(default)]
    pub data: Vec<Collection>,
}

/// 搜索参数。与 wallhaven API 一一对应。
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub q: String,
    /// "111" = general+anime+people
    pub categories: String,
    /// "100" = sfw only
    pub purity: String,
    pub sorting: String,
    pub order: String,
    pub top_range: Option<String>,
    pub atleast: Option<String>,
    pub ratios: Option<String>,
    pub colors: Option<String>,
    pub seed: Option<String>,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            q: String::new(),
            categories: "111".into(),
            purity: "100".into(),
            sorting: "date_added".into(),
            order: "desc".into(),
            top_range: None,
            atleast: None,
            ratios: None,
            colors: None,
            seed: None,
        }
    }
}

impl SearchQuery {
    /// 生成查询串（不含 page / apikey）
    pub fn to_params(&self, page: u32) -> Vec<(String, String)> {
        let mut v = vec![
            ("categories".into(), self.categories.clone()),
            ("purity".into(), self.purity.clone()),
            ("sorting".into(), self.sorting.clone()),
            ("order".into(), self.order.clone()),
            ("page".into(), page.to_string()),
        ];
        if let Some(seed) = &self.seed {
            v.push(("seed".into(), seed.clone()));
        }
        if !self.q.trim().is_empty() {
            v.push(("q".into(), self.q.trim().to_string()));
        }
        if self.sorting == "toplist" {
            if let Some(r) = &self.top_range {
                v.push(("topRange".into(), r.clone()));
            }
        }
        if let Some(a) = &self.atleast {
            if !a.is_empty() {
                v.push(("atleast".into(), a.clone()));
            }
        }
        if let Some(r) = &self.ratios {
            if !r.is_empty() {
                v.push(("ratios".into(), r.clone()));
            }
        }
        if let Some(c) = &self.colors {
            if !c.is_empty() {
                v.push(("colors".into(), c.clone()));
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portrait_cards_choose_uncropped_thumbnail_with_safe_fallbacks() {
        let mut wallpaper = Wallpaper {
            dimension_x: Some(3978),
            dimension_y: Some(7087),
            thumbs: Some(Thumbs {
                original: Some("https://th.wallhaven.cc/orig/6l/6lw16w.jpg".into()),
                large: Some("https://th.wallhaven.cc/lg/6l/6lw16w.jpg".into()),
                small: Some("https://th.wallhaven.cc/small/6l/6lw16w.jpg".into()),
            }),
            ..Default::default()
        };
        assert!(
            wallpaper.thumb_url().unwrap().contains("/orig/"),
            "cropped landscape thumbnail causes letterboxing in a portrait card"
        );
        wallpaper.thumbs.as_mut().unwrap().original = Some(String::new());
        assert!(wallpaper.thumb_url().unwrap().contains("/lg/"));
        wallpaper.thumbs.as_mut().unwrap().large = None;
        assert!(wallpaper.thumb_url().unwrap().contains("/small/"));
    }
    #[test]
    fn aspect_from_dimensions() {
        let w = Wallpaper {
            dimension_x: Some(1920),
            dimension_y: Some(1080),
            ..Default::default()
        };
        assert!((w.aspect_ratio() - 1.777777).abs() < 0.001);
    }

    #[test]
    fn aspect_falls_back_to_ratio_string() {
        let w = Wallpaper {
            ratio: Some("1.5".into()),
            ..Default::default()
        };
        assert!((w.aspect_ratio() - 1.5).abs() < 0.001);
    }

    #[test]
    fn aspect_defaults_when_missing() {
        let w = Wallpaper::default();
        assert!((w.aspect_ratio() - 16.0 / 9.0).abs() < 0.001);
    }

    #[test]
    fn parses_real_api_shape() {
        // 实测抓取的真实响应片段
        let json = r##"{
            "data": [{
                "id": "kx33m1",
                "url": "https://wallhaven.cc/w/kx33m1",
                "dimension_x": 7680,
                "dimension_y": 4320,
                "resolution": "7680x4320",
                "ratio": "1.78",
                "file_size": 1385000,
                "file_type": "image/jpeg",
                "purity": "sfw",
                "category": "anime",
                "colors": ["#000000", "#66cccc"],
                "path": "https://w.wallhaven.cc/full/kx/wallhaven-kx33m1.jpg",
                "thumbs": {
                    "large": "https://th.wallhaven.cc/lg/kx/kx33m1.jpg",
                    "original": "https://th.wallhaven.cc/orig/kx/kx33m1.jpg",
                    "small": "https://th.wallhaven.cc/small/kx/kx33m1.jpg"
                }
            }],
            "meta": { "current_page": 1, "last_page": 120, "per_page": 24, "total": 2871 }
        }"##;

        let r: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.data.len(), 1);
        assert_eq!(r.data[0].id, "kx33m1");
        assert_eq!(r.meta.last_page, Some(120));
        assert_eq!(r.data[0].colors.len(), 2);
        assert!(r.data[0].thumb_url().unwrap().contains("/orig/"));
    }

    #[test]
    fn tolerates_missing_fields() {
        // API 变更后字段消失也不能崩
        let json = r#"{"data":[{"id":"abc"}],"meta":{}}"#;
        let r: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.data[0].id, "abc");
        assert!(r.data[0].dimension_x.is_none());
        assert!((r.data[0].aspect_ratio() - 16.0 / 9.0).abs() < 0.001);
    }

    #[test]
    fn query_params_omit_empty() {
        let q = SearchQuery::default();
        let p = q.to_params(1);
        assert!(!p.iter().any(|(k, _)| k == "q"));
        assert!(p.iter().any(|(k, v)| k == "page" && v == "1"));
    }

    #[test]
    fn query_params_include_toprange_only_for_toplist() {
        let mut q = SearchQuery {
            top_range: Some("1M".into()),
            ..Default::default()
        };
        assert!(!q.to_params(1).iter().any(|(k, _)| k == "topRange"));

        q.sorting = "toplist".into();
        assert!(q.to_params(1).iter().any(|(k, _)| k == "topRange"));
    }
}

#[cfg(test)]
mod numeric_wire_tests {
    use super::*;
    #[test]
    fn authenticated_response_accepts_string_numbers() {
        let response: SearchResponse = serde_json::from_str(
            r#"{
            "data":[{"id":"abc123","dimension_x":"1920","dimension_y":1080,
                "file_size":"1234567","views":"42","favorites":3}],
            "meta":{"current_page":"1","last_page":12,"per_page":"24","total":"288"}
        }"#,
        )
        .expect("API key response must accept numeric strings");
        assert_eq!(response.meta.per_page, Some(24));
        assert_eq!(response.meta.total, Some(288));
        assert_eq!(response.data[0].dimension_x, Some(1920));
        assert_eq!(response.data[0].views, Some(42));
    }
    #[test]
    fn absent_and_null_numbers_stay_none() {
        let response: SearchResponse =
            serde_json::from_str(r#"{"data":[],"meta":{"per_page":null}}"#).unwrap();
        assert_eq!(response.meta.per_page, None);
        assert_eq!(response.meta.current_page, None);
    }
    #[test]
    fn malformed_or_overflowing_numbers_are_rejected() {
        for value in [r#""oops""#, "-1", "4294967296"] {
            let payload = format!(r#"{{"data":[],"meta":{{"per_page":{value}}}}}"#);
            assert!(serde_json::from_str::<SearchResponse>(&payload).is_err());
        }
    }
}

#[cfg(test)]
mod account_wire_tests {
    use super::*;

    #[test]
    fn settings_match_official_shape_and_accept_string_per_page() {
        let response: UserSettingsResponse = serde_json::from_str(
            r#"{
                "data": {
                    "thumb_size": "orig",
                    "per_page": "24",
                    "purity": ["sfw", "sketchy"],
                    "categories": ["general", "anime"],
                    "resolutions": [],
                    "aspect_ratios": [],
                    "toplist_range": "6M",
                    "tag_blacklist": [],
                    "user_blacklist": []
                }
            }"#,
        )
        .unwrap();

        let settings = response.data.unwrap();
        assert_eq!(settings.thumb_size.as_deref(), Some("orig"));
        assert_eq!(settings.per_page, Some(24));
        assert_eq!(settings.purity, ["sfw", "sketchy"]);
        assert_eq!(settings.toplist_range.as_deref(), Some("6M"));
    }

    #[test]
    fn settings_data_and_optional_numbers_tolerate_missing_or_null() {
        let missing: UserSettingsResponse = serde_json::from_str("{}").unwrap();
        let null_data: UserSettingsResponse = serde_json::from_str(r#"{"data":null}"#).unwrap();
        let null_per_page: UserSettingsResponse =
            serde_json::from_str(r#"{"data":{"per_page":null}}"#).unwrap();

        assert!(missing.data.is_none());
        assert!(null_data.data.is_none());
        assert_eq!(null_per_page.data.unwrap().per_page, None);
    }

    #[test]
    fn collections_accept_numeric_strings_and_visibility_variants() {
        let response: CollectionsResponse = serde_json::from_str(
            r#"{
                "data": [
                    {"id":15,"label":"Default","views":38,"public":1,"count":10},
                    {"id":"16","views":"39","public":0,"count":"11"},
                    {"public":true},
                    {"public":false},
                    {"public":"1"},
                    {"public":"0"},
                    {"id":null,"views":null,"public":null,"count":null},
                    {}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(response.data[0].id, Some(15));
        assert_eq!(response.data[0].views, Some(38));
        assert_eq!(response.data[0].public, Some(true));
        assert_eq!(response.data[1].id, Some(16));
        assert_eq!(response.data[1].count, Some(11));
        assert_eq!(response.data[1].public, Some(false));
        assert_eq!(response.data[2].public, Some(true));
        assert_eq!(response.data[3].public, Some(false));
        assert_eq!(response.data[4].public, Some(true));
        assert_eq!(response.data[5].public, Some(false));
        assert_eq!(response.data[6].id, None);
        assert_eq!(response.data[6].views, None);
        assert_eq!(response.data[6].public, None);
        assert_eq!(response.data[6].count, None);
        assert_eq!(response.data[7].public, None);
    }

    #[test]
    fn malformed_or_overflowing_account_fields_are_rejected() {
        for per_page in [r#""oops""#, "-1", "4294967296"] {
            let payload = format!(r#"{{"data":{{"per_page":{per_page}}}}}"#);
            assert!(serde_json::from_str::<UserSettingsResponse>(&payload).is_err());
        }

        for public in [r#""true""#, "2", "-1", "256"] {
            let payload = format!(r#"{{"data":[{{"public":{public}}}]}}"#);
            assert!(serde_json::from_str::<CollectionsResponse>(&payload).is_err());
        }

        for (field, value) in [
            ("id", r#""oops""#),
            ("id", "4294967296"),
            ("count", "-1"),
            ("views", r#""18446744073709551616""#),
        ] {
            let payload = format!(r#"{{"data":[{{"{field}":{value}}}]}}"#);
            assert!(serde_json::from_str::<CollectionsResponse>(&payload).is_err());
        }
    }
}
