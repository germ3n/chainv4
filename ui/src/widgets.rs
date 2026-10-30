//! The pieces pages are built from. Each takes its colors from a palette, and its sizes from the
//! style apply set: a card is roomy in a comfortable look and tight in a compact one.

use {
    crate::theme::{DISPLAY, Palette, SEMIBOLD},
    egui::{Color32, FontFamily, FontId, Frame, Margin, Response, RichText, Stroke, TextStyle, Ui},
};

/// A page's title, in the display face, with a line under it saying what the page is for.
pub fn title(ui: &mut Ui, palette: &Palette, title: &str, lead: &str) {
    let size = ui.style().text_styles.get(&TextStyle::Heading).map_or(26.0, |font| font.size) * 1.15;
    ui.label(RichText::new(title).font(FontId::new(size, FontFamily::Name(DISPLAY.into()))).color(palette.ink));

    if !lead.is_empty() {
        ui.add_space(2.0);
        ui.label(RichText::new(lead).color(palette.muted));
    }

    ui.add_space(size * 0.55);
}

/// A small label in capitals, spaced out: a section's heading.
pub fn caps(ui: &mut Ui, palette: &Palette, text: &str) -> Response {
    let mut job = egui::text::LayoutJob::default();
    job.append(&text.to_uppercase(), 0.0, egui::TextFormat {
        font_id: FontId::new(10.5, FontFamily::Name(SEMIBOLD.into())),
        color: palette.heading,
        extra_letter_spacing: 1.6,
        ..Default::default()
    });

    ui.label(job)
}

/// One line of text that shrinks to fit the width there is, down to smallest, and is cut short past
/// that: a row never grows past the screen, however large what it shows.
pub fn fitted(ui: &mut Ui, text: &str, size: f32, smallest: f32, family: FontFamily, color: Color32) -> Response {
    let width = ui.available_width().max(1.0);
    let natural = ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_string(), FontId::new(size, family.clone()), color).size().x);
    let size = match natural > width {
        true => (size * width / natural).max(smallest),
        false => size,
    };

    ui.add(egui::Label::new(RichText::new(text).font(FontId::new(size, family)).color(color)).truncate())
}

/// Text in the semibold face.
pub fn strong(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text.into()).font(FontId::new(size, FontFamily::Name(SEMIBOLD.into())))
}

/// Text in the monospace face, for addresses, hashes and numbers to compare.
pub fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace()
}

/// A card: what each part of a page sits in, with the look's radius and margin.
pub fn card<R>(ui: &mut Ui, palette: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    let (radius, margin) = (ui.visuals().window_corner_radius, ui.spacing().window_margin);

    Frame::new()
        .fill(palette.card)
        .stroke(Stroke::new(1.0, palette.line))
        .corner_radius(radius)
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// The one button a step leads to, filled with the accent.
pub fn primary(ui: &mut Ui, palette: &Palette, text: &str, enabled: bool) -> Response {
    let (size, height) = (button_size(ui), ui.spacing().interact_size.y);
    let button = egui::Button::new(strong(text, size).color(if enabled { palette.on_accent } else { palette.muted }))
        .fill(if enabled { palette.accent } else { palette.row })
        .stroke(Stroke::NONE)
        .corner_radius(ui.visuals().widgets.inactive.corner_radius)
        .min_size(egui::vec2(height * 4.0, height + 4.0));

    ui.add_enabled(enabled, button)
}

/// A choice of a few, one of them on, joined in a row: the one on filled with the accent, so it
/// reads as on at a glance, the others quiet. Whether the choice changed.
pub fn segmented<T: Copy + PartialEq>(ui: &mut Ui, palette: &Palette, current: &mut T, options: &[(T, &str)]) -> bool {
    let radius = ui.visuals().widgets.inactive.corner_radius;
    let mut changed = false;

    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        Frame::new().stroke(Stroke::new(1.0, palette.line)).corner_radius(radius).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (value, name) in options {
                    let on = *current == *value;
                    let text = match on {
                        true => strong(*name, button_size(ui)).color(palette.on_accent),
                        false => RichText::new(*name).color(palette.soft),
                    };
                    let button = egui::Button::new(text).fill(if on { palette.accent } else { Color32::TRANSPARENT }).stroke(Stroke::NONE).corner_radius(radius);

                    if ui.add(button).clicked() && !on {
                        *current = *value;
                        changed = true;
                    }
                }
            });
        });
    });

    changed
}

/// A quiet button, outlined.
pub fn quiet(ui: &mut Ui, palette: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(palette.soft))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, palette.line))
            .corner_radius(ui.visuals().widgets.inactive.corner_radius),
    )
}

/// A rounded tag in a color: a status at a glance.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = ui.style().text_styles.get(&TextStyle::Small).cloned().unwrap_or_else(|| egui::FontId::proportional(11.5));
    let size = font.size;
    // Whole, never a letter a line: in a row that wraps, one that would not fit starts the next line.
    let measured = ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_string(), font, color).size());
    let width = measured.x + 20.0;

    if ui.layout().main_wrap && ui.available_size_before_wrap().x < width {
        ui.end_row();
    }

    // Its own size, however tall the row: in a row beside taller things it would stretch to their
    // height, a pill grown into a circle.
    let height = measured.y + 6.0;
    ui.allocate_ui_with_layout(egui::vec2(width, height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        Frame::new()
            .fill(color.gamma_multiply(0.14))
            .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
            .corner_radius(99.0)
            .inner_margin(Margin::symmetric(9, 2))
            .show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).size(size).color(color)).extend());
            })
            .response
    })
    .inner
}

/// In a row that wraps, starts the next line unless buttons with these texts all fit on this one:
/// a choice of a few moves to the next line whole, rather than one of them left behind.
pub fn together(ui: &mut Ui, texts: &[&str]) {
    let font = TextStyle::Button.resolve(ui.style());
    let (padding, gap) = (ui.spacing().button_padding.x, ui.spacing().item_spacing.x);
    let width: f32 = texts
        .iter()
        .map(|text| ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x) + 2.0 * padding + gap)
        .sum();

    if ui.layout().main_wrap && ui.available_size_before_wrap().x < width {
        ui.end_row();
    }
}

/// A small dot in a color, before a status.
pub fn dot(ui: &mut Ui, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);

    response
}

/// A thin line across, between the parts of a card.
pub fn rule(ui: &mut Ui, palette: &Palette) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, palette.line);
}

/// How long a copy says it was done, in seconds.
const COPIED_FOR: f64 = 1.6;

/// A label that copies some text when clicked or tapped, and says so beside it for a moment. The
/// label may show less than it copies: an address cut short copies whole.
pub fn copy_on_click(ui: &mut Ui, palette: &Palette, label: egui::Label, text: &str) -> Response {
    let response = ui.add(label.sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
    let id = response.id.with("copied");
    let now = ui.input(|input| input.time);

    if response.clicked() {
        ui.ctx().copy_text(text.to_string());
        ui.data_mut(|data| data.insert_temp(id, now));
    }

    if ui.data(|data| data.get_temp::<f64>(id)).is_some_and(|at| now - at < COPIED_FOR) {
        ui.label(RichText::new("Copied").small().color(palette.good));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }

    response
}

/// A text field with a button on its right: the button takes the room it needs, and the field the
/// rest, however narrow the screen. Returns whether the button was pressed.
pub fn beside(ui: &mut Ui, button: impl FnOnce(&mut Ui) -> bool, field: impl FnOnce(&mut Ui, f32)) -> bool {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let pressed = button(ui);
            field(ui, ui.available_width());

            pressed
        })
        .inner
    })
    .inner
}

/// The most of a row its end takes, leaving the rest for what the row starts with, however long.
const END_AT_MOST: f32 = 0.6;

/// A row with something kept at its right end, such as a button: `end` is laid out first, at the
/// width it needs, and `start` has what is left. However long what `start` shows, `end` stays where
/// it is, at the row's end and on its first line: a line in `start` that does not fit is cut short,
/// or wraps within its own column if it asks to, and never pushes or runs into `end`. What `start`
/// adds directly is centered on `end`; several lines, in a `ui.vertical`, start at its top.
pub fn end_row<S, E>(ui: &mut Ui, start: impl FnOnce(&mut Ui) -> S, end: impl FnOnce(&mut Ui) -> E) -> egui::InnerResponse<(S, E)> {
    ui.horizontal_top(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let most = ui.available_width() * END_AT_MOST;
            let end = ui
                .scope(|ui| {
                    ui.set_max_width(most);
                    end(ui)
                })
                .inner;
            let size = egui::vec2(ui.available_width().max(1.0), ui.min_rect().height());
            let start = ui
                .allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_size(size);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    start(ui)
                })
                .inner;

            (start, end)
        })
        .inner
    })
}

/// Text in which each word starting with `--`, a command's flag, goes in the mono face: in the text
/// face two dashes read as one long one. Plain text otherwise, in the text style and color given.
pub fn flagged(ui: &Ui, text: &str, style: TextStyle, color: Color32) -> egui::text::LayoutJob {
    let (plain, mono) = (style.resolve(ui.style()), TextStyle::Monospace.resolve(ui.style()));
    let mono = FontId::new(mono.size.min(plain.size), mono.family);
    let mut job = egui::text::LayoutJob::default();

    for (part, is_flag) in flag_parts(text) {
        job.append(part, 0.0, egui::TextFormat {
            font_id: if is_flag { mono.clone() } else { plain.clone() },
            color,
            ..Default::default()
        });
    }

    job
}

/// A text cut into runs of plain words and of flags, in order: each flag is a word that starts with
/// `--` and goes on with a letter, up to where the word ends, a stop or a comma left out of it.
fn flag_parts(text: &str) -> Vec<(&str, bool)> {
    let mut parts = Vec::new();
    let mut plain = 0;
    let mut at = 0;

    while let Some(found) = text[at..].find("--") {
        let start = at + found;
        let word_start = start == 0 || text[..start].ends_with(char::is_whitespace);
        let end = start + 2 + text[start + 2..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).unwrap_or(text.len() - start - 2);

        if word_start && text[start + 2..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            if plain < start {
                parts.push((&text[plain..start], false));
            }

            parts.push((&text[start..end], true));
            plain = end;
        }

        at = end;
    }

    if plain < text.len() {
        parts.push((&text[plain..], false));
    }

    parts
}

/// One page in a list of pages, as a phone's More lists them: a card, the whole of it what a finger
/// taps, with what the page is called, what it is for under that, and an arrow.
pub fn page_link(ui: &mut Ui, palette: &Palette, name: &str, about: &str) -> Response {
    /// The room kept on the card's right for its arrow.
    const ARROW: f32 = 24.0;

    let drawn = ui
        .scope(|ui| {
            card(ui, palette, |ui| {
                ui.horizontal(|ui| {
                    // What the page is for wraps short of the arrow, rather than under it.
                    let width = ui.available_width() - ARROW;
                    ui.vertical(|ui| {
                        ui.set_max_width(width);
                        ui.label(strong(name, 15.0).color(palette.ink));
                        ui.add(egui::Label::new(RichText::new(about).color(palette.muted)).wrap());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("›").size(20.0).color(palette.muted));
                    });
                });
            });
        })
        .response;

    ui.interact(drawn.rect, ui.scope_id().with(("page link", name)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Where a proxy is set, in an app's settings: a field with Use, which takes what is typed, then
/// Orbot, which takes Orbot's port on the device, I2P, i2pd's, and Off. Gives the setting to keep, empty to go
/// direct, once one is chosen that reads as a proxy; why one does not waits in trouble. set is the
/// setting in use. In the browser, which makes every request itself, it says where to set one.
pub fn proxy(ui: &mut Ui, palette: &Palette, typed: &mut String, trouble: &mut Option<String>, set: &str) -> Option<String> {
    let note = |ui: &mut Ui, text: String| ui.add(egui::Label::new(RichText::new(text).small().color(palette.muted)).wrap());

    if cfg!(target_arch = "wasm32") {
        note(ui, "In the browser, the browser makes every request, through the proxy set in it: to use Tor, open this page in Tor Browser.".to_string());

        return None;
    }

    let shown = crate::net::Proxy::parse(set).map(|proxy| proxy.shown().to_string());
    note(ui, match shown {
        Ok(shown) => format!("Every request goes through the proxy at {shown}, and only through it: while it is down, nothing is sent."),
        Err(_) => "None: requests go straight to the node. Set one to send them all through it, such as Orbot's or Tor Browser's to reach the node over Tor, as an .onion node needs, or I2P's for a .i2p node.".to_string(),
    });
    ui.add_space(4.0);

    let mut chosen = None;
    let other = typed.trim() != set;
    let used = beside(
        ui,
        |ui| primary(ui, palette, "Use", other).clicked(),
        |ui, width| {
            ui.add(egui::TextEdit::singleline(typed).hint_text(crate::net::Proxy::ORBOT).font(TextStyle::Monospace).desired_width(width));
        },
    );

    if used {
        chosen = Some(typed.clone());
    }

    ui.horizontal(|ui| {
        if quiet(ui, palette, "Orbot").on_hover_text("Orbot's SOCKS port on this device, which is Tor's own too").clicked() {
            chosen = Some(crate::net::Proxy::ORBOT.to_string());
        }

        if quiet(ui, palette, "Tor Browser").on_hover_text("Tor Browser's Tor, on this machine while it runs").clicked() {
            chosen = Some(crate::net::Proxy::TOR_BROWSER.to_string());
        }

        if quiet(ui, palette, "I2P")
            .on_hover_text("i2pd's SOCKS port on this device, reaching .i2p nodes; for the Java I2P router, type its HTTP proxy, http://127.0.0.1:4444")
            .clicked()
        {
            chosen = Some(crate::net::Proxy::I2P.to_string());
        }

        if !set.is_empty() && quiet(ui, palette, "Off").on_hover_text("Go straight to the node").clicked() {
            chosen = Some(String::new());
        }
    });

    if let Some(trouble) = trouble {
        ui.colored_label(palette.bad, trouble.as_str());
    }

    match crate::net::setting(&chosen?) {
        Ok(setting) => {
            *typed = setting.clone();
            *trouble = None;

            Some(setting)
        }
        Err(why) => {
            *trouble = Some(why);

            None
        }
    }
}

fn button_size(ui: &Ui) -> f32 {
    ui.style().text_styles.get(&TextStyle::Button).map_or(14.0, |font| font.size)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PALETTE: Palette = Palette {
        paper: Color32::BLACK,
        nav: Color32::BLACK,
        card: Color32::from_gray(20),
        row: Color32::from_gray(30),
        line: Color32::from_gray(40),
        ink: Color32::WHITE,
        soft: Color32::LIGHT_GRAY,
        muted: Color32::GRAY,
        heading: Color32::GRAY,
        accent: Color32::LIGHT_BLUE,
        on_accent: Color32::BLACK,
        good: Color32::GREEN,
        warn: Color32::YELLOW,
        bad: Color32::RED,
        dark: true,
    };

    /// Draws once to settle fonts, then again, and hands back what the last frame measured.
    fn measured<T>(width: f32, mut draw: impl FnMut(&mut Ui) -> T) -> T {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 600.0));
        let mut last = None;

        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| last = Some(draw(ui)));
            output.textures_delta.clear();
        }

        last.expect("drawn")
    }

    #[test]
    fn an_end_row_keeps_its_button_put_however_long_the_text_beside_it() {
        let long = "registered · expires 2027-10-30 18:52:03 UTC, and a good deal more than a phone has room for on one line";
        let row = |ui: &mut Ui, text: &str, wrap: bool| {
            end_row(
                ui,
                |ui| {
                    let label = egui::Label::new(text);

                    ui.add(if wrap { label.wrap() } else { label }).rect
                },
                |ui| ui.button("Manage").rect,
            )
            .inner
        };
        let ((short_text, short_button), (cut_text, cut_button), (wrapped_text, wrapped_button)) = measured(300.0, |ui| {
            (row(ui, "gm", false), row(ui, long, false), row(ui, long, true))
        });

        assert!((short_button.right() - cut_button.right()).abs() < 0.5 && (short_button.width() - cut_button.width()).abs() < 0.5, "not pushed: {short_button:?} {cut_button:?}");
        assert!((short_button.right() - wrapped_button.right()).abs() < 0.5, "not by wrapping either: {wrapped_button:?}");
        assert!(short_button.right() > 290.0, "at the row's end: {short_button:?}");
        assert!(cut_text.right() <= cut_button.left(), "a long line is cut short of it: {cut_text:?} {cut_button:?}");
        assert!(cut_text.height() < 24.0, "on one line: {cut_text:?}");
        assert!(wrapped_text.right() <= wrapped_button.left(), "wrapped short of it: {wrapped_text:?} {wrapped_button:?}");
        assert!(wrapped_text.height() > 2.0 * short_text.height(), "on several lines: {wrapped_text:?}");
        assert!((wrapped_text.top() - wrapped_button.top()).abs() < 1.0, "the button keeps to the first line: {wrapped_text:?} {wrapped_button:?}");
        assert!((short_text.center().y - short_button.center().y).abs() < 1.0, "a short line centers on it: {short_text:?} {short_button:?}");
    }

    #[test]
    fn a_tall_end_row_makes_room_below_it() {
        let (first, next) = measured(300.0, |ui| {
            let first = end_row(
                ui,
                |ui| {
                    ui.vertical(|ui| {
                        for line in 0..6 {
                            ui.label(format!("line {line}"));
                        }
                    });
                },
                |ui| {
                    let _ = ui.button("Close");
                },
            )
            .response
            .rect;

            (first, ui.label("next").rect)
        });

        assert!(first.height() > 60.0, "as tall as its lines: {first:?}");
        assert!(next.top() >= first.bottom(), "what follows starts under them: {next:?} {first:?}");
    }

    #[test]
    fn segmented_marks_the_one_on_and_takes_a_tap_on_another() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 200.0));
        let mut oldest = false;
        let mut changed = false;
        let frame = |events: Vec<egui::Event>, oldest: &mut bool, changed: &mut bool| {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ui| {
                *changed |= segmented(ui, &PALETTE, oldest, &[(false, "Newest"), (true, "Oldest")]);
            });
            output.textures_delta.clear();
        };
        frame(Vec::new(), &mut oldest, &mut changed);
        frame(Vec::new(), &mut oldest, &mut changed);
        // Oldest, the second: past Newest, whose button is some eighty points wide.
        let at = egui::pos2(100.0, 15.0);
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(vec![egui::Event::PointerMoved(at), press(true)], &mut oldest, &mut changed);
        frame(vec![press(false)], &mut oldest, &mut changed);

        assert!(oldest, "the tap took");
        assert!(changed);

        changed = false;
        frame(vec![press(true)], &mut oldest, &mut changed);
        frame(vec![press(false)], &mut oldest, &mut changed);

        assert!(oldest && !changed, "a tap on the one on changes nothing");
    }

    #[test]
    fn flags_go_in_mono_and_the_rest_stays_as_it_is() {
        assert_eq!(flag_parts("no flags here"), [("no flags here", false)]);
        assert_eq!(flag_parts("Add --solo to mine alone."), [("Add ", false), ("--solo", true), (" to mine alone.", false)]);
        assert_eq!(flag_parts("with --mining."), [("with ", false), ("--mining", true), (".", false)]);
        assert_eq!(flag_parts("--token FILE"), [("--token", true), (" FILE", false)]);
        assert_eq!(flag_parts("start --index-addresses, --mining"), [("start ", false), ("--index-addresses", true), (", ", false), ("--mining", true)]);
        assert_eq!(flag_parts("a -- dash, 1--2 and -- -- "), [("a -- dash, 1--2 and -- -- ", false)], "no flags");
        assert_eq!(flag_parts(""), Vec::<(&str, bool)>::new());
    }

    #[test]
    fn a_pill_at_the_end_of_a_full_row_moves_to_the_next_line_whole() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(220.0, 400.0));
        let mut pills = Vec::new();

        // A frame to settle fonts, then the one measured.
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Account 1 · Ed25519 ed25519:0x06b6…318c");
                    pills.push(pill(ui, "may vote", Color32::GREEN).rect);
                });
            });
            output.textures_delta.clear();
        }

        let rect = pills.last().copied().unwrap();
        let line = ctx.fonts_mut(|fonts| fonts.row_height(&egui::FontId::proportional(11.5)));

        assert!(rect.height() < 3.0 * line, "one line, not a letter a line: {rect:?}");
        assert!(rect.width() > rect.height(), "wider than tall: {rect:?}");
        assert!(rect.right() <= screen.right() + 1.0, "within the row: {rect:?}");
    }

    #[test]
    fn a_pill_beside_taller_things_keeps_its_own_height() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(380.0, 400.0));
        let (mut pill_rect, mut row) = (egui::Rect::NOTHING, egui::Rect::NOTHING);

        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                row = ui
                    .horizontal_wrapped(|ui| {
                        ui.vertical(|ui| {
                            ui.label("DEV");
                            ui.label("the coin");
                            ui.label("nonce 0");
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            pill_rect = pill(ui, "Proven", Color32::GOLD).rect;
                            ui.label("0");
                        });
                    })
                    .response
                    .rect;
            });
            output.textures_delta.clear();
        }

        let line = ctx.fonts_mut(|fonts| fonts.row_height(&egui::FontId::proportional(11.5)));

        assert!(row.height() > 3.0 * line, "the row is tall: {row:?}");
        assert!(pill_rect.height() < 2.0 * line, "the pill is not: {pill_rect:?}");
        assert!(pill_rect.width() > pill_rect.height(), "wider than tall: {pill_rect:?}");
    }

    #[test]
    fn a_choice_that_does_not_fit_after_its_label_moves_to_the_next_line_whole() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(260.0, 400.0));
        let (mut label, mut first, mut second) = (egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING);

        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                ui.horizontal_wrapped(|ui| {
                    label = ui.label("Follow its test network:").rect;
                    together(ui, &["testnet4", "signet"]);
                    first = ui.selectable_label(true, "testnet4").rect;
                    second = ui.selectable_label(false, "signet").rect;
                });
            });
            output.textures_delta.clear();
        }

        assert!(first.top() > label.bottom() - 1.0, "below the label: {first:?} {label:?}");
        assert!((first.top() - second.top()).abs() < 1.0, "side by side: {first:?} {second:?}");
    }
}
