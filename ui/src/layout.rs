//! What a page needs to know about where it runs: how wide the screen is, and whether it is touched.

/// Below this width, in points, as on a phone, an app trades its sidebar for a bar on top and tabs
/// along the bottom, and its tables for stacked rows.
pub const NARROW: f32 = 640.0;

/// Whether a width is a phone's.
pub fn is_narrow(width: f32) -> bool {
    width < NARROW
}

/// How tall a button or a field is at least on a phone, or any touch screen: what a finger taps
/// without missing.
pub const FINGER: f32 = 40.0;

/// The gap between a page's sections, such as its cards, in points: one number for every page.
pub const SECTION: f32 = 14.0;

/// Keeps the section gap below what was just drawn, whatever item spacing the look has: put after a
/// section that is no card, as a card that keeps the gap itself needs none.
pub fn section_gap(ui: &mut egui::Ui) {
    ui.add_space((SECTION - ui.spacing().item_spacing.y).max(0.0));
}

/// Pads the buttons and fields in this ui, and in all it holds, to a finger's height, as a phone
/// needs. Their text keeps its size, and a line of plain text its own height: only what is tapped
/// grows.
pub fn finger_sized(ui: &mut egui::Ui) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let text = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let padding = ((FINGER - text) / 2.0).ceil();
    let spacing = ui.spacing_mut();
    spacing.button_padding.y = spacing.button_padding.y.max(padding);
}

/// A bar's row a finger tall from the start, as its buttons are, so what sits beside them, such as
/// a name, centers on them rather than on egui's shorter row.
pub fn finger_row(ui: &mut egui::Ui) {
    finger_sized(ui);
    ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(FINGER);
}

/// Whether what is tapped wants a finger's size here: on a phone's width, or on any touch screen.
/// Pages get it from their app; a dialog, drawn apart from the page, asks for itself.
pub fn fingers(ctx: &egui::Context) -> bool {
    is_narrow(ctx.content_rect().width()) || touch(ctx)
}

/// Suits egui to a touch screen once it finds one: a drag that starts on text scrolls the page,
/// rather than selecting the text, as egui's labels are selectable by default and selecting would
/// take the drag. A mouse keeps selecting. Call it every frame: it changes the style only when it
/// must, and again after a theme is applied anew.
pub fn suit_touch(ctx: &egui::Context) {
    let touch = touch(ctx);

    if ctx.global_style().interaction.selectable_labels == touch {
        ctx.all_styles_mut(|style| style.interaction.selectable_labels = !touch);
    }
}

/// Whether the screen is touched: Android is all touch, from the first drag on; elsewhere egui
/// knows once a finger lands. A phone on its side is as wide as a small window, and as touched as
/// the phone upright.
pub fn touch(ctx: &egui::Context) -> bool {
    cfg!(target_os = "android") || ctx.input(|input| input.has_touch_screen())
}

/// Where the page's scroll area keeps what it was last shown in, for what is drawn over a page.
#[cfg(any(target_os = "android", target_os = "ios", test))]
pub(crate) fn page_clip(ctx: &egui::Context) -> Option<egui::Rect> {
    ctx.data(|data| data.get_temp(egui::Id::unique(PAGE_CLIP)))
}

const PAGE_CLIP: &str = "chainv4-ui page clip";

/// Keeps the text field being typed in on screen: once the keyboard comes up and the page shrinks
/// above it, or the focus moves to another field, the page scrolls as little as brings the field
/// back into sight. Call it in the page's scroll area, before what the page shows.
pub fn keep_focus_in_view(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let focused = ctx.memory(|memory| memory.focused());
    let height = ui.clip_rect().height();
    let id = ui.scope_id().with("chainv4-ui focus in view");
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique(PAGE_CLIP), ui.clip_rect()));
    let (was, was_height, waiting) = ctx.data(|data| data.get_temp::<(Option<egui::Id>, f32, bool)>(id)).unwrap_or((None, height, false));
    let changed = was != focused || height < was_height - 1.0;
    // A change is acted on once it has held for a frame: the keyboard has finished coming up, and
    // a focus left from a field no longer shown, as the lock screen's once the wallet opens, is
    // let go of by then rather than scrolled to.
    let act = waiting && !changed;
    ctx.data_mut(|data| data.insert_temp(id, (focused, height, changed)));

    if let Some(field) = focused.filter(|_| act).and_then(|focused| ctx.read_response(focused)) {
        ui.scroll_to_rect(field.rect, None);
    }
}

/// How many of something fit across a width, each at least this wide with a gap between, and at
/// least one.
pub fn fit(width: f32, each: f32, gap: f32) -> usize {
    (((width + gap) / (each + gap)).floor() as usize).max(1)
}

/// How many tiles go across a width, each at least this wide with a gap between, and how wide each
/// then is to fill it: a grid's columns end with the row, with no slack on the right.
pub fn tiles(width: f32, each: f32, gap: f32) -> (usize, f32) {
    let columns = fit(width, each, gap);

    (columns, ((width - gap * (columns - 1) as f32) / columns as f32).max(1.0))
}

/// How long a page is given to grow tall enough to scroll back to where it was: its content may
/// still be arriving from the node.
const RESTORE_FOR: f64 = 1.5;

/// Where a page was scrolled to, given back when someone returns to it: kept with the page as it
/// is left, then set on the page's scroll area for a moment while what it shows arrives, until it
/// is that far down or the person moves it themselves. A new page starts at the top.
#[derive(Debug, Default)]
pub struct Scrolled {
    /// The offset to reach, and since when, from the frame it is first set on.
    wanted: Option<(f32, Option<f64>)>,
    /// Where the page is now, as it was last shown.
    now: f32,
}

impl Scrolled {
    /// Where the page shown is scrolled to now: what to keep with it as it is left.
    pub fn offset(&self) -> f32 {
        self.now
    }

    /// The page shown next is one returned to, scrolled to where it was.
    pub fn back_to(&mut self, offset: f32) {
        self.wanted = Some((offset, None));
    }

    /// The page shown next is a new one, from the top.
    pub fn top(&mut self) {
        self.wanted = Some((0.0, None));
    }

    /// Shows the page in this scroll area, where it should be, and notes where it is after.
    pub fn show<R>(&mut self, ui: &mut egui::Ui, scroll: egui::ScrollArea, page: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let scroll = match self.wanted {
            Some((offset, _)) => scroll.vertical_scroll_offset(offset),
            None => scroll,
        };
        let shown = scroll.show(ui, page);
        let time = ui.input(|input| input.time);
        // A finger or a wheel on the page is the person's say: it stops there.
        let moved = ui.input(|input| input.pointer.any_down() || input.smooth_scroll_delta.y != 0.0);
        self.now = shown.state.clamped_offset().y;

        if let Some((offset, since)) = &mut self.wanted {
            let since = *since.get_or_insert(time);

            if self.now + 1.0 >= *offset || time - since > RESTORE_FOR || moved {
                self.wanted = None;
            }
        }

        if self.wanted.is_some() {
            ui.ctx().request_repaint();
        }

        shown.inner
    }
}

/// Below this height, as a phone on its side with its keyboard up, a sidebar has no room to keep
/// its foot apart from its pages: the whole of it scrolls instead, as if this tall.
const SIDEBAR_AT_LEAST: f32 = 280.0;

/// A sidebar on a wide screen: `foot` keeps its bottom, and `pages` fill what is left above it,
/// scrolling when the window is too short to show them all. Both are handed `state`, as each
/// usually needs the app. In a window too short even for that, the sidebar scrolls whole, the
/// foot under the pages, never drawn over them.
pub fn sidebar<S, R>(ui: &mut egui::Ui, state: &mut S, foot: impl FnOnce(&mut S, &mut egui::Ui), pages: impl FnOnce(&mut S, &mut egui::Ui) -> R) -> R {
    if ui.available_height() >= SIDEBAR_AT_LEAST {
        return footed(ui, state, foot, pages);
    }

    egui::ScrollArea::vertical()
        .id_salt("sidebar whole")
        .auto_shrink(false)
        .show(ui, |ui| {
            let size = egui::vec2(ui.available_width(), SIDEBAR_AT_LEAST);
            ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_height(SIDEBAR_AT_LEAST);
                footed(ui, state, foot, pages)
            })
            .inner
        })
        .inner
}

/// The foot at the bottom of the room there is, the pages scrolling above it.
fn footed<S, R>(ui: &mut egui::Ui, state: &mut S, foot: impl FnOnce(&mut S, &mut egui::Ui), pages: impl FnOnce(&mut S, &mut egui::Ui) -> R) -> R {
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        foot(state, ui);
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            egui::ScrollArea::vertical().id_salt("sidebar").auto_shrink(false).show(ui, |ui| pages(state, ui)).inner
        })
        .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phones_are_narrow_and_tiles_fit_what_room_there_is() {
        assert!(is_narrow(411.0) && !is_narrow(800.0));
        assert_eq!(fit(1000.0, 190.0, 8.0), 5);
        assert_eq!(fit(380.0, 160.0, 8.0), 2);
        assert_eq!(fit(100.0, 160.0, 8.0), 1, "always one");
    }

    #[test]
    fn tiles_fill_the_row_they_are_in() {
        let (columns, each) = tiles(349.0, 148.0, 10.0);

        assert_eq!(columns, 2);
        assert!((each * 2.0 + 10.0 - 349.0).abs() < 0.01, "no slack on the right: {each}");
        assert_eq!(tiles(100.0, 148.0, 10.0), (1, 100.0), "one, as wide as there is");
    }

    #[test]
    fn the_section_gap_is_the_same_whatever_the_spacing() {
        for spacing in [5.0, 8.0] {
            let ctx = egui::Context::default();
            let (mut above, mut below) = (egui::Rect::NOTHING, egui::Rect::NOTHING);
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.spacing_mut().item_spacing.y = spacing;
                above = ui.label("above").rect;
                section_gap(ui);
                below = ui.label("below").rect;
            });
            output.textures_delta.clear();

            assert!((below.top() - above.bottom() - SECTION).abs() < 0.5, "{above:?} {below:?}");
        }
    }

    #[test]
    fn a_sidebar_shorter_than_its_foot_scrolls_whole_and_draws_nothing_over_its_pages() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 150.0));
        let (mut foot, mut first, mut clip) = (egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING);

        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                sidebar(
                    ui,
                    &mut (),
                    |_, ui| {
                        for line in 0..6 {
                            foot = foot.union(ui.label(format!("foot {line}")).rect);
                        }
                    },
                    |_, ui| {
                        clip = ui.clip_rect();
                        first = ui.label("page 0").rect;

                        for row in 1..10 {
                            ui.label(format!("page {row}"));
                        }
                    },
                );
            });
            output.textures_delta.clear();
            foot = foot.intersect(screen);
        }

        assert!(first.top() >= screen.top() - 1.0, "the pages start at the top: {first:?}");
        assert!(clip.bottom() <= foot.top() + 1.0 || foot.height() <= 0.0, "the pages stop above the foot: {clip:?} {foot:?}");
    }

    #[test]
    fn on_a_phone_buttons_and_fields_are_a_finger_tall_and_text_keeps_its_height() {
        let ctx = egui::Context::default();
        let (mut button, mut field, mut text, mut plain) = (egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING);

        // A frame to settle fonts, then the one measured.
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                plain = ui.button("before").rect;
                finger_sized(ui);
                button = ui.button("Send").rect;
                field = ui.text_edit_singleline(&mut String::new()).rect;
                text = ui.label("a line").rect;
            });
            output.textures_delta.clear();
        }

        assert!(plain.height() < FINGER - 4.0, "egui's own is shorter: {plain:?}");
        assert!(button.height() >= FINGER - 1.0, "a button a finger tall: {button:?}");
        assert!(field.height() >= FINGER - 1.0, "a field too: {field:?}");
        assert!(text.height() < 24.0, "a line of text keeps its height: {text:?}");
    }

    #[test]
    fn a_field_the_keyboard_comes_up_over_scrolls_back_into_sight() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let frame = |height: f32, text: &mut String| {
            let mut field = egui::Rect::NOTHING;
            let mut clip = egui::Rect::NOTHING;
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, height));
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                egui::ScrollArea::vertical().id_salt("page").show(ui, |ui| {
                    keep_focus_in_view(ui);
                    clip = ui.clip_rect();

                    for row in 0..20 {
                        ui.label(format!("row {row}"));
                    }

                    let response = ui.text_edit_singleline(text);
                    response.request_focus();
                    field = response.rect;
                });
            });
            output.textures_delta.clear();
            (field, clip)
        };

        // Tall enough at first; then the keyboard takes most of the screen.
        for _ in 0..3 {
            frame(800.0, &mut text);
        }

        let (field, clip) = frame(800.0, &mut text);
        assert!(clip.contains_rect(field), "in sight before the keyboard: {field:?} {clip:?}");

        // The page scrolls there smoothly, over a moment's frames.
        for _ in 0..90 {
            frame(200.0, &mut text);
        }

        let (field, clip) = frame(200.0, &mut text);
        assert!(clip.expand(1.0).contains_rect(field), "in sight above the keyboard: {field:?} {clip:?}");
    }

    #[test]
    fn on_a_touch_screen_a_drag_on_text_scrolls() {
        let ctx = egui::Context::default();
        let frame = |events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| suit_touch(ui.ctx()));
            output.textures_delta.clear();
        };
        frame(Vec::new());

        assert!(ctx.global_style().interaction.selectable_labels, "a mouse selects");

        frame(vec![egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: egui::TouchId(1),
            phase: egui::TouchPhase::Start,
            pos: egui::pos2(10.0, 10.0),
            force: None,
        }]);
        frame(Vec::new());

        assert!(!ctx.global_style().interaction.selectable_labels, "a finger scrolls");
    }

    #[test]
    fn a_page_returned_to_scrolls_back_once_it_is_tall_enough_and_a_new_one_starts_at_the_top() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 300.0));
        let mut scrolled = Scrolled::default();
        let frame = |scrolled: &mut Scrolled, rows: usize, time: f64| {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), time: Some(time), ..Default::default() }, |ui| {
                scrolled.show(ui, egui::ScrollArea::vertical().id_salt("page"), |ui| {
                    for row in 0..rows {
                        ui.label(format!("row {row}"));
                    }
                });
            });
            output.textures_delta.clear();
        };

        scrolled.back_to(400.0);
        frame(&mut scrolled, 5, 0.0);

        assert!(scrolled.offset() < 400.0, "not that tall yet");

        // What it shows arrives: it gets there.
        for step in 1..5 {
            frame(&mut scrolled, 200, f64::from(step) * 0.1);
        }

        assert!((scrolled.offset() - 400.0).abs() < 1.0, "back where it was: {}", scrolled.offset());

        scrolled.top();
        frame(&mut scrolled, 200, 1.0);

        assert_eq!(scrolled.offset(), 0.0, "a new page starts at the top");

        // One that never grows tall enough is let go after a moment, wherever it got to.
        scrolled.back_to(10_000.0);
        frame(&mut scrolled, 200, 2.0);
        frame(&mut scrolled, 200, 2.0 + RESTORE_FOR + 0.1);
        let settled = scrolled.offset();
        frame(&mut scrolled, 5, 4.0);

        assert!(scrolled.offset() <= settled, "no longer held");
    }

    #[test]
    fn a_sidebar_too_tall_for_its_window_scrolls_its_pages_above_a_foot_kept_at_the_bottom() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 300.0));
        let frame = |rows: usize| {
            let (mut foot, mut last, mut clip) = (egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING);
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                sidebar(
                    ui,
                    &mut (),
                    |_, ui| foot = ui.label("node").rect,
                    |_, ui| {
                        clip = ui.clip_rect();

                        for row in 0..rows {
                            last = ui.label(format!("page {row}")).rect;
                        }
                    },
                );
            });
            output.textures_delta.clear();
            (foot, last, clip)
        };

        let (foot, last, clip) = frame(60);

        assert!(foot.bottom() <= screen.bottom() && foot.bottom() > screen.bottom() - 40.0, "the foot at the bottom: {foot:?}");
        assert!(clip.bottom() <= foot.top(), "the pages stop above the foot");
        assert!(last.bottom() > clip.bottom(), "the last pages are scrolled to, not drawn over the foot");

        let (short_foot, short_last, _) = frame(3);

        assert_eq!(short_foot, foot, "a few pages leave the foot where it was");
        assert!(short_last.bottom() < foot.top());
    }
}
