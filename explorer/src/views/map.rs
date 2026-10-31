//! Every block loaded, laid out by height: the main chain along the middle, side branches in
//! lanes above and below, each block joined to its parent. Drag to pan, scroll or pinch to zoom,
//! click a block to see it, double-click to open it.

use {
    super::{Ctx, Look, View},
    crate::format,
    crate::model::Summary,
    crate::store::Store,
    chainv4_ui::widgets,
    egui::{Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2},
    std::collections::{BTreeMap, HashMap, HashSet},
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
    /// The lanes, laid out once for what the store held: done again only when that changes.
    layout: Option<Layout>,
}

impl Default for MapState {
    fn default() -> MapState {
        MapState {
            rect: Rect::ZERO,
            selected: None,
            follow: true,
            followed: None,
            layout: None,
        }
    }
}

/// What the layout was made from: the blocks held, the main chain's length and both its ends.
/// A reorganization moves the tip, so it changes too.
type Made = (usize, usize, Option<String>, Option<String>);

fn made(store: &Store) -> Made {
    let end = |entry: Option<(&u64, &String)>| entry.map(|(_, id)| id.clone());

    (store.blocks.len(), store.main.len(), end(store.main.first_key_value()), end(store.main.last_key_value()))
}

/// The side blocks' lanes, and the side blocks by height, to find those in view without looking
/// through every block.
pub struct Layout {
    made: Made,
    side: HashMap<String, i32>,
    by_height: BTreeMap<u64, Vec<String>>,
}

impl Layout {
    /// Which lane each side block sits in: its parent's, or for one that starts a branch, or whose
    /// lane is taken, the nearest free one, alternating above and below. The main chain is lane 0.
    pub fn of(store: &Store) -> Layout {
        let mut side_lanes: HashMap<String, i32> = HashMap::new();
        let mut by_height: BTreeMap<u64, Vec<String>> = BTreeMap::new();
        let mut taken: HashSet<(i32, u64)> = HashSet::new();
        let mut side: Vec<&Summary> = store.blocks.values().filter(|block| !store.on_main(block)).collect();
        // At each height, branches going on take their lanes before new ones look for one.
        let starts = |block: &Summary| store.blocks.get(&block.parent).is_none_or(|parent| store.on_main(parent));
        side.sort_by(|a, b| (a.height, starts(a), &a.id).cmp(&(b.height, starts(b), &b.id)));

        for block in side {
            let parent = side_lanes.get(&block.parent).copied();
            let lane = parent.filter(|&lane| !taken.contains(&(lane, block.height))).unwrap_or_else(|| {
                (1..)
                    .flat_map(|distance| [distance, -distance])
                    .find(|&lane| !taken.contains(&(lane, block.height)))
                    .expect("some lane is always free")
            });

            taken.insert((lane, block.height));
            side_lanes.insert(block.id.clone(), lane);
            by_height.entry(block.height).or_default().push(block.id.clone());
        }

        Layout { made: made(store), side: side_lanes, by_height }
    }

    /// A block's lane: 0 on the main chain; None for one the store does not hold.
    pub fn lane(&self, store: &Store, id: &str) -> Option<i32> {
        let block = store.blocks.get(id)?;

        match store.on_main(block) {
            true => Some(0),
            false => self.side.get(id).copied(),
        }
    }

    /// The blocks at these heights, main and side.
    fn at<'a>(&self, store: &'a Store, heights: std::ops::RangeInclusive<u64>) -> Vec<&'a Summary> {
        let main = store.main.range(heights.clone()).map(|(_, id)| id);
        let side = self.by_height.range(heights).flat_map(|(_, ids)| ids);

        main.chain(side).filter_map(|id| store.blocks.get(id)).collect()
    }
}

fn place(height: u64, lane: i32) -> Rect {
    Rect::from_center_size(pos2(height as f32 * STEP, -lane as f32 * LANE), SIZE)
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut MapState) {
    let look = ctx.look;
    let roles = look.roles;
    let store = ctx.store;
    let Some(top) = store.top() else {
        super::waiting(ui, ctx, &crate::api::Ask::Status);

        return;
    };

    super::heading(
        ui,
        look,
        "Block map",
        &format!(
            "{} blocks loaded, {} off the main chain, {} leaves. Drag to pan, pinch or scroll to zoom, tap a block.",
            store.blocks.len(),
            store.blocks.len().saturating_sub(store.main.len()),
            store.tips.len()
        ),
        |_| {},
    );
    if let Some(standing) = ctx.reading.standing().filter(|_| ctx.settings.read_blocks) {
        super::note(ui, look, format!("Final blocks only, read here, from block {}. Side branches are the node's to know, and show nothing here. Read further back in Settings.", format::thousands(&standing.known_from.to_string())));
    }

    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut state.follow, "Follow the tip");

        if widgets::quiet(ui, &look.palette, "Load older").clicked()
            && let Some(older) = store.older()
        {
            ctx.api.ask(older);
        }

        if widgets::quiet(ui, &look.palette, "Reset view").clicked() {
            state.followed = None;
            state.follow = true;
            state.rect = Rect::ZERO;
        }
    });
    ui.horizontal_wrapped(|ui| {
        legend(ui, look, roles.main, "main chain");
        legend(ui, look, roles.final_, "finalized");
        legend(ui, look, roles.side, "side branch");
        legend(ui, look, roles.justified, "justified");
    });
    ui.add_space(6.0);

    // At first, as large as it is drawn: as many blocks across as the screen has room for.
    if state.follow && state.followed != Some(top) {
        let width = if state.rect.width() > 0.0 { state.rect.width() } else { ui.available_width().max(STEP * 3.0) };
        let height = if state.rect.height() > 0.0 { state.rect.height() } else { ui.available_height().max(LANE * 3.0) };
        let right = top as f32 * STEP + STEP * 2.0;
        state.rect = Rect::from_min_max(pos2(right - width, -height / 2.0), pos2(right, height / 2.0));
        state.followed = Some(top);
    }

    let selected = state.selected.as_ref().and_then(|id| store.blocks.get(id)).cloned();

    // The block picked, beside the map, or under it on a phone.
    if let Some(block) = &selected {
        let frame = egui::Frame::new().fill(look.palette.card).stroke(Stroke::new(1.0, look.palette.line)).inner_margin(12);

        match look.narrow {
            true => egui::Panel::bottom("map_selected").frame(frame).show(ui, |ui| selected_block(ui, ctx, block)),
            false => egui::Panel::right("map_selected").default_size(270.0).frame(frame).show(ui, |ui| selected_block(ui, ctx, block)),
        };
    }

    if state.layout.as_ref().is_none_or(|layout| layout.made != made(store)) {
        state.layout = Some(Layout::of(store));
    }

    let layout = state.layout.as_ref().expect("laid out above");
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
        let shown = layout.at(store, heights);

        for block in &shown {
            let Some(lane) = layout.lane(store, &block.id) else { continue };
            let Some(parent) = store.blocks.get(&block.parent) else { continue };
            let Some(parent_lane) = layout.lane(store, &parent.id) else { continue };
            let (from, to) = (place(block.height, lane), place(parent.height, parent_lane));
            let color = if lane == 0 && parent_lane == 0 { roles.main } else { roles.side };

            painter.line_segment([from.left_center(), to.right_center()], Stroke::new(2.0 / zoom.max(0.3), color.gamma_multiply(0.7)));
        }

        for block in shown {
            let Some(lane) = layout.lane(store, &block.id) else { continue };
            let rect = place(block.height, lane);
            let main = lane == 0;
            let base = match (main, block.height <= finalized && finalized > 0) {
                (true, true) => roles.final_,
                (true, false) => roles.main,
                (false, _) => roles.side,
            };
            let busy = (block.tx_count as f32 / 20.0).min(1.0);
            let fill = base.gamma_multiply(0.45 + 0.55 * busy);
            let ink = crate::theme::ink_on(fill, &look.palette);
            painter.rect_filled(rect, 6.0, fill);

            let outline = if state.selected.as_ref() == Some(&block.id) {
                Some(Stroke::new(3.0, look.palette.warn))
            } else if tip.as_ref() == Some(&block.id) {
                Some(Stroke::new(2.5, look.palette.ink))
            } else if justified.as_ref() == Some(&block.id) {
                Some(Stroke::new(2.5, roles.justified))
            } else {
                None
            };

            if let Some(outline) = outline {
                painter.rect_stroke(rect, 6.0, outline, StrokeKind::Outside);
            }

            if zoom * SIZE.x > 40.0 {
                painter.text(rect.center() - vec2(0.0, 7.0), Align2::CENTER_CENTER, block.height, FontId::monospace(11.0), ink);
                painter.text(rect.center() + vec2(0.0, 8.0), Align2::CENTER_CENTER, format!("{} tx", block.tx_count), FontId::proportional(9.0), ink.gamma_multiply(0.8));
            }

            let response = ui.interact(rect, ui.scope_id().with(("block", &block.id)), Sense::click());

            if response.clicked() {
                clicked = Some(block.id.clone());
            }

            if response.double_clicked() {
                opened = Some(block.id.clone());
            }

            response.on_hover_ui(|ui| {
                ui.label(widgets::strong(format!("Block {}", block.height), 13.5));
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

fn legend(ui: &mut Ui, look: Look, color: Color32, name: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
    ui.label(RichText::new(name).small().color(look.palette.soft));
}

fn selected_block(ui: &mut Ui, ctx: &mut Ctx, block: &Summary) {
    let look = ctx.look;
    // On a phone, the facts sit beside each other, so the map keeps most of the screen.
    let facts_look = Look { narrow: false, ..look };

    ui.horizontal(|ui| {
        ui.label(widgets::strong(format!("Block {}", format::thousands(&block.height.to_string())), 16.0).color(look.palette.ink));

        match ctx.store.on_main(block) {
            true => widgets::pill(ui, "main chain", look.roles.main),
            false => widgets::pill(ui, "side branch", look.roles.side),
        };
    });
    ui.add_space(4.0);
    super::facts(ui, facts_look, "map_block", |facts| {
        facts.row("id", |ui| super::copyable_as(ui, RichText::new(format::short(&block.id)).monospace(), &block.id));
        facts.row("parent", |ui| super::block(ui, ctx, &block.parent, &format::short(&block.parent)));
        facts.text("time", format!("{} UTC", format::utc(block.timestamp)));
        facts.row("producer", |ui| super::address(ui, ctx, &block.producer));
        facts.text("transactions", block.tx_count.to_string());
        facts.text("gas used", format::thousands(&block.gas_used.to_string()));
    });
    ui.add_space(4.0);

    if widgets::primary(ui, &look.palette, "Open block", true).clicked() {
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

        let layout = Layout::of(&store);
        let lane = |id: &str| layout.lane(&store, id).unwrap();

        assert_eq!(["a", "b", "c", "d"].map(lane), [0; 4]);
        assert_eq!(lane("x"), lane("y"), "a branch stays in its lane");
        assert_ne!(lane("x"), 0);

        let at_three: HashSet<i32> = ["c", "y", "s", "t"].into_iter().map(lane).collect();

        assert_eq!(at_three.len(), 4, "blocks at one height never share a lane");
        assert_eq!(layout.lane(&store, "nowhere"), None);

        // Those in view, by height, main and side alike; and laid out again only once the store
        // changed.
        let mut seen: Vec<&str> = layout.at(&store, 3..=3).iter().map(|block| block.id.as_str()).collect();
        seen.sort();

        assert_eq!(seen, ["c", "s", "t", "y"]);
        assert_eq!(made(&store), layout.made);
        store.side_block(block(4, "z", "y"));

        assert_ne!(made(&store), layout.made);
    }
}
