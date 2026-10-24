//! Every block loaded, laid out by height: the main chain along the middle, side branches in
//! lanes above and below, each block joined to its parent. Drag to pan, scroll or pinch to zoom,
//! click a block to see it, double-click to open it.

use {
    super::{Ctx, FINAL, JUSTIFIED, MAIN, SIDE, View},
    crate::format,
    crate::model::Summary,
    crate::store::Store,
    egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2},
    std::collections::{HashMap, HashSet},
};

/// The distance between heights.
const STEP: f32 = 84.0;
/// The distance between lanes.
const LANE: f32 = 58.0;
/// How big a block is drawn.
const SIZE: Vec2 = vec2(62.0, 38.0);

pub struct MapState {
    /// The part of the map in view, in map coordinates.
    pub rect: Rect,
    pub selected: Option<String>,
    /// Whether the view moves along with the tip.
    pub follow: bool,
    /// The tip the view last moved to.
    followed: Option<u64>,
}

impl Default for MapState {
    fn default() -> MapState {
        MapState {
            rect: Rect::ZERO,
            selected: None,
            follow: true,
            followed: None,
        }
    }
}

/// Which lane each block sits in: 0 for the main chain, and for a side block its parent's lane,
/// or for one that starts a branch, or whose lane is taken, the nearest free one, alternating
/// above and below.
pub fn lanes(store: &Store) -> HashMap<String, i32> {
    let mut lanes: HashMap<String, i32> = store.main.values().map(|id| (id.clone(), 0)).collect();
    let mut taken: HashSet<(i32, u64)> = store.main.keys().map(|&height| (0, height)).collect();
    let mut side: Vec<&Summary> = store.blocks.values().filter(|block| !store.on_main(block)).collect();
    // At each height, branches going on take their lanes before new ones look for one.
    let starts = |block: &Summary| store.blocks.get(&block.parent).is_none_or(|parent| store.on_main(parent));
    side.sort_by(|a, b| (a.height, starts(a), &a.id).cmp(&(b.height, starts(b), &b.id)));

    for block in side {
        let parent = lanes.get(&block.parent).copied().filter(|&lane| lane != 0);
        let lane = parent
            .filter(|&lane| !taken.contains(&(lane, block.height)))
            .unwrap_or_else(|| {
                (1..)
                    .flat_map(|distance| [distance, -distance])
                    .find(|&lane| !taken.contains(&(lane, block.height)))
                    .expect("some lane is always free")
            });

        taken.insert((lane, block.height));
        lanes.insert(block.id.clone(), lane);
    }

    lanes
}

fn place(height: u64, lane: i32) -> Rect {
    Rect::from_center_size(pos2(height as f32 * STEP, -lane as f32 * LANE), SIZE)
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut MapState) {
    let store = ctx.store;
    let Some(top) = store.top() else {
        super::waiting(ui, ctx, &crate::api::Ask::Status);

        return;
    };

    ui.horizontal(|ui| {
        ui.checkbox(&mut state.follow, "Follow the tip");

        if ui.button("Load older blocks").clicked()
            && let Some(older) = store.older()
        {
            ctx.api.ask(older);
        }

        if ui.button("Reset view").clicked() {
            state.followed = None;
            state.follow = true;
        }

        ui.separator();
        legend(ui, MAIN, "main chain");
        legend(ui, FINAL, "finalized");
        legend(ui, SIDE, "side branch");
        legend(ui, JUSTIFIED, "justified checkpoint");
        ui.separator();
        ui.weak(format!(
            "{} blocks loaded, {} off the main chain, {} leaves",
            store.blocks.len(),
            store.blocks.len().saturating_sub(store.main.len()),
            store.tips.len()
        ));
    });

    if state.follow && state.followed != Some(top) {
        let width = if state.rect.width() > 0.0 { state.rect.width() } else { STEP * 16.0 };
        let height = if state.rect.height() > 0.0 { state.rect.height() } else { LANE * 5.0 };
        let right = top as f32 * STEP + STEP * 2.0;
        state.rect = Rect::from_min_max(pos2(right - width, -height / 2.0), pos2(right, height / 2.0));
        state.followed = Some(top);
    }

    let selected = state.selected.as_ref().and_then(|id| store.blocks.get(id)).cloned();

    if let Some(block) = &selected {
        egui::Panel::right("map_selected").default_size(260.0).show(ui, |ui| {
            selected_block(ui, ctx, block);
        });
    }

    let lanes = lanes(store);
    let (finalized, _) = store.finality_heights();
    let justified = store.status.as_ref().and_then(|status| status.finality.as_ref()).map(|finality| finality.justified.block.clone());
    let zoom = ui.available_width() / state.rect.width().max(1.0);
    let mut clicked = None;
    let mut opened = None;
    let tip = store.main.get(&top).cloned();

    let scene = egui::Scene::new().zoom_range(0.02..=6.0).show(ui, &mut state.rect, |ui| {
        let visible = ui.clip_rect().expand(STEP * 2.0);
        let painter = ui.painter().clone();
        let heights = (visible.min.x / STEP).floor().max(0.0) as u64..=(visible.max.x / STEP).ceil().max(0.0) as u64;
        let shown: Vec<&Summary> = store
            .blocks
            .values()
            .filter(|block| heights.contains(&block.height))
            .collect();

        for block in &shown {
            let Some(&lane) = lanes.get(&block.id) else { continue };
            let Some(parent) = store.blocks.get(&block.parent) else { continue };
            let Some(&parent_lane) = lanes.get(&parent.id) else { continue };
            let (from, to) = (place(block.height, lane), place(parent.height, parent_lane));
            let color = if lane == 0 && parent_lane == 0 { MAIN } else { SIDE };

            painter.line_segment([from.left_center(), to.right_center()], Stroke::new(2.0 / zoom.max(0.3), color.gamma_multiply(0.7)));
        }

        for block in shown {
            let Some(&lane) = lanes.get(&block.id) else { continue };
            let rect = place(block.height, lane);
            let main = lane == 0;
            let base = match (main, block.height <= finalized && finalized > 0) {
                (true, true) => FINAL,
                (true, false) => MAIN,
                (false, _) => SIDE,
            };
            let busy = (block.tx_count as f32 / 20.0).min(1.0);
            painter.rect_filled(rect, 6.0, base.gamma_multiply(0.45 + 0.55 * busy));

            let outline = if state.selected.as_ref() == Some(&block.id) {
                Some(Stroke::new(3.0, Color32::YELLOW))
            } else if tip.as_ref() == Some(&block.id) {
                Some(Stroke::new(2.5, Color32::WHITE))
            } else if justified.as_ref() == Some(&block.id) {
                Some(Stroke::new(2.5, JUSTIFIED))
            } else {
                None
            };

            if let Some(outline) = outline {
                painter.rect_stroke(rect, 6.0, outline, StrokeKind::Outside);
            }

            if zoom * SIZE.x > 40.0 {
                painter.text(rect.center() - vec2(0.0, 7.0), Align2::CENTER_CENTER, block.height, FontId::monospace(11.0), Color32::WHITE);
                painter.text(
                    rect.center() + vec2(0.0, 8.0),
                    Align2::CENTER_CENTER,
                    format!("{} tx", block.tx_count),
                    FontId::proportional(9.0),
                    Color32::from_white_alpha(200),
                );
            }

            let response = ui.interact(rect, ui.scope_id().with(("block", &block.id)), Sense::click());

            if response.clicked() {
                clicked = Some(block.id.clone());
            }

            if response.double_clicked() {
                opened = Some(block.id.clone());
            }

            response.on_hover_ui(|ui| {
                ui.strong(format!("Block {}", block.height));
                ui.monospace(format::short(&block.id));
                ui.label(format!("{} transactions, {} gas", block.tx_count, block.gas_used));
                ui.label(if main { "main chain" } else { "side branch" });
            });
        }
    });

    if scene.response.changed() {
        state.follow = false;
    }

    if let Some(id) = clicked {
        state.selected = Some(id);
    }

    if let Some(id) = opened {
        ctx.go(View::Block(id));
    }
}

fn legend(ui: &mut Ui, color: Color32, name: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
    ui.painter().rect_filled(rect, 3.0, color);
    ui.small(name);
}

fn selected_block(ui: &mut Ui, ctx: &mut Ctx, block: &Summary) {
    super::title(ui, &format!("Block {}", format::thousands(&block.height.to_string())));

    match ctx.store.on_main(block) {
        true => super::badge(ui, "main chain", MAIN),
        false => super::badge(ui, "side branch", SIDE),
    }

    super::facts(ui, "map_block", |ui| {
        super::fact(ui, "id");
        super::copyable(ui, &format::short(&block.id));
        ui.end_row();
        super::fact(ui, "parent");
        super::block(ui, ctx, &block.parent, &format::short(&block.parent));
        ui.end_row();
        super::fact(ui, "time");
        ui.label(format!("{} UTC", format::utc(block.timestamp)));
        ui.end_row();
        super::fact(ui, "producer");
        super::address(ui, ctx, &block.producer);
        ui.end_row();
        super::fact(ui, "transactions");
        ui.label(block.tx_count.to_string());
        ui.end_row();
        super::fact(ui, "gas used");
        ui.label(format::thousands(&block.gas_used.to_string()));
        ui.end_row();
    });

    if ui.button("Open block").clicked() {
        ctx.go(View::Block(block.id.clone()));
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::store::tests::block};

    #[test]
    fn side_branches_keep_their_lane_and_never_overlap() {
        let mut store = Store::default();

        for (height, id, parent) in [(1, "a", "g"), (2, "b", "a"), (3, "c", "b"), (4, "d", "c")] {
            store.main_block(block(height, id, parent));
        }

        // One branch off a, two blocks long, and two siblings of c.
        for (height, id, parent) in [(2, "x", "a"), (3, "y", "x"), (3, "s", "b"), (3, "t", "b")] {
            store.side_block(block(height, id, parent));
        }

        let lanes = lanes(&store);

        assert_eq!(["a", "b", "c", "d"].map(|id| lanes[id]), [0; 4]);
        assert_eq!(lanes["x"], lanes["y"], "a branch stays in its lane");
        assert_ne!(lanes["x"], 0);

        let at_three: HashSet<i32> = ["c", "y", "s", "t"].iter().map(|id| lanes[*id]).collect();

        assert_eq!(at_three.len(), 4, "blocks at one height never share a lane");
    }
}
