//! Small charts that only show: a line or bars over block heights, with a scale on the left. They
//! sense the pointer to say what lies under it, and never take a drag, so a page on a phone scrolls
//! over them as over anything else.

use {
    super::Look,
    egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, pos2, vec2},
};

/// How a series is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Line,
    Bars,
}

/// What one chart shows.
pub struct Series {
    pub name: String,
    /// Each point's block height and value, in order.
    pub points: Vec<[f64; 2]>,
    pub kind: Kind,
    pub color: Color32,
    /// A value to mark across the chart, and what it is: a block's gas limit.
    pub limit: Option<(f64, &'static str)>,
    /// How a value reads.
    pub unit: fn(f64) -> String,
    /// How a point's place along reads: a block's height, or a time.
    pub across: fn(f64) -> String,
}

/// A point's place along, as a block height.
pub fn block(height: f64) -> String {
    format!("block {}", height as u64)
}

/// The highest value a scale must reach, rounded up to one with a single significant digit or two,
/// so its labels read cleanly: 47 becomes 50, 1,240,000 becomes 1,500,000. At least 1.
pub fn ceiling(value: f64) -> f64 {
    if value <= 0.0 || !value.is_finite() {
        return 1.0;
    }

    let power = 10f64.powf(value.log10().floor());

    [1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0]
        .into_iter()
        .map(|step| step * power)
        .find(|step| *step >= value * 0.999)
        .unwrap_or(10.0 * power)
}

/// A number short enough for a scale: 950, 1.2k, 50M, 3B.
pub fn short(value: f64) -> String {
    let (scaled, suffix) = match value.abs() {
        value if value >= 1e9 => (value / 1e9, "B"),
        value if value >= 1e6 => (value / 1e6, "M"),
        value if value >= 1e3 => (value / 1e3, "k"),
        _ => (value.abs(), ""),
    };
    let sign = if value < 0.0 { "-" } else { "" };
    let text = match (scaled.fract() == 0.0 || scaled >= 100.0, scaled < 10.0) {
        (true, _) => format!("{scaled:.0}"),
        (false, true) => format!("{scaled:.2}").trim_end_matches('0').trim_end_matches('.').to_string(),
        (false, false) => format!("{scaled:.1}").trim_end_matches('0').trim_end_matches('.').to_string(),
    };

    format!("{sign}{text}{suffix}")
}

pub fn chart(ui: &mut Ui, look: Look, series: &Series, height: f32) {
    let palette = look.palette;
    let name = match series.limit {
        Some((limit, what)) => format!("{} · {what} {}", series.name, short(limit)),
        None => series.name.clone(),
    };
    ui.label(RichText::new(name).small().color(palette.muted));

    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect);
    let highest = series.points.iter().map(|[_, y]| *y).fold(0.0, f64::max);
    // A limit far above everything would flatten the chart: it shows only once values near it.
    let limit = series.limit.filter(|(limit, _)| highest >= limit * 0.2);
    let top = limit.map_or(highest, |(limit, _)| highest.max(limit));
    let ceiling = ceiling(top);
    // Proportional: at this size a monospace B reads as an 8, so 1B looked like 18.
    let font = FontId::proportional(10.5);
    let label_width = [ceiling, ceiling / 2.0]
        .iter()
        .map(|value| ui.fonts_mut(|fonts| fonts.layout_no_wrap(short(*value), font.clone(), palette.muted).size().x))
        .fold(0.0, f32::max);
    // Room above and below for half a label, as the top and the nothing line are labelled at their middles.
    let plot = Rect::from_min_max(pos2(rect.left() + label_width + 6.0, rect.top() + 7.0), pos2(rect.right() - 2.0, rect.bottom() - 7.0));

    // The scale: lines at nothing, half and the ceiling, labelled on the left.
    for fraction in [0.0, 0.5, 1.0] {
        let y = plot.bottom() - fraction * plot.height();
        let color = if fraction == 0.0 { palette.line } else { palette.line.gamma_multiply(0.6) };
        painter.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, color));
        painter.text(pos2(plot.left() - 5.0, y), Align2::RIGHT_CENTER, short(ceiling * fraction as f64), font.clone(), palette.muted);
    }

    let Some((first, last)) = series.points.first().zip(series.points.last()) else {
        return;
    };
    let (left, right) = (first[0], last[0].max(first[0] + 1.0));
    let count = series.points.len().max(1) as f32;
    let slot = plot.width() / count;
    let at = |[x, y]: [f64; 2]| -> Pos2 {
        let across = match series.kind {
            Kind::Bars => plot.left() + slot * (((x - left) / (right - left)) as f32 * (count - 1.0) + 0.5),
            Kind::Line => plot.left() + ((x - left) / (right - left)) as f32 * plot.width(),
        };

        pos2(across, plot.bottom() - (y / ceiling) as f32 * plot.height())
    };

    match series.kind {
        Kind::Line => {
            let points: Vec<Pos2> = series.points.iter().map(|point| at(*point)).collect();
            painter.add(Shape::line(points, Stroke::new(1.6, series.color)));
        }
        Kind::Bars => {
            let width = (slot * 0.62).max(1.0);

            for point in &series.points {
                let top = at(*point);
                let bar = Rect::from_min_max(pos2(top.x - width / 2.0, top.y), pos2(top.x + width / 2.0, plot.bottom()));
                painter.rect_filled(bar, 1.5, series.color.gamma_multiply(0.75));
            }
        }
    }

    if let Some((limit, name)) = limit {
        let y = plot.bottom() - (limit / ceiling) as f32 * plot.height();
        painter.extend(Shape::dashed_line(&[pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, look.roles.side), 5.0, 4.0));
        painter.text(pos2(plot.right() - 2.0, y + 2.0), Align2::RIGHT_TOP, name, FontId::proportional(10.0), look.roles.side);
    }

    // What lies under the pointer: the nearest block, marked, and its value said.
    if let Some(pointer) = response.hover_pos()
        && let Some(nearest) = series.points.iter().min_by(|a, b| (at(**a).x - pointer.x).abs().total_cmp(&(at(**b).x - pointer.x).abs()))
    {
        let point = at(*nearest);
        painter.line_segment([pos2(point.x, plot.top()), pos2(point.x, plot.bottom())], Stroke::new(1.0, palette.soft.gamma_multiply(0.4)));
        painter.circle_filled(point, 3.0, series.color);
        response.on_hover_ui_at_pointer(|ui| {
            ui.label(RichText::new((series.across)(nearest[0])).small().color(palette.muted));
            ui.label(RichText::new((series.unit)(nearest[1])).monospace());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_end_on_round_numbers_and_say_them_briefly() {
        assert_eq!(ceiling(47.0), 50.0);
        assert_eq!(ceiling(2.0), 2.0);
        assert_eq!(ceiling(1_240_000.0), 1_500_000.0);
        assert_eq!(ceiling(0.0), 1.0, "an empty chart still has a scale");
        assert_eq!([short(950.0), short(1_200.0), short(50_000_000.0), short(2.5), short(3e9)], ["950", "1.2k", "50M", "2.5", "3B"]);
        assert_eq!([short(1.25), short(12.5), short(1_250.0)], ["1.25", "12.5", "1.25k"]);
    }
}
