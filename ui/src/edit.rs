//! Editing text with no keyboard shortcuts, as on Android, where egui reaches no clipboard and the
//! soft keyboard offers no Paste. Holding a finger on a text field brings a bar over it with Paste,
//! Select all, and Copy and Cut over a selection; and while a field has focus and the keyboard is
//! up, a Paste button sits above the keyboard. They send the field the events a keyboard would, so
//! the field does the rest: it inserts what is pasted, and hands what is copied to the app's copy
//! output, which the Android glue puts on the clipboard.

use egui::{Align2, Context, Event, Id, Key, Modifiers, Order, Pos2, RawInput, text::CCursorRange, text_edit::TextEditState, vec2};

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

/// The bar and the Paste button, for an app's frames.
#[derive(Default)]
pub(crate) struct Editing {
    /// The bar a held finger brought up: over which field, and where the finger was.
    bar: Option<(Id, Pos2)>,
    /// The text field that had focus last frame, and what was selected in it.
    field: Option<Id>,
    kept: Option<(Id, CCursorRange)>,
    /// What the field is to do in the next frame, as a keyboard would have said it.
    queued: Vec<Event>,
    /// Whether the clipboard held text when last asked, and when that was.
    clip: Option<(f64, bool)>,
}

impl Editing {
    /// After the app's frame: brings the bar up or takes it down, draws it and the Paste button,
    /// and queues what they are asked to do. The Paste button shows while the keyboard is up.
    pub(crate) fn show(&mut self, ctx: &Context, keyboard: bool, clipboard: &mut dyn Clipboard) {
        let (time, lifted) = ctx.input(|input| (input.time, input.pointer.any_released()));
        let focused = focused_field(ctx);
        // A tap on the bar or the Paste button takes the field's focus as it lifts, before either
        // sees the tap: for that frame they act for the field that had it, as they showed it.
        let field = focused.or(self.field.filter(|_| lifted));
        self.field = focused;

        // A field that loses focus loses its selection too, so what was selected is kept for the
        // bar to offer Copy and Cut over, and is selected again before they act.
        if let Some(id) = focused
            && let Some(range) = TextEditState::load(ctx, id).and_then(|state| state.cursor.char_range())
        {
            self.kept = Some((id, range));
        }

        // Asked again once a second while a field has focus: something else may have copied.
        let has_text = match (field, self.clip) {
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

        if let Some((id, at)) = self.bar {
            self.bar(ctx, id, at, has_text, clipboard);
        }

        // A finger held on a field brings the bar up over it, and gives the field focus: egui
        // counts the hold as no tap.
        if let Some(id) = ctx
            .interaction_snapshot(|interaction| interaction.long_touched)
            .filter(|id| TextEditState::load(ctx, *id).is_some())
        {
            let at = ctx.input(|input| input.pointer.press_origin()).unwrap_or_else(|| ctx.content_rect().center());
            self.bar = Some((id, at));
            ctx.memory_mut(|memory| memory.request_focus(id));
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

        if let Some(id) = field
            && keyboard
            && has_text
            && self.bar.is_none()
        {
            let corner = ctx.content_rect().right_bottom() - vec2(12.0, 12.0);
            let pasted = egui::Area::new(Id::unique("chainv4-ui paste"))
                .order(Order::Foreground)
                .pivot(Align2::RIGHT_BOTTOM)
                .fixed_pos(corner)
                .show(ctx, |ui| ui.add(egui::Button::new(Action::Paste.name()).min_size(vec2(72.0, 38.0))).clicked())
                .inner;

            if pasted {
                self.act(ctx, id, Action::Paste, clipboard);
            }
        }
    }

    /// Before the next frame: hands it what was queued.
    pub(crate) fn feed(&mut self, raw_input: &mut RawInput) {
        raw_input.events.append(&mut self.queued);
    }

    fn bar(&mut self, ctx: &Context, id: Id, at: Pos2, has_text: bool, clipboard: &mut dyn Clipboard) {
        let selected = self.kept.is_some_and(|(kept, range)| kept == id && !range.is_empty());
        let mut chosen = None;
        let drawn = egui::Area::new(Id::unique("chainv4-ui edit bar"))
            .order(Order::Foreground)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(at - vec2(0.0, 28.0))
            .constrain_to(ctx.content_rect())
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    // Near a screen's edge the area is narrow: its labels stay whole, never a letter a line.
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.horizontal(|ui| {
                        for action in actions(has_text, selected) {
                            if ui.add(egui::Button::new(action.name()).min_size(vec2(0.0, 38.0))).clicked() {
                                chosen = Some(action);
                            }
                        }
                    });
                });
            });

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
                Event::Text(_) => true,
                Event::Key { pressed, modifiers, .. } => *pressed && !modifiers.command,
                _ => false,
            };

            (input.pointer.any_click(), input.events.iter().any(typed))
        });
        let moved = ctx.memory(|memory| memory.focused()).is_some_and(|focused| focused != id);

        if (clicked && !drawn.response.contains_pointer()) || typed || moved {
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

        /// One frame, later by some seconds, with these events, the keyboard up or not.
        fn frame(&mut self, later: f64, mut events: Vec<Event>, keyboard: bool) -> Vec<egui::OutputCommand> {
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
                editing.show(ui.ctx(), keyboard, clipboard);
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

    #[test]
    fn the_bar_offers_what_can_be_done() {
        assert_eq!(actions(true, false), [Action::Paste, Action::SelectAll]);
        assert_eq!(actions(false, true), [Action::SelectAll, Action::Copy, Action::Cut]);
    }

    #[test]
    fn a_finger_held_on_a_field_brings_the_bar_and_a_tap_elsewhere_takes_it() {
        let mut app = App::new("words", Some("pasted"));
        app.frame(0.0, Vec::new(), false);
        let on = app.field().center();
        app.frame(0.1, finger(on, true), false);
        app.frame(0.5, Vec::new(), false);

        assert!(!app.editing.bar_is_up(), "not yet held long enough");

        app.frame(0.5, Vec::new(), false);

        assert!(app.focused(), "holding the field focuses it");
        assert!(app.editing.bar_is_up());

        app.frame(0.1, finger(on, false), false);

        assert!(app.editing.bar_is_up(), "lifting the finger leaves it up");

        let elsewhere = pos2(200.0, 500.0);
        app.frame(0.1, finger(elsewhere, true), false);
        app.frame(0.1, finger(elsewhere, false), false);

        assert!(!app.editing.bar_is_up(), "a tap elsewhere takes it down");
    }

    #[test]
    fn paste_on_the_bar_pastes_into_the_field_held() {
        let mut app = App::new("", Some("ed25519:0xab"));
        app.frame(0.0, Vec::new(), false);
        let on = app.field().center();
        app.frame(0.1, finger(on, true), false);
        app.frame(0.5, Vec::new(), false);
        app.frame(0.5, Vec::new(), false);
        app.frame(0.1, finger(on, false), false);
        app.frame(0.1, Vec::new(), false);
        let bar = app.ctx.memory(|memory| memory.area_rect(Id::unique("chainv4-ui edit bar"))).expect("the bar is up");
        // Paste comes first.
        let paste = pos2(bar.left() + 24.0, bar.center().y);
        app.frame(0.1, finger(paste, true), false);
        app.frame(0.1, finger(paste, false), false);
        app.frame(0.1, Vec::new(), false);

        assert_eq!(app.text, "ed25519:0xab");
        assert!(app.focused(), "the field keeps its focus");
        assert!(!app.editing.bar_is_up(), "the bar goes once used");
    }

    #[test]
    fn select_all_on_the_bar_leaves_it_up_for_cut() {
        let mut app = App::new("secret words", None);
        app.frame(0.0, Vec::new(), false);
        let on = app.field().center();
        app.frame(0.1, finger(on, true), false);
        app.frame(0.5, Vec::new(), false);
        app.frame(0.5, Vec::new(), false);
        app.frame(0.1, finger(on, false), false);
        app.frame(0.1, Vec::new(), false);
        let bar = app.ctx.memory(|memory| memory.area_rect(Id::unique("chainv4-ui edit bar"))).expect("the bar is up");
        // With nothing to paste, Select all comes first.
        let select_all = pos2(bar.left() + 24.0, bar.center().y);
        app.frame(0.1, finger(select_all, true), false);
        app.frame(0.1, finger(select_all, false), false);
        app.frame(0.1, Vec::new(), false);
        app.frame(0.1, Vec::new(), false);

        assert!(selected(&app.ctx, Id::unique(FIELD)));
        assert!(app.editing.bar_is_up(), "it stays up for Copy and Cut");

        // Cut comes last.
        let bar = app.ctx.memory(|memory| memory.area_rect(Id::unique("chainv4-ui edit bar"))).expect("still up");
        let cut = pos2(bar.right() - 20.0, bar.center().y);
        app.frame(0.1, finger(cut, true), false);
        let commands = app.frame(0.1, finger(cut, false), false);
        let commands = [commands, app.frame(0.1, Vec::new(), false)].concat();

        assert_eq!(app.text, "");
        assert!(commands.contains(&egui::OutputCommand::CopyText("secret words".to_string())));
        assert!(!app.editing.bar_is_up());
    }

    #[test]
    fn a_drag_that_selects_brings_the_bar() {
        let mut app = App::new("some words to select", None);
        app.ctx.memory_mut(|memory| memory.request_focus(Id::unique(FIELD)));
        app.frame(0.0, Vec::new(), false);
        let field = app.field();
        let (from, to) = (pos2(field.left() + 6.0, field.center().y), pos2(field.right() - 6.0, field.center().y));
        app.frame(0.1, finger(from, true), false);

        for step in 1..=4 {
            let at = from + (to - from) * (step as f32 / 4.0);
            app.frame(0.05, vec![Event::PointerMoved(at)], false);
        }

        app.frame(0.05, finger(to, false), false);

        assert!(selected(&app.ctx, Id::unique(FIELD)), "the drag selected");
        assert!(app.editing.bar_is_up(), "and brought the bar for Copy");
    }

    #[test]
    fn the_paste_button_over_the_keyboard_pastes_into_the_field() {
        let mut app = App::new("gm ", Some("ed25519:0xab"));
        app.ctx.memory_mut(|memory| memory.request_focus(Id::unique(FIELD)));
        app.frame(0.0, Vec::new(), false);
        app.frame(0.1, Vec::new(), false);

        assert!(app.ctx.read_response(Id::unique("chainv4-ui paste")).is_none(), "no button without the keyboard");

        // Its first frame only sizes the button; it takes taps from the next.
        app.frame(0.1, Vec::new(), true);
        app.frame(0.1, Vec::new(), true);
        // The button sits in the corner over the keyboard.
        let button = pos2(400.0 - 30.0, 600.0 - 30.0);
        app.frame(0.1, finger(button, true), true);
        app.frame(0.1, finger(button, false), true);
        app.frame(0.1, Vec::new(), true);

        assert_eq!(app.text, "gm ed25519:0xab");
        assert!(app.focused(), "the field keeps its focus");
    }

    #[test]
    fn nothing_shows_to_paste_with_an_empty_clipboard() {
        let mut app = App::new("", None);
        app.ctx.memory_mut(|memory| memory.request_focus(Id::unique(FIELD)));
        app.frame(0.0, Vec::new(), true);
        app.frame(0.1, Vec::new(), true);

        assert!(app.ctx.memory(|memory| memory.area_rect(Id::unique("chainv4-ui paste"))).is_none());

        let button = pos2(400.0 - 30.0, 600.0 - 30.0);
        app.frame(0.1, finger(button, true), true);
        app.frame(0.1, finger(button, false), true);
        app.frame(0.1, Vec::new(), true);

        assert_eq!(app.text, "");
    }

    #[test]
    fn select_all_and_copy_reach_the_field_as_a_keyboard_would() {
        let mut app = App::new("secret words", None);
        app.ctx.memory_mut(|memory| memory.request_focus(Id::unique(FIELD)));
        app.frame(0.0, Vec::new(), false);
        let ctx = app.ctx.clone();
        app.editing.act(&ctx, Id::unique(FIELD), Action::SelectAll, &mut app.clipboard);
        app.frame(0.1, Vec::new(), false);
        app.editing.act(&ctx, Id::unique(FIELD), Action::Copy, &mut app.clipboard);
        let commands = app.frame(0.1, Vec::new(), false);

        assert!(app.focused());

        assert!(commands.contains(&egui::OutputCommand::CopyText("secret words".to_string())));
    }
}
