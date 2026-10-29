//! What a page needs to know about where it runs: how wide the screen is, and whether it is touched.

/// Below this width, in points, as on a phone, an app trades its sidebar for a bar on top and tabs
/// along the bottom, and its tables for stacked rows.
pub const NARROW: f32 = 640.0;

/// Whether a width is a phone's.
pub fn is_narrow(width: f32) -> bool {
    width < NARROW
}

/// Suits egui to a touch screen once it finds one: a drag that starts on text scrolls the page,
/// rather than selecting the text, as egui's labels are selectable by default and selecting would
/// take the drag. A mouse keeps selecting. Call it every frame: it changes the style only when it
/// must, and again after a theme is applied anew.
pub fn suit_touch(ctx: &egui::Context) {
    // Android is all touch, from the first drag on; elsewhere egui knows once a finger lands.
    let touch = cfg!(target_os = "android") || ctx.input(|input| input.has_touch_screen());

    if ctx.global_style().interaction.selectable_labels == touch {
        ctx.all_styles_mut(|style| style.interaction.selectable_labels = !touch);
    }
}

/// How many of something fit across a width, each at least this wide with a gap between, and at
/// least one.
pub fn fit(width: f32, each: f32, gap: f32) -> usize {
    (((width + gap) / (each + gap)).floor() as usize).max(1)
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

/// A sidebar on a wide screen: `foot` keeps its bottom, and `pages` fill what is left above it,
/// scrolling when the window is too short to show them all. Both are handed `state`, as each
/// usually needs the app.
pub fn sidebar<S, R>(ui: &mut egui::Ui, state: &mut S, foot: impl FnOnce(&mut S, &mut egui::Ui), pages: impl FnOnce(&mut S, &mut egui::Ui) -> R) -> R {
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
