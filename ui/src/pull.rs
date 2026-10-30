//! Pulling a page down to reload it, on a touch screen: from the very top of its scroll area only,
//! and only by a pull that means it. A page that can scroll is left to scroll: a finger that starts
//! anywhere else, goes up or sideways first, or is a second one, is not a pull, and neither is a
//! drag something on the page took, such as a slider. A circle comes down over the page, and fills
//! as the pull nears what reloads, as a phone's own apps do.

use {
    crate::theme::Palette,
    egui::{Pos2, Rect, Stroke, Ui},
};

/// How far a finger goes for each point the circle comes down: a pull is meant, and stiff.
const STIFFNESS: f32 = 0.5;
/// How far the circle comes down before letting go reloads, and the most it comes down.
const ENOUGH: f32 = 64.0;
const MOST: f32 = 96.0;
/// How far a finger may stray before it says which way it means to go.
const SLOP: f32 = 12.0;
/// How far the page may be from its top and still count as there.
const TOP: f32 = 0.5;
/// How wide the circle is.
const CIRCLE: f32 = 34.0;

/// One frame of what a finger does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Touching {
    /// Where the finger is, while one is down.
    pub at: Option<Pos2>,
    /// Whether it is in the page's scroll area.
    pub in_area: bool,
    /// Whether the page is at its very top.
    pub at_top: bool,
    /// Whether more than one finger is down, or something on the page took the drag.
    pub taken: bool,
}

/// How a pull stands, from one frame's touch to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Gesture {
    #[default]
    Idle,
    /// A finger down that is not a pull, until it lifts: nothing it does now becomes one.
    Passing,
    /// A finger pulling, from where it landed.
    Pulling { from: Pos2, by: f32 },
}

impl Gesture {
    /// Takes one frame. Whether this was the frame letting go of a pull that went far enough.
    pub fn step(&mut self, touch: Touching) -> bool {
        let Some(at) = touch.at else {
            let fired = matches!(*self, Gesture::Pulling { by, .. } if by >= ENOUGH);
            *self = Gesture::Idle;

            return fired;
        };

        *self = match *self {
            Gesture::Idle if touch.in_area && touch.at_top && !touch.taken => Gesture::Pulling { from: at, by: 0.0 },
            Gesture::Idle | Gesture::Passing => Gesture::Passing,
            Gesture::Pulling { from, .. } => {
                let moved = at - from;
                let sideways = moved.x.abs() > SLOP && moved.x.abs() > moved.y;

                match touch.taken || !touch.at_top || moved.y < -SLOP || sideways {
                    true => Gesture::Passing,
                    false => Gesture::Pulling { from, by: (moved.y * STIFFNESS).clamp(0.0, MOST) },
                }
            }
        };

        false
    }

    /// How far the circle has come down.
    pub fn by(&self) -> f32 {
        match *self {
            Gesture::Pulling { by, .. } => by,
            _ => 0.0,
        }
    }

    /// Whether letting go now reloads.
    pub fn armed(&self) -> bool {
        self.by() >= ENOUGH
    }
}

/// A page's pull: what the finger does, and the circle shown for it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pull {
    gesture: Gesture,
    /// Where the circle sits while a reload works, as it eases back up after.
    resting: f32,
}

impl Pull {
    /// Takes the frame's touch over a page's scroll area, and draws the circle over it: pulling
    /// down, spinning while `working`, however the reload started. Only on a touch screen. Call it
    /// after the page's scroll area is shown, with the rect it was in, the offset it is at, and the
    /// drag it takes for its own. Whether the person just asked to reload.
    pub fn show(&mut self, ui: &Ui, palette: &Palette, area: Rect, offset: f32, scroll_drag: Option<egui::Id>, working: bool) -> bool {
        let ctx = ui.ctx();
        let fired = crate::layout::touch(ctx) && {
            let (down, at, fingers) = ctx.input(|input| (input.pointer.any_down(), input.pointer.interact_pos(), input.multi_touch().is_some()));
            let dragged = ctx.dragged_id().is_some_and(|dragged| Some(dragged) != scroll_drag);

            self.gesture.step(Touching {
                at: at.filter(|_| down),
                in_area: at.is_some_and(|at| area.contains(at)),
                at_top: offset <= TOP,
                taken: fingers || dragged,
            })
        };
        let by = self.gesture.by();
        // A reload keeps the circle down, spinning, till it is done, then it goes back up.
        let wanted = if working { ENOUGH } else { 0.0 };
        let dt = ctx.input(|input| input.stable_dt).min(0.1);
        self.resting += (wanted.max(if fired { ENOUGH } else { 0.0 }) - self.resting) * (dt * 12.0).min(1.0);
        let down = by.max(self.resting);

        if by > 0.0 || working || self.resting > 1.0 {
            circle(ui, palette, area, down, (by / ENOUGH).min(1.0), self.gesture.armed(), working);
            ctx.request_repaint();
        } else {
            self.resting = 0.0;
        }

        fired
    }
}

/// The circle, this far down over the top of the area: how far the pull got, as an arc, its color
/// saying once letting go reloads; or a turning arc while it works.
fn circle(ui: &Ui, palette: &Palette, area: Rect, down: f32, progress: f32, armed: bool, working: bool) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("chainv4-ui pull"))).with_clip_rect(area);
    let center = egui::pos2(area.center().x, area.top() + down - CIRCLE / 2.0);
    let radius = CIRCLE / 2.0;
    let color = if armed || working { palette.accent } else { palette.muted };
    let time = ui.input(|input| input.time) as f32;
    let (start, sweep) = match working {
        true => (time * 5.0, std::f32::consts::TAU * 0.7),
        false => (-std::f32::consts::FRAC_PI_2, std::f32::consts::TAU * progress.max(0.02)),
    };
    let points: Vec<Pos2> = (0..=32).map(|step| center + (radius - 8.0) * egui::Vec2::angled(start + sweep * step as f32 / 32.0)).collect();

    painter.circle(center, radius, palette.card, Stroke::new(1.0, palette.line));
    painter.add(egui::Shape::line(points, Stroke::new(2.5, color)));
}

#[cfg(test)]
mod tests {
    use {super::*, egui::Color32};

    const PALETTE: Palette = Palette {
        paper: Color32::BLACK,
        nav: Color32::BLACK,
        card: Color32::BLACK,
        row: Color32::BLACK,
        line: Color32::BLACK,
        ink: Color32::WHITE,
        soft: Color32::WHITE,
        muted: Color32::WHITE,
        heading: Color32::WHITE,
        accent: Color32::WHITE,
        on_accent: Color32::BLACK,
        good: Color32::WHITE,
        warn: Color32::WHITE,
        bad: Color32::WHITE,
        dark: true,
    };

    fn at(y: f32) -> Touching {
        Touching { at: Some(egui::pos2(100.0, y)), in_area: true, at_top: true, taken: false }
    }

    fn lift() -> Touching {
        Touching { at: None, in_area: false, at_top: true, taken: false }
    }

    /// Goes through these touches, and says whether any fired.
    fn run(gesture: &mut Gesture, touches: &[Touching]) -> bool {
        let mut fired = false;

        for touch in touches {
            fired |= gesture.step(*touch);
        }

        fired
    }

    #[test]
    fn a_pull_from_the_top_far_enough_reloads_once_as_the_finger_lifts() {
        let mut gesture = Gesture::default();

        assert!(!run(&mut gesture, &[at(100.0), at(150.0)]));
        assert_eq!(gesture.by(), 25.0, "the circle comes down slower than the finger");
        assert!(!gesture.armed());

        assert!(!run(&mut gesture, &[at(100.0 + ENOUGH / STIFFNESS)]));
        assert!(gesture.armed());
        assert!(gesture.step(lift()), "fires on letting go");
        assert_eq!(gesture, Gesture::Idle);
        assert!(!gesture.step(lift()), "and once");
    }

    #[test]
    fn a_pull_let_go_of_too_soon_or_taken_back_does_nothing() {
        let mut gesture = Gesture::default();

        assert!(!run(&mut gesture, &[at(100.0), at(150.0), lift()]), "not far enough");
        assert_eq!(gesture.by(), 0.0);

        // Far enough, then back up to where it started: changed its mind.
        assert!(!run(&mut gesture, &[at(100.0), at(300.0), at(105.0), lift()]));
    }

    #[test]
    fn it_goes_no_further_than_its_most_however_far_the_finger_goes() {
        let mut gesture = Gesture::default();

        run(&mut gesture, &[at(0.0), at(5_000.0)]);
        assert_eq!(gesture.by(), MOST);
    }

    #[test]
    fn a_finger_that_starts_below_the_top_scrolls_and_never_becomes_a_pull() {
        let mut gesture = Gesture::default();
        let scrolled = |y: f32| Touching { at_top: false, ..at(y) };

        // Down the page, dragging it all the way back to the top and on: still that one drag.
        let touches = [scrolled(100.0), scrolled(200.0), at(300.0), at(700.0)];
        assert!(!run(&mut gesture, &touches));
        assert_eq!(gesture, Gesture::Passing);
        assert!(!gesture.step(lift()));

        // The next finger, once at the top, can pull.
        assert!(run(&mut gesture, &[at(100.0), at(400.0), lift()]));
    }

    #[test]
    fn a_finger_going_up_or_sideways_first_is_no_pull() {
        let mut gesture = Gesture::default();

        assert!(!run(&mut gesture, &[at(300.0), at(250.0), at(900.0), lift()]), "up, to scroll the page");

        let right = |x: f32, y: f32| Touching { at: Some(egui::pos2(x, y)), ..at(0.0) };
        assert!(!run(&mut gesture, &[at(100.0), right(200.0, 110.0), at(900.0), lift()]), "sideways, along a row");
    }

    #[test]
    fn a_page_that_scrolled_under_the_finger_stops_the_pull() {
        let mut gesture = Gesture::default();
        let scrolled = |y: f32| Touching { at_top: false, ..at(y) };

        assert!(!run(&mut gesture, &[at(100.0), at(150.0), scrolled(400.0), at(900.0), lift()]));
    }

    #[test]
    fn a_finger_outside_the_page_a_second_finger_and_a_drag_on_a_slider_are_no_pull() {
        let mut gesture = Gesture::default();
        let outside = |y: f32| Touching { in_area: false, ..at(y) };
        let taken = |y: f32| Touching { taken: true, ..at(y) };

        assert!(!run(&mut gesture, &[outside(100.0), at(900.0), lift()]), "on the tabs");
        assert!(!run(&mut gesture, &[taken(100.0), at(900.0), lift()]));
        assert!(!run(&mut gesture, &[at(100.0), taken(150.0), at(900.0), lift()]), "pinching, or sliding, part way");
    }

    /// One frame of a page on a touch screen: the finger's events, the page, and the pull over it.
    /// Whether it fired, and where the page is scrolled to.
    fn frame(ctx: &egui::Context, pull: &mut Pull, scrolled: &mut crate::layout::Scrolled, events: Vec<egui::Event>, time: f64) -> (bool, f32) {
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(320.0, 400.0));
        let mut fired = false;
        let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events, time: Some(time), ..Default::default() }, |ui| {
            let area = ui.available_rect_before_wrap();

            scrolled.show(ui, egui::ScrollArea::vertical().id_salt("page").auto_shrink(false), |ui| {
                for row in 0..100 {
                    ui.label(format!("row {row}"));
                }
            });
            fired = pull.show(ui, &PALETTE, area, scrolled.offset(), scrolled.drag(), false);
        });
        output.textures_delta.clear();

        (fired, scrolled.offset())
    }

    fn touch(phase: egui::TouchPhase, y: f32) -> Vec<egui::Event> {
        let pos = egui::pos2(160.0, y);
        let pointer = match phase {
            egui::TouchPhase::Start => vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }],
            egui::TouchPhase::Move => vec![egui::Event::PointerMoved(pos)],
            _ => vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }, egui::Event::PointerGone],
        };

        [vec![egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos, force: None }], pointer].concat()
    }

    #[test]
    fn on_a_real_page_a_pull_at_the_top_reloads_and_a_scroll_lower_down_scrolls() {
        let ctx = egui::Context::default();
        let (mut pull, mut scrolled) = (Pull::default(), crate::layout::Scrolled::default());
        let mut time = 0.0;
        let mut step = |pull: &mut Pull, scrolled: &mut crate::layout::Scrolled, events: Vec<egui::Event>| {
            time += 0.05;
            frame(&ctx, pull, scrolled, events, time)
        };

        step(&mut pull, &mut scrolled, Vec::new());

        // Down from the top: nothing to scroll to, so a pull; it fires on letting go.
        step(&mut pull, &mut scrolled, touch(egui::TouchPhase::Start, 50.0));

        for y in [80.0, 120.0, 160.0, 200.0, 240.0] {
            let (fired, offset) = step(&mut pull, &mut scrolled, touch(egui::TouchPhase::Move, y));
            assert!(!fired);
            assert_eq!(offset, 0.0, "the page stays");
        }

        assert!(step(&mut pull, &mut scrolled, touch(egui::TouchPhase::End, 240.0)).0, "let go: reload");

        // Let any swing of the page settle, then scroll it down by dragging up.
        for _ in 0..40 {
            step(&mut pull, &mut scrolled, Vec::new());
        }

        step(&mut pull, &mut scrolled, touch(egui::TouchPhase::Start, 300.0));

        for y in [260.0, 220.0, 180.0, 140.0, 100.0] {
            assert!(!step(&mut pull, &mut scrolled, touch(egui::TouchPhase::Move, y)).0);
        }

        step(&mut pull, &mut scrolled, touch(egui::TouchPhase::End, 100.0));

        for _ in 0..40 {
            step(&mut pull, &mut scrolled, Vec::new());
        }

        let (_, offset) = step(&mut pull, &mut scrolled, Vec::new());
        assert!(offset > 50.0, "the page scrolled: {offset}");

        // Now a drag downward scrolls it back: no pull however far it goes, though it reaches the top.
        step(&mut pull, &mut scrolled, touch(egui::TouchPhase::Start, 50.0));
        let mut fired = false;

        for y in (6..40).map(|step| step as f32 * 10.0) {
            fired |= step(&mut pull, &mut scrolled, touch(egui::TouchPhase::Move, y)).0;
        }

        fired |= step(&mut pull, &mut scrolled, touch(egui::TouchPhase::End, 390.0)).0;

        assert!(!fired, "scrolling back up the page never reloads it");
    }
}
