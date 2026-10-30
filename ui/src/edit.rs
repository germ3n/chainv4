//! Editing text with no keyboard shortcuts, as on Android, where egui reaches no clipboard. Holding
//! a finger on a text field, or tapping one that already has focus, brings a bar beside it with
//! Paste, Select all, and Copy and Cut over a selection. It comes only when asked for, never on the
//! tap that gives a field focus, and goes as soon as typing starts. It sends the field the events a
//! keyboard would, so the field does the rest: it inserts what is pasted, and hands what is copied to
//! the app's copy output, which the Android glue puts on the clipboard. A keyboard's own Paste button
//! sends Ctrl+V, which [`ctrl_key`] puts right.

use egui::{Context, Event, Id, Key, Modifiers, Order, Pos2, RawInput, Rect, RichText, Vec2, text::CCursorRange, text_edit::TextEditState, vec2};

/// How long a finger stays on a field before the bar comes, in seconds: as long as Android's own
/// hold, and shorter than egui's, which is 0.8.
const HOLD: f64 = 0.5;
/// How far a finger may stray from where it landed and still be holding, in points: egui's own hold
/// ends after 6, less than a finger rolls.
const HOLD_SLOP: f32 = 20.0;
/// How far the bar keeps from the field and from the edges of the room it has, in points.
const BAR_GAP: f32 = 8.0;
/// How big the bar is taken to be before it has been drawn once.
const BAR_GUESS: Vec2 = vec2(160.0, 48.0);

/// Where a bar of a size goes in the room it has, its left and top: just above the field, else just
/// below it, else whichever end of the room leaves the field more in sight; with no field in sight,
/// at the room's bottom, which is what is just above the keyboard. Always all of it inside the room,
/// centered on `x` as far as that lets it, and over the field: a screen turned leaves `x` elsewhere.
pub(crate) fn place(size: Vec2, field: Option<Rect>, x: f32, room: Rect) -> Pos2 {
    let (left, right) = (room.left() + BAR_GAP, room.right() - BAR_GAP - size.x);
    let (top, bottom) = (room.top() + BAR_GAP, room.bottom() - BAR_GAP - size.y);
    let field = field.filter(|field| field.intersects(room));
    let x = field.map_or(x, |field| x.max(field.left()).min(field.right()));
    let y = match field {
        None => bottom,
        Some(field) if field.top() - BAR_GAP - size.y >= top => field.top() - BAR_GAP - size.y,
        Some(field) if field.bottom() + BAR_GAP <= bottom => field.bottom() + BAR_GAP,
        Some(field) if field.center().y > room.center().y => top,
        Some(_) => bottom,
    };

    // A room too small for it keeps its start in sight.
    Pos2::new((x - size.x / 2.0).min(right).max(left), y.min(bottom).max(top))
}

/// The system's clipboard, as editing needs it.
pub(crate) trait Clipboard {
    /// Whether it holds text: asked while a field has focus, so cheap, and without reading it.
    fn has_text(&mut self) -> bool;

    /// What it holds, if text.
    fn text(&mut self) -> Option<String>;
}

/// What the bar offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Paste,
    SelectAll,
    Copy,
    Cut,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Action::Paste => "Paste",
            Action::SelectAll => "Select all",
            Action::Copy => "Copy",
            Action::Cut => "Cut",
        }
    }

    /// The event a keyboard would send for it, if there is anything to send.
    fn event(self, clipboard: &mut dyn Clipboard) -> Option<Event> {
        match self {
            Action::Paste => clipboard.text().filter(|text| !text.is_empty()).map(Event::Paste),
            Action::SelectAll => Some(Event::Key {
                key: Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            }),
            Action::Copy => Some(Event::Copy),
            Action::Cut => Some(Event::Cut),
        }
    }
}

/// What the bar offers: Paste when the clipboard holds text, Select all, and Copy and Cut when
/// some text is selected.
pub(crate) fn actions(has_text: bool, selected: bool) -> Vec<Action> {
    let mut actions = Vec::new();

    if has_text {
        actions.push(Action::Paste);
    }

    actions.push(Action::SelectAll);

    if selected {
        actions.extend([Action::Copy, Action::Cut]);
    }

    actions
}

/// The bar, for an app's frames.
#[derive(Default)]
pub(crate) struct Editing {
    /// The bar a held finger brought up: over which field, and where the finger was.
    bar: Option<(Id, Pos2)>,
    /// The field a finger is on, and since when, while it may yet become a hold.
    held: Option<(Id, f64)>,
    /// Whether the press under way already brought the bar up: lifting it is no tap.
    press_used: bool,
    /// The text field that had focus last frame, and what was selected in it.
    field: Option<Id>,
    kept: Option<(Id, CCursorRange)>,
    /// What the field is to do in the next frame, as a keyboard would have said it.
    queued: Vec<Event>,
    /// Whether the clipboard held text when last asked, and when that was.
    clip: Option<(f64, bool)>,
}

impl Editing {
    /// After the app's frame: brings the bar up or takes it down, draws it, and queues what it is
    /// asked to do.
    pub(crate) fn show(&mut self, ctx: &Context, clipboard: &mut dyn Clipboard) {
        let (time, lifted) = ctx.input(|input| (input.time, input.pointer.any_released()));
        let focused = focused_field(ctx);
        let was_focused = self.field;
        // A tap on the bar takes the field's focus as it lifts, before the bar sees the tap: for that
        // frame it acts for the field that had it, as it showed it.
        let field = focused.or(self.field.filter(|_| lifted));
        self.field = focused;

        // A field that loses focus loses its selection too, so what was selected is kept for the
        // bar to offer Copy and Cut over, and is selected again before they act.
        if let Some(id) = focused
            && let Some(range) = TextEditState::load(ctx, id).and_then(|state| state.cursor.char_range())
        {
            self.kept = Some((id, range));
        }

        // Asked again once a second while the bar is up: something else may have copied.
        let has_text = match (field.filter(|_| self.bar.is_some()), self.clip) {
            (None, _) => {
                self.clip = None;

                false
            }
            (Some(_), Some((asked, has))) if time - asked < 1.0 => has,
            (Some(_), _) => {
                let has = clipboard.has_text();
                self.clip = Some((time, has));

                has
            }
        };

        let room = room(ctx);
        let bar_was_up = self.bar.is_some();

        if let Some((id, at)) = self.bar {
            self.bar(ctx, id, at, has_text, room, clipboard);
        }

        // A finger held on a field brings the bar up beside it, and gives the field focus.
        if let Some(id) = self.held(ctx, time) {
            let at = ctx.input(|input| input.pointer.press_origin()).unwrap_or_else(|| ctx.content_rect().center());
            self.bar = Some((id, at));
            self.press_used = true;
            ctx.memory_mut(|memory| memory.request_focus(id));
            ctx.request_repaint();
        }

        // So does a tap on a field that had focus already: a tap on one without it only gives it
        // focus, and a tap that took the bar down leaves it down.
        if let Some(id) = focused.filter(|id| was_focused == Some(*id))
            && !bar_was_up
            && !self.press_used
            && self.bar.is_none()
            && ctx.read_response(id).is_some_and(|response| response.clicked())
        {
            let at = ctx.input(|input| input.pointer.interact_pos()).unwrap_or_else(|| ctx.content_rect().center());
            self.bar = Some((id, at));
            ctx.request_repaint();
        }

        // So does a drag across a field's text that leaves some selected, where it ended: holding
        // the field again would put the cursor where the finger is, and lose the selection.
        if let Some(id) = focused
            && self.bar.is_none()
            && ctx.read_response(id).is_some_and(|response| response.drag_stopped())
            && selected(ctx, id)
        {
            let at = ctx.input(|input| input.pointer.latest_pos()).unwrap_or_else(|| ctx.content_rect().center());
            self.bar = Some((id, at));
            ctx.request_repaint();
        }

        if !ctx.input(|input| input.pointer.any_down()) {
            self.press_used = false;
        }
    }

    /// Watches a finger on a text field, and says which field once it has been held there long
    /// enough. The finger may roll a little, and the field may be one a drag on would scroll the page
    /// or select its text: egui's own hold is ended by either.
    fn held(&mut self, ctx: &Context, time: f64) -> Option<Id> {
        let (pressed, down, origin, at) = ctx.input(|input| (input.pointer.any_pressed(), input.pointer.primary_down(), input.pointer.press_origin(), input.pointer.latest_pos()));

        if pressed {
            let under = ctx.interaction_snapshot(|interaction| interaction.contains_pointer.clone());
            let field = under.iter().copied().find(|id| TextEditState::load(ctx, *id).is_some());
            self.held = field.map(|field| (field, time));
        }

        let strayed = origin.zip(at).is_some_and(|(origin, at)| origin.distance(at) > HOLD_SLOP);

        if !down || strayed {
            self.held = None;
        }

        let (field, since) = self.held?;

        if time - since < HOLD {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(HOLD - (time - since)));

            return None;
        }

        self.held = None;

        Some(field)
    }

    /// Before the next frame: hands it what was queued.
    pub(crate) fn feed(&mut self, raw_input: &mut RawInput) {
        raw_input.events.append(&mut self.queued);
    }

    fn bar(&mut self, ctx: &Context, id: Id, at: Pos2, has_text: bool, room: Rect, clipboard: &mut dyn Clipboard) {
        let selected = self.kept.is_some_and(|(kept, range)| kept == id && !range.is_empty());
        let area = Id::unique("chainv4-ui edit bar");
        let field = ctx.read_response(id).map(|response| response.rect);
        let spot = place(drawn_size(ctx, area), field, at.x, room);
        let (chosen, drawn) = show_bar(ctx, area, spot, room, &actions(has_text, selected));

        // Select all leaves it up, now offering Copy and Cut; the rest are done with it.
        if let Some(action) = chosen {
            self.act(ctx, id, action, clipboard);

            if action != Action::SelectAll {
                self.bar = None;
            }

            return;
        }

        // It goes with a tap anywhere else, typing, or the focus going to another field. A
        // shortcut is no typing: Select all sends one.
        let (clicked, typed) = ctx.input(|input| {
            let typed = |event: &Event| match event {
                Event::Text(_) | Event::Paste(_) | Event::Ime(_) => true,
                Event::Key { pressed, modifiers, .. } => *pressed && !modifiers.command,
                _ => false,
            };

            (input.pointer.any_click(), input.events.iter().any(typed))
        });
        let moved = ctx.memory(|memory| memory.focused()).is_some_and(|focused| focused != id);

        if (clicked && !self.press_used && !drawn.response.contains_pointer()) || typed || moved {
            self.bar = None;
        }
    }

    fn act(&mut self, ctx: &Context, id: Id, action: Action, clipboard: &mut dyn Clipboard) {
        if let Some((kept, range)) = self.kept
            && kept == id
            && let Some(mut state) = TextEditState::load(ctx, id)
        {
            state.cursor.set_char_range(Some(range));
            state.store(ctx, id);
        }

        if let Some(event) = action.event(clipboard) {
            self.queued.push(event);
        }

        // The tap that asked took the field's focus, as a tap anywhere off a field does: it gets
        // it back for what it was asked to do.
        ctx.memory_mut(|memory| memory.request_focus(id));
        ctx.request_repaint();
    }

    #[cfg(test)]
    fn bar_is_up(&self) -> bool {
        self.bar.is_some()
    }
}

/// A letter pressed with Ctrl held, put right in what egui is about to be told: Android's keyboards
/// send Ctrl+V for their Paste button, which reaches egui as a plain v typed, Ctrl+C as a plain c.
/// The letter is taken out, and what the shortcut means goes in its place: the clipboard's text, a
/// copy, a cut, or the key with Ctrl held, for a field's own shortcuts.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn ctrl_key(events: &mut Vec<Event>, key: Key, paste: &mut dyn FnMut() -> Option<String>) {
    let Some(at) = events.iter().position(|event| matches!(event, Event::Key { key: down, pressed: true, .. } if *down == key)) else {
        return;
    };

    if matches!(events.get(at + 1), Some(Event::Text(_))) {
        events.remove(at + 1);
    }

    let meant = match key {
        Key::V => paste().filter(|text| !text.is_empty()).map(Event::Paste),
        Key::C => Some(Event::Copy),
        Key::X => Some(Event::Cut),
        _ => Some(Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }),
    };

    match meant {
        Some(meant) => events[at] = meant,
        None => {
            events.remove(at);
        }
    }
}

/// A Paste key, as some keyboards have: egui-winit answers it from a clipboard of its own, empty on
/// Android, so egui hears nothing of it.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn paste_key(events: &mut Vec<Event>, paste: &mut dyn FnMut() -> Option<String>) {
    events.extend(paste().filter(|text| !text.is_empty()).map(Event::Paste));
}

/// The room a bar has: the screen as far as can be seen, keyboard and system bars left out, and down
/// only to where the page ends, above any tabs along the bottom.
fn room(ctx: &Context) -> Rect {
    let mut room = ctx.content_rect();

    if let Some(page) = crate::layout::page_clip(ctx).filter(|page| page.bottom() > room.top()) {
        room.max.y = room.max.y.min(page.bottom());
    }

    room
}

/// How big an area was when last drawn.
fn drawn_size(ctx: &Context, area: Id) -> Vec2 {
    ctx.memory(|memory| memory.area_rect(area)).map_or(BAR_GUESS, |rect| rect.size())
}

/// Draws a bar of these actions over everything, at this spot, and says which was tapped. It is
/// in the text's color with the page's on it, so it stands out from any page in either theme.
fn show_bar(ctx: &Context, area: Id, spot: Pos2, room: Rect, actions: &[Action]) -> (Option<Action>, egui::InnerResponse<()>) {
    let visuals = ctx.global_style().visuals.clone();
    let (fill, ink) = (visuals.text_color(), visuals.panel_fill);
    let mut chosen = None;
    let drawn = egui::Area::new(area)
        .order(Order::Foreground)
        .fixed_pos(spot)
        .constrain_to(room)
        .show(ctx, |ui| {
            egui::Frame::new().fill(fill).corner_radius(10).inner_margin(4).shadow(visuals.popup_shadow).show(ui, |ui| {
                // Near a screen's edge the area is narrow: its labels stay whole, never a letter a line.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;

                    for action in actions {
                        let text = RichText::new(action.name()).strong().color(ink);
                        let button = egui::Button::new(text).fill(egui::Color32::TRANSPARENT).stroke(egui::Stroke::NONE).min_size(vec2(64.0, crate::layout::FINGER));

                        if ui.add(button).clicked() {
                            chosen = Some(*action);
                        }
                    }
                });
            });
        });

    (chosen, drawn)
}

/// The text field with focus, if one has it.
fn focused_field(ctx: &Context) -> Option<Id> {
    ctx.memory(|memory| memory.focused()).filter(|id| TextEditState::load(ctx, *id).is_some())
}

/// Whether a text field has focus: on screen or not, as one scrolled away or under the keyboard
/// keeps it.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn field_focused(ctx: &Context) -> bool {
    focused_field(ctx).is_some()
}

/// Whether some of a field's text is selected.
fn selected(ctx: &Context, id: Id) -> bool {
    TextEditState::load(ctx, id)
        .and_then(|state| state.cursor.char_range())
        .is_some_and(|range| !range.is_empty())
}

#[cfg(test)]
mod tests {
    use {super::*, egui::pos2};

    /// A clipboard that holds what a test puts in it.
    struct Held(Option<String>);

    impl Clipboard for Held {
        fn has_text(&mut self) -> bool {
            self.0.is_some()
        }

        fn text(&mut self) -> Option<String> {
            self.0.clone()
        }
    }

    /// An app of one text field, with the editing glue after it, as the Android glue runs them.
    struct App {
        ctx: Context,
        text: String,
        editing: Editing,
        clipboard: Held,
        time: f64,
    }

    const FIELD: &str = "field";
    const BAR: &str = "chainv4-ui edit bar";

    impl App {
        fn new(text: &str, clipboard: Option<&str>) -> App {
            App {
                ctx: Context::default(),
                text: text.to_string(),
                editing: Editing::default(),
                clipboard: Held(clipboard.map(str::to_string)),
                time: 0.0,
            }
        }

        /// One frame, later by some seconds, with these events.
        fn frame(&mut self, later: f64, mut events: Vec<Event>) -> Vec<egui::OutputCommand> {
            self.time += later;
            let mut raw_input = RawInput {
                time: Some(self.time),
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(400.0, 600.0))),
                ..Default::default()
            };
            self.editing.feed(&mut raw_input);
            raw_input.events.append(&mut events);
            let App { text, editing, clipboard, .. } = self;
            let mut output = self.ctx.run_ui(raw_input, |ui| {
                ui.add(egui::TextEdit::singleline(text).id(Id::unique(FIELD)));
                editing.show(ui.ctx(), clipboard);
            });
            output.textures_delta.clear();

            output.platform_output.commands
        }

        fn field(&self) -> egui::Rect {
            self.ctx.read_response(Id::unique(FIELD)).expect("drawn").rect
        }

        fn focused(&self) -> bool {
            self.ctx.memory(|memory| memory.has_focus(Id::unique(FIELD)))
        }
    }

    /// A finger landing, or lifting, where egui-winit would say so on a touch screen.
    fn finger(at: Pos2, down: bool) -> Vec<Event> {
        vec![
            Event::Touch {
                device_id: egui::TouchDeviceId(1),
                id: egui::TouchId(1),
                phase: if down { egui::TouchPhase::Start } else { egui::TouchPhase::End },
                pos: at,
                force: None,
            },
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: down,
                modifiers: Modifiers::NONE,
            },
        ]
    }

    /// A bar's size, on a phone's screen with the keyboard taking the bottom half.
    const SIZE: Vec2 = vec2(160.0, 48.0);

    fn room_of(width: f32, height: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, 24.0), vec2(width, height))
    }

    fn inside(spot: Pos2, room: Rect) -> bool {
        room.contains_rect(Rect::from_min_size(spot, SIZE))
    }

    #[test]
    fn the_bar_goes_just_above_the_field() {
        let room = room_of(400.0, 400.0);
        let field = Rect::from_min_size(pos2(14.0, 200.0), vec2(372.0, 40.0));
        let spot = place(SIZE, Some(field), 200.0, room);

        assert_eq!(spot, pos2(120.0, 200.0 - BAR_GAP - SIZE.y), "centered on the finger, over the field");
    }

    #[test]
    fn with_no_room_above_the_bar_goes_below_the_field() {
        let room = room_of(400.0, 400.0);
        let field = Rect::from_min_size(pos2(14.0, 30.0), vec2(372.0, 40.0));
        let spot = place(SIZE, Some(field), 200.0, room);

        assert_eq!(spot.y, 70.0 + BAR_GAP);
        assert!(inside(spot, room));
    }

    #[test]
    fn a_bar_never_leaves_the_room_at_either_side() {
        let room = room_of(400.0, 400.0);
        let field = Rect::from_min_size(pos2(14.0, 200.0), vec2(372.0, 40.0));

        assert_eq!(place(SIZE, Some(field), 0.0, room).x, BAR_GAP, "a finger at the left edge");
        assert_eq!(place(SIZE, Some(field), 400.0, room).x, 400.0 - BAR_GAP - SIZE.x, "and at the right");
    }

    #[test]
    fn a_finger_left_elsewhere_by_a_screen_turned_leaves_the_bar_over_the_field() {
        let room = room_of(1000.0, 400.0);
        let field = Rect::from_min_size(pos2(400.0, 200.0), vec2(400.0, 40.0));
        let spot = place(SIZE, Some(field), 100.0, room);

        assert!(spot.x + SIZE.x / 2.0 >= field.left(), "{spot:?}");
    }

    #[test]
    fn a_field_out_of_sight_leaves_the_bar_just_above_the_keyboard() {
        let room = room_of(400.0, 300.0);
        let far_below = Rect::from_min_size(pos2(14.0, 900.0), vec2(372.0, 40.0));

        for field in [None, Some(far_below)] {
            let spot = place(SIZE, field, 200.0, room);

            assert_eq!(spot.y, room.bottom() - BAR_GAP - SIZE.y);
            assert!(inside(spot, room));
        }
    }

    #[test]
    fn in_a_room_as_short_as_a_phone_on_its_side_with_its_keyboard_the_bar_stays_in_it() {
        // Too short for the bar on either side of the field.
        let room = room_of(800.0, 90.0);
        let low = Rect::from_min_size(pos2(14.0, 50.0), vec2(372.0, 40.0));
        let high = Rect::from_min_size(pos2(14.0, 30.0), vec2(372.0, 40.0));

        assert!(inside(place(SIZE, Some(low), 100.0, room), room));
        assert_eq!(place(SIZE, Some(low), 100.0, room).y, room.top() + BAR_GAP, "over a field low in the room, at its top");
        assert!(inside(place(SIZE, Some(high), 100.0, room), room));
    }

    #[test]
    fn a_room_smaller_than_the_bar_keeps_its_start_in_sight() {
        let room = room_of(100.0, 30.0);
        let spot = place(SIZE, None, 50.0, room);

        assert_eq!(spot, pos2(BAR_GAP, room.top() + BAR_GAP));
    }

    #[test]
    fn the_bar_offers_what_can_be_done() {
        assert_eq!(actions(true, false), [Action::Paste, Action::SelectAll]);
        assert_eq!(actions(false, true), [Action::SelectAll, Action::Copy, Action::Cut]);
    }

    #[test]
    fn a_finger_held_on_a_field_brings_the_bar_and_a_tap_elsewhere_takes_it() {
        let mut app = App::new("words", Some("pasted"));
        app.frame(0.0, Vec::new());
        let on = app.field().center();
        app.frame(0.1, finger(on, true));
        app.frame(0.3, Vec::new());

        assert!(!app.editing.bar_is_up(), "not yet held long enough");

        app.frame(0.3, Vec::new());

        assert!(app.focused(), "holding the field focuses it");
        assert!(app.editing.bar_is_up());

        app.frame(0.1, finger(on, false));

        assert!(app.editing.bar_is_up(), "lifting the finger leaves it up");

        let elsewhere = pos2(200.0, 500.0);
        app.frame(0.1, finger(elsewhere, true));
        app.frame(0.1, finger(elsewhere, false));

        assert!(!app.editing.bar_is_up(), "a tap elsewhere takes it down");
    }

    #[test]
    fn paste_on_the_bar_pastes_into_the_field_held() {
        let mut app = App::new("", Some("ed25519:0xab"));
        app.frame(0.0, Vec::new());
        let on = app.field().center();
        app.frame(0.1, finger(on, true));
        app.frame(0.5, Vec::new());
        app.frame(0.5, Vec::new());
        app.frame(0.1, finger(on, false));
        app.frame(0.1, Vec::new());
        let bar = app.ctx.memory(|memory| memory.area_rect(Id::unique(BAR))).expect("the bar is up");
        // Paste comes first.
        let paste = pos2(bar.left() + 24.0, bar.center().y);
        app.frame(0.1, finger(paste, true));
        app.frame(0.1, finger(paste, false));
        app.frame(0.1, Vec::new());

        assert_eq!(app.text, "ed25519:0xab");
        assert!(app.focused(), "the field keeps its focus");
        assert!(!app.editing.bar_is_up(), "the bar goes once used");
    }

    #[test]
    fn select_all_on_the_bar_leaves_it_up_for_cut() {
        let mut app = App::new("secret words", None);
        app.frame(0.0, Vec::new());
        let on = app.field().center();
        app.frame(0.1, finger(on, true));
        app.frame(0.5, Vec::new());
        app.frame(0.5, Vec::new());
        app.frame(0.1, finger(on, false));
        app.frame(0.1, Vec::new());
        let bar = app.ctx.memory(|memory| memory.area_rect(Id::unique(BAR))).expect("the bar is up");
        // With nothing to paste, Select all comes first.
        let select_all = pos2(bar.left() + 24.0, bar.center().y);
        app.frame(0.1, finger(select_all, true));
        app.frame(0.1, finger(select_all, false));
        app.frame(0.1, Vec::new());
        app.frame(0.1, Vec::new());

        assert!(selected(&app.ctx, Id::unique(FIELD)));
        assert!(app.editing.bar_is_up(), "it stays up for Copy and Cut");

        // Cut comes last.
        let bar = app.ctx.memory(|memory| memory.area_rect(Id::unique(BAR))).expect("still up");
        let cut = pos2(bar.right() - 20.0, bar.center().y);
        app.frame(0.1, finger(cut, true));
        let commands = app.frame(0.1, finger(cut, false));
        let commands = [commands, app.frame(0.1, Vec::new())].concat();

        assert_eq!(app.text, "");
        assert!(commands.contains(&egui::OutputCommand::CopyText("secret words".to_string())));
        assert!(!app.editing.bar_is_up());
    }

    #[test]
    fn a_drag_that_selects_brings_the_bar() {
        let mut app = App::new("some words to select", None);
        app.ctx.memory_mut(|memory| memory.request_focus(Id::unique(FIELD)));
        app.frame(0.0, Vec::new());
        let field = app.field();
        let (from, to) = (pos2(field.left() + 6.0, field.center().y), pos2(field.right() - 6.0, field.center().y));
        app.frame(0.1, finger(from, true));

        for step in 1..=4 {
            let at = from + (to - from) * (step as f32 / 4.0);
            app.frame(0.05, vec![Event::PointerMoved(at)]);
        }

        app.frame(0.05, finger(to, false));

        assert!(selected(&app.ctx, Id::unique(FIELD)), "the drag selected");
        assert!(app.editing.bar_is_up(), "and brought the bar for Copy");
    }

    #[test]
    fn the_tap_that_gives_a_field_focus_brings_nothing_and_neither_does_typing_after_it() {
        let mut app = App::new("gm ", Some("ed25519:0xab"));
        app.frame(0.0, Vec::new());
        let on = app.field().center();
        app.frame(0.1, finger(on, true));
        app.frame(0.1, finger(on, false));

        for _ in 0..3 {
            app.frame(0.2, Vec::new());
        }

        assert!(app.focused(), "the tap focused it");
        assert!(app.ctx.memory(|memory| memory.area_rect(Id::unique(BAR))).is_none(), "and nothing was offered");

        app.frame(0.1, vec![Event::Text("x".to_string())]);
        app.frame(0.1, Vec::new());

        assert_eq!(app.text, "gm x");
        assert!(!app.editing.bar_is_up());
    }

    #[test]
    fn typing_takes_the_bar_down_at_once_and_it_comes_back_when_asked() {
        for typed in [Event::Text("x".to_string()), Event::Key { key: Key::Backspace, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }] {
            let mut app = App::new("words", Some("pasted"));
            app.frame(0.0, Vec::new());
            let on = app.field().center();
            app.frame(0.1, finger(on, true));
            app.frame(0.1, finger(on, false));
            app.frame(1.0, finger(on, true));
            app.frame(0.1, finger(on, false));
            app.frame(0.1, Vec::new());

            assert!(app.editing.bar_is_up(), "tapped with focus");

            app.frame(0.1, vec![typed.clone()]);

            assert!(!app.editing.bar_is_up(), "typing starts, and it goes: {typed:?}");

            app.frame(0.1, Vec::new());
            app.frame(1.0, finger(on, true));
            app.frame(0.1, finger(on, false));
            app.frame(0.1, Vec::new());

            assert!(app.editing.bar_is_up(), "another tap brings it back");

            app.frame(1.0, finger(on, true));
            app.frame(0.1, finger(on, false));
            app.frame(0.1, Vec::new());
            assert!(!app.editing.bar_is_up());

            app.frame(1.0, finger(on, true));
            app.frame(0.6, Vec::new());
            app.frame(0.1, Vec::new());

            assert!(app.editing.bar_is_up(), "and so does a hold");
        }
    }

    #[test]
    fn nothing_is_asked_of_the_clipboard_until_the_bar_is_up_and_then_it_offers_paste() {
        /// A clipboard that counts how often it is asked.
        struct Counted(u32);

        impl Clipboard for Counted {
            fn has_text(&mut self) -> bool {
                self.0 += 1;

                true
            }

            fn text(&mut self) -> Option<String> {
                None
            }
        }

        let ctx = Context::default();
        let (mut editing, mut text, mut clipboard) = (Editing::default(), String::new(), Counted(0));
        ctx.memory_mut(|memory| memory.request_focus(Id::unique(FIELD)));
        let mut frame = |clipboard: &mut Counted, editing: &mut Editing, time: f64| {
            let mut output = ctx.run_ui(RawInput { time: Some(time), ..Default::default() }, |ui| {
                ui.add(egui::TextEdit::singleline(&mut text).id(Id::unique(FIELD)));
                editing.show(ui.ctx(), clipboard);
            });
            output.textures_delta.clear();
        };

        for step in 0..5 {
            frame(&mut clipboard, &mut editing, f64::from(step));
        }

        assert_eq!(clipboard.0, 0, "a phone's clipboard is not read for a field alone");

        editing.bar = Some((Id::unique(FIELD), pos2(10.0, 10.0)));
        frame(&mut clipboard, &mut editing, 6.0);

        assert_eq!(clipboard.0, 1);
    }

    /// What winit on Android has egui told of a letter pressed with Ctrl held, as it would of the
    /// letter typed.
    fn letter(key: Key, text: &str) -> Vec<Event> {
        let event = |pressed| Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: Modifiers::NONE };

        vec![event(true), Event::Text(text.to_string()), event(false)]
    }

    #[test]
    fn a_keyboards_ctrl_v_pastes_rather_than_typing_a_v() {
        let mut events = letter(Key::V, "v");
        ctrl_key(&mut events, Key::V, &mut || Some("ed25519:0xab".to_string()));

        assert_eq!(events[0], Event::Paste("ed25519:0xab".to_string()));
        assert!(!events.iter().any(|event| matches!(event, Event::Text(_))), "no letter: {events:?}");

        // The field takes it, where it is.
        let mut app = App::new("gm ", None);
        app.ctx.memory_mut(|memory| memory.request_focus(Id::unique(FIELD)));
        app.frame(0.0, Vec::new());
        let mut events = letter(Key::V, "v");
        ctrl_key(&mut events, Key::V, &mut || Some("ed25519:0xab".to_string()));
        app.frame(0.1, events);

        assert_eq!(app.text, "gm ed25519:0xab");
    }

    #[test]
    fn a_v_typed_stays_a_v() {
        let mut app = App::new("", None);
        app.ctx.memory_mut(|memory| memory.request_focus(Id::unique(FIELD)));
        app.frame(0.0, Vec::new());
        app.frame(0.1, letter(Key::V, "v"));

        assert_eq!(app.text, "v");
    }

    #[test]
    fn ctrl_v_with_nothing_to_paste_leaves_nothing_typed() {
        let mut events = letter(Key::V, "v");
        ctrl_key(&mut events, Key::V, &mut || None);

        assert_eq!(events.len(), 1, "only the key's lifting: {events:?}");
        assert!(matches!(events[0], Event::Key { pressed: false, .. }));
    }

    #[test]
    fn the_other_shortcuts_mean_what_they_do_on_a_desktop() {
        let mut app = App::new("secret words", None);
        app.ctx.memory_mut(|memory| memory.request_focus(Id::unique(FIELD)));
        app.frame(0.0, Vec::new());
        let mut events = letter(Key::A, "a");
        ctrl_key(&mut events, Key::A, &mut || None);
        app.frame(0.1, events);

        assert!(selected(&app.ctx, Id::unique(FIELD)), "Ctrl+A selects, and types no a");
        assert_eq!(app.text, "secret words");

        let mut events = letter(Key::C, "c");
        ctrl_key(&mut events, Key::C, &mut || None);
        let commands = app.frame(0.1, events);

        assert!(commands.contains(&egui::OutputCommand::CopyText("secret words".to_string())));

        let mut events = letter(Key::X, "x");
        ctrl_key(&mut events, Key::X, &mut || None);
        app.frame(0.1, events);

        assert_eq!(app.text, "");
    }

    #[test]
    fn a_shortcut_noted_for_a_key_egui_never_heard_of_changes_nothing() {
        let mut events = letter(Key::B, "b");
        let before = events.clone();
        ctrl_key(&mut events, Key::V, &mut || panic!("nothing to read the clipboard for"));

        assert_eq!(events, before);
    }

    #[test]
    fn a_paste_key_pastes_and_an_empty_clipboard_says_nothing() {
        let mut events = Vec::new();
        paste_key(&mut events, &mut || Some("words".to_string()));

        assert_eq!(events, [Event::Paste("words".to_string())]);

        let mut events = Vec::new();
        paste_key(&mut events, &mut || Some(String::new()));
        paste_key(&mut events, &mut || None);

        assert!(events.is_empty());
    }

    #[test]
    fn a_finger_that_rolls_a_little_while_held_still_brings_the_bar() {
        let mut app = App::new("words", Some("pasted"));
        app.frame(0.0, Vec::new());
        let on = app.field().center();
        app.frame(0.1, finger(on, true));
        app.frame(0.2, vec![Event::PointerMoved(on + vec2(10.0, 3.0))]);
        app.frame(0.2, vec![Event::PointerMoved(on + vec2(12.0, 6.0))]);
        app.frame(0.2, Vec::new());

        assert!(app.editing.bar_is_up(), "more than egui's six points, and still a hold");
    }

    #[test]
    fn a_finger_that_moves_on_is_no_hold() {
        let mut app = App::new("words", Some("pasted"));
        app.frame(0.0, Vec::new());
        let on = app.field().center();
        app.frame(0.1, finger(on, true));
        app.frame(0.1, vec![Event::PointerMoved(on + vec2(0.0, 40.0))]);
        app.frame(0.6, Vec::new());

        assert!(!app.editing.bar_is_up());
    }

    #[test]
    fn a_hold_shorter_than_a_click_is_long_still_leaves_the_bar_up_when_lifted() {
        let mut app = App::new("words", Some("pasted"));
        app.frame(0.0, Vec::new());
        let on = app.field().center();
        app.frame(0.1, finger(on, true));
        app.frame(0.3, Vec::new());
        app.frame(0.25, Vec::new());

        assert!(app.editing.bar_is_up());

        // Under egui's 0.8 seconds, so egui calls lifting it a click.
        app.frame(0.05, finger(on, false));
        app.frame(0.1, Vec::new());

        assert!(app.editing.bar_is_up(), "lifting the finger that brought it leaves it");
    }

    #[test]
    fn a_tap_on_a_field_with_focus_brings_the_bar_and_a_second_takes_it_away() {
        let mut app = App::new("words", Some("pasted"));
        app.frame(0.0, Vec::new());
        let on = app.field().center();

        // The first tap only gives it focus.
        app.frame(0.1, finger(on, true));
        app.frame(0.1, finger(on, false));
        app.frame(0.1, Vec::new());

        assert!(app.focused());
        assert!(!app.editing.bar_is_up());

        app.frame(1.0, finger(on, true));
        app.frame(0.1, finger(on, false));
        app.frame(0.1, Vec::new());

        assert!(app.editing.bar_is_up(), "tapped with focus");

        app.frame(1.0, finger(on, true));
        app.frame(0.1, finger(on, false));
        app.frame(0.1, Vec::new());

        assert!(!app.editing.bar_is_up(), "tapped again");
    }

    #[test]
    fn the_bar_stays_off_the_tabs_and_beside_the_field() {
        let ctx = Context::default();
        let (mut editing, mut text, mut clipboard) = (Editing::default(), String::new(), Held(Some("words".to_string())));
        ctx.memory_mut(|memory| memory.request_focus(Id::unique(FIELD)));
        editing.bar = Some((Id::unique(FIELD), pos2(200.0, 340.0)));
        let (mut tabs, mut field) = (Rect::NOTHING, Rect::NOTHING);

        // A page with tabs along the bottom, its field as low as the page goes, and the bar above the
        // keyboard's top edge, where the screen ends.
        for frame in 0..4 {
            let raw_input = RawInput {
                time: Some(f64::from(frame)),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0))),
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw_input, |ui| {
                tabs = egui::Panel::bottom("tabs").exact_size(46.0).show(ui, |ui| ui.allocate_space(ui.available_size())).response.rect;
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        crate::layout::keep_focus_in_view(ui);
                        ui.add_space(ui.available_height() - 40.0);
                        field = ui.add(egui::TextEdit::singleline(&mut text).id(Id::unique(FIELD))).rect;
                    });
                });
                editing.show(ui.ctx(), &mut clipboard);
            });
            output.textures_delta.clear();
        }

        let bar = ctx.memory(|memory| memory.area_rect(Id::unique(BAR))).expect("drawn");

        assert!(bar.bottom() <= tabs.top(), "above the tabs: {bar:?} {tabs:?}");
        assert!(!bar.intersects(field), "not over the field: {bar:?} {field:?}");
        assert!(bar.bottom() <= field.top(), "just above it: {bar:?} {field:?}");
    }

    #[test]
    fn select_all_and_copy_reach_the_field_as_a_keyboard_would() {
        let mut app = App::new("secret words", None);
        app.ctx.memory_mut(|memory| memory.request_focus(Id::unique(FIELD)));
        app.frame(0.0, Vec::new());
        let ctx = app.ctx.clone();
        app.editing.act(&ctx, Id::unique(FIELD), Action::SelectAll, &mut app.clipboard);
        app.frame(0.1, Vec::new());
        app.editing.act(&ctx, Id::unique(FIELD), Action::Copy, &mut app.clipboard);
        let commands = app.frame(0.1, Vec::new());

        assert!(app.focused());

        assert!(commands.contains(&egui::OutputCommand::CopyText("secret words".to_string())));
    }
}
