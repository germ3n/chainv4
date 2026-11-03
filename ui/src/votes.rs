//! Governance's votes as both apps draw them: how far a change got toward what it needs and toward
//! its quorum, as a ring; who voted which way, a dot each; what a change would move, field by
//! field; decisions and rejections along the chain's heights; and the voters' turnout. The numbers
//! are plain data, read from the node's JSON as decimal strings, and tested as such; the drawing
//! takes the shared palette. Each vote has a shape as well as a color, so a person who cannot tell
//! the colors apart tells the votes apart all the same.

use {
    crate::theme::Palette,
    egui::{Align2, Color32, FontId, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, Ui, pos2, vec2},
    serde_json::Value,
    std::f32::consts::TAU,
};

/// How an account voted on a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Choice {
    Yes,
    No,
    Abstain,
}

impl Choice {
    /// The vote the node names: yes, no or abstain.
    pub fn of(name: &str) -> Option<Choice> {
        match name {
            "yes" => Some(Choice::Yes),
            "no" => Some(Choice::No),
            "abstain" => Some(Choice::Abstain),
            _ => None,
        }
    }

    /// Its word, for a person.
    pub fn word(self) -> &'static str {
        match self {
            Choice::Yes => "Yes",
            Choice::No => "No",
            Choice::Abstain => "Abstain",
        }
    }

    /// Its color: good for Yes, bad for No, warn for Abstain.
    pub fn color(self, palette: &Palette) -> Color32 {
        match self {
            Choice::Yes => palette.good,
            Choice::No => palette.bad,
            Choice::Abstain => palette.warn,
        }
    }
}

/// How a change's votes stand, as numbers: each side's weight, what every voter weighs, what Yes
/// needs and the quorum's weight, and whether it can still pass, as the node said.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tally {
    pub yes: f64,
    pub no: f64,
    pub abstain: f64,
    pub total: f64,
    pub needed: f64,
    pub quorum: f64,
    pub can_pass: bool,
}

/// A decimal weight of any size as a number: 0 when it does not read.
pub fn number(text: &str) -> f64 {
    text.parse::<f64>().ok().filter(|value| value.is_finite()).unwrap_or(0.0)
}

impl Tally {
    /// From the node's tally of a change, as GET /governance writes it.
    pub fn of(value: &Value) -> Tally {
        let weight = |name: &str| value[name].as_str().map_or(0.0, number);

        Tally {
            yes: weight("yes"),
            no: weight("no"),
            abstain: weight("abstain"),
            total: weight("total"),
            needed: weight("needed"),
            quorum: weight("quorum"),
            can_pass: value["can_pass"].as_bool().unwrap_or(true),
        }
    }

    /// What every vote weighs together, Abstain included.
    pub fn voted(&self) -> f64 {
        self.yes + self.no + self.abstain
    }

    /// How far Yes got toward what it needs, 1 once it weighs enough.
    pub fn support(&self) -> f64 {
        fraction(self.yes, self.needed)
    }

    /// How far the votes got toward the quorum, or None when there is none to reach.
    pub fn quorum_progress(&self) -> Option<f64> {
        (self.quorum > 0.0).then(|| fraction(self.voted(), self.quorum))
    }

    /// How much more Yes needs to weigh.
    pub fn more_needed(&self) -> f64 {
        (self.needed - self.yes).max(0.0)
    }

    /// Where it stands, in words, weights said by say: rejected-to-be, passing, or what it lacks.
    pub fn standing(&self, say: &dyn Fn(f64) -> String) -> String {
        let quorum_short = self.quorum_progress().is_some_and(|progress| progress < 1.0);

        match (self.can_pass, self.more_needed() > 0.0, quorum_short) {
            (false, _, _) => "Can no longer pass: too much has voted No".to_string(),
            (true, true, _) => format!("{} more Yes needed", say(self.more_needed())),
            (true, false, true) => format!("Yes is enough; {} more votes of any kind make the quorum", say((self.quorum - self.voted()).max(0.0))),
            (true, false, false) => "Passes with the next vote counted".to_string(),
        }
    }
}

/// A part's share of a whole, from 0 to 1: 0 when the whole is nothing.
pub fn fraction(part: f64, whole: f64) -> f64 {
    match whole > 0.0 {
        true => (part / whole).clamp(0.0, 1.0),
        false => 0.0,
    }
}

/// How long until old votes stop counting, in words: a vote counts through the epoch it says, so
/// it stops at the first block of the next.
pub fn closing(counts_until: u64, tip: u64, checkpoint_interval: u64, block_ms: Option<u64>) -> String {
    let stops = counts_until.saturating_add(1).saturating_mul(checkpoint_interval.max(1));
    let blocks = stops.saturating_sub(tip.saturating_add(1));

    match (blocks, block_ms) {
        (0, _) => "The oldest votes stop counting with the next block".to_string(),
        (blocks, Some(ms)) if ms > 0 => format!("The oldest votes stop counting in {} blocks, about {}", thousands(blocks), duration(blocks.saturating_mul(ms))),
        (blocks, _) => format!("The oldest votes stop counting in {} blocks", thousands(blocks)),
    }
}

/// Milliseconds as a rough duration: 40 s, 12 min, 3 h 20 min, 2 days.
pub fn duration(ms: u64) -> String {
    let seconds = ms / 1_000;

    match seconds {
        0..60 => format!("{seconds} s"),
        60..3_600 => format!("{} min", seconds / 60),
        3_600..86_400 => match (seconds % 3_600) / 60 {
            0 => format!("{} h", seconds / 3_600),
            minutes => format!("{} h {minutes} min", seconds / 3_600),
        },
        _ => match seconds / 86_400 {
            1 => "1 day".to_string(),
            days => format!("{days} days"),
        },
    }
}

/// A whole number with its thousands apart: 12,345.
pub fn thousands(number: u64) -> String {
    let digits = number.to_string();
    let mut said = String::new();

    for (place, digit) in digits.chars().enumerate() {
        if place > 0 && (digits.len() - place).is_multiple_of(3) {
            said.push(',');
        }

        said.push(digit);
    }

    said
}

/// A ring toward what a change needs, with the share of Yes in its middle, and inside it a thinner
/// one toward the quorum when there is one. Yes fills from the top; No shows as a short red run
/// from the end, so a ring that could no longer close says so.
pub fn ring(ui: &mut Ui, palette: &Palette, tally: &Tally, diameter: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(diameter, diameter), Sense::hover());
    let painter = ui.painter_at(rect.expand(2.0));
    let center = rect.center();
    let outer = diameter / 2.0 - 4.0;
    let width = (diameter / 9.0).clamp(4.0, 9.0);
    let yes = tally.support() as f32;

    painter.circle_stroke(center, outer, Stroke::new(width, palette.line));
    arc(&painter, center, outer, 0.0, yes, Stroke::new(width, if tally.can_pass { palette.good } else { palette.muted }));

    // No, from where the ring ends, against what is needed: past what leaves room for Yes, it cannot close.
    let no = fraction(tally.no, tally.needed.max(tally.total - tally.needed).max(1.0)) as f32;
    arc(&painter, center, outer, 1.0 - no.min(1.0 - yes), 1.0, Stroke::new(width * 0.5, palette.bad));

    if let Some(quorum) = tally.quorum_progress() {
        let inner = outer - width - 3.0;
        painter.circle_stroke(center, inner, Stroke::new(2.0, palette.line));
        arc(&painter, center, inner, 0.0, quorum as f32, Stroke::new(2.5, palette.accent));
    }

    let percent = format!("{:.0}%", tally.support() * 100.0);
    painter.text(center, Align2::CENTER_CENTER, percent, FontId::proportional((diameter / 4.2).clamp(10.0, 20.0)), palette.ink);

    response.on_hover_text(match tally.quorum_progress() {
        Some(quorum) => format!("Yes has {:.0}% of what it needs; votes of any kind have {:.0}% of the quorum", tally.support() * 100.0, quorum * 100.0),
        None => format!("Yes has {:.0}% of what it needs", tally.support() * 100.0),
    })
}

/// An arc of a circle, from one share of the way round to another, clockwise from the top.
fn arc(painter: &egui::Painter, center: Pos2, radius: f32, from: f32, to: f32, stroke: Stroke) {
    if to <= from {
        return;
    }

    let steps = ((to - from) * 64.0).ceil().max(2.0) as usize;
    let points: Vec<Pos2> = (0..=steps)
        .map(|step| {
            let angle = (from + (to - from) * step as f32 / steps as f32) * TAU - TAU / 4.0;

            center + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    painter.add(Shape::line(points, stroke));
}

/// One voter, as a dot: who, how it voted if it did, and what its vote weighs, in words.
#[derive(Clone, Debug, PartialEq)]
pub struct Dot {
    pub label: String,
    pub vote: Option<Choice>,
    pub about: String,
}

/// How wide one voter's dot and label are, at most.
const DOT_WIDTH: f32 = 112.0;

/// A dot a voter, colored and shaped by its vote, each labelled, wrapping to the width there is. A
/// hover or a tap says who and what its vote weighs.
pub fn dots(ui: &mut Ui, palette: &Palette, voters: &[Dot]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);

        for voter in voters {
            let label = short_label(&voter.label, 14);
            let (rect, response) = ui.allocate_exact_size(vec2(DOT_WIDTH.min(ui.available_width().max(40.0)), 22.0), Sense::click());
            let at = pos2(rect.left() + 9.0, rect.center().y);
            mark(ui.painter(), palette, at, voter.vote, 5.5);
            ui.painter().text(pos2(rect.left() + 20.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(11.5), palette.soft);

            let vote = voter.vote.map_or("has not voted", Choice::word);
            response.on_hover_text(format!("{}: {vote}. {}", voter.label, voter.about));
        }
    });
}

/// A vote's mark: Yes a filled dot, No a cross, Abstain a half-filled ring, none an empty ring.
pub fn mark(painter: &egui::Painter, palette: &Palette, at: Pos2, vote: Option<Choice>, radius: f32) {
    match vote {
        Some(Choice::Yes) => {
            painter.circle_filled(at, radius, palette.good);
        }
        Some(Choice::No) => {
            let r = radius * 0.8;
            let stroke = Stroke::new(2.2, palette.bad);
            painter.line_segment([at + vec2(-r, -r), at + vec2(r, r)], stroke);
            painter.line_segment([at + vec2(-r, r), at + vec2(r, -r)], stroke);
        }
        Some(Choice::Abstain) => {
            painter.circle_stroke(at, radius - 1.0, Stroke::new(1.6, palette.warn));
            painter.circle_filled(at, radius * 0.45, palette.warn);
        }
        None => {
            painter.circle_stroke(at, radius - 1.0, Stroke::new(1.4, palette.muted));
        }
    }
}

/// A label cut to so many characters, an ellipsis in the middle of an address.
pub fn short_label(text: &str, most: usize) -> String {
    let chars: Vec<char> = text.chars().collect();

    match chars.len() <= most {
        true => text.to_string(),
        false => {
            let head: String = chars[..most / 2].iter().collect();
            let tail: String = chars[chars.len() - (most - most / 2 - 1)..].iter().collect();

            format!("{head}…{tail}")
        }
    }
}

/// One field of a setting, as it stands and as a change would make it: None where it has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// Where it is: fees.pricing.min, signers[2].
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

impl Field {
    /// Whether the change moves it.
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

/// Every field of a setting as it stands beside what a change would make it, by path, in the
/// order the change lists them and then any it leaves out: what both apps' before and after shows,
/// for a setting of any kind, so a new kind needs no code of its own.
pub fn fields(before: &Value, after: &Value) -> Vec<Field> {
    let (mut before_all, mut after_all) = (Vec::new(), Vec::new());
    leaves(before, String::new(), &mut before_all);
    leaves(after, String::new(), &mut after_all);
    // An empty list or object where the other side has things in it is those things' absence, not
    // a field of its own.
    let filled = |path: &str, other: &[(String, String, bool)]| other.iter().any(|(inner, _, _)| inner != path && (path == "value" || inner.starts_with(&format!("{path}[")) || inner.starts_with(&format!("{path}."))));
    let kept = |side: &[(String, String, bool)], other: &[(String, String, bool)]| -> Vec<(String, String)> {
        side.iter().filter(|(path, _, empty)| !(*empty && filled(path, other))).map(|(path, value, _)| (path.clone(), value.clone())).collect()
    };
    let (was, will) = (kept(&before_all, &after_all), kept(&after_all, &before_all));

    let mut found: Vec<Field> = will.iter().map(|(path, value)| Field { path: path.clone(), before: was.iter().find(|(old, _)| old == path).map(|(_, value)| value.clone()), after: Some(value.clone()) }).collect();

    for (path, value) in was {
        if !found.iter().any(|field| field.path == path) {
            found.push(Field { path, before: Some(value), after: None });
        }
    }

    found
}

/// A value's leaves, each by its path: objects by key, lists by place, an empty one as itself,
/// marked as empty.
fn leaves(value: &Value, path: String, out: &mut Vec<(String, String, bool)>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, inner) in map {
                let path = match path.is_empty() {
                    true => key.clone(),
                    false => format!("{path}.{key}"),
                };
                leaves(inner, path, out);
            }
        }
        Value::Array(list) if !list.is_empty() => {
            for (place, inner) in list.iter().enumerate() {
                leaves(inner, format!("{path}[{place}]"), out);
            }
        }
        leaf => {
            let empty = matches!(leaf, Value::Array(_) | Value::Object(_));
            out.push((if path.is_empty() { "value".to_string() } else { path }, said(leaf), empty));
        }
    }
}

/// A leaf as text: a string as itself, null as none, anything else as JSON.
fn said(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "none".to_string(),
        Value::Array(list) if list.is_empty() => "none".to_string(),
        Value::Object(map) if map.is_empty() => "none".to_string(),
        other => other.to_string(),
    }
}

/// A field's name for a person, from its path: pricing min, signers 3.
pub fn field_name(path: &str) -> String {
    path.replace(['.', '_'], " ").replace('[', " ").replace(']', "")
}

/// What a change would move: each field that moves, its old value struck beside the new, with how
/// many stay as they are; or, asked, every field.
pub fn diff(ui: &mut Ui, palette: &Palette, fields: &[Field], every: bool) {
    let moved = fields.iter().filter(|field| field.changed()).count();

    if moved == 0 {
        ui.label(RichText::new("It changes nothing that stands").small().color(palette.muted));
    }

    for field in fields.iter().filter(|field| every || field.changed()) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let name = RichText::new(field_name(&field.path)).small().color(palette.muted);
            ui.label(name);

            match (field.changed(), &field.before, &field.after) {
                (false, _, Some(value)) => {
                    ui.add(egui::Label::new(RichText::new(value).monospace().color(palette.soft)).wrap());
                }
                (_, before, after) => {
                    ui.add(egui::Label::new(RichText::new(before.as_deref().unwrap_or("none")).monospace().strikethrough().color(palette.muted)).wrap());
                    ui.label(RichText::new("→").color(palette.muted));
                    ui.add(egui::Label::new(RichText::new(after.as_deref().unwrap_or("none")).monospace().strong().color(palette.accent).background_color(palette.accent.gamma_multiply(0.12))).wrap());
                }
            }
        });
    }

    let same = fields.len() - moved;

    if !every && same > 0 && moved > 0 {
        let said = match same {
            1 => "1 other field stays as it is".to_string(),
            same => format!("{same} other fields stay as they are"),
        };
        ui.label(RichText::new(said).small().color(palette.muted));
    }
}

/// A kind of change's color, the same on every page: known kinds a fixed hue each, a kind added
/// later one from its name.
pub fn kind_color(kind: &str, palette: &Palette) -> Color32 {
    const HUES: [(&str, f32); 17] = [
        ("consensus", 0.58),
        ("finality", 0.62),
        ("switch_finality", 0.66),
        ("bootnodes", 0.08),
        ("name_prices", 0.14),
        ("names", 0.14),
        ("collections", 0.80),
        ("tokens", 0.85),
        ("federation", 0.50),
        ("directory", 0.45),
        ("fees", 0.03),
        ("reward", 0.11),
        ("limits", 0.92),
        ("upgrade", 0.33),
        ("name", 0.72),
        ("governance", 0.0),
        ("dex", 0.40),
    ];
    let hue = HUES.iter().find(|(known, _)| *known == kind).map_or_else(|| kind.bytes().fold(7u32, |sum, byte| sum.wrapping_mul(31).wrapping_add(u32::from(byte))) as f32 % 360.0 / 360.0, |(_, hue)| *hue);
    let (saturation, value) = if palette.dark { (0.55, 0.92) } else { (0.75, 0.62) };

    egui::ecolor::Hsva::new(hue, saturation, value, 1.0).into()
}

/// One decision or rejection along the chain: where it landed, its kind, what to call it and
/// whether it was a rejection.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub height: u64,
    pub kind: String,
    /// What it is, short: fees v3, or fees rejected.
    pub label: String,
    pub rejected: bool,
    /// What a hover says.
    pub about: String,
}

/// The decisions so far by height, one more at each: a chart's line of decisions over time.
pub fn cumulative(marks: &[Mark]) -> Vec<[f64; 2]> {
    let mut decided: Vec<u64> = marks.iter().filter(|mark| !mark.rejected).map(|mark| mark.height).collect();
    decided.sort_unstable();

    decided.iter().enumerate().map(|(count, height)| [*height as f64, (count + 1) as f64]).collect()
}

/// Decisions and rejections along the heights they landed at: decisions above the line, filled and
/// colored by kind, rejections below it, crossed; the ends say the heights. A hover says what each
/// is. Marks too close to tell apart stack upward.
pub fn timeline(ui: &mut Ui, palette: &Palette, marks: &[Mark], height: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect);
    let font = FontId::monospace(10.0);
    let line = rect.center().y;
    painter.line_segment([pos2(rect.left() + 4.0, line), pos2(rect.right() - 4.0, line)], Stroke::new(1.0, palette.line));

    let Some(low) = marks.iter().map(|mark| mark.height).min() else {
        painter.text(rect.center(), Align2::CENTER_CENTER, "nothing decided or rejected yet", FontId::proportional(11.0), palette.muted);

        return response;
    };
    let high = marks.iter().map(|mark| mark.height).max().unwrap_or(low).max(low + 1);
    let plot = Rect::from_min_max(pos2(rect.left() + 10.0, rect.top()), pos2(rect.right() - 10.0, rect.bottom()));
    let across = |height: u64| plot.left() + (height - low) as f32 / (high - low) as f32 * plot.width();
    painter.text(pos2(plot.left(), rect.bottom() - 1.0), Align2::LEFT_BOTTOM, format!("block {}", thousands(low)), font.clone(), palette.muted);
    painter.text(pos2(plot.right(), rect.bottom() - 1.0), Align2::RIGHT_BOTTOM, format!("block {}", thousands(high)), font, palette.muted);

    // Each mark's place, stacking those that would overlap on its side of the line.
    let mut placed: Vec<(Pos2, &Mark)> = Vec::new();

    for mark in marks {
        let x = across(mark.height);
        let side = if mark.rejected { 1.0 } else { -1.0 };
        let stacked = placed.iter().filter(|(at, other)| other.rejected == mark.rejected && (at.x - x).abs() < 9.0).count() as f32;
        let y = (line + side * (9.0 + stacked * 9.0)).clamp(rect.top() + 5.0, rect.bottom() - 14.0);
        let at = pos2(x, y);
        let color = kind_color(&mark.kind, palette);
        painter.line_segment([pos2(x, line), at], Stroke::new(1.0, color.gamma_multiply(0.5)));

        match mark.rejected {
            true => {
                let stroke = Stroke::new(2.0, color);
                painter.line_segment([at + vec2(-3.5, -3.5), at + vec2(3.5, 3.5)], stroke);
                painter.line_segment([at + vec2(-3.5, 3.5), at + vec2(3.5, -3.5)], stroke);
            }
            false => {
                painter.circle_filled(at, 4.0, color);
            }
        }

        placed.push((at, mark));
    }

    if let Some(pointer) = response.hover_pos()
        && let Some((at, mark)) = placed.iter().min_by(|a, b| a.0.distance(pointer).total_cmp(&b.0.distance(pointer)))
        && at.distance(pointer) < 24.0
    {
        painter.circle_stroke(*at, 7.0, Stroke::new(1.0, palette.soft));
        let (label, about) = (mark.label.clone(), mark.about.clone());
        response.clone().on_hover_ui_at_pointer(|ui| {
            ui.label(RichText::new(label).strong());
            ui.label(RichText::new(about).small().color(palette.muted));
        });
    }

    response
}

/// The kinds a timeline shows, each with its color.
pub fn legend(ui: &mut Ui, palette: &Palette, kinds: &[String], name: &dyn Fn(&str) -> String) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;

        for kind in kinds {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, kind_color(kind, palette));
                ui.label(RichText::new(name(kind)).small().color(palette.soft));
            });
        }

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            let (at, stroke) = (rect.center(), Stroke::new(1.6, palette.muted));
            ui.painter().line_segment([at + vec2(-3.0, -3.0), at + vec2(3.0, 3.0)], stroke);
            ui.painter().line_segment([at + vec2(-3.0, 3.0), at + vec2(3.0, -3.0)], stroke);
            ui.label(RichText::new("rejected, below the line").small().color(palette.muted));
        });
    });
}

/// One voter's turnout: who, what share of the electorate it weighs, what share of the changes put
/// to a vote it voted on, and its last vote, in words.
#[derive(Clone, Debug, PartialEq)]
pub struct Turnout {
    pub label: String,
    /// Its share of every voter's weight, from 0 to 1.
    pub share: f64,
    /// Its share of the changes voted on, from 0 to 1, or None while its votes are being read.
    pub participation: Option<f64>,
    pub last: Option<String>,
}

/// How wide a voter's tile is at least.
const TILE: f32 = 190.0;

/// A tile a voter, as many across as fit: its name, its turnout as a bar, its last vote, and, when
/// votes weigh stake, its share of it as a bar of its own.
pub fn turnout(ui: &mut Ui, palette: &Palette, voters: &[Turnout], stake: bool) {
    let gap = 8.0;
    let (columns, width) = crate::layout::tiles(ui.available_width(), TILE, gap);

    for row in voters.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;

            for voter in row {
                ui.allocate_ui_with_layout(vec2(width, 10.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_max_width(width);
                    egui::Frame::new().fill(palette.row).corner_radius(6.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                        ui.set_width(width - 16.0);
                        ui.add(egui::Label::new(RichText::new(&voter.label).strong().color(palette.ink)).truncate());
                        let (turnout, said) = match voter.participation {
                            Some(share) => (share, format!("voted on {:.0}% of changes", share * 100.0)),
                            None => (0.0, "reading its votes…".to_string()),
                        };
                        bar(ui, palette, turnout, palette.accent, &said);

                        if stake {
                            bar(ui, palette, voter.share, palette.good, &format!("{:.1}% of the stake", voter.share * 100.0));
                        }

                        ui.label(RichText::new(voter.last.as_deref().unwrap_or("no vote read")).small().color(palette.muted));
                    });
                });
            }
        });
        ui.add_space(gap / 2.0);
    }
}

/// A thin bar filled to a share, what it is said under it.
pub fn bar(ui: &mut Ui, palette: &Palette, share: f64, color: Color32, said: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    ui.painter().rect_filled(rect, 3.0, palette.line);
    let filled = Rect::from_min_size(rect.min, vec2(rect.width() * share.clamp(0.0, 1.0) as f32, rect.height()));
    ui.painter().rect_filled(filled, 3.0, color);
    ui.label(RichText::new(said).small().color(palette.soft));
}

/// What the votes on a change came to, as a row of words: Yes, No and Abstain, each by its mark,
/// with what each weighs, said as the app says weights.
pub fn sides(ui: &mut Ui, palette: &Palette, weights: &[String; 3]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;

        for (choice, weight) in [Choice::Yes, Choice::No, Choice::Abstain].into_iter().zip(weights) {
            let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            mark(ui.painter(), palette, rect.center(), Some(choice), 5.0);
            ui.label(RichText::new(format!("{} {weight}", choice.word())).small().color(palette.soft));
            ui.add_space(6.0);
        }
    });
}

/// A change put to a vote, as both apps' cards show it: what it is, how its votes stand, who voted
/// which way, when old votes stop counting, and what it would move.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    /// Its kind, for a person: Fees.
    pub title: String,
    /// Where it stands among its kind: version 2, voting on the fees' change 3.
    pub subtitle: String,
    /// What it does, in words.
    pub what: String,
    pub tally: Tally,
    /// What Yes, No and Abstain weigh, and what it lacks, said as the app says weights.
    pub weights: [String; 3],
    pub standing: String,
    /// When old votes stop counting, if any count.
    pub closing: Option<String>,
    pub voters: Vec<Dot>,
    /// The setting as it stands beside what the change would make it.
    pub fields: Vec<Field>,
}

/// How wide the ring is on a card.
const RING: f32 = 64.0;

/// A change's card: its ring beside what it is and where it stands, a dot a voter, what it would
/// move, and whatever the app adds below, its buttons or its links. Narrow, the ring sits above.
pub fn card(ui: &mut Ui, palette: &Palette, card: &Card, narrow: bool, more: impl FnOnce(&mut Ui)) {
    let words = |ui: &mut Ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&card.title).strong().color(palette.soft));
            ui.label(RichText::new(&card.subtitle).small().color(palette.muted));
        });
        ui.add(egui::Label::new(RichText::new(&card.what).strong().size(14.0).color(palette.ink)).wrap());
        let standing = if card.tally.can_pass { palette.soft } else { palette.bad };
        ui.add(egui::Label::new(RichText::new(&card.standing).color(standing)).wrap());
        sides(ui, palette, &card.weights);

        if let Some(closing) = &card.closing {
            ui.add(egui::Label::new(RichText::new(closing).small().color(palette.muted)).wrap());
        }
    };

    match narrow {
        true => {
            ring(ui, palette, &card.tally, RING * 0.85);
            words(ui);
        }
        false => {
            ui.horizontal(|ui| {
                ring(ui, palette, &card.tally, RING);
                ui.vertical(words);
            });
        }
    }

    if !card.voters.is_empty() {
        ui.add_space(4.0);
        dots(ui, palette, &card.voters);
    }

    if !card.fields.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("What it moves").small().color(palette.muted));
        diff(ui, palette, &card.fields, false);
    }

    more(ui);
}

#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

    fn tally() -> Tally {
        Tally::of(&json!({ "yes": "1", "no": "1", "abstain": "0", "total": "4", "needed": "3", "quorum": "2", "can_pass": true }))
    }

    #[test]
    fn a_tally_reads_the_node_s_weights_and_says_what_it_lacks() {
        let tally = tally();
        let say = |weight: f64| format!("{weight}");

        assert_eq!((tally.yes, tally.no, tally.voted(), tally.needed), (1.0, 1.0, 2.0, 3.0));
        assert!((tally.support() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(tally.quorum_progress(), Some(1.0));
        assert_eq!(tally.standing(&say), "2 more Yes needed");
        assert_eq!(Tally { can_pass: false, ..tally }.standing(&say), "Can no longer pass: too much has voted No");
        assert_eq!(Tally { yes: 3.0, no: 0.0, quorum: 4.0, ..tally }.standing(&say), "Yes is enough; 1 more votes of any kind make the quorum");
        assert_eq!(Tally { quorum: 0.0, ..tally }.quorum_progress(), None, "no quorum to reach");
        assert_eq!(Tally::of(&json!({ "yes": "lots" })).yes, 0.0, "what does not read weighs nothing");
        let huge = Tally::of(&json!({ "yes": "6666666666666666666666666666666666666", "needed": "10000000000000000000000000000000000000" }));
        assert!((huge.support() - 0.6667).abs() < 0.001, "weights of any size");
    }

    #[test]
    fn old_votes_stop_counting_at_the_next_epoch_s_first_block() {
        // Counting through epoch 3 of 10 blocks: they stop at block 40, so at tip 29 eleven blocks on.
        assert_eq!(closing(3, 29, 10, Some(5_000)), "The oldest votes stop counting in 10 blocks, about 50 s");
        assert_eq!(closing(3, 39, 10, None), "The oldest votes stop counting with the next block");
        assert_eq!(closing(3, 0, 10, None), "The oldest votes stop counting in 39 blocks");
        assert_eq!([duration(40_000), duration(720_000), duration(12_000_000), duration(172_800_000)], ["40 s", "12 min", "3 h 20 min", "2 days"]);
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn a_change_s_fields_beside_what_stands_mark_what_moves() {
        let standing = json!({ "block_gas_limit": 50_000_000, "burn_permille": 500, "pricing": { "type": "base_fee", "min": "10" }, "signers": ["a", "b"] });
        let change = json!({ "block_gas_limit": 50_000_000, "burn_permille": 600, "pricing": { "type": "base_fee", "min": "20" }, "signers": ["a"] });
        let found = fields(&standing, &change);
        let moved: Vec<(&str, Option<&str>, Option<&str>)> = found.iter().filter(|field| field.changed()).map(|field| (field.path.as_str(), field.before.as_deref(), field.after.as_deref())).collect();

        assert_eq!(moved, [("burn_permille", Some("500"), Some("600")), ("pricing.min", Some("10"), Some("20")), ("signers[1]", Some("b"), None)]);
        assert_eq!(found.iter().filter(|field| !field.changed()).count(), 3, "the gas limit, the pricing's type and the first signer stay");
        assert_eq!(fields(&json!("old"), &json!("new")), [Field { path: "value".to_string(), before: Some("old".to_string()), after: Some("new".to_string()) }], "a setting that is one value");
        assert_eq!(fields(&json!(null), &json!({ "a": 1 }))[0].before, None, "a setting there was none of");
        assert_eq!(field_name("pricing.min_price"), "pricing min price");
        let filled = fields(&json!([]), &json!(["/ip4/1"]));
        assert_eq!(filled, [Field { path: "[0]".to_string(), before: None, after: Some("/ip4/1".to_string()) }], "a list that was empty gains its first");
        assert_eq!(fields(&json!({ "signers": ["a"] }), &json!({ "signers": [] }))[0], Field { path: "signers[0]".to_string(), before: Some("a".to_string()), after: None });
    }

    #[test]
    fn decisions_count_up_by_height_and_rejections_do_not() {
        let mark = |height: u64, rejected: bool| Mark { height, kind: "fees".to_string(), label: String::new(), rejected, about: String::new() };

        assert_eq!(cumulative(&[mark(30, false), mark(10, false), mark(20, true)]), [[10.0, 1.0], [30.0, 2.0]]);
        assert_ne!(kind_color("fees", &crate::DARK), kind_color("reward", &crate::DARK));
        assert_eq!(kind_color("later", &crate::DARK), kind_color("later", &crate::DARK), "a new kind's color holds");
        assert_eq!(short_label("ed25519:0x0123456789abcdef", 12), "ed2551…bcdef");
        assert_eq!([Choice::of("yes"), Choice::of("no"), Choice::of("abstain"), Choice::of("maybe")], [Some(Choice::Yes), Some(Choice::No), Some(Choice::Abstain), None]);
    }

    #[test]
    fn the_drawings_stay_in_a_phone_s_width() {
        let ctx = egui::Context::default();

        for width in [360.0, 740.0, 1200.0] {
            let screen = Rect::from_min_size(Pos2::ZERO, vec2(width, 800.0));
            let mut drawn = Rect::NOTHING;
            let mut parts = Vec::new();

            for _ in 0..2 {
                parts.clear();
                let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                    drawn = ui
                        .scope(|ui| {
                            parts.push(ring(ui, &crate::DARK, &tally(), 72.0).rect.right());
                            let voters: Vec<Dot> = (0..9).map(|place| Dot { label: format!("ed25519:0x{place:064}"), vote: [Some(Choice::Yes), Some(Choice::No), None][place % 3], about: String::new() }).collect();
                            parts.push(ui.scope(|ui| dots(ui, &crate::DARK, &voters)).response.rect.right());
                            parts.push(ui.scope(|ui| diff(ui, &crate::DARK, &fields(&json!({ "a": "x".repeat(200) }), &json!({ "a": "y" })), false)).response.rect.right());
                            let marks: Vec<Mark> = (0..20).map(|place| Mark { height: place * 7, kind: "fees".to_string(), label: String::new(), rejected: place % 4 == 0, about: String::new() }).collect();
                            parts.push(timeline(ui, &crate::DARK, &marks, 70.0).rect.right());
                            let tiles: Vec<Turnout> = (0..5).map(|place| Turnout { label: format!("member {place}"), share: 0.2, participation: Some(0.5), last: None }).collect();
                            parts.push(ui.scope(|ui| turnout(ui, &crate::DARK, &tiles, true)).response.rect.right());
                            let shown = Card { title: "Fees".to_string(), what: "w ".repeat(80), tally: tally(), voters: voters.clone(), fields: fields(&json!({ "a": 1 }), &json!({ "a": 2 })), ..Card::default() };
                            parts.push(ui.scope(|ui| card(ui, &crate::DARK, &shown, width < 600.0, |_| {})).response.rect.right());
                        })
                        .response
                        .rect;
                });
                output.textures_delta.clear();
            }

            assert!(drawn.right() <= width + 1.0, "within {width}: {drawn:?} {parts:?}");
        }
    }
}
