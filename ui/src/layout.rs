//! What a page needs to know about where it runs: how wide the screen is, and what the platform
//! can do.

/// Below this width, in points, as on a phone, an app trades its sidebar for a bar on top and tabs
/// along the bottom, and its tables for stacked rows.
pub const NARROW: f32 = 640.0;

/// Whether a width is a phone's.
pub fn is_narrow(width: f32) -> bool {
    width < NARROW
}

/// Whether the app can open a file picker: Android has none an egui app can reach yet.
pub const CAN_PICK_FILES: bool = cfg!(not(target_os = "android"));

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
}
