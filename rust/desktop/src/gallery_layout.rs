//! Keep the default canvas row membership while scaling to the actual viewport.
use super::{CardModel, GAP};
use slint::{Model, ModelRc, VecModel};
use wh_core::layout::{self, LayoutConfig};

pub(super) const MAIN_CANVAS_WIDTH: f32 = 1264.0;
pub(super) const PROFILE_CANVAS_WIDTH: f32 = 912.0;

pub(super) fn arrange(cards: &mut [CardModel], width: f32, mode: i32, base: f32, row_height: f32) {
    let scale = width.max(1.0) / base;
    if mode == 1 {
        let columns = ((base + GAP) / (260.0 + GAP)).floor().max(1.0) as usize;
        let w = (base - GAP * (columns - 1) as f32) / columns as f32;
        for (i, card) in cards.iter_mut().enumerate() {
            card.x = (i % columns) as f32 * (w + GAP) * scale;
            card.y = (i / columns) as f32 * (w / 1.6 + GAP) * scale;
            card.w = w * scale;
            card.h = w / 1.6 * scale;
        }
    } else {
        let ratios = cards
            .iter()
            .map(|card| {
                if card.aspect.is_finite() && card.aspect > 0.0 {
                    card.aspect as f64
                } else {
                    16.0 / 9.0
                }
            })
            .collect::<Vec<_>>();
        let layout = layout::compute(
            &ratios,
            &LayoutConfig {
                container_width: base as f64,
                target_row_height: row_height as f64,
                gap: GAP as f64,
                min_item_height: 1.0,
            },
        );
        for (card, item) in cards.iter_mut().zip(layout.items()) {
            card.x = item.rect.x as f32 * scale;
            card.y = item.rect.y as f32 * scale;
            card.w = item.rect.w as f32 * scale;
            card.h = item.rect.h as f32 * scale;
        }
    }
}

/// Reuse the model/delegates. A thumbnail completion should update one row, not
/// tear down the whole visible gallery. Viewport shifts remove/append only edges.
pub(super) fn sync_model<T: Clone + PartialEq + 'static>(
    current: ModelRc<T>,
    rows: Vec<T>,
    id: impl Fn(&T) -> &str,
) -> ModelRc<T> {
    let Some(model) = current.as_any().downcast_ref::<VecModel<T>>() else {
        return ModelRc::new(VecModel::from(rows));
    };
    let mut index = 0;
    while index < rows.len() {
        if index >= model.row_count() {
            model.push(rows[index].clone());
        } else if model
            .row_data(index)
            .is_some_and(|row| id(&row) == id(&rows[index]))
        {
            if model.row_data(index).as_ref() != Some(&rows[index]) {
                model.set_row_data(index, rows[index].clone());
            }
        } else {
            let later = (index + 1..model.row_count()).find(|&j| {
                model
                    .row_data(j)
                    .is_some_and(|row| id(&row) == id(&rows[index]))
            });
            if let Some(later) = later {
                for _ in index..later {
                    model.remove(index);
                }
                continue;
            }
            model.insert(index, rows[index].clone());
        }
        index += 1;
    }
    while model.row_count() > rows.len() {
        model.remove(model.row_count() - 1);
    }
    current
}
