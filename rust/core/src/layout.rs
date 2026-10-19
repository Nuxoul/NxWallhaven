//! 大小自适应瀑布流布局（Justified / Row-based Masonry）
//!
//! 这正是用户要的效果：**每行高度统一，行内图片宽度按原始比例分配，整行铺满容器宽度**。
//! 与「等宽多列瀑布流」的区别：
//!   - 等宽多列：列宽固定，行不对齐，像 Pinterest
//!   - 本算法：行高固定（目标高度），行内宽度自适应，每行右边缘对齐，像 Flickr / 你给的截图
//!
//! 算法：
//!   1. 依次把图片加入当前行
//!   2. 当"按目标行高排列后总宽 >= 容器宽"时，结算这一行
//!   3. 结算时按比例缩放该行所有图片，使总宽恰好等于容器宽
//!
//! 复杂度 O(n)，宽度变化时重算即可，无需重新请求网络。

/// 一张图在布局中的最终矩形
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl LayoutRect {
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
}

/// 一行
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutRow {
    pub y: f64,
    pub height: f64,
    /// 每张图在行内的矩形（x 相对容器）
    pub items: Vec<LayoutRect>,
}

impl LayoutRow {
    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }
}

/// 布局中的一项：索引 + 矩形（索引指向原始数据数组）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutItem {
    pub index: usize,
    pub rect: LayoutRect,
}

/// 布局结果
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JustifiedLayout {
    pub rows: Vec<LayoutRow>,
    /// 内容总高度
    pub content_height: f64,
}

impl JustifiedLayout {
    /// 拍平为一维列表（方便按索引查矩形）
    pub fn items(&self) -> Vec<LayoutItem> {
        let mut out = Vec::new();
        for row in &self.rows {
            for r in &row.items {
                out.push(LayoutItem {
                    index: out.len(),
                    rect: *r,
                });
            }
        }
        out
    }

    /// 根据 y 坐标找到可见的行范围 [start, end)
    pub fn visible_rows(&self, scroll_y: f64, viewport_h: f64) -> (usize, usize) {
        let top = scroll_y;
        let bottom = scroll_y + viewport_h;
        let mut start = 0;
        let mut end = self.rows.len();
        for (i, row) in self.rows.iter().enumerate() {
            if row.bottom() < top {
                start = i + 1;
            } else if row.y > bottom {
                end = i;
                break;
            }
        }
        if start > end {
            start = end;
        }
        (start, end)
    }
}

/// 布局参数
#[derive(Debug, Clone)]
pub struct LayoutConfig {
    /// 容器宽度（可用像素）
    pub container_width: f64,
    /// 目标行高（图片会在此基础上微调以铺满）
    pub target_row_height: f64,
    /// 图片间距
    pub gap: f64,
    /// 单图最小高度限制（避免超宽图太扁）
    pub min_item_height: f64,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            container_width: 1200.0,
            target_row_height: 220.0,
            gap: 8.0,
            min_item_height: 80.0,
        }
    }
}

/// 计算 justified 布局
///
/// `aspect_ratios`: 每张图的 宽/高
pub fn compute(aspect_ratios: &[f64], config: &LayoutConfig) -> JustifiedLayout {
    let n = aspect_ratios.len();
    if n == 0 || config.container_width <= 0.0 {
        return JustifiedLayout::default();
    }

    let gap = config.gap.max(0.0);
    let cw = config.container_width;

    let mut rows: Vec<LayoutRow> = Vec::new();
    let mut y = 0.0f64;

    // 当前正在累积的行
    let mut row_ar: Vec<f64> = Vec::new();
    let mut row_idx: Vec<usize> = Vec::new();

    let flush = |row_ar: &mut Vec<f64>,
                 row_idx: &mut Vec<usize>,
                 y: &mut f64,
                 rows: &mut Vec<LayoutRow>,
                 is_last: bool| {
        if row_ar.is_empty() {
            return;
        }
        let count = row_ar.len();
        let gaps_total = gap * (count as f64 - 1.0).max(0.0);
        let ar_sum: f64 = row_ar.iter().sum();
        if ar_sum <= 0.0 {
            row_ar.clear();
            row_idx.clear();
            return;
        }

        // 该行按目标高度排列后的总宽度
        let natural_w = ar_sum * config.target_row_height + gaps_total;

        // 结算行高：让总宽恰好等于容器宽
        // 末行（不满）不拉伸，用目标高度，避免最后一行被过度放大
        let mut row_h = if natural_w > 0.0 {
            (cw - gaps_total) / ar_sum
        } else {
            config.target_row_height
        };
        if is_last && natural_w < cw {
            row_h = config.target_row_height;
        }
        row_h = row_h.max(config.min_item_height);

        // 按比例分配宽度
        let mut x = 0.0f64;
        let mut items = Vec::with_capacity(count);
        for (i, ar) in row_ar.iter().enumerate() {
            let w = if is_last && natural_w < cw {
                // 末行不铺满：按目标高度给宽度，保持比例
                ar * row_h
            } else {
                ar * row_h
            };
            items.push(LayoutRect {
                x,
                y: *y,
                w,
                h: row_h,
            });
            x += w + gap;
            let _ = i;
        }
        let _ = row_idx;

        rows.push(LayoutRow {
            y: *y,
            height: row_h,
            items,
        });
        *y += row_h + gap;

        row_ar.clear();
        row_idx.clear();
    };

    for (i, &ar) in aspect_ratios.iter().enumerate() {
        let ar = if ar.is_finite() && ar > 0.0 {
            ar
        } else {
            16.0 / 9.0
        };
        row_ar.push(ar);
        row_idx.push(i);

        let gaps_total = gap * (row_ar.len() as f64 - 1.0).max(0.0);
        let natural_w: f64 = row_ar.iter().sum::<f64>() * config.target_row_height + gaps_total;

        if natural_w >= cw {
            flush(&mut row_ar, &mut row_idx, &mut y, &mut rows, false);
        }
    }

    // 末行
    flush(&mut row_ar, &mut row_idx, &mut y, &mut rows, true);

    let content_height = rows.last().map(|r| r.bottom()).unwrap_or(0.0);
    JustifiedLayout {
        rows,
        content_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(width: f64) -> LayoutConfig {
        LayoutConfig {
            container_width: width,
            target_row_height: 200.0,
            gap: 10.0,
            min_item_height: 50.0,
        }
    }

    #[test]
    fn empty_input_gives_empty_layout() {
        let l = compute(&[], &cfg(1000.0));
        assert_eq!(l.rows.len(), 0);
        assert_eq!(l.content_height, 0.0);
    }

    #[test]
    fn rows_are_flushed_and_widths_fill_container() {
        // 10 张 16:9 图，容器 1000
        let ars = vec![16.0 / 9.0; 10];
        let l = compute(&ars, &cfg(1000.0));
        assert!(l.rows.len() > 1);

        // 除最后一行外，每行总宽应接近容器宽
        for row in &l.rows[..l.rows.len().saturating_sub(1)] {
            let total: f64 = row.items.iter().map(|r| r.w).sum::<f64>()
                + 10.0 * (row.items.len() as f64 - 1.0).max(0.0);
            assert!(
                (total - 1000.0).abs() < 1.0,
                "row width {total} should fill 1000"
            );
        }
    }

    #[test]
    fn all_items_are_preserved() {
        let ars: Vec<f64> = (0..37)
            .map(|i| match i % 3 {
                0 => 16.0 / 9.0,
                1 => 9.0 / 16.0,
                _ => 1.0,
            })
            .collect();
        let l = compute(&ars, &cfg(1200.0));
        let count: usize = l.rows.iter().map(|r| r.items.len()).sum();
        assert_eq!(count, 37);
    }

    #[test]
    fn aspect_ratio_is_preserved_within_row() {
        let ars = vec![2.0, 1.0, 0.5]; // 很宽、方形、很窄
        let l = compute(&ars, &cfg(600.0));
        let row = &l.rows[0];
        for (i, r) in row.items.iter().enumerate() {
            let actual = r.w / r.h;
            assert!(
                (actual - ars[i]).abs() < 0.01,
                "item {i} ratio {actual} != {}",
                ars[i]
            );
        }
    }

    #[test]
    fn items_do_not_overlap_horizontally() {
        let ars = vec![1.5, 2.0, 0.7, 1.0, 3.0, 0.5];
        let l = compute(&ars, &cfg(800.0));
        for row in &l.rows {
            for w in row.items.windows(2) {
                assert!(
                    w[1].x >= w[0].right() - 0.001,
                    "overlap detected: {} < {}",
                    w[1].x,
                    w[0].right()
                );
            }
        }
    }

    #[test]
    fn y_positions_stack_without_gaps() {
        let ars = vec![1.5; 20];
        let l = compute(&ars, &cfg(900.0));
        for w in l.rows.windows(2) {
            assert!((w[1].y - (w[0].y + w[0].height + 10.0)).abs() < 0.001);
        }
    }

    #[test]
    fn last_row_is_not_stretched() {
        // 3 张图，容器很宽 → 不应被拉伸到满宽
        let ars = vec![1.0, 1.0, 1.0];
        let l = compute(&ars, &cfg(3000.0));
        assert_eq!(l.rows.len(), 1);
        let row = &l.rows[0];
        assert!(
            (row.height - 200.0).abs() < 0.001,
            "last row keeps target height"
        );
        let total: f64 = row.items.iter().map(|r| r.w).sum();
        assert!(total < 900.0, "last row should not fill 3000, got {total}");
    }

    #[test]
    fn invalid_ratio_falls_back() {
        let ars = vec![f64::NAN, 0.0, -1.0, 1.5];
        let l = compute(&ars, &cfg(800.0));
        let count: usize = l.rows.iter().map(|r| r.items.len()).sum();
        assert_eq!(count, 4);
        for row in &l.rows {
            for r in &row.items {
                assert!(r.w.is_finite() && r.w > 0.0);
                assert!(r.h.is_finite() && r.h > 0.0);
            }
        }
    }

    #[test]
    fn visible_rows_filters_correctly() {
        let ars = vec![1.5; 30];
        let l = compute(&ars, &cfg(1000.0));
        // 滚到 0，视口 400 → 应包含前几行
        let (s, e) = l.visible_rows(0.0, 400.0);
        assert_eq!(s, 0);
        assert!(e > 0);
        assert!(e < l.rows.len());

        // 滚到很下面
        let (s2, e2) = l.visible_rows(l.content_height - 300.0, 400.0);
        assert!(s2 > 0);
        assert_eq!(e2, l.rows.len());
    }

    #[test]
    fn narrow_container_still_produces_rows() {
        let ars = vec![1.5; 5];
        let l = compute(&ars, &cfg(200.0));
        assert!(!l.rows.is_empty());
        for row in &l.rows {
            for r in &row.items {
                assert!(r.w > 0.0);
            }
        }
    }
}
