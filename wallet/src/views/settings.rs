//! The wallet open: its name, its words, and removing it. Then which node to talk to, the proxy
//! every request goes through, which chain to trust there, the light client's saved progress, and
//! how the wallet keeps its keys: when it locks itself, what opens it, and its password if it has
//! one.

mod opening;

use {
    super::Ctx,
    crate::app::Role,
    crate::biometrics::Proof,
    crate::format,
    crate::guard::{self, Factors, Guard, confirmed},
    crate::lock::{self, Typed},
    crate::theme,
    crate::vault::{Vault, Words},
    chainv4::crypto::mnemonic::Phrase,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::version::Upgrade,
    egui::{RichText, Ui},
};

#[derive(Default)]
pub struct SettingsState {
    reading: super::reading::ReadingState,
    /// The node being typed, and a genesis id to trust instead.
    node: Option<String>,
    genesis: String,
    /// A checkpoint's block id to start from, being typed.
    checkpoint: String,
    /// The proxy being typed, and why the one chosen would not do.
    proxy: Option<String>,
    proxy_trouble: Option<String>,
    current: Typed,
    new: Typed,
    again: Typed,
    /// What came of changing the password, and a change under way on a wallet that also needs
    /// hardware, waiting for its check.
    changed: Option<Result<String, String>>,
    changing: Option<Proof>,
    /// The wallet's name, being typed.
    name: Option<String>,
    /// Asking for what opens the wallet before a phrase's words show, by its place, and which one
    /// shows: one at a time.
    asking_words: Option<(usize, Proof)>,
    showing_words: Option<usize>,
    /// Words typed for a wallet that has none.
    words: Typed,
    words_error: Option<String>,
    /// A phrase being renamed, by its place, with its name as typed.
    naming_words: Option<(usize, String)>,
    /// A phrase being removed, once its preview is read.
    removing_words: Option<RemovingWords>,
    /// Clearing what the signer signed, once asked whether it means to.
    clearing_signed: bool,
    /// Asking for what opens the wallet before it is removed.
    removing: Option<Proof>,
    /// A switch to another way of opening under way, and what came of the last.
    switching: Option<opening::Switching>,
    opening_said: Option<Result<String, String>>,
}

/// A signer's history of what it signed: whether to keep one, and clearing it, asked twice. It is
/// sealed in the wallet with the keys, so it opens with what opens them.
fn signed_history(ui: &mut Ui, ctx: &mut Ctx, state: &mut SettingsState) {
    let palette = *ctx.palette;
    ui.add_space(10.0);
    theme::caps(ui, &palette, "History of what it signs");
    let mut keep = !ctx.book.signed_off;

    if ui.checkbox(&mut keep, "Keep a history on this signer").changed() {
        ctx.book.signed_off = !keep;
        *ctx.changed = true;
    }

    let held = ctx.book.signed.len();
    let told = match (keep, held) {
        (true, _) => "Each thing signed is noted on the Sign page, sealed in this wallet with its keys: the last 200, no key or signature, and what a message said left out.",
        (false, 0) => "Nothing is noted.",
        (false, _) => "Nothing more is noted. What is there stays until cleared.",
    };
    ui.label(RichText::new(told).small().color(palette.muted));

    if held == 0 {
        state.clearing_signed = false;

        return;
    }

    if !state.clearing_signed {
        if theme::quiet(ui, &palette, &format!("Clear the history ({held})")).clicked() {
            state.clearing_signed = true;
        }

        return;
    }

    ui.colored_label(palette.warn, format!("All {held} go from this device, for good."));
    ui.horizontal_wrapped(|ui| {
        if theme::quiet(ui, &palette, "Clear it").clicked() {
            ctx.book.signed.clear();
            *ctx.changed = true;
            state.clearing_signed = false;
        }

        if theme::quiet(ui, &palette, "Keep it").clicked() {
            state.clearing_signed = false;
        }
    });
}

/// Removing a further phrase: which, by its place, what is typed to confirm it, and what opens the
/// wallet.
#[derive(Default)]
struct RemovingWords {
    index: usize,
    confirm: String,
    proof: Proof,
    error: Option<String>,
}

impl SettingsState {
    /// Puts away the words, if shown, what was asked for them, a phrase's removal half asked, and a
    /// switch half done, whose new key, unused, nothing names.
    pub fn hide(&mut self) {
        self.showing_words = None;
        self.asking_words = None;
        self.removing_words = None;
        self.clearing_signed = false;
        self.switching = None;
        self.changing = None;
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SettingsState) {
    let palette = *ctx.palette;
    let node = ctx.settings.node.clone();

    theme::title(ui, &palette, "Settings", "This wallet, the node and the way to it, the chain it trusts there, and how it keeps your keys.");

    wallet(ui, ctx, state);
    words_question(ui, ctx, state);
    // An offline signer follows no node and no chain.
    if ctx.settings.role() != Role::Signer {
        online(ui, ctx, state, node);
        super::reading::card(ui, ctx, &mut state.reading);
        super::dapp::settings(ui, ctx);
        #[cfg(feature = "swap")]
        swaps(ui, ctx);
        #[cfg(feature = "nft")]
        nfts(ui, ctx);
        #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
        bitcoin(ui, ctx);
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "On this device");
        let role = ctx.settings.role();
        ui.label(RichText::new(match role {
            Role::Signer => format!("{}: it makes no network requests, and signs what the sender asks by QR code.", role.name()),
            _ => format!("{}.", role.name()),
        }).small().color(palette.muted));
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Theme").color(palette.soft));
            ui.selectable_value(&mut ctx.settings.light_theme, false, "Dark");
            ui.selectable_value(&mut ctx.settings.light_theme, true, "Light");
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Lock after").color(palette.soft));
            ui.add(egui::DragValue::new(&mut ctx.settings.lock_after).range(0..=240).suffix(" min"));
            ui.label(RichText::new(if ctx.settings.lock_after == 0 { "never, until you lock it" } else { "without input" }).small().color(palette.muted));
        });

        opening::show(ui, ctx, state);

        if role == Role::Signer {
            your_chain(ui, ctx);
            signed_history(ui, ctx, state);
        }

        ui.add_space(10.0);
        theme::caps(ui, &palette, "Password");
        let guard = ctx.settings.wallets.get(ctx.settings.wallet).map_or(Guard::Password, crate::app::WalletEntry::guard);

        if guard == Guard::Biometric {
            ui.label(RichText::new("This wallet has none: biometrics alone open it. Choosing to use a password, or to require both, is above.").small().color(palette.muted));

            return;
        }

        for (field, hint) in [(&mut state.current, "current password"), (&mut state.new, "new password"), (&mut state.again, "the new one again")] {
            crate::lock::secret(ui, egui::TextEdit::singleline(&mut **field).password(true).hint_text(hint).desired_width(f32::INFINITY));
        }

        if theme::quiet(ui, &palette, "Change password").clicked() && state.changing.is_none() {
            match guard {
                Guard::Both => start_changing_both(ctx, state),
                _ => state.changed = Some(change_password(ctx, state)),
            }
        }

        finish_changing_both(ctx, state);

        match &state.changed {
            Some(Ok(done)) => {
                ui.colored_label(palette.good, done);
            }
            Some(Err(why)) => {
                ui.colored_label(palette.bad, why);
            }
            None => {}
        }
    });
}

/// What an install that goes online sets: the node, the proxy, the chain it trusts there, and the
/// light client's saved progress.
/// Which node the wallet asks, and the proxy it asks through, before it opens: on the unlock
/// screen, so a wallet whose node is gone, or that needs Tor to reach it, is never stuck there.
/// The node to connect to afresh, once either changes.
pub fn reach(ui: &mut Ui, palette: &theme::Palette, settings: &mut crate::app::Settings, state: &mut SettingsState) -> Option<String> {
    let node = settings.node.clone();
    let typed = state.node.get_or_insert_with(|| node.clone());
    let mut connect = None;

    theme::caps(ui, palette, "Node");
    let other = typed.trim() != node;

    if super::beside(
        ui,
        |ui| theme::primary(ui, palette, "Connect", other).clicked(),
        |ui, width| {
            ui.add(egui::TextEdit::singleline(typed).font(egui::TextStyle::Monospace).desired_width(width));
        },
    ) {
        connect = Some(typed.trim().to_string());
    }

    ui.add_space(10.0);
    theme::caps(ui, palette, "Proxy");
    let set = settings.proxy.clone();
    let typed = state.proxy.get_or_insert_with(|| set.clone());

    if let Some(chosen) = chainv4_ui::widgets::proxy(ui, palette, typed, &mut state.proxy_trouble, &set) {
        settings.proxy = chosen;
        connect = connect.or(Some(node));
    }

    connect
}

fn online(ui: &mut Ui, ctx: &mut Ctx, state: &mut SettingsState, node: String) {
    let palette = *ctx.palette;
    let typed = state.node.get_or_insert_with(|| node.clone());

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Node");
        ui.label(RichText::new("Where the wallet asks. It is never trusted: what it sends shows only once it checks.").small().color(palette.muted));
        let other = typed.trim() != node;
        let connect = super::beside(
            ui,
            |ui| theme::primary(ui, &palette, "Connect", other).clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(typed).font(egui::TextStyle::Monospace).desired_width(width));
            },
        );

        if connect {
            *ctx.connect = Some(typed.trim().to_string());
        }

        if let Some(upgrade) = ctx.node_stops {
            node_stops(ui, ctx, upgrade);
        }
    });

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Proxy");
        let set = ctx.settings.proxy.clone();
        let typed = state.proxy.get_or_insert_with(|| set.clone());

        if let Some(chosen) = chainv4_ui::widgets::proxy(ui, &palette, typed, &mut state.proxy_trouble, &set) {
            ctx.settings.proxy = chosen;
            *ctx.connect = Some(node.clone());
        }
    });

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Trusted chain");
        let pinned = ctx.settings.pinned.get(&node).cloned();

        match &pinned {
            Some(genesis) => {
                ui.label(RichText::new("The genesis this wallet trusts at this node: the one it showed first, unless you gave another.").small().color(palette.muted));
                super::address(ui, &palette, genesis);
            }
            None => {
                ui.label(RichText::new("None yet: the wallet trusts the genesis the node shows first. Paste one below to trust that instead.").small().color(palette.muted));
            }
        }

        if let Some(trouble) = &ctx.chain.trouble {
            ui.add_space(4.0);
            ui.colored_label(palette.bad, trouble);
        }

        ui.add_space(8.0);
        let trust = super::beside(
            ui,
            |ui| theme::quiet(ui, &palette, "Trust").clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(&mut state.genesis).hint_text("a genesis id, 0x…, as someone you trust shows it").font(egui::TextStyle::Monospace).desired_width(width));
            },
        );

        if trust {
            match from_hex(state.genesis.trim()) {
                Some(bytes) if bytes.len() >= 16 => {
                    ctx.settings.pinned.insert(node.clone(), to_hex(&bytes));
                    state.genesis.clear();
                    *ctx.connect = Some(node.clone());
                }
                _ => state.genesis = "not a genesis id".to_string(),
            }
        }

        if pinned.is_some() && theme::quiet(ui, &palette, "Forget it").on_hover_text("Trust whatever genesis the node shows next, and start the light client over").clicked() {
            if let Some(genesis) = ctx.settings.pinned.remove(&node) {
                ctx.settings.light.remove(&genesis);
            }

            *ctx.connect = Some(node.clone());
        }

        ui.add_space(12.0);
        theme::caps(ui, &palette, "Start from a checkpoint");
        let checkpoint = ctx.settings.checkpoints.get(&node).cloned();
        let start_over = |settings: &mut crate::app::Settings| {
            if let Some(genesis) = settings.pinned.get(&node) {
                settings.light.remove(genesis);
            }
        };

        match &checkpoint {
            Some(checkpoint) => {
                ui.label(RichText::new("The light client starts at this block, trusted as the genesis is, rather than following from genesis:").small().color(palette.muted));
                super::address(ui, &palette, checkpoint);

                if theme::quiet(ui, &palette, "Forget it").on_hover_text("Follow from genesis, checking every block").clicked() {
                    ctx.settings.checkpoints.remove(&node);
                    start_over(ctx.settings);
                    *ctx.connect = Some(node.clone());
                }
            }
            None => {
                ui.label(
                    RichText::new("A finalized checkpoint's block id, as someone you trust shows it, starts the light client there: quicker than from genesis, trusting that block as you trust the genesis. Met for the first time with no genesis given, it starts at the node's own.")
                        .small()
                        .color(palette.muted),
                );
            }
        }

        ui.add_space(6.0);
        let start = super::beside(
            ui,
            |ui| theme::quiet(ui, &palette, "Start there").clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(&mut state.checkpoint).hint_text("a checkpoint's block id, 0x…").font(egui::TextStyle::Monospace).desired_width(width));
            },
        );

        if start {
            match from_hex(state.checkpoint.trim()) {
                Some(bytes) if bytes.len() >= 16 => {
                    ctx.settings.checkpoints.insert(node.clone(), to_hex(&bytes));
                    state.checkpoint.clear();
                    start_over(ctx.settings);
                    *ctx.connect = Some(node.clone());
                }
                _ => state.checkpoint = "not a block id".to_string(),
            }
        }
    });

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Light client");

        match ctx.chain.following() {
            Some((tip, final_height)) => {
                super::row(ui, &palette, "Followed to", |ui| {
                    ui.label(format::thousands(&tip.to_string()));
                });
                super::row(ui, &palette, "Final to", |ui| {
                    ui.label(format::thousands(&final_height.to_string()));
                });
            }
            None => super::waiting(ui, &palette, "Starting…"),
        }

        if let Some(said) = super::fast_resumed(ctx.chain) {
            ui.add(egui::Label::new(RichText::new(said).color(palette.warn)).wrap());
        }

        let saved = ctx.chain.genesis.and_then(|genesis| ctx.settings.light.get(&genesis.to_string())).map_or(0, String::len) / 2;
        super::row(ui, &palette, "Saved", |ui| {
            ui.label(format!("{} KiB, to resume from", format::thousands(&(saved / 1024).to_string())));
        });
        ui.add_space(6.0);
        fast_resume(ui, ctx, &node);
        ui.add_space(6.0);

        if theme::quiet(ui, &palette, "Start over").on_hover_text("Forget what it checked and follow the chain again from genesis").clicked() {
            if let Some(genesis) = ctx.chain.genesis {
                ctx.settings.light.remove(&genesis.to_string());
            }

            *ctx.connect = Some(node.clone());
        }
    });
}

/// The switch that trusts this node for fast resume: off until someone turns it on, and saying
/// plainly what that gives up. Turned on, it takes effect at once, as the wallet connects afresh.
fn fast_resume(ui: &mut Ui, ctx: &mut Ctx, node: &str) {
    let palette = *ctx.palette;
    let mut on = ctx.settings.fast_resume.get(node).copied().unwrap_or(false);

    let opening = format!("Opening the wallet skips the headers since it last ran, once it is more than {} epochs behind", chainv4::light::FEW_EPOCHS);

    if ctx.settings.read_blocks {
        ui.label(RichText::new("Fast resume is off while Read from checked blocks is on: that rests on what the node says, and this on nothing it says.").small().color(palette.muted));

        return;
    }

    if chainv4_ui::widgets::fast_resume_switch(ui, &palette, &mut on, &opening, "what the wallet checked before") {
        match on {
            true => {
                ctx.settings.fast_resume.insert(node.to_string(), true);
                *ctx.connect = Some(node.to_string());
            }
            false => {
                ctx.settings.fast_resume.remove(node);
            }
        }
    }
}

/// The upgrade that stops the node: the node's own chainv4 is older than the upgrade's least, so
/// it imports no block from the upgrade's height on, and the wallet follows it no further.
fn node_stops(ui: &mut Ui, ctx: &Ctx, upgrade: Upgrade) {
    let palette = *ctx.palette;
    let from = format::thousands(&upgrade.from.to_string());
    let reached = ctx.chain.following().is_some_and(|(tip, _)| tip + 1 >= upgrade.from);

    ui.add_space(6.0);
    ui.colored_label(palette.warn, match reached {
        true => format!("Your node stopped before block {from}: it needs updating to chainv4 {}.", upgrade.least),
        false => format!("Your node stops at block {from} unless updated to chainv4 {}.", upgrade.least),
    });
    ui.label(
        RichText::new("Past it, the node imports no blocks, and the wallet sees none. Until whoever runs it updates it, another node keeps you following the chain.")
            .small()
            .color(palette.muted),
    );
}

/// The wallet open: its name, its words, and removing it.
fn wallet(ui: &mut Ui, ctx: &mut Ctx, state: &mut SettingsState) {
    let palette = *ctx.palette;
    let Some(entry) = ctx.settings.wallets.get(ctx.settings.wallet) else {
        return;
    };
    let named = entry.name.clone();
    let others = ctx.settings.wallets.len() - 1;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Wallet");
        let name = state.name.get_or_insert_with(|| named.clone());
        let rename = super::beside(
            ui,
            |ui| theme::quiet(ui, &palette, "Rename").clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(name).desired_width(width));
            },
        );

        if rename
            && !name.trim().is_empty()
            && let Some(entry) = ctx.settings.wallets.get_mut(ctx.settings.wallet)
        {
            entry.name = name.trim().to_string();
        }
        let more = match others {
            0 => "The only wallet on this device. Add another from the lock screen.".to_string(),
            1 => "One more wallet on this device: pick it when unlocking.".to_string(),
            others => format!("{others} more wallets on this device: pick one when unlocking."),
        };
        ui.label(RichText::new(more).small().color(palette.muted));

        ui.add_space(10.0);
        theme::caps(ui, &palette, "Its words");

        match (ctx.book.watch, ctx.book.phrases.len()) {
            (true, _) => {
                ui.label(RichText::new("None here: a watch-only wallet holds public keys alone. The words are the signer's, and stay with it.").small().color(palette.muted));
            }
            (false, 1) => {
                ui.label(RichText::new("Every account made from them comes back from them, on any device, with any wallet that follows BIP39.").small().color(palette.muted));
                show_words(ui, ctx, state, 0);
            }
            (false, count) if count > 1 => several_words(ui, ctx, state),
            (false, _) => {
                ui.label(
                    RichText::new("This wallet has none: its accounts are keys brought in. Give it words, and new accounts come from them; the keys brought in stay as they are, with their own copies to keep.")
                        .small()
                        .color(palette.muted),
                );
                ui.add(egui::TextEdit::multiline(&mut *state.words).desired_rows(2).desired_width(f32::INFINITY).hint_text("words you have, or none for new ones"));
                ui.horizontal_wrapped(|ui| {
                    if theme::quiet(ui, &palette, if state.words.trim().is_empty() { "Make new words" } else { "Use these words" }).clicked() {
                        let phrase = match state.words.trim() {
                            "" => Phrase::generate(12).map_err(|why| why.to_string()),
                            typed => lock::read_words(typed),
                        };

                        match phrase {
                            Ok(phrase) => {
                                ctx.book.phrases.push(Words::new(phrase.to_string(), String::new()));
                                *ctx.changed = true;
                                // New ones are to be written down now; typed ones are known.
                                state.showing_words = state.words.trim().is_empty().then_some(0);
                                state.words.clear();
                                state.words_error = None;
                            }
                            Err(why) => state.words_error = Some(why),
                        }
                    }
                });

                if let Some(why) = &state.words_error {
                    ui.colored_label(palette.bad, why);
                }
            }
        }

        ui.add_space(10.0);

        match &mut state.removing {
            None => {
                if theme::quiet(ui, &palette, "Remove this wallet").clicked() {
                    state.removing = Some(Proof::default());
                }
            }
            Some(proof) => {
                ui.label(
                    RichText::new("Its words and keys leave this device. Without the words written down, or copies of its keys, what its accounts hold is out of reach for good.")
                        .color(palette.bad),
                );
                proof.field(ui, ctx.settings, "its password, to be sure");
                proof.trouble(ui, &palette);

                let mut keep = false;
                ui.horizontal_wrapped(|ui| {
                    if theme::primary(ui, &palette, "Remove it", proof.ready(ctx.settings)).clicked() {
                        proof.start(ctx.wake, ctx.settings, &format!("Remove {named}"));
                    }

                    keep = theme::quiet(ui, &palette, "Keep it").clicked();
                });

                if proof.take(ctx.settings).is_some() {
                    guard::remove_wallet(ctx.settings, ctx.settings.wallet);
                    *ctx.leave = true;
                }

                if keep {
                    state.removing = None;
                }
            }
        }
    });
}

/// A phrase's words, once what opens the wallet was given, or the button that asks for it.
fn show_words(ui: &mut Ui, ctx: &Ctx, state: &mut SettingsState, index: usize) {
    let palette = *ctx.palette;

    if state.showing_words != Some(index) {
        if theme::quiet(ui, &palette, "Show the words").clicked() {
            state.asking_words = Some((index, Proof::default()));
        }

        return;
    }

    let Some(words) = ctx.book.phrases.get(index) else {
        state.showing_words = None;

        return;
    };
    ui.add_space(6.0);

    if let Ok(phrase) = Phrase::parse(words.words()) {
        lock::words_grid(ui, &palette, &phrase);
    }

    if !words.passphrase().is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new(format!("With the passphrase: {}", words.passphrase())).monospace().color(palette.warn));
    }

    ui.add_space(6.0);

    if theme::quiet(ui, &palette, "Hide them").clicked() {
        state.showing_words = None;
    }
}

/// A wallet's phrases, when it has several: each one's name, fingerprint and accounts, to show one
/// at a time, rename, or remove. The first stays.
fn several_words(ui: &mut Ui, ctx: &mut Ctx, state: &mut SettingsState) {
    let palette = *ctx.palette;
    ui.label(RichText::new("Each brings back the accounts made from it, on any device, with any wallet that follows BIP39. Write down every one: none brings back another's.").small().color(palette.muted));

    for index in 0..ctx.book.phrases.len() {
        ui.add_space(8.0);
        let from = ctx.book.phrases[index].fingerprint().filter(|_| index > 0);
        let fingerprint = ctx.book.phrases[index].fingerprint().map(|fingerprint| to_hex(&fingerprint).trim_start_matches("0x").to_string()).unwrap_or_default();
        let accounts = ctx.holders.iter().filter(|holder| holder.is_of(from)).count();
        ui.horizontal_wrapped(|ui| {
            ui.label(theme::strong(ctx.book.name_of(index), 14.5).color(palette.ink));
            ui.label(theme::mono(fingerprint).small().color(palette.muted));
            ui.label(RichText::new(if accounts == 1 { "1 account".to_string() } else { format!("{accounts} accounts") }).small().color(palette.muted));
        });

        if let Some((_, name)) = state.naming_words.as_mut().filter(|(naming, _)| *naming == index) {
            let done = super::beside(
                ui,
                |ui| theme::quiet(ui, &palette, "Done").clicked(),
                |ui, width| {
                    ui.add(egui::TextEdit::singleline(name).desired_width(width));
                },
            );

            if done {
                if !name.trim().is_empty() {
                    ctx.book.phrases[index].set_name(name);
                    *ctx.changed = true;
                }

                state.naming_words = None;
            }
        }

        show_words(ui, ctx, state, index);
        ui.horizontal_wrapped(|ui| {
            if theme::quiet(ui, &palette, "Rename").clicked() {
                state.naming_words = Some((index, ctx.book.name_of(index)));
            }

            // The first is what an account naming none comes from: only removing the wallet takes it.
            if index > 0 && theme::quiet(ui, &palette, "Remove these words").clicked() {
                state.removing_words = Some(RemovingWords { index, ..RemovingWords::default() });
            }
        });

        if state.removing_words.as_ref().is_some_and(|removing| removing.index == index) {
            let before = ctx.book.phrases.len();
            remove_words(ui, ctx, state, index);

            // Gone: the places after it are other phrases' now, so the rest are drawn next frame.
            if ctx.book.phrases.len() != before {
                break;
            }
        }
    }
}

/// What removing a phrase takes, shown first; then a typed yes and what opens the wallet before it
/// goes.
fn remove_words(ui: &mut Ui, ctx: &mut Ctx, state: &mut SettingsState, index: usize) {
    let palette = *ctx.palette;
    let Some(removing) = &mut state.removing_words else {
        return;
    };
    let preview = match crate::keys::removal(ctx.book, ctx.holders, index) {
        Ok(preview) => preview,
        Err(why) => {
            ui.colored_label(palette.bad, why);

            return;
        }
    };

    ui.add_space(6.0);
    ui.label(RichText::new("These words leave this device, with every account made from them. Without the words written down, what those accounts hold is out of reach for good.").color(palette.bad));

    match preview.accounts.is_empty() {
        true => {
            ui.label(RichText::new("No account in this wallet comes from them now.").small().color(palette.soft));
        }
        false => {
            for (name, id) in &preview.accounts {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(name).color(palette.ink));
                    ui.label(theme::mono(format::short(id)).small().color(palette.muted));
                });
            }
        }
    }

    if preview.swaps > 0 {
        ui.colored_label(palette.warn, format!("{} swap{} kept with those accounts stay listed, but nothing here can spend for them any more.", preview.swaps, if preview.swaps == 1 { "" } else { "s" }));
    }

    ui.add(egui::TextEdit::singleline(&mut removing.confirm).hint_text("type yes").desired_width(f32::INFINITY));
    removing.proof.field(ui, ctx.settings, "the wallet's password");
    removing.proof.trouble(ui, &palette);

    if let Some(error) = &removing.error {
        ui.colored_label(palette.bad, error.as_str());
    }

    let (mut go, mut keep) = (false, false);
    let ready = confirmed(&removing.confirm) && removing.proof.ready(ctx.settings);
    ui.horizontal_wrapped(|ui| {
        go = theme::primary(ui, &palette, "Remove them", ready).clicked();
        keep = theme::quiet(ui, &palette, "Keep them").clicked();
    });

    if go && ready {
        removing.proof.start(ctx.wake, ctx.settings, "Remove these words");
    }

    if removing.proof.take(ctx.settings).is_some() {
        match crate::keys::remove_phrase(ctx.book, ctx.holders, index) {
            Ok(()) => {
                // The one in use moves to the first left if it went.
                if !ctx.holders.iter().any(|holder| holder.id() == ctx.settings.current()) {
                    ctx.settings.set_current(ctx.holders.first().map(crate::keys::Holder::id).unwrap_or_default());
                }

                *ctx.changed = true;
                state.showing_words = None;
                state.naming_words = None;
                state.removing_words = None;
            }
            Err(why) => removing.error = Some(why),
        }
    } else if keep {
        state.removing_words = None;
    }
}

/// Asks, in a box over the page, for what opens the wallet before the words show.
fn words_question(ui: &mut Ui, ctx: &Ctx, state: &mut SettingsState) {
    let Some((index, proof)) = &mut state.asking_words else {
        return;
    };
    let index = *index;
    let palette = *ctx.palette;
    let mut answer = None;

    let modal = egui::Modal::new(ui.make_persistent_id("show words"))
        .frame(egui::Frame::new().fill(palette.card).stroke(egui::Stroke::new(1.0, palette.line)).corner_radius(12.0).inner_margin(egui::Margin::same(20)))
        .show(ui.ctx(), |ui| {
            if chainv4_ui::layout::fingers(ui.ctx()) {
                chainv4_ui::layout::finger_sized(ui);
            }

            ui.set_max_width(360.0);
            theme::caps(ui, &palette, "Show the words?");
            ui.add_space(4.0);
            ui.label(RichText::new("Anyone who sees them can take everything every account made from them holds, now and later.").color(palette.ink));
            ui.label(RichText::new("Show them only to write them down, with no one watching your screen and nothing recording it.").small().color(palette.muted));
            ui.add_space(8.0);
            let enter = proof.field(ui, ctx.settings, "the wallet's password");
            proof.trouble(ui, &palette);
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, &palette, "Show them", proof.ready(ctx.settings)).clicked() || enter {
                    answer = Some(true);
                }

                if theme::quiet(ui, &palette, "Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });

    match answer {
        Some(true) if proof.ready(ctx.settings) => proof.start(ctx.wake, ctx.settings, "Show the words"),
        Some(true) => {}
        Some(false) => {
            state.asking_words = None;

            return;
        }
        None if modal.should_close() => {
            state.asking_words = None;

            return;
        }
        None => {}
    }

    if proof.take(ctx.settings).is_some() {
        state.showing_words = Some(index);
        state.asking_words = None;
    }
}

/// Whether the password opens the wallet open, for one with a password alone or with biometrics as
/// an alternative.
fn opens(ctx: &Ctx, password: &str) -> bool {
    ctx.settings.wallets.get(ctx.settings.wallet).is_some_and(|entry| Vault::open(&entry.vault, password).is_ok())
}

/// Seals the accounts with a new password, or none, once the current one opens them.
fn change_password(ctx: &mut Ctx, state: &mut SettingsState) -> Result<String, String> {
    if !opens(ctx, &state.current) {
        return Err("the current password is not right".to_string());
    }

    if !state.new.is_empty() && state.new.len() < 8 {
        return Err("use at least 8 characters, or none".to_string());
    }

    if state.new != state.again {
        return Err("the two new ones do not match".to_string());
    }

    *ctx.vault = Some(Vault::create(&state.new)?);
    *ctx.changed = true;
    let none = state.new.is_empty();

    // The key the old password stretched to opens nothing now. Retire its record rather than forget
    // it at once, as the mode switch does: settings on disk still name it until the new seal is
    // written, so a kill in between strands nothing, and retire_all forgets it once settings that no
    // longer name it have been read back. Have settings written now, not at the next autosave, so
    // the new seal reaches disk before anything relies on it. A hardware wallet does not come here.
    let old = ctx.settings.wallets.get_mut(ctx.settings.wallet).and_then(|entry| {
        entry.unprotected = none;

        entry.biometric.take()
    });
    let had = old.is_some();
    ctx.settings.retired.extend(old);
    ctx.settings.persist_now = true;
    state.current.clear();
    state.new.clear();
    state.again.clear();

    let sealed = match none {
        true => "No password now: it opens with a tap, for anyone who can read this app's storage.",
        false => "Sealed with the new password.",
    };

    Ok(match had {
        true => format!("{sealed} Biometrics are off: choose a mode above to use them again."),
        false => sealed.to_string(),
    })
}

/// Asks for the password and the check of a wallet that needs both, once the new password will do.
fn start_changing_both(ctx: &Ctx, state: &mut SettingsState) {
    if state.new.len() < 8 {
        state.changed = Some(Err("use at least 8 characters: this wallet needs its password".to_string()));
    } else if state.new != state.again {
        state.changed = Some(Err("the two new ones do not match".to_string()));
    } else {
        let mut proof = Proof::default();
        proof.password = state.current.clone();
        proof.start(ctx.wake, ctx.settings, "Change the password");
        state.changing = Some(proof);
    }
}

/// The new password of a wallet that needs both, once the check gave back the secret it holds: its
/// key made anew from the two, the vault sealed under it and opened again, before it replaces the
/// old one. The secret stays what it was, so the same hardware key goes on opening it. Whatever
/// fails leaves it as it was.
fn finish_changing_both(ctx: &mut Ctx, state: &mut SettingsState) {
    let Some(proof) = &mut state.changing else {
        return;
    };

    if let Some(proven) = proof.take(ctx.settings) {
        state.changing = None;
        let record = ctx.settings.wallets.get(ctx.settings.wallet).and_then(|entry| entry.biometric.clone());
        let replaced = proven.secret.ok_or("the check gave no secret".to_string()).and_then(|secret| {
            let (opened, sealed) = guard::sealed_anew(&Factors::Both(&state.new, &secret), ctx.book)?;
            guard::adopt(ctx.settings, sealed, record)?;
            *ctx.vault = Some(opened);

            Ok(())
        });

        state.changed = Some(replaced.map(|()| {
            state.current.clear();
            state.new.clear();
            state.again.clear();

            "Sealed with the new password, and the same hardware key.".to_string()
        }));
    } else if let Some(why) = proof.error.take() {
        state.changing = None;
        state.changed = Some(Err(why));
    } else if !proof.waiting() {
        // Turned down: nothing happened.
        state.changing = None;
    }
}

/// An offline signer's word for which chain is its: MetaMask's requests for it are named so, and
/// any other's are warned of; this wallet's own for any other are refused.
fn your_chain(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    let id = egui::Id::unique("your chain typed");
    let mut typed = ui.data(|data| data.get_temp::<String>(id)).unwrap_or_else(|| ctx.settings.signer_chain.map(|chain| chain.to_string()).unwrap_or_default());

    ui.add_space(10.0);
    theme::caps(ui, &palette, "Your chain");
    ui.label(
        RichText::new("Its Ethereum chain id, as GET /status shows chain_id: requests for it read as your chain. A sender's for any other are refused, and MetaMask's warned of. Empty for none.")
            .small()
            .color(palette.muted),
    );
    ui.add(egui::TextEdit::singleline(&mut typed).hint_text("38262171930895").font(egui::TextStyle::Monospace).desired_width(220.0));

    match typed.trim() {
        "" => ctx.settings.signer_chain = None,
        text => match text.parse::<u64>() {
            Ok(chain) => ctx.settings.signer_chain = Some(chain),
            Err(_) => {
                ui.colored_label(palette.bad, "A chain id is a whole number.");
            }
        },
    }

    ui.data_mut(|data| data.insert_temp(id, typed));
}

/// The Swaps page's settings: the swap contract, when not the node's, an Esplora API to look
/// Bitcoin up at, and EVM chains' JSON-RPC.
#[cfg(feature = "swap")]
fn swaps(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Swaps");
        ui.label(RichText::new("The swap contract, empty for the one the node names, and an Esplora API, such as https://mempool.space/api or https://mempool.space/testnet4/api, for the Swaps page to look Bitcoin up at and broadcast through. Empty sends nothing anywhere: you paste what it needs and broadcast yourself.").small().color(palette.muted));
        ui.add_space(6.0);
        super::row(ui, &palette, "Contract", |ui| {
            ui.add(egui::TextEdit::singleline(&mut ctx.settings.swap_contract).hint_text(ctx.swap_contract.unwrap_or("contract:0x…")).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        });

        if !ctx.settings.swap_contract.trim().is_empty() && ctx.settings.swap_contract.trim().parse::<chainv4::structs::address::Address>().is_err() {
            ui.colored_label(palette.bad, "That is not an address.");
        }

        super::row(ui, &palette, "Esplora", |ui| {
            ui.add(egui::TextEdit::singleline(&mut ctx.settings.esplora).hint_text("empty: nothing leaves the wallet").desired_width(f32::INFINITY));
        });

        if ctx.settings.esplora.trim().starts_with("http://") {
            ui.colored_label(palette.warn, "Plain http: anyone on the way sees your swap's addresses.");
        }

        super::row(ui, &palette, "Bitcoin depth", |ui| {
            ui.add(egui::DragValue::new(&mut ctx.settings.bitcoin_depth).range(1..=100));
            ui.label(RichText::new("confirmations before a lock on Bitcoin is acted on").small().color(palette.muted));
        });

        ui.add_space(8.0);
        ui.label(RichText::new("EVM chains' JSON-RPC, one URL a line, such as https://ethereum-sepolia-rpc.publicnode.com: a swap's leg on Ethereum, Sepolia or any EVM chain is read and sent through the one whose chain it is. Empty: such legs cannot be followed here.").small().color(palette.muted));
        super::row(ui, &palette, "EVM RPC", |ui| {
            ui.add(egui::TextEdit::multiline(&mut ctx.settings.eth_rpc).hint_text("https://…").desired_rows(2).desired_width(f32::INFINITY));
        });

        if ctx.settings.eth_rpc.lines().any(|url| url.trim().starts_with("http://") && !url.contains("127.0.0.1") && !url.contains("localhost")) {
            ui.colored_label(palette.warn, "Plain http: anyone on the way sees and could change what goes through it.");
        }
    });
}

/// The NFTs page's settings: whether it fetches tokens' metadata and pictures, and the IPFS
/// gateway it fetches ipfs:// through.
#[cfg(feature = "nft")]
fn nfts(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "NFTs");
        ui.label(RichText::new("What an NFT is called and looks like lives where its token URI points: a web server or IPFS, which learn your IP address and which tokens you look at when the wallet fetches from them. Off, the NFTs page shows ids and owners alone, all of it proven; data: URIs, carried on the chain itself, show either way.").small().color(palette.muted));
        ui.add_space(6.0);
        ui.checkbox(&mut ctx.settings.nft_media, "Fetch NFTs' metadata and pictures");
        super::row(ui, &palette, "IPFS gateway", |ui| {
            ui.add(egui::TextEdit::singleline(&mut ctx.settings.ipfs_gateway).hint_text("https://ipfs.io/ipfs/").desired_width(f32::INFINITY));
        });

        if !ctx.settings.ipfs_gateway.trim().is_empty() && !ctx.settings.ipfs_gateway.trim().ends_with('/') {
            ui.colored_label(palette.warn, "A gateway ends in a slash: ipfs://CID is fetched as the gateway, then CID.");
        }
    });
}

/// The Bitcoin light client's settings: which networks it follows, a peer to try first, and how
/// each is doing.
#[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
fn bitcoin(ui: &mut Ui, ctx: &mut Ctx) {
    use crate::light::Net;

    let palette = *ctx.palette;
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Bitcoin");
        ui.label(RichText::new("A light client that asks Bitcoin's own peers for each block's compact filter, and fetches only the blocks that pay or spend your addresses and your swaps' HTLCs: no server learns which are yours. A first sync takes some minutes and a few tens of megabytes; later ones, the blocks since. Through the proxy above when one is set, SOCKS5 alone.").small().color(palette.muted));
        ui.add_space(6.0);
        ui.checkbox(&mut ctx.settings.bitcoin_main, "Follow Bitcoin");
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut ctx.settings.bitcoin_test, "Follow its test network:");
            chainv4_ui::widgets::together(ui, &["testnet4", "signet"]);
            ui.selectable_value(&mut ctx.settings.bitcoin_test_network, "testnet4".to_string(), "testnet4");
            ui.selectable_value(&mut ctx.settings.bitcoin_test_network, "signet".to_string(), "signet");
        });
        super::row(ui, &palette, "Peer", |ui| {
            ui.add(egui::TextEdit::singleline(&mut ctx.settings.bitcoin_peer).hint_text("empty: Bitcoin's DNS seeds; or host:port, such as your own node").desired_width(f32::INFINITY));
        });

        if let Some(trouble) = &ctx.lights.trouble {
            ui.colored_label(palette.bad, trouble);
        }

        for testnet in [false, true] {
            let Some(light) = ctx.lights.on(testnet) else {
                continue;
            };
            let (status, seen, net) = (light.status(), light.seen(), light.net);
            let line = match (&status.stopped, status.synced, status.connected) {
                (Some(why), ..) => format!("stopped: {why}"),
                (None, true, _) => format!("synced to block {}", status.headers.max(seen.height)),
                (None, false, true) => format!("syncing: filters to {} of {}", status.filters.max(seen.height), status.headers),
                (None, false, false) => "finding peers…".to_string(),
            };
            super::row(ui, &palette, net.name(), |ui| {
                ui.label(RichText::new(line).color(palette.ink));

                if ui.small_button("Scan again").on_hover_text("Forget what it found, and sync again from the checkpoint built in.").clicked() {
                    ctx.lights.rescan(testnet);
                }
            });

            if let Some(warning) = status.warning.filter(|_| !status.synced) {
                ui.label(RichText::new(warning).small().color(palette.warn));
            }

            if net == Net::Main && ctx.settings.proxy.trim().is_empty() {
                ui.label(RichText::new("Without a proxy, Bitcoin's peers see your IP address, though not which addresses are yours.").small().color(palette.muted));
            }
        }
    });
}


#[cfg(test)]
mod tests {
    use {super::*, crate::vault::Book};

    #[test]
    fn a_signers_history_turns_off_and_clears_only_when_asked_twice() {
        use crate::views::drawn;

        let mut book = Book::default();
        book.note_signed(1, "Send", "1 CV4", "ed25519:0x01");
        let mut holders = Vec::new();
        let mut state = SettingsState::default();

        // The first tap only asks: nothing has gone yet.
        drawn::tap(&mut book, &mut holders, (360.0, 740.0), "Clear the history (1)", |ui, ctx| signed_history(ui, ctx, &mut state));
        assert!(state.clearing_signed);
        assert_eq!(book.signed.len(), 1);

        let said = drawn::said(&mut book, &mut holders, (360.0, 740.0), |ui, ctx| signed_history(ui, ctx, &mut state));
        assert!(said.contains("for good"), "{said}");

        drawn::tap(&mut book, &mut holders, (360.0, 740.0), "Clear it", |ui, ctx| signed_history(ui, ctx, &mut state));
        assert!(book.signed.is_empty());
        assert!(!state.clearing_signed);

        drawn::tap(&mut book, &mut holders, (360.0, 740.0), "Keep a history on this signer", |ui, ctx| signed_history(ui, ctx, &mut state));
        assert!(book.signed_off, "off");
        assert!(!book.note_signed(2, "Send", "not this", "ed25519:0x01"));
    }

    #[test]
    fn only_a_typed_yes_removes_a_phrase() {
        assert!(confirmed("yes") && confirmed(" YES "));

        for typed in ["", "y", "yes please", "no", "ye s"] {
            assert!(!confirmed(typed), "{typed}");
        }
    }

    #[test]
    fn several_phrases_draw_on_every_screen_and_nothing_goes_unasked() {
        for size in crate::views::drawn::SCREENS {
            for (showing, naming, removing) in [(None, None, None), (Some(1), None, None), (None, Some((1, "Savings".to_string())), None), (None, None, Some(1)), (Some(0), None, Some(1))] {
                let mut book = crate::views::drawn::two_phrases();
                let mut holders = crate::keys::open(&book).unwrap();
                let before = book.clone();
                let mut state = SettingsState {
                    showing_words: showing,
                    naming_words: naming,
                    removing_words: removing.map(|index| RemovingWords { index, confirm: "yes".to_string(), ..RemovingWords::default() }),
                    ..SettingsState::default()
                };
                let (width, changed) = crate::views::drawn::page(&mut book, &mut holders, size, |ui, ctx| wallet(ui, ctx, &mut state));

                assert!(width <= size.0 + 0.5, "{width} on a screen {} wide", size.0);
                assert!(!changed, "a typed yes alone, with no tap, removes nothing");
                assert_eq!((&book, holders.len()), (&before, 2));
                assert_eq!(state.showing_words, showing, "shown until hidden");
            }
        }
    }

    #[test]
    fn a_wallet_of_one_phrase_shows_as_it_did_and_stale_places_ask_nothing() {
        let mut one = Book {
            phrases: vec![Words::new("test test test test test test test test test test test junk".to_string(), String::new())],
            ..Book::default()
        };
        let mut holders = Vec::new();
        // A place past the end, from words since removed, shows nothing and breaks nothing.
        let mut state = SettingsState { showing_words: Some(4), ..SettingsState::default() };

        crate::views::drawn::page(&mut one, &mut holders, (360.0, 740.0), |ui, ctx| wallet(ui, ctx, &mut state));

        assert_eq!(one.phrases.len(), 1);

        state.showing_words = Some(0);
        crate::views::drawn::page(&mut one, &mut holders, (360.0, 740.0), |ui, ctx| wallet(ui, ctx, &mut state));

        assert_eq!(state.showing_words, Some(0));

        state.hide();

        assert_eq!((state.showing_words, state.asking_words.is_none(), state.removing_words.is_none()), (None, true, true), "out of sight as the page goes");
    }

    #[test]
    fn the_unlock_screen_offers_the_node_and_proxy_and_leaves_them_untouched_until_asked() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let palette = theme::DARK;
        let mut settings = crate::app::Settings::default();
        settings.node = "http://127.0.0.1:8090".to_string();
        settings.proxy = "socks5h://127.0.0.1:9050".to_string();
        let mut state = SettingsState::default();
        let mut connect = Some(String::new());
        let mut drawn = ctx.run_ui(egui::RawInput::default(), |ui| connect = reach(ui, &palette, &mut settings, &mut state));
        drawn.textures_delta.clear();

        assert_eq!(connect, None, "nothing typed, nothing chosen");
        assert_eq!(state.node.as_deref(), Some("http://127.0.0.1:8090"), "the field starts at the node set");
        assert_eq!(state.proxy.as_deref(), Some("socks5h://127.0.0.1:9050"));
        assert_eq!((settings.node.as_str(), settings.proxy.as_str()), ("http://127.0.0.1:8090", "socks5h://127.0.0.1:9050"));
    }

    /// What opens every kind of wallet, for what asks for it on these pages.
    mod guarded {
        use {
            super::*,
            crate::app::{Settings, WalletEntry},
            crate::biometrics::fake,
            crate::vault::{Sealed, fresh_secret},
            crate::views::drawn::{self, World},
        };

        const PHONE: (f32, f32) = (360.0, 740.0);

        fn entry(vault: Sealed, biometric: Option<String>) -> WalletEntry {
            WalletEntry { id: String::new(), name: "Mine".to_string(), vault, current: String::new(), biometric, unprotected: false, born: None }
        }

        /// A wallet of this guard, open, with two phrases.
        fn needing(guard: Guard) -> (World, Book, zeroize::Zeroizing<[u8; 32]>) {
            fake::fresh();
            let book = drawn::two_phrases();
            let secret = fresh_secret().unwrap();
            let vault = match guard {
                Guard::Both => Vault::both("correct horse", &secret).unwrap(),
                Guard::Biometric => Vault::keyed(&secret).unwrap(),
                _ => Vault::create("correct horse").unwrap(),
            };
            let sealed = vault.seal(&book).unwrap();
            let record = if guard.needs_hardware() { guard::tagged(guard, &fake::keep(&secret)) } else { None };

            (World { settings: Settings::with_wallets(vec![entry(sealed, record)]), vault: Some(vault), ..World::default() }, book, secret)
        }

        #[test]
        fn removing_a_wallet_asks_for_what_opens_it_and_nothing_less() {
            for guard in [Guard::Password, Guard::Biometric, Guard::Both] {
                let (mut world, mut book, _) = needing(guard);
                let mut state = SettingsState::default();
                let mut holders = crate::keys::open(&book).unwrap();
                drawn::tap_in(&mut world, &mut book, &mut holders, PHONE, "Remove this wallet", |ui, ctx| wallet(ui, ctx, &mut state));
                let said = drawn::said_in(&mut world, &mut book, &mut holders, PHONE, |ui, ctx| wallet(ui, ctx, &mut state));

                assert!(said.contains("Remove it"), "{guard:?}: {said}");

                // Turned down, or with the wrong password, it stays.
                match guard {
                    Guard::Biometric => fake::device(|device| device.answers.push(fake::Answer::Canceled)),
                    _ => state.removing.as_mut().unwrap().password = Typed::new("not the password".to_string()),
                }

                drawn::tap_in(&mut world, &mut book, &mut holders, PHONE, "Remove it", |ui, ctx| wallet(ui, ctx, &mut state));
                assert_eq!(world.settings.wallets.len(), 1, "{guard:?}");

                state.removing.as_mut().unwrap().password = Typed::new("correct horse".to_string());
                drawn::tap_in(&mut world, &mut book, &mut holders, PHONE, "Remove it", |ui, ctx| wallet(ui, ctx, &mut state));

                assert!(world.settings.wallets.is_empty(), "{guard:?}");
                assert!(world.leave, "{guard:?}");
                assert_eq!(world.settings.retired.len(), usize::from(guard.needs_hardware()), "{guard:?}: its key forgotten once settings say so");
            }
        }

        #[test]
        fn the_words_show_only_for_what_opens_the_wallet() {
            for guard in [Guard::Password, Guard::Biometric, Guard::Both] {
                let (mut world, mut book, _) = needing(guard);
                let mut state = SettingsState { asking_words: Some((0, Proof::default())), ..SettingsState::default() };
                let mut holders = crate::keys::open(&book).unwrap();

                match guard {
                    Guard::Biometric => fake::device(|device| device.answers.push(fake::Answer::Failed("too many tries"))),
                    _ => state.asking_words.as_mut().unwrap().1.password = Typed::new("not the password".to_string()),
                }

                let said = drawn::tap_in(&mut world, &mut book, &mut holders, PHONE, "Show them", |ui, ctx| words_question(ui, ctx, &mut state));

                assert_eq!(state.showing_words, None, "{guard:?}: {said}");

                state.asking_words.as_mut().unwrap().1.password = Typed::new("correct horse".to_string());
                drawn::tap_in(&mut world, &mut book, &mut holders, PHONE, "Show them", |ui, ctx| words_question(ui, ctx, &mut state));

                assert_eq!(state.showing_words, Some(0), "{guard:?}");
                assert!(state.asking_words.is_none(), "{guard:?}");
            }
        }

        #[test]
        fn removing_a_phrase_takes_a_yes_and_what_opens_the_wallet() {
            for guard in [Guard::Password, Guard::Biometric, Guard::Both] {
                let (mut world, mut book, _) = needing(guard);
                let mut holders = crate::keys::open(&book).unwrap();
                let mut state = SettingsState { removing_words: Some(RemovingWords { index: 1, confirm: "yes".to_string(), ..RemovingWords::default() }), ..SettingsState::default() };

                if guard == Guard::Biometric {
                    fake::device(|device| device.answers.push(fake::Answer::Canceled));
                } else {
                    state.removing_words.as_mut().unwrap().proof.password = Typed::new("not it".to_string());
                }

                drawn::tap_in(&mut world, &mut book, &mut holders, PHONE, "Remove them", |ui, ctx| remove_words(ui, ctx, &mut state, 1));
                assert_eq!(book.phrases.len(), 2, "{guard:?}");

                state.removing_words.as_mut().unwrap().proof.password = Typed::new("correct horse".to_string());
                drawn::tap_in(&mut world, &mut book, &mut holders, PHONE, "Remove them", |ui, ctx| remove_words(ui, ctx, &mut state, 1));
                assert_eq!(book.phrases.len(), 1, "{guard:?}");
            }
        }

        #[test]
        fn changing_the_password_of_a_wallet_needing_both_keeps_the_hardware_key_and_checks_first() {
            let (mut world, mut book, secret) = needing(Guard::Both);
            let record = world.settings.wallets[0].biometric.clone();
            let old = world.settings.wallets[0].clone();
            let mut state = SettingsState::default();
            let (type_in, new) = (|state: &mut SettingsState, current: &str, new: &str, again: &str| {
                state.current = Typed::new(current.to_string());
                state.new = Typed::new(new.to_string());
                state.again = Typed::new(again.to_string());
            }, "battery staple");

            let draw = |world: &mut World, book: &mut Book, state: &mut SettingsState| drawn::tap_in(world, book, &mut Vec::new(), PHONE, "Change", |ui, ctx| {
                if theme::quiet(ui, ctx.palette, "Change").clicked() {
                    start_changing_both(ctx, state);
                }

                finish_changing_both(ctx, state);
            });

            // None at all is refused: it would no longer need both.
            type_in(&mut state, "correct horse", "", "");
            draw(&mut world, &mut book, &mut state);
            assert!(state.changed.take().unwrap().unwrap_err().contains("at least 8"));

            type_in(&mut state, "correct horse", new, "something else");
            draw(&mut world, &mut book, &mut state);
            assert!(state.changed.take().unwrap().unwrap_err().contains("do not match"));
            assert_eq!(fake::device(|device| device.prompts), 0);

            // The wrong current one, the check passing, and the check turned down.
            type_in(&mut state, "not the password", new, new);
            draw(&mut world, &mut book, &mut state);
            assert!(state.changed.take().unwrap().is_err());
            assert_eq!(world.settings.wallets[0], old);

            type_in(&mut state, "correct horse", new, new);
            fake::device(|device| device.answers.push(fake::Answer::Canceled));
            draw(&mut world, &mut book, &mut state);
            assert_eq!(world.settings.wallets[0], old, "turned down");

            draw(&mut world, &mut book, &mut state);
            assert!(state.changed.take().unwrap().is_ok());

            let entry = &world.settings.wallets[0];

            assert_eq!(entry.biometric, record, "the same record, and so the same key");
            assert_ne!(entry.vault.salt, old.vault.salt, "under a salt of its own");
            assert_eq!(Vault::open_both(&entry.vault, new, &secret).unwrap().1, book);
            assert!(Vault::open_both(&entry.vault, "correct horse", &secret).is_err());
            assert!(world.settings.retired.is_empty(), "nothing goes with it");
            assert!(world.settings.persist_now);
            assert_eq!(Vault::open_both(&world.vault.as_ref().unwrap().seal(&book).unwrap(), new, &secret).unwrap().1, book, "and what is open seals under it");
        }

        #[test]
        fn a_wallet_with_biometrics_alone_has_no_password_to_change() {
            let (mut world, mut book, _) = needing(Guard::Biometric);
            let mut holders = Vec::new();
            let mut state = SettingsState::default();
            // Tall enough for the whole page, as what is off a screen is not drawn.
            let said = drawn::said_in(&mut world, &mut book, &mut holders, (360.0, 6000.0), |ui, ctx| show(ui, ctx, &mut state));

            assert!(!said.contains("Change password"), "{said}");
            assert!(said.contains("This wallet has none"), "{said}");
        }

        #[test]
        fn a_wallet_needing_both_is_never_offered_no_password_and_the_page_draws_on_every_screen() {
            for size in drawn::SCREENS {
                for guard in [Guard::Password, Guard::Biometric, Guard::Both] {
                    let (mut world, mut book, _) = needing(guard);
                    let mut holders = crate::keys::open(&book).unwrap();
                    let mut state = SettingsState { removing: Some(Proof::default()), asking_words: Some((0, Proof::default())), ..SettingsState::default() };
                    let (width, changed) = drawn::page_in(&mut world, &mut book, &mut holders, size, |ui, ctx| {
                        wallet(ui, ctx, &mut state);
                        words_question(ui, ctx, &mut state);
                    });

                    assert!(width <= size.0 + 0.5, "{guard:?}: {width} on a screen {} wide", size.0);
                    assert!(!changed, "{guard:?}");
                }
            }
        }

        #[test]
        fn changing_the_password_writes_at_once_and_retires_the_old_biometric_key() {
            // An Either wallet: a password opens it, with biometrics kept as an alternative. Changing
            // the password must write the new seal at once, not at the next autosave, and must not
            // forget the old biometric key before that seal is on disk, or a kill in between would
            // leave the file holding the old seal with no biometrics to open it either.
            let book = drawn::two_phrases();
            let vault = Vault::create("correct horse").unwrap();
            let sealed = vault.seal(&book).unwrap();
            let mut world = World { settings: Settings::with_wallets(vec![entry(sealed, Some("a-key-tag".to_string()))]), vault: Some(vault), ..World::default() };
            let mut book = book;
            let mut holders = crate::keys::open(&book).unwrap();
            let mut state = SettingsState { current: Typed::new("correct horse".to_string()), new: Typed::new("a new password".to_string()), again: Typed::new("a new password".to_string()), ..SettingsState::default() };
            let mut done = None;

            drawn::page_in(&mut world, &mut book, &mut holders, (360.0, 740.0), |_ui, ctx| {
                if done.is_none() {
                    done = Some(change_password(ctx, &mut state));
                }
            });

            assert!(done.unwrap().unwrap().contains("Biometrics are off"), "it says the key was retired");
            assert!(world.settings.persist_now, "the new seal is written at once");
            assert_eq!(world.settings.wallets[0].biometric, None, "the wallet no longer names the old key");
            assert_eq!(world.settings.retired, vec!["a-key-tag".to_string()], "retired, not forgotten, until the settings are on disk");
        }
    }
}
