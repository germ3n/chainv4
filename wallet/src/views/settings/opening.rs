//! How the wallet open opens: what it gives, and switching to another way. A switch takes it step by
//! step, each on a tap as a browser shows a passkey's prompt only right after one: what opens it now
//! is proven, then the new key is made, then the new key is opened again. Only then is the vault
//! sealed anew, opened once more under what now opens it, and put in place of the old one, the old
//! record going only after settings saying so are on disk. Until then the wallet is as it was, and
//! going back or failing at any step leaves it so.

use {
    super::{SettingsState, Ctx},
    crate::biometrics::{self, Check, Enrolling, Proof},
    crate::guard::{self, Factors, Guard, confirmed},
    crate::lock::{self, Typed},
    crate::theme,
    crate::vault,
    chainv4_ui::biometric::{Availability, Enrolled, Failure, Kind, Secret},
    egui::{RichText, Ui},
};

/// A switch under way.
pub struct Switching {
    to: Guard,
    /// The yes typed to what the hint says.
    yes: String,
    /// What opens it now.
    proof: Proof,
    /// The password it is to have, twice.
    new: Typed,
    again: Typed,
    step: Step,
    trouble: Option<String>,
}

/// How far a switch has come. The secret is the new key's, wiped as it drops.
enum Step {
    /// The hint read, and what opens it now being asked for.
    Asking,
    /// What opens it now was proven.
    Proved,
    Enrolling(Enrolling, Secret),
    /// The new key made, not yet seen to open.
    Enrolled(Enrolled, Secret),
    Reading(Check, Enrolled, Secret),
}

impl Switching {
    fn new(to: Guard) -> Switching {
        Switching { to, yes: String::new(), proof: Proof::default(), new: Typed::default(), again: Typed::default(), step: Step::Asking, trouble: None }
    }
}

/// Why a new password will not do, if it will not. One is needed here: taking a password away is for
/// the change of password below, on a wallet that has one alone.
fn refused_password(new: &str, again: &str) -> Option<&'static str> {
    if new.len() < 8 {
        Some("use a password of at least 8 characters")
    } else if new != again {
        Some("the two passwords do not match")
    } else {
        None
    }
}

pub(super) fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SettingsState) {
    let palette = *ctx.palette;
    ui.add_space(10.0);
    theme::caps(ui, &palette, "How it opens");
    let Some(entry) = ctx.settings.wallets.get(ctx.settings.wallet) else {
        return;
    };
    let (guard, unprotected) = (entry.guard(), entry.unprotected);
    let bound = biometrics::bound(ctx.wake);
    let here = match &bound {
        Availability::Ready(kind) => Some(*kind),
        Availability::Unavailable(_) => None,
    };
    let kind = biometrics::kind_of(entry).or(here);

    ui.label(RichText::new(guard.name()).color(palette.ink));
    ui.label(RichText::new(guard.says(kind, unprotected)).small().color(palette.soft));
    ui.add_space(4.0);

    if guard == Guard::Either {
        // Left from when biometrics were an alternative: they keep working until turned off here,
        // or until another way is chosen below. Turned off, they are not offered back.
        let named = match biometrics::availability(ctx.wake) {
            Availability::Ready(kind) => kind.name().to_string(),
            Availability::Unavailable(_) => "biometrics".to_string(),
        };

        if theme::quiet(ui, &palette, &format!("Turn off {named}")).clicked() {
            biometrics::turn_off(ctx.wake, ctx.settings);
            state.opening_said = Some(Ok("Off: only the password unlocks this wallet now.".to_string()));
        }

        ui.label(RichText::new("New wallets are not offered this way any more, as it adds nothing against a copy of this app's storage. Turned off, it does not come back.").small().color(palette.muted));
        ui.add_space(4.0);
    }

    match state.switching.is_some() {
        true => switching(ui, ctx, state),
        false => choices(ui, ctx, state, guard, &bound),
    }

    match &state.opening_said {
        Some(Ok(said)) => {
            ui.colored_label(palette.good, said);
        }
        Some(Err(why)) => {
            ui.colored_label(palette.bad, why);
        }
        None => {}
    }

    ui.add_space(4.0);
    ui.label(RichText::new(guard::IN_MEMORY).small().color(palette.muted));
}

/// The ways it can be switched to, each only where it can work here.
fn choices(ui: &mut Ui, ctx: &Ctx, state: &mut SettingsState, guard: Guard, bound: &Availability) {
    let palette = *ctx.palette;
    let offered = [
        (Guard::Password, "Use a password only", "A copy of this app's storage can be guessed at, however long the password makes that."),
        (Guard::Biometric, "Use biometrics only", "No password. A copy of this app's storage opens nothing."),
        (Guard::Both, "Require both", "A password and a key in hardware. A copy of this app's storage opens nothing, even with the password."),
    ];

    for (to, button, what) in offered {
        if to == guard || (to.needs_hardware() && matches!(bound, Availability::Unavailable(_))) {
            continue;
        }

        if theme::quiet(ui, &palette, button).clicked() {
            state.switching = Some(Switching::new(to));
            state.opening_said = None;
        }

        ui.label(RichText::new(what).small().color(palette.muted));
        ui.add_space(4.0);
    }

    if let Availability::Unavailable(why) = bound {
        ui.label(RichText::new(format!("Biometrics only, and requiring both, are not offered here: {why}.")).small().color(palette.muted));
    }
}

/// A switch, at the step it has come to.
fn switching(ui: &mut Ui, ctx: &mut Ctx, state: &mut SettingsState) {
    let palette = *ctx.palette;
    let Some(switch) = state.switching.as_mut() else {
        return;
    };
    let to = switch.to;
    let name = ctx.settings.wallets.get(ctx.settings.wallet).map(|entry| entry.name.clone()).unwrap_or_default();
    let kind = match biometrics::bound(ctx.wake) {
        Availability::Ready(kind) => Some(kind),
        Availability::Unavailable(_) => None,
    };
    let named = kind.map_or("biometrics", Kind::name);

    // What a prompt came to.
    let mut made = None;
    switch.step = match std::mem::replace(&mut switch.step, Step::Asking) {
        Step::Enrolling(enrolling, secret) => match enrolling.take() {
            None => Step::Enrolling(enrolling, secret),
            Some(Ok(enrolled)) => Step::Enrolled(enrolled, secret),
            Some(Err(Failure::Canceled)) => Step::Proved,
            Some(Err(why)) => {
                switch.trouble = Some(why.to_string());
                Step::Proved
            }
        },
        Step::Reading(check, enrolled, secret) => match check.take() {
            None => Step::Reading(check, enrolled, secret),
            Some(Ok(back)) if *back == *secret => {
                made = Some((enrolled, secret));
                Step::Asking
            }
            Some(Ok(_)) => {
                biometrics::forget(ctx.wake, &enrolled);
                switch.trouble = Some("The new key gave back something else, so it was not used. Nothing changed.".to_string());
                Step::Proved
            }
            // Turned down or failing, the new key may be fine: it can be tried again.
            Some(Err(Failure::Canceled)) => Step::Enrolled(enrolled, secret),
            Some(Err(Failure::Failed(why))) => {
                switch.trouble = Some(why);
                Step::Enrolled(enrolled, secret)
            }
            Some(Err(Failure::Invalidated)) => {
                biometrics::forget(ctx.wake, &enrolled);
                switch.trouble = Some("The new key is gone already, so it was not used. Nothing changed.".to_string());
                Step::Proved
            }
        },
        step => step,
    };

    if let Some((enrolled, secret)) = made {
        let factors = match to {
            Guard::Both => Factors::Both(&switch.new, &secret),
            _ => Factors::Biometric(&secret),
        };

        match put_in_place(ctx, &factors, guard::tagged(to, &enrolled), to.needs_hardware()) {
            Ok(()) => {
                state.opening_said = Some(Ok(format!("Done: {}. Your words stay the way back if {named} is ever lost.", to.name())));
                state.switching = None;

                return;
            }
            Err(why) => {
                biometrics::forget(ctx.wake, &enrolled);
                switch.step = Step::Proved;
                switch.trouble = Some(why);
            }
        }
    }

    ui.label(RichText::new(to.name()).color(palette.ink));
    ui.label(RichText::new(to.says(kind, false)).small().color(palette.soft));
    ui.add_space(4.0);
    let mut back = false;

    match &mut switch.step {
        Step::Asking => {
            let backup = if to.needs_hardware() { Some(guard::refusal(ctx.book)) } else { None };

            if let Some(Err(refused)) = &backup {
                ui.colored_label(palette.bad, "This wallet holds what its words cannot bring back, so it cannot depend on hardware yet:");

                for why in refused {
                    ui.colored_label(palette.bad, why);
                }

                back = theme::quiet(ui, &palette, "Back").clicked();
            } else {
                if let (Some(Ok(backup)), Some(kind)) = (&backup, kind) {
                    for line in guard::hint(kind, backup) {
                        ui.label(RichText::new(line).color(palette.warn));
                        ui.add_space(2.0);
                    }

                    ui.add_space(4.0);
                    ui.label(RichText::new(guard::confirm(backup)).color(palette.ink));
                    ui.add(egui::TextEdit::singleline(&mut switch.yes).hint_text("type yes").desired_width(f32::INFINITY));
                } else if to.needs_hardware() {
                    ui.colored_label(palette.bad, "Biometrics cannot be used here right now, so this cannot be chosen.");
                }

                if !ctx.settings.wallets.get(ctx.settings.wallet).is_some_and(crate::app::WalletEntry::opens_with_a_tap) {
                    ui.add_space(4.0);
                    ui.label(RichText::new("What opens it now, to prove it is you:").small().color(palette.muted));
                    switch.proof.field(ui, ctx.settings, "its current password");
                }

                if to != Guard::Biometric {
                    ui.add_space(4.0);
                    ui.label(RichText::new("The password it is to have, of at least 8 characters:").small().color(palette.muted));
                    lock::secret(ui, egui::TextEdit::singleline(&mut *switch.new).password(true).hint_text("password").desired_width(f32::INFINITY));
                    lock::secret(ui, egui::TextEdit::singleline(&mut *switch.again).password(true).hint_text("the same again").desired_width(f32::INFINITY));
                }

                switch.proof.trouble(ui, &palette);

                let accepted = !to.needs_hardware() || confirmed(&switch.yes);
                let ready = accepted && switch.proof.ready(ctx.settings) && (to == Guard::Biometric || !switch.new.is_empty());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if theme::primary(ui, &palette, "Continue", ready).clicked() {
                        switch.trouble = None;

                        match refused_password(&switch.new, &switch.again).filter(|_| to != Guard::Biometric) {
                            Some(why) => switch.trouble = Some(why.to_string()),
                            None => switch.proof.start(ctx.wake, ctx.settings, "Prove it is you"),
                        }
                    }

                    back = theme::quiet(ui, &palette, "Keep it as it is").clicked();
                });

                if switch.proof.waiting() {
                    ui.label(RichText::new(format!("Waiting for {named}…")).small().color(palette.muted));
                }

                if switch.proof.take(ctx.settings).is_some() {
                    if to.needs_hardware() {
                        switch.step = Step::Proved;
                    } else {
                        match put_in_place(ctx, &Factors::Password(&switch.new), None, false) {
                            Ok(()) => {
                                state.opening_said = Some(Ok(format!("Done: {}. Its biometrics go as the app next starts.", to.name())));
                                state.switching = None;

                                return;
                            }
                            Err(why) => switch.trouble = Some(why),
                        }
                    }
                }
            }
        }
        Step::Proved => {
            ui.label(RichText::new("Proven. Next, a new key is made. Nothing has changed yet.").small().color(palette.muted));
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, &palette, &format!("Make it with {named}"), kind.is_some()).clicked() {
                    switch.trouble = None;

                    match vault::fresh_secret() {
                        Ok(secret) => switch.step = Step::Enrolling(Enrolling::start(ctx.wake, &name, secret.clone()), secret),
                        Err(why) => switch.trouble = Some(why),
                    }
                }

                back = theme::quiet(ui, &palette, "Keep it as it is").clicked();
            });
        }
        Step::Enrolling(..) => {
            ui.label(RichText::new(format!("Waiting for {named}…")).small().color(palette.muted));
        }
        Step::Enrolled(enrolled, _) => {
            ui.label(RichText::new("The new key is made. Last, check that it opens, before anything depends on it. Nothing has changed yet.").small().color(palette.muted));
            let mut check = None;
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, &palette, &format!("Check it with {named}"), true).clicked() {
                    switch.trouble = None;
                    check = Some(Check::start(ctx.wake, enrolled, &format!("Check the new key of {name}")));
                }

                back = theme::quiet(ui, &palette, "Keep it as it is").clicked();
            });

            if let Some(check) = check
                && let Step::Enrolled(enrolled, secret) = std::mem::replace(&mut switch.step, Step::Asking)
            {
                switch.step = Step::Reading(check, enrolled, secret);
            }
        }
        Step::Reading(..) => {
            ui.label(RichText::new(format!("Waiting for {named}…")).small().color(palette.muted));
            back = theme::quiet(ui, &palette, "Keep it as it is").clicked();
        }
    }

    if let Some(why) = &switch.trouble {
        ui.colored_label(palette.bad, why);
    }

    if back {
        // A key made and not used goes now: nothing names it.
        if let Step::Enrolled(enrolled, _) | Step::Reading(_, enrolled, _) = &switch.step {
            biometrics::forget(ctx.wake, enrolled);
        }

        state.switching = None;
    }
}

/// Seals the wallet open under what will open it, opens that again, and only then puts it in place.
/// What will not seal or open leaves everything as it was.
fn put_in_place(ctx: &mut Ctx, factors: &Factors, record: Option<String>, needs_hardware: bool) -> Result<(), String> {
    if needs_hardware && let Err(refused) = guard::refusal(ctx.book) {
        return Err(refused.join(" "));
    }

    let (opened, sealed) = guard::sealed_anew(factors, ctx.book)?;
    guard::adopt(ctx.settings, sealed, record)?;
    *ctx.vault = Some(opened);

    Ok(())
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::app::{Settings, WalletEntry},
        crate::biometrics::fake,
        crate::guard::tagged,
        crate::vault::{Book, Sealed, Source, StoredAccount, SwapRecord, Vault, Words},
        crate::views::drawn::{self, World},
    };

    const PASSWORD: &str = "correct horse";
    const NEW: &str = "battery staple";

    fn book() -> Book {
        Book {
            phrases: vec![Words::new("test test test test test test test test test test test junk".to_string(), String::new())],
            accounts: vec![StoredAccount { name: "Main".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from: None } }],
            ..Book::default()
        }
    }

    fn entry(vault: Sealed, biometric: Option<String>, unprotected: bool) -> WalletEntry {
        WalletEntry { id: String::new(), name: "Mine".to_string(), vault, current: String::new(), biometric, unprotected, born: None }
    }

    /// A world with one wallet of this guard, open, on a fresh fake device.
    fn world(guard: Guard) -> (World, Book) {
        fake::fresh();
        let book = book();
        let (vault, biometric, unprotected) = match guard {
            Guard::Password => (Vault::create(PASSWORD).unwrap(), None, false),
            Guard::Bare => (Vault::create("").unwrap(), None, true),
            Guard::Either => {
                let vault = Vault::create(PASSWORD).unwrap();
                let enrolled = fake::keep(&vault.key());

                (vault, Some(enrolled.0), false)
            }
            Guard::Biometric => {
                let secret = vault::fresh_secret().unwrap();

                (Vault::keyed(&secret).unwrap(), tagged(guard, &fake::keep(&secret)), false)
            }
            Guard::Both => {
                let secret = vault::fresh_secret().unwrap();

                (Vault::both(PASSWORD, &secret).unwrap(), tagged(guard, &fake::keep(&secret)), false)
            }
        };
        let sealed = vault.seal(&book).unwrap();

        (World { settings: Settings::with_wallets(vec![entry(sealed, biometric, unprotected)]), vault: Some(vault), ..World::default() }, book)
    }

    /// What opens this wallet as it stands, by its guard and what is kept in the fake device.
    fn opens(entry: &WalletEntry, password: &str) -> Result<Book, String> {
        let secret = |entry: &WalletEntry| {
            let alias = guard::untagged(entry.biometric.as_ref().unwrap()).0.strip_prefix("fake:").unwrap().to_string();

            fake::device(|device| device.keys.get(&alias).copied()).ok_or("the key is gone".to_string())
        };

        match entry.guard() {
            Guard::Bare => Vault::open(&entry.vault, ""),
            Guard::Password | Guard::Either => Vault::open(&entry.vault, password),
            Guard::Biometric => Vault::open_with_key(&entry.vault, &secret(entry)?),
            Guard::Both => Vault::open_both(&entry.vault, password, &secret(entry)?),
        }
        .map(|(_, book)| book)
    }

    /// Draws Settings' section with a tap on this text, as a phone shows it.
    fn tap(world: &mut World, book: &mut Book, state: &mut SettingsState, text: &str) -> String {
        drawn::tap_in(world, book, &mut Vec::new(), (360.0, 740.0), text, |ui, ctx| show(ui, ctx, state))
    }

    fn said(world: &mut World, book: &mut Book, state: &mut SettingsState) -> String {
        drawn::said_in(world, book, &mut Vec::new(), (360.0, 740.0), |ui, ctx| show(ui, ctx, state))
    }

    fn typed(text: &str) -> Typed {
        Typed::new(text.to_string())
    }

    /// Fills in what the first step asks, and taps Continue.
    fn answer_first_step(world: &mut World, book: &mut Book, state: &mut SettingsState, current: &str) {
        let switch = state.switching.as_mut().unwrap();
        switch.yes = "yes".to_string();
        switch.proof.password = typed(current);
        switch.new = typed(NEW);
        switch.again = typed(NEW);
        tap(world, book, state, "Continue");
    }

    /// What the wallet takes now, and what the old one took, for each switch that can be made.
    const FROM: [Guard; 5] = [Guard::Bare, Guard::Password, Guard::Either, Guard::Biometric, Guard::Both];

    fn button(to: Guard) -> &'static str {
        match to {
            Guard::Password => "Use a password only",
            Guard::Biometric => "Use biometrics only",
            _ => "Require both",
        }
    }

    /// Walks a switch to its end through what is drawn, every prompt passing.
    fn switch(world: &mut World, book: &mut Book, state: &mut SettingsState, to: Guard) {
        tap(world, book, state, button(to));
        answer_first_step(world, book, state, PASSWORD);

        if to.needs_hardware() {
            tap(world, book, state, "Make it with fingerprint or face");
            tap(world, book, state, "Check it with fingerprint or face");
        }
    }

    #[test]
    fn every_way_of_opening_switches_to_every_way_offered_and_opens_only_with_what_it_should() {
        for from in FROM {
            for to in [Guard::Password, Guard::Biometric, Guard::Both] {
                if from == to {
                    continue;
                }

                let (mut world, mut book) = world(from);
                let old = world.settings.wallets[0].clone();
                let mut state = SettingsState::default();
                switch(&mut world, &mut book, &mut state, to);
                let entry = world.settings.wallets[0].clone();

                assert_eq!(entry.guard(), to, "{from:?} to {to:?}");
                assert!(state.switching.is_none(), "{from:?} to {to:?}: done");
                assert_ne!(entry.vault.salt, old.vault.salt, "{from:?} to {to:?}: a salt of its own");
                assert_ne!(entry.vault.nonce, old.vault.nonce, "{from:?} to {to:?}: and a nonce");
                assert!(!entry.unprotected, "{from:?} to {to:?}");
                assert_eq!(opens(&entry, NEW).unwrap(), book, "{from:?} to {to:?}");
                assert!(world.settings.persist_now, "{from:?} to {to:?}: written at once");
                assert_eq!(world.vault.as_ref().unwrap().seal(&book).map(|sealed| opens(&WalletEntry { vault: sealed, ..entry.clone() }, NEW).unwrap()).unwrap(), book, "{from:?} to {to:?}: and what is open seals under it");
                assert_eq!(world.settings.retired, old.biometric.iter().cloned().collect::<Vec<_>>(), "{from:?} to {to:?}: its old record goes later, not now");
                assert!(fake::device(|device| device.forgotten.is_empty()), "{from:?} to {to:?}: nothing forgotten until settings are on disk");

                // Nothing opens it from storage alone, or from what opened it before.
                assert_eq!(Vault::open(&entry.vault, "").err(), Some("wrong password".to_string()), "{from:?} to {to:?}");

                if to != Guard::Password {
                    assert_eq!(Vault::open(&entry.vault, NEW).err(), Some("wrong password".to_string()), "{from:?} to {to:?}");
                    assert_eq!(Vault::open(&entry.vault, PASSWORD).err(), Some("wrong password".to_string()), "{from:?} to {to:?}");
                }
            }
        }
    }

    /// Asserts a switch changed nothing kept, and that the old wallet still opens as it did.
    fn nothing_changed(world: &World, old: &WalletEntry, book: &Book, why: &str) {
        assert_eq!(world.settings.wallets[0], *old, "{why}");
        assert!(world.settings.retired.is_empty() && !world.settings.persist_now, "{why}");
        assert_eq!(opens(&world.settings.wallets[0], PASSWORD).unwrap(), *book, "{why}");
    }

    #[test]
    fn a_failure_at_any_step_of_a_switch_leaves_the_old_vault_opening_as_it_did() {
        for from in FROM {
            for to in [Guard::Biometric, Guard::Both] {
                if from == to {
                    continue;
                }

                let at = format!("{from:?} to {to:?}");
                let (mut world, mut book) = world(from);
                let old = world.settings.wallets[0].clone();
                let mut state = SettingsState::default();
                let keys = fake::device(|device| device.keys.len());

                // The current factors wrong, or the prompt for them turned down.
                tap(&mut world, &mut book, &mut state, button(to));
                match from {
                    Guard::Password | Guard::Either | Guard::Both => answer_first_step(&mut world, &mut book, &mut state, "not the password"),
                    Guard::Biometric => {
                        fake::device(|device| device.answers.push(fake::Answer::Canceled));
                        answer_first_step(&mut world, &mut book, &mut state, "");
                    }
                    Guard::Bare => answer_first_step(&mut world, &mut book, &mut state, ""),
                }

                if from != Guard::Bare {
                    assert!(matches!(state.switching.as_ref().unwrap().step, Step::Asking), "{at}: not past what opens it now");
                    nothing_changed(&world, &old, &book, &at);
                    answer_first_step(&mut world, &mut book, &mut state, PASSWORD);
                }

                assert!(matches!(state.switching.as_ref().unwrap().step, Step::Proved), "{at}");
                nothing_changed(&world, &old, &book, &at);

                // The new key's prompt turned down, then failing.
                fake::device(|device| device.answers.push(fake::Answer::Canceled));
                tap(&mut world, &mut book, &mut state, "Make it with fingerprint or face");
                assert!(matches!(state.switching.as_ref().unwrap().step, Step::Proved), "{at}");
                assert_eq!(fake::device(|device| device.keys.len()), keys, "{at}: no key made");
                fake::device(|device| device.answers.push(fake::Answer::Failed("the sensor is busy")));
                let shown = tap(&mut world, &mut book, &mut state, "Make it with fingerprint or face");
                assert!(shown.contains("the sensor is busy"), "{at}: {shown}");
                nothing_changed(&world, &old, &book, &at);

                // Made: then its check turned down, failing, and giving back something else.
                tap(&mut world, &mut book, &mut state, "Make it with fingerprint or face");
                assert!(matches!(state.switching.as_ref().unwrap().step, Step::Enrolled(..)), "{at}");
                assert_eq!(fake::device(|device| device.keys.len()), keys + 1, "{at}");
                nothing_changed(&world, &old, &book, &at);

                fake::device(|device| device.answers.push(fake::Answer::Canceled));
                tap(&mut world, &mut book, &mut state, "Check it with fingerprint or face");
                assert!(matches!(state.switching.as_ref().unwrap().step, Step::Enrolled(..)), "{at}: to try again");
                fake::device(|device| device.answers.push(fake::Answer::Failed("too many tries")));
                tap(&mut world, &mut book, &mut state, "Check it with fingerprint or face");
                assert!(matches!(state.switching.as_ref().unwrap().step, Step::Enrolled(..)), "{at}");
                nothing_changed(&world, &old, &book, &at);

                fake::device(|device| device.answers.push(fake::Answer::Wrong));
                let shown = tap(&mut world, &mut book, &mut state, "Check it with fingerprint or face");
                assert!(shown.contains("gave back something else"), "{at}: {shown}");
                assert!(matches!(state.switching.as_ref().unwrap().step, Step::Proved), "{at}");
                assert_eq!(fake::device(|device| device.keys.len()), keys, "{at}: the key that lied is gone");
                nothing_changed(&world, &old, &book, &at);

                // The key lost as soon as it was made.
                tap(&mut world, &mut book, &mut state, "Make it with fingerprint or face");
                let forgotten = fake::device(|device| {
                    let alias = format!("key{}", device.made);
                    device.keys.remove(&alias);

                    alias
                });
                tap(&mut world, &mut book, &mut state, "Check it with fingerprint or face");
                assert!(matches!(state.switching.as_ref().unwrap().step, Step::Proved), "{at}");
                assert!(fake::device(|device| device.forgotten.contains(&forgotten)), "{at}");
                nothing_changed(&world, &old, &book, &at);

                // Going back with a key made takes that key away, and only that key.
                tap(&mut world, &mut book, &mut state, "Make it with fingerprint or face");
                tap(&mut world, &mut book, &mut state, "Keep it as it is");
                assert!(state.switching.is_none(), "{at}");
                assert_eq!(fake::device(|device| device.keys.len()), keys, "{at}");
                nothing_changed(&world, &old, &book, &at);
            }
        }
    }

    #[test]
    fn leaving_for_a_password_takes_what_opens_it_now_and_failing_leaves_it_as_it_was() {
        for from in [Guard::Biometric, Guard::Both] {
            let at = format!("{from:?}");
            let (mut world, mut book) = world(from);
            let old = world.settings.wallets[0].clone();
            let mut state = SettingsState::default();
            tap(&mut world, &mut book, &mut state, "Use a password only");

            // Turned down, failing, and for Both the wrong password with the check passing.
            fake::device(|device| device.answers.push(fake::Answer::Canceled));
            answer_first_step(&mut world, &mut book, &mut state, PASSWORD);
            nothing_changed(&world, &old, &book, &at);
            fake::device(|device| device.answers.push(fake::Answer::Failed("too many tries")));
            let shown = tap(&mut world, &mut book, &mut state, "Continue");
            assert!(shown.contains("too many tries"), "{at}: {shown}");
            nothing_changed(&world, &old, &book, &at);

            if from == Guard::Both {
                answer_first_step(&mut world, &mut book, &mut state, "not the password");
                assert!(state.switching.is_some(), "{at}");
                nothing_changed(&world, &old, &book, &at);
            }

            // With its key gone it cannot prove anything, says why, and stays.
            let keys = fake::device(|device| std::mem::take(&mut device.keys));
            let shown = {
                state.switching.as_mut().unwrap().proof.error = None;
                answer_first_step(&mut world, &mut book, &mut state, PASSWORD);
                said(&mut world, &mut book, &mut state)
            };
            assert!(shown.contains("gone from this device"), "{at}: {shown}");
            assert_eq!(world.settings.wallets[0], old, "{at}");
            assert!(world.settings.retired.is_empty() && !world.settings.persist_now, "{at}");
            fake::device(|device| device.keys = keys);

            // And with it back, it goes through.
            answer_first_step(&mut world, &mut book, &mut state, PASSWORD);
            assert_eq!(world.settings.wallets[0].guard(), Guard::Password, "{at}");
            assert_eq!(opens(&world.settings.wallets[0], NEW).unwrap(), book, "{at}");
            assert_eq!(world.settings.wallets[0].biometric, None, "{at}");
            assert_eq!(world.settings.retired, old.biometric.iter().cloned().collect::<Vec<_>>(), "{at}");
        }
    }

    #[test]
    fn a_vault_that_will_not_seal_or_open_is_not_put_in_place() {
        let (mut world, mut book) = world(Guard::Password);
        let old = world.settings.wallets[0].clone();
        let mut state = SettingsState::default();
        tap(&mut world, &mut book, &mut state, "Require both");
        answer_first_step(&mut world, &mut book, &mut state, PASSWORD);
        tap(&mut world, &mut book, &mut state, "Make it with fingerprint or face");

        // Something it cannot seal, arriving before the last step.
        let keep = book.clone();
        book.phrases.clear();
        let shown = tap(&mut world, &mut book, &mut state, "Check it with fingerprint or face");

        assert!(shown.contains("does not hold"), "{shown}");
        assert!(matches!(state.switching.as_ref().unwrap().step, Step::Proved), "the new key is not used");
        assert_eq!(world.settings.wallets[0], old, "and all is as it was");
        assert_eq!(fake::device(|device| device.keys.len()), 0, "and the new key is gone");
        assert_eq!(opens(&world.settings.wallets[0], PASSWORD).unwrap(), keep);
    }

    #[test]
    fn a_new_password_is_needed_and_checked_before_anything_is_asked() {
        for to in [Guard::Password, Guard::Both] {
            let (mut world, mut book) = world(Guard::Biometric);
            let mut state = SettingsState::default();
            tap(&mut world, &mut book, &mut state, button(to));
            let switch = state.switching.as_mut().unwrap();
            switch.yes = "yes".to_string();
            switch.new = typed("short");
            switch.again = typed("short");
            let shown = tap(&mut world, &mut book, &mut state, "Continue");

            assert!(shown.contains("at least 8 characters"), "{shown}");

            let switch = state.switching.as_mut().unwrap();
            switch.new = typed(NEW);
            switch.again = typed("another one");
            let shown = tap(&mut world, &mut book, &mut state, "Continue");

            assert!(shown.contains("do not match"), "{shown}");
            assert_eq!(fake::device(|device| device.prompts), 0, "no prompt for what will not do");
        }
    }

    #[test]
    fn the_hint_and_the_typed_yes_come_before_any_step_that_needs_hardware() {
        let (mut world, mut book) = world(Guard::Password);
        book.add_phrase(Words::new("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string(), String::new())).unwrap();
        let mut state = SettingsState::default();
        let shown = tap(&mut world, &mut book, &mut state, "Use biometrics only");

        for words in ["Adding a fingerprint or face destroys", "restoring to another phone", "restore from your words", "all 2 of this wallet's seed phrases", "I have all 2 seed phrases of this wallet written down. Type yes:"] {
            assert!(shown.contains(words), "{words}: {shown}");
        }

        // Without a yes, it goes nowhere, whatever else is typed.
        let switch = state.switching.as_mut().unwrap();
        switch.proof.password = typed(PASSWORD);
        switch.yes = "y".to_string();
        tap(&mut world, &mut book, &mut state, "Continue");
        assert!(matches!(state.switching.as_ref().unwrap().step, Step::Asking), "not a yes");
        assert_eq!(fake::device(|device| device.prompts), 0);

        // Going to a password takes no yes: words are not at stake, and it says no such thing.
        let (mut world, mut book) = self::world(Guard::Biometric);
        let mut state = SettingsState::default();
        let shown = tap(&mut world, &mut book, &mut state, "Use a password only");

        assert!(!shown.contains("Type yes") && !shown.contains("restore from your words"), "{shown}");
    }

    #[test]
    fn a_wallet_with_keys_or_secrets_words_cannot_bring_back_is_refused_saying_which() {
        for (holds, says) in [
            (Some(Source::Key { key: chainv4::crypto::keypair::Keypair::development().to_key_file() }), "Brought in"),
            (None, "1 swap keeps its secret"),
        ] {
            let (mut world, mut book) = world(Guard::Bare);
            let old = world.settings.wallets[0].clone();

            match holds {
                Some(source) => book.accounts.push(StoredAccount { name: "Brought in".to_string(), source }),
                None => book.swaps.push(SwapRecord { terms: "t".to_string(), secret: Some(format!("0x{}", "ab".repeat(32))), account: "a".to_string(), made: 1, started: true, funding: None, multisig: None }),
            }

            for to in ["Use biometrics only", "Require both"] {
                let mut state = SettingsState::default();
                let shown = tap(&mut world, &mut book, &mut state, to);

                assert!(shown.contains(says), "{to}: {shown}");
                assert!(!shown.contains("Continue"), "{to}: and no way on: {shown}");

                tap(&mut world, &mut book, &mut state, "Back");
                assert!(state.switching.is_none());
                assert_eq!(world.settings.wallets[0], old);
            }

            // A password is still there to choose.
            let mut state = SettingsState::default();
            assert!(tap(&mut world, &mut book, &mut state, "Use a password only").contains("Continue"));
        }
    }

    #[test]
    fn a_platform_that_cannot_keep_a_key_offers_neither_mode_and_says_why() {
        for why in ["no strong biometrics here", "this browser's passkeys cannot release a key"] {
            let (mut world, mut book) = world(Guard::Password);
            fake::device(|device| device.bound = Availability::Unavailable(why.to_string()));
            let mut state = SettingsState::default();
            let shown = said(&mut world, &mut book, &mut state);

            assert!(shown.contains(why), "{shown}");
            assert!(!shown.contains("Use biometrics only") && !shown.contains("Require both"), "{shown}");
        }
    }

    #[test]
    fn a_wallet_of_today_keeps_what_it_has_with_no_way_of_turning_either_on_again() {
        let (mut world, mut book) = world(Guard::Either);
        let mut state = SettingsState::default();
        let shown = said(&mut world, &mut book, &mut state);

        assert!(shown.contains("Turn off fingerprint or face"), "{shown}");
        assert!(shown.contains("adds nothing against a copy"), "{shown}");

        tap(&mut world, &mut book, &mut state, "Turn off fingerprint or face");
        assert_eq!(world.settings.wallets[0].guard(), Guard::Password);
        assert_eq!(fake::device(|device| device.keys.len()), 0, "its key goes with it, as ever");

        let shown = said(&mut world, &mut book, &mut state);

        assert!(!shown.contains("Turn on"), "{shown}");
        assert!(!shown.contains("Turn off"), "{shown}");
    }

    #[test]
    fn every_step_and_every_way_of_opening_draws_on_every_screen() {
        for size in drawn::SCREENS {
            for guard in FROM {
                let (mut world, mut book) = world(guard);
                let mut holders = crate::keys::open(&book).unwrap();
                let mut state = SettingsState::default();
                let (width, _) = drawn::page_in(&mut world, &mut book, &mut holders, size, |ui, ctx| show(ui, ctx, &mut state));

                assert!(width <= size.0 + 0.5, "{guard:?}: {width} on a screen {} wide", size.0);

                for to in [Guard::Password, Guard::Biometric, Guard::Both] {
                    let secret = vault::fresh_secret().unwrap();

                    for step in [
                        Step::Asking,
                        Step::Proved,
                        Step::Enrolling(Enrolling::default(), secret.clone()),
                        Step::Enrolled(Enrolled("fake:drawn".to_string()), secret.clone()),
                        Step::Reading(Check::default(), Enrolled("fake:drawn".to_string()), secret.clone()),
                    ] {
                        let mut state = SettingsState { switching: Some(Switching { step, trouble: Some("Something went wrong, and nothing changed.".to_string()), ..Switching::new(to) }), ..SettingsState::default() };
                        let (width, changed) = drawn::page_in(&mut world, &mut book, &mut holders, size, |ui, ctx| show(ui, ctx, &mut state));

                        assert!(width <= size.0 + 0.5, "{guard:?} to {to:?}: {width} on a screen {} wide", size.0);
                        assert!(!changed, "{guard:?} to {to:?}");
                    }
                }
            }
        }
    }
}
