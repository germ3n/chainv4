//! Where a transaction went, two ways at once: a call tree, like a debugger's stack, and a graph
//! of everyone it touched with a numbered arrow for each call, delegate and transfer. Selecting a
//! hop in one selects it in the other.

use {
    super::{Ctx, Look, View},
    crate::format,
    crate::model::{Hop, Outcome, TxView},
    egui::{Color32, Pos2, Rect, RichText, Shape, Stroke, Ui, Vec2, pos2, vec2},
    serde_json::Value,
    std::collections::HashMap,
};

/// The distance between the graph's columns and rows.
const COLUMN: f32 = 360.0;
const ROW: f32 = 110.0;
/// How big an address is drawn.
const NODE: Vec2 = vec2(170.0, 34.0);

pub struct HopState {
    /// The transaction these are for: a new one starts afresh.
    tx: String,
    pub selected: Option<usize>,
    rect: Rect,
}

impl Default for HopState {
    fn default() -> HopState {
        HopState {
            tx: String::new(),
            selected: None,
            rect: Rect::ZERO,
        }
    }
}

impl HopState {
    fn reset_for(&mut self, tx: &str) {
        if self.tx != tx {
            *self = HopState {
                tx: tx.to_string(),
                ..HopState::default()
            };
        }
    }
}

/// A transaction's hops: its trace, or for one that ran no contract, the one step it took.
pub fn of(tx: &TxView, outcome: Option<&Outcome>) -> Vec<Hop> {
    if let Some(outcome) = outcome
        && !outcome.trace.hops.is_empty()
    {
        return outcome.trace.hops.clone();
    }

    let field = |name: &str| tx.tx.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
    let error = outcome.and_then(|outcome| outcome.error.clone());
    let (kind, to, function, value) = match tx.kind() {
        "transfer" => ("transfer", field("to"), String::new(), field("amount")),
        "mint" => ("mint", field("to"), String::new(), field("amount")),
        "burn" => ("burn", "(burned)".to_string(), String::new(), field("amount")),
        "contract_call" => ("call", field("to"), field("func"), field("amount")),
        "evm_call" => ("EVM call", field("to"), String::new(), field("value")),
        "deploy" => (
            "deploy",
            outcome.and_then(|outcome| outcome.contract.clone()).unwrap_or_else(|| "(new contract)".to_string()),
            String::new(),
            "0".to_string(),
        ),
        "upgrade" => ("upgrade", field("contract"), String::new(), "0".to_string()),
        "bond" => ("bond", "(staking)".to_string(), String::new(), field("amount")),
        "unbond" | "withdraw" | "slash" => (tx.kind(), "(staking)".to_string(), String::new(), "0".to_string()),
        "approve_signers" => ("approve", "(signers)".to_string(), String::new(), "0".to_string()),
        "withdraw_approval" => ("withdraw approval", "(signers)".to_string(), String::new(), "0".to_string()),
        "approve_change" => ("approve", "(governance)".to_string(), String::new(), "0".to_string()),
        "withdraw_change" => ("withdraw approval", "(governance)".to_string(), String::new(), "0".to_string()),
        "name" => {
            let action = tx.tx.get("action").and_then(|action| action.get("type")).and_then(Value::as_str).unwrap_or("name");

            ("name", format!("{} ({action})", field("name")), String::new(), "0".to_string())
        }
        "ethereum" => ("Ethereum", field("to"), String::new(), field("value")),
        "primary_name" => ("primary name", tx.tx.get("name").and_then(Value::as_str).unwrap_or("(cleared)").to_string(), String::new(), "0".to_string()),
        other => (other, "?".to_string(), String::new(), "0".to_string()),
    };

    vec![Hop {
        depth: 0,
        kind: kind.to_string(),
        from: tx.sender.clone(),
        to,
        function,
        value,
        fuel_used: outcome.map_or(0, |outcome| outcome.fuel_used),
        error,
        events: Vec::new(),
    }]
}

/// Where the graph puts everyone the hops touched: whoever started a hop sits one column before
/// whoever it reached, each column filling downwards in the order they first appear.
pub fn layout(hops: &[Hop]) -> Vec<(String, Pos2)> {
    let mut placed: Vec<(String, Pos2)> = Vec::new();
    let mut rows: HashMap<usize, usize> = HashMap::new();
    let mut place = |placed: &mut Vec<(String, Pos2)>, address: &str, column: usize| -> usize {
        if let Some((_, position)) = placed.iter().find(|(known, _)| known == address) {
            return (position.x / COLUMN).round() as usize;
        }

        let row = rows.entry(column).or_default();
        placed.push((address.to_string(), pos2(column as f32 * COLUMN, *row as f32 * ROW)));
        *row += 1;

        column
    };

    for hop in hops {
        let from = place(&mut placed, &hop.from, usize::from(hop.depth));
        place(&mut placed, &hop.to, from + 1);
    }

    placed
}

fn color(look: Look, hop: &Hop) -> Color32 {
    match (hop.error.is_some(), hop.kind.as_str()) {
        (true, _) => look.palette.bad,
        (false, "transfer" | "mint" | "burn" | "bond") => look.palette.good,
        (false, "delegate") => Color32::from_rgb(0xb4, 0x8c, 0xf0),
        _ => look.roles.main,
    }
}

/// A hop in one line: what it was, where it went and what it sent.
fn describe(ctx: &Ctx, hop: &Hop) -> String {
    let mut text = match hop.function.is_empty() {
        true => hop.kind.clone(),
        false => format!("{} {}", hop.kind, hop.function),
    };

    if hop.value != "0" && !hop.value.is_empty() {
        text.push_str(&format!(" · {}", ctx.coins(&hop.value)));
    }

    text
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, tx: &TxView, outcome: Option<&Outcome>, state: &mut HopState) {
    state.reset_for(&tx.id);
    let hops = of(tx, outcome);

    if let Some(dropped) = outcome.map(|outcome| outcome.trace.dropped).filter(|&dropped| dropped > 0) {
        ui.colored_label(ctx.look.palette.bad, format!("{dropped} more hops were made than a receipt keeps; only the first {} are shown.", hops.len()));
    }

    match ctx.look.narrow {
        true => {
            tree(ui, ctx, &hops, outcome, state);
            ui.add_space(8.0);
            graph(ui, ctx, &hops, state);
        }
        false => {
            ui.columns(2, |columns| {
                tree(&mut columns[0], ctx, &hops, outcome, state);
                graph(&mut columns[1], ctx, &hops, state);
            });
        }
    }
}

fn tree(ui: &mut Ui, ctx: &mut Ctx, hops: &[Hop], outcome: Option<&Outcome>, state: &mut HopState) {
    let look = ctx.look;
    chainv4_ui::widgets::caps(ui, &look.palette, "Call tree");
    egui::ScrollArea::both()
        .id_salt("hop_tree")
        .max_height(if look.narrow { 260.0 } else { 420.0 })
        .scroll_source(super::NO_DRAG)
        .show(ui, |ui| {
        for (index, hop) in hops.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.add_space(f32::from(hop.depth) * 18.0);
                super::dot(ui, color(look, hop));
                let text = format!(
                    "#{index} {} -> {}  {}",
                    format::short(&hop.from),
                    format::short(&hop.to),
                    describe(ctx, hop)
                );

                if ui.selectable_label(state.selected == Some(index), RichText::new(text).monospace()).clicked() {
                    state.selected = Some(index);
                }

                if hop.fuel_used > 0 {
                    ui.label(RichText::new(format!("{} fuel", format::thousands(&hop.fuel_used.to_string()))).small().color(look.palette.muted));
                }
            });

            if let Some(error) = &hop.error {
                ui.horizontal(|ui| {
                    ui.add_space(f32::from(hop.depth) * 18.0 + 22.0);
                    ui.colored_label(look.palette.bad, RichText::new(error).small());
                });
            }

            for event in &hop.events {
                if let Some(event) = outcome.and_then(|outcome| outcome.events.get(*event as usize)) {
                    ui.horizontal(|ui| {
                        ui.add_space(f32::from(hop.depth) * 18.0 + 22.0);
                        ui.label(RichText::new(format!("emitted {}", event.name)).small().color(look.palette.muted));
                    });
                }
            }
        }
    });

    if let Some(hop) = state.selected.and_then(|index| hops.get(index)) {
        chainv4_ui::widgets::rule(ui, &look.palette);
        ui.add_space(4.0);
        super::facts(ui, look, "hop", |facts| {
            facts.text("kind", hop.kind.clone());
            facts.row("from", |ui| super::address(ui, ctx, &hop.from));
            facts.row("to", |ui| super::address(ui, ctx, &hop.to));

            if !hop.function.is_empty() {
                facts.row("function", |ui| {
                    ui.monospace(&hop.function);
                });
            }

            facts.text("value", ctx.coins(&hop.value));
            facts.text("fuel", format::thousands(&hop.fuel_used.to_string()));
            facts.row("result", |ui| match &hop.error {
                Some(error) => {
                    ui.colored_label(look.palette.bad, error);
                }
                None => {
                    ui.colored_label(look.palette.good, "went through");
                }
            });
        });
    }
}

fn graph(ui: &mut Ui, ctx: &mut Ctx, hops: &[Hop], state: &mut HopState) {
    let look = ctx.look;
    chainv4_ui::widgets::caps(ui, &look.palette, "Who it touched");
    let nodes = layout(hops);
    let position: HashMap<&str, Pos2> = nodes.iter().map(|(address, position)| (address.as_str(), *position)).collect();
    let mut chosen = None;
    let mut open = None;

    egui::Frame::new()
        .fill(look.palette.paper)
        .stroke(Stroke::new(1.0, look.palette.line))
        .corner_radius(ui.visuals().widgets.inactive.corner_radius)
        .show(ui, |ui| {
        ui.set_height(if look.narrow { 300.0 } else { 420.0 });
        // On a phone the graph only shows, fitted, so a swipe over it scrolls the page.
        let sense = if look.narrow { egui::Sense::hover() } else { egui::Sense::click_and_drag() };
        egui::Scene::new().zoom_range(0.1..=3.0).sense(sense).show(ui, &mut state.rect, |ui| {
            let painter = ui.painter().clone();
            // How many hops join each pair so far, to fan them out.
            let mut between: HashMap<(&str, &str), usize> = HashMap::new();
            let totals = hops.iter().fold(HashMap::new(), |mut totals: HashMap<(&str, &str), usize>, hop| {
                *totals.entry(pair(&hop.from, &hop.to)).or_default() += 1;
                totals
            });

            for (index, hop) in hops.iter().enumerate() {
                let (Some(&from), Some(&to)) = (position.get(hop.from.as_str()), position.get(hop.to.as_str())) else {
                    continue;
                };
                let key = pair(&hop.from, &hop.to);
                let nth = between.entry(key).or_default();
                let fan = *nth as f32 - (totals[&key] as f32 - 1.0) / 2.0;
                *nth += 1;
                let selected = state.selected == Some(index);
                let stroke = Stroke::new(if selected { 3.5 } else { 1.8 }, color(look, hop));
                let points = curve(from, to, fan);

                match hop.kind.as_str() {
                    "delegate" => painter.extend(Shape::dashed_line(&points, stroke, 8.0, 5.0)),
                    _ => {
                        painter.add(Shape::line(points.clone(), stroke));
                    }
                }

                arrowhead(&painter, &points, stroke);
                let middle = points[points.len() / 2];
                let text = format!("#{index} {}", short_function(hop));
                let width = text.chars().count() as f32 * 6.5 + 14.0;
                let label = egui::Button::new(RichText::new(text).small())
                    .selected(selected)
                    .fill(ui.visuals().extreme_bg_color);

                if ui.put(Rect::from_center_size(middle, vec2(width, 20.0)), label).clicked() {
                    chosen = Some(index);
                }
            }

            for (address, position) in &nodes {
                let rect = Rect::from_center_size(*position, NODE);
                let first = hops.first().is_some_and(|hop| &hop.from == address);
                let fill = if first { look.roles.main.gamma_multiply(0.5) } else { ui.visuals().widgets.inactive.bg_fill };
                let response = ui.put(rect, egui::Button::new(RichText::new(format::short(address)).monospace()).fill(fill));

                if response.on_hover_text(address).clicked() && super::is_address(address) {
                    open = Some(address.clone());
                }
            }
        });
    });

    if let Some(index) = chosen {
        state.selected = Some(index);
    }

    if let Some(address) = open {
        ctx.go(View::Account(address));
    }
}

/// The same key for a pair whichever way a hop goes, so hops both ways fan out apart.
fn pair<'a>(from: &'a str, to: &'a str) -> (&'a str, &'a str) {
    if from <= to { (from, to) } else { (to, from) }
}

fn short_function(hop: &Hop) -> String {
    match hop.function.is_empty() {
        true => hop.kind.clone(),
        false if hop.function.len() > 16 => format::short(&hop.function),
        false => hop.function.clone(),
    }
}

/// A gentle curve from one address to another, edge to edge, bowed by fan so hops between the
/// same two spread apart; a loop for a contract reaching itself.
fn curve(from: Pos2, to: Pos2, fan: f32) -> Vec<Pos2> {
    if from == to {
        let center = from + vec2(NODE.x / 2.0 + 20.0, 0.0);

        return (0..=24)
            .map(|step| {
                let angle = std::f32::consts::TAU * step as f32 / 24.0;

                center + vec2(-angle.cos(), angle.sin()) * 20.0
            })
            .collect();
    }

    let edge = match (to.x - from.x).abs() > f32::EPSILON {
        true => vec2((to.x - from.x).signum() * NODE.x / 2.0, 0.0),
        false => vec2(0.0, (to.y - from.y).signum() * NODE.y / 2.0),
    };
    let (start, end) = (from + edge, to - edge);
    let along = end - start;
    let bend = start + along / 2.0 + vec2(-along.y, along.x).normalized() * fan * 34.0;

    (0..=16)
        .map(|step| {
            let t = step as f32 / 16.0;

            start.lerp(bend, t).lerp(bend.lerp(end, t), t)
        })
        .collect()
}

fn arrowhead(painter: &egui::Painter, points: &[Pos2], stroke: Stroke) {
    let [.., before, tip] = points else {
        return;
    };
    let back = (*before - *tip).normalized() * 10.0;
    let side = vec2(-back.y, back.x) * 0.5;

    painter.add(Shape::convex_polygon(vec![*tip, *tip + back + side, *tip + back - side], stroke.color, Stroke::NONE));
}

/// Whether any two addresses the graph placed overlap.
#[cfg(test)]
fn overlapping(nodes: &[(String, Pos2)]) -> bool {
    nodes.iter().enumerate().any(|(index, (_, a))| {
        nodes[index + 1..]
            .iter()
            .any(|(_, b)| Rect::from_center_size(*a, NODE).intersects(Rect::from_center_size(*b, NODE)))
    })
}

#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

    fn hop(depth: u8, kind: &str, from: &str, to: &str) -> Hop {
        Hop {
            depth,
            kind: kind.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            function: "f".to_string(),
            value: "0".to_string(),
            fuel_used: 0,
            error: None,
            events: Vec::new(),
        }
    }

    #[test]
    fn everyone_gets_a_place_once_and_callees_sit_after_callers() {
        let hops = [
            hop(0, "call", "alice", "router"),
            hop(1, "call", "router", "pool"),
            hop(2, "transfer", "pool", "alice"),
            hop(1, "delegate", "router", "logic"),
            hop(1, "call", "router", "pool"),
        ];
        let nodes = layout(&hops);
        let column = |address: &str| nodes.iter().find(|(known, _)| known == address).unwrap().1.x / COLUMN;

        assert_eq!(nodes.iter().map(|(address, _)| address.as_str()).collect::<Vec<_>>(), ["alice", "router", "pool", "logic"]);
        assert_eq!([column("alice"), column("router"), column("pool"), column("logic")], [0.0, 1.0, 2.0, 2.0]);
        assert!(!overlapping(&nodes));
    }

    #[test]
    fn transactions_without_contracts_still_have_their_one_hop() {
        let tx: TxView = serde_json::from_value(json!({
            "id": "0x01", "sender": "ed25519:0xaa", "nonce": 0, "signer": "ed25519:0xaa", "signature": "0x00",
            "fee": { "gas_limit": 0, "max_price": "0", "tip": "0" },
            "tx": { "type": "transfer", "from": "ed25519:0xaa", "to": "ed25519:0xbb", "amount": "5" }
        }))
        .unwrap();
        let hops = of(&tx, None);

        assert_eq!(hops.len(), 1);
        assert_eq!((hops[0].kind.as_str(), hops[0].to.as_str(), hops[0].value.as_str()), ("transfer", "ed25519:0xbb", "5"));
    }

    #[test]
    fn curves_end_where_they_point() {
        let points = curve(pos2(0.0, 0.0), pos2(COLUMN, 0.0), 1.0);

        assert_eq!(points.first(), Some(&pos2(NODE.x / 2.0, 0.0)));
        assert_eq!(points.last(), Some(&pos2(COLUMN - NODE.x / 2.0, 0.0)));
        assert!(points[8].y.abs() > 10.0, "a fanned curve bows away from the straight line");
    }
}
