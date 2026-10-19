//! Validation and API mapping for desktop search controls; no network or UI side effects.
use wh_core::SearchQuery;

pub const COLORS: &[&str] = &[
    "660000", "990000", "cc0000", "cc3333", "ea4c88", "993399", "663399", "333399", "0066cc",
    "0099cc", "66cccc", "77cc33", "669900", "336600", "666600", "999900", "cccc33", "ffff00",
    "ffcc33", "ff9900", "ff6600", "cc6633", "996633", "663300", "000000", "999999", "cccccc",
    "ffffff", "424153",
];

#[derive(Default)]
pub struct Filters {
    pub ascending: bool,
    pub resolution: i32,
    pub width: String,
    pub height: String,
    pub ratios: Vec<&'static str>,
    pub color: String,
    pub top_range: i32,
}

impl Filters {
    pub fn apply(&self, query: &mut SearchQuery) -> Result<(), String> {
        let atleast = match self.resolution {
            0 => None,
            1 => Some("1920x1080".into()),
            2 => Some("2560x1440".into()),
            3 => Some("3840x2160".into()),
            4 => {
                let dimension = |value: &str| {
                    value
                        .trim()
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=32768).contains(n))
                };
                let width = dimension(&self.width).ok_or("最小宽度必须是 1–32768 的整数")?;
                let height = dimension(&self.height).ok_or("最小高度必须是 1–32768 的整数")?;
                Some(format!("{width}x{height}"))
            }
            _ => return Err("请选择有效的分辨率条件".into()),
        };
        let color = self
            .color
            .trim()
            .trim_start_matches('#')
            .to_ascii_lowercase();
        if !color.is_empty() && (color.len() != 6 || !color.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err("颜色应为 6 位十六进制值，例如 66cccc".into());
        }
        const RATIOS: &[&str] = &["16x9", "16x10", "21x9", "4x3", "9x16", "1x1"];
        if self.ratios.iter().any(|ratio| !RATIOS.contains(ratio)) {
            return Err("请选择有效的宽高比".into());
        }
        let top_range = if query.sorting == "toplist" {
            Some(
                ["1d", "3d", "1w", "1M", "3M", "6M", "1y"]
                    .get(self.top_range as usize)
                    .ok_or("请选择有效的榜单时间范围")?
                    .to_string(),
            )
        } else {
            None
        };
        query.order = if self.ascending { "asc" } else { "desc" }.into();
        query.atleast = atleast;
        query.ratios = (!self.ratios.is_empty()).then(|| self.ratios.join(","));
        query.colors = (!color.is_empty()).then_some(color);
        query.top_range = top_range;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_all_controls_to_api_parameters() {
        let mut query = SearchQuery {
            sorting: "toplist".into(),
            ..Default::default()
        };
        Filters {
            ascending: true,
            resolution: 4,
            width: " 3440 ".into(),
            height: "1440".into(),
            ratios: vec!["16x9", "21x9"],
            color: "#66CCCC".into(),
            top_range: 2,
        }
        .apply(&mut query)
        .unwrap();
        let params = query.to_params(1);
        for (key, value) in [
            ("order", "asc"),
            ("atleast", "3440x1440"),
            ("ratios", "16x9,21x9"),
            ("colors", "66cccc"),
            ("topRange", "1w"),
        ] {
            assert!(
                params.iter().any(|(k, v)| k == key && v == value),
                "missing {key}={value}"
            );
        }
    }

    #[test]
    fn invalid_input_is_not_silently_sent() {
        for (width, height) in [
            ("0", "1080"),
            ("abc", "1080"),
            ("1920", ""),
            ("999999", "1080"),
        ] {
            let filters = Filters {
                resolution: 4,
                width: width.into(),
                height: height.into(),
                ..Default::default()
            };
            assert!(filters.apply(&mut SearchQuery::default()).is_err());
        }
        for color in ["#123", "gggggg", "ffffff&purity=111"] {
            assert!(Filters {
                color: color.into(),
                ..Default::default()
            }
            .apply(&mut SearchQuery::default())
            .is_err());
        }
    }

    #[test]
    fn presets_defaults_and_toplist_scope() {
        for (index, expected) in [
            (0, None),
            (1, Some("1920x1080")),
            (2, Some("2560x1440")),
            (3, Some("3840x2160")),
        ] {
            let mut query = SearchQuery::default();
            Filters {
                resolution: index,
                ..Default::default()
            }
            .apply(&mut query)
            .unwrap();
            assert_eq!(query.atleast.as_deref(), expected);
            assert!(query.ratios.is_none());
            assert!(query.colors.is_none());
            assert!(query.top_range.is_none());
        }
        assert!(COLORS
            .iter()
            .all(|color| color.len() == 6 && color.bytes().all(|c| c.is_ascii_hexdigit())));
    }
}
