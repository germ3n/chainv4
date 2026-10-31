//! The accounts this wallet holds keys for: making a new one, from a seed phrase, with an address
//! in every scheme, or bringing one in from a key file or its text, and naming, using, exporting or
//! removing each. A further seed phrase comes in here too, read or made new, with its first account.
//! On a watch-only sender, the accounts are an offline signer's, each read off its Pair page. Every
//! change seals the vault again.

use {
    super::Ctx,
    super::scan::Scan,
    crate::airgap::Message,
    crate::app::Role,
    crate::format,
    crate::keys::{self, Holder},
    crate::lock::{self, Writing},
    crate::theme,
    crate::vault::{Book, Fingerprint, Source, Words},
    chainv4::crypto::keypair::{KeyType, Keypair},
    chainv4::crypto::mnemonic::Phrase,
    chainv4::structs::u256::u256,
    chainv4_ui::files::{self, Pick},
    egui::{RichText, Ui},
    std::sync::{Arc, Mutex},
};

/// A key file picked, by name, once it is read, or why it could not be.
type Picked = Arc<Mutex<Option<Result<(String, Vec<u8>), String>>>>;

#[derive(Default)]
pub struct AccountsState {
    name: String,
    /// The scheme of a new account's key, in a wallet without a seed phrase: Ed25519 until another
    /// is picked.
    scheme: Option<KeyType>,
    /// A key's text, pasted, and a key file picked, once it is read: wiped as it drops.
    pasted: crate::lock::Typed,
    picked: Picked,
    error: Option<String>,
    renaming: Option<(usize, String)>,
    /// The account a further address is being chosen for.
    adding: Option<usize>,
    /// The account whose key it asks about showing, the one whose key shows, and the one asked to
    /// be removed.
    asking_reveal: Option<usize>,
    revealing: Option<usize>,
    removing: Option<usize>,
    /// Reading a signer's account, on a watch-only sender.
    scan: Scan,
    /// MetaMask's account once it connected, or why it did not.
    connecting: Arc<Mutex<Option<Result<chainv4::crypto::keypair::PublicKey, String>>>>,
    /// Which phrase a new account comes from, by the reference it will take: the first until
    /// another is picked.
    from: Option<Fingerprint>,
    /// A further phrase being added.
    more: Option<MoreWords>,
    /// How many accounts there were last frame: the indices above are stale once that changes,
    /// as when Settings removes a phrase and its accounts.
    seen: usize,
}

/// A further phrase being added: what it and its first account will be called, its passphrase, and
/// how far along it is. Wiped as it drops.
#[derive(Default)]
struct MoreWords {
    name: String,
    account: String,
    words: lock::Typed,
    passphrase: lock::Typed,
    error: Option<String>,
    stage: Stage,
}

#[derive(Default)]
enum Stage {
    /// Words written down before, being typed.
    #[default]
    Typing,
    /// New words asked for, not made yet.
    Making,
    /// New words, made, being written down.
    Writing { phrase: Phrase, writing: Writing },
}

impl AccountsState {
    /// Puts away any keys shown, and any words: new ones not yet written down are not kept, so
    /// they never wait on a page out of sight.
    pub fn hide(&mut self) {
        self.revealing = None;
        self.asking_reveal = None;
        self.more = None;
    }

    /// Forgets which account each question was about.
    fn forget_places(&mut self) {
        self.renaming = None;
        self.adding = None;
        self.asking_reveal = None;
        self.revealing = None;
        self.removing = None;
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut AccountsState) {
    let palette = *ctx.palette;

    if state.seen != ctx.holders.len() {
        state.forget_places();
        state.seen = ctx.holders.len();
    }

    // A phrase picked that has gone since never makes an account from another without saying so.
    if ctx.book.phrase_of(state.from).is_none() {
        state.from = None;
    }

    take_picked(ctx, state);
    let connected = state.connecting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();

    match connected {
        Some(Ok(key)) => {
            if state.name.trim().is_empty() {
                state.name = "MetaMask".to_string();
            }

            add(ctx, state, Ok(Holder::metamask(String::new(), key)));
        }
        Some(Err(why)) => state.error = Some(why),
        None => {}
    }

    theme::title(ui, &palette, "Accounts", "The keys this wallet holds, sealed with your password on this device.");

    if ctx.holders.is_empty() {
        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, "Start here");
            let text = match (ctx.book.watch, ctx.book.phrases.is_empty()) {
                (true, _) => "Add an account from the offline signer: on it, open Pair, and read its code below.",
                (false, false) => "Make a new account from the wallet's words, or bring in a key you have. Either way the keys stay on this device, sealed.",
                (false, true) => "Make a new account, or bring in a key you have. Either way the key stays on this device, sealed.",
            };
            ui.label(RichText::new(text).color(palette.soft));
        });
    }

    // An account asked to go goes once every card is drawn, so none is drawn past the end.
    let mut removed = None;

    for index in 0..ctx.holders.len() {
        if holder(ui, ctx, state, index) {
            removed = Some(index);
        }

        ui.add_space(10.0);
    }

    reveal_question(ui, ctx, state);

    if let Some(index) = removed {
        let mut current = ctx.settings.current().to_string();
        remove(ctx.holders, &mut current, index);
        ctx.settings.set_current(current);
        state.removing = None;
        state.revealing = None;
        state.asking_reveal = None;
        state.renaming = None;
        state.adding = None;
        state.seen = ctx.holders.len();
        *ctx.changed = true;
    }

    ui.add_space(4.0);

    if ctx.book.watch {
        from_signer(ui, ctx, state);

        return;
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Add an account");
        ui.add_space(4.0);
        ui.add(egui::TextEdit::singleline(&mut state.name).hint_text("name, such as Spending").desired_width(f32::INFINITY));
        ui.add_space(6.0);

        match ctx.book.phrases.is_empty() {
            false => {
                // With several, which of them; with one, nothing to ask.
                let several = ctx.book.phrases.len() > 1;

                if several {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("From").color(palette.soft));

                        for (from, name) in ctx.book.references() {
                            if ui.selectable_label(state.from == from, name).clicked() {
                                state.from = from;
                            }
                        }
                    });
                }

                let number = keys::unused(ctx.holders, state.from);
                let whose = match several {
                    true => format!("{}'s", ctx.book.name_from(state.from)),
                    false => "the words'".to_string(),
                };
                ui.label(
                    RichText::new(format!("A new account is {whose} index {number}: an Ed25519, an EdBlake and an Ethereum address, all from the same words."))
                        .small()
                        .color(palette.muted),
                );
            }
            // Without words, a new account is one key, in the scheme picked.
            true => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Scheme").color(palette.soft));

                    let picked = *state.scheme.get_or_insert(KeyType::Ed25519);

                    for scheme in KeyType::ALL {
                        if ui.selectable_label(picked == *scheme, keys::label(*scheme)).clicked() {
                            state.scheme = Some(*scheme);
                        }
                    }
                });
            }
        }

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if theme::primary(ui, &palette, "New account", true).clicked() {
                let made = new_account(ctx.book, ctx.holders, state.scheme.unwrap_or(KeyType::Ed25519), state.from);
                add(ctx, state, made);
            }

            // On Android, only when the APK carries the picker.
            if files::can_pick(ui.ctx()) && theme::quiet(ui, &palette, "Open key file…").clicked() {
                state.error = None;
                pick(ctx.wake, state.picked.clone());
            }

            // In the browser, with MetaMask there.
            if crate::metamask::available()
                && theme::quiet(ui, &palette, "Connect MetaMask").on_hover_text("Its account, signed for by MetaMask, or a hardware wallet through it").clicked()
            {
                state.error = None;
                let (inbox, wake) = (state.connecting.clone(), ctx.wake.clone());
                crate::metamask::connect(move |connected| {
                    *inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(connected);
                    wake.request_repaint();
                });
            }
        });

        ui.add_space(10.0);
        ui.label(RichText::new("Or paste a key, as chainv4 keygen writes it:").small().color(palette.muted));
        let import = super::beside(
            ui,
            |ui| theme::quiet(ui, &palette, "Import").clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(&mut *state.pasted).password(true).hint_text("ed25519:0x…, secp256k1:0x…, bitcoin:0x…").desired_width(width));
            },
        );

        if import {
            let read = Keypair::from_key_file(state.pasted.trim()).map(brought_in);
            add(ctx, state, read);
        }

        ui.add_space(10.0);
        egui::CollapsingHeader::new(RichText::new("Development key").color(palette.soft)).id_salt("development").show(ui, |ui| {
            ui.label(
                RichText::new("Development chains give their coin to a well-known key whose seed is in chainv4's source. Anyone can spend from it: use it to try things on a dev chain, never for anything of value.")
                    .small()
                    .color(palette.warn),
            );

            if theme::quiet(ui, &palette, "Add the development key").clicked() {
                if state.name.trim().is_empty() {
                    state.name = "Development".to_string();
                }

                add(ctx, state, Ok(brought_in(Keypair::development())));
            }
        });

        if let Some(error) = &state.error {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, error);
        }
    });

    ui.add_space(10.0);
    more_words(ui, ctx, state);
}

/// Adding a further seed phrase: words written down before, with their passphrase if they had one,
/// or new ones, written down and three of them asked back as a new wallet's are. It comes with its
/// first account, and is sealed with this wallet's one password.
fn more_words(ui: &mut Ui, ctx: &mut Ctx, state: &mut AccountsState) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        let Some(more) = &mut state.more else {
            theme::caps(ui, &palette, "More words");
            ui.label(RichText::new("This wallet can hold accounts from another seed phrase too, each counted on its own, as a restore of those words elsewhere counts them. All of them are sealed with this wallet's password.").small().color(palette.muted));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                // What an earlier try said of another way in says nothing of this one.
                if theme::quiet(ui, &palette, "Add words you have").clicked() {
                    state.more = Some(MoreWords::default());
                    state.error = None;
                }

                if theme::quiet(ui, &palette, "Make new words").clicked() {
                    state.more = Some(MoreWords { stage: Stage::Making, ..MoreWords::default() });
                    state.error = None;
                }
            });

            return;
        };
        let (mut go, mut leave) = (false, false);

        match &mut more.stage {
            Stage::Writing { phrase, writing } => {
                go = lock::write_down(ui, &palette, phrase, writing, "Add them");
                ui.add_space(6.0);
                leave = theme::quiet(ui, &palette, "Cancel").clicked();
            }
            stage => {
                let typing = matches!(stage, Stage::Typing);
                theme::caps(ui, &palette, if typing { "Add words you have" } else { "Make new words" });

                if typing {
                    ui.label(RichText::new("The 12 or 24 words, in order, with spaces between.").small().color(palette.muted));
                    lock::secret(ui, egui::TextEdit::multiline(&mut *more.words).desired_rows(3).desired_width(f32::INFINITY).hint_text("word word word …"));
                }

                // A name is kept only beside other words, so the first in a wallet is asked none.
                if !ctx.book.phrases.is_empty() {
                    ui.add(egui::TextEdit::singleline(&mut more.name).hint_text(format!("what to call them, or Words {}", ctx.book.phrases.len() + 1)).desired_width(f32::INFINITY));
                }

                ui.add(egui::TextEdit::singleline(&mut more.account).hint_text(format!("name of their first account, or Account {}", ctx.holders.len() + 1)).desired_width(f32::INFINITY));
                ui.add_space(4.0);
                lock::passphrase_field(ui, &palette, &mut more.passphrase);
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    go = theme::primary(ui, &palette, if typing { "Add them" } else { "Make the words" }, true).clicked();
                    leave = theme::quiet(ui, &palette, "Back").clicked();
                });
            }
        }

        if let Some(error) = &more.error {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, error);
        }

        if go {
            state.error = None;
        }

        // What the tap on go comes to: new words to write down, or words to add.
        let phrase = match (&mut more.stage, go) {
            (Stage::Making, true) => match Phrase::generate(12) {
                Ok(phrase) => {
                    more.stage = Stage::Writing { phrase, writing: Writing::default() };
                    more.error = None;
                    None
                }
                Err(why) => {
                    more.error = Some(why.to_string());
                    None
                }
            },
            (Stage::Typing, true) => match lock::read_words(&more.words) {
                Ok(phrase) => Some(phrase),
                Err(why) => {
                    more.error = Some(why);
                    None
                }
            },
            (Stage::Writing { phrase, .. }, true) => Some(phrase.clone()),
            _ => None,
        };

        if let Some(phrase) = phrase {
            match add_words(ctx.book, ctx.holders, &phrase, &more.passphrase, &more.name, &more.account) {
                Ok(id) => {
                    ctx.settings.set_current(id);
                    *ctx.changed = true;
                    state.error = None;
                    leave = true;
                }
                Err(why) => more.error = Some(why),
            }
        }

        if leave {
            state.more = None;
        }
    });
}

/// Adds a further phrase and its first account, account 0 of those words, as a restore of them
/// elsewhere makes it, whatever other phrases hold. Refused with nothing changed: a phrase the
/// wallet holds, by its fingerprint, and one whose first account has an address the wallet holds.
/// What the new account goes by.
fn add_words(book: &mut Book, holders: &mut Vec<Holder>, phrase: &Phrase, passphrase: &str, name: &str, account: &str) -> Result<String, String> {
    let mut words = Words::new(phrase.to_string(), passphrase.to_string());
    words.set_name(name);
    book.refuses(&words)?;
    let keys = keys::derive(words.seed().ok_or("These words do not read.")?, 0);

    if let Some(held) = keys.iter().map(|key| key.public().address()).find(|address| holders.iter().any(|holder| holder.addresses().contains(address))) {
        return Err(format!("{} is in this wallet already", format::short(&held.to_string())));
    }

    let from = book.add_phrase(words)?;
    let name = match account.trim() {
        "" => format!("Account {}", holders.len() + 1),
        name => name.to_string(),
    };
    let holder = Holder::signing(name, Source::Phrase { account: 0, extra: Vec::new(), from }, keys);
    let id = holder.id();
    holders.push(holder);

    Ok(id)
}

/// One account's card. Returns whether it was asked to go.
fn holder(ui: &mut Ui, ctx: &mut Ctx, state: &mut AccountsState, index: usize) -> bool {
    let palette = *ctx.palette;
    let addresses = ctx.holders[index].addresses();
    let number = ctx.holders[index].number();
    let current = index == ctx.current;
    let mut remove = false;

    theme::card(ui, &palette, |ui| {
        ui.horizontal_wrapped(|ui| {
            match &mut state.renaming {
                Some((renamed, name)) if *renamed == index => {
                    let field = ui.add(egui::TextEdit::singleline(name).desired_width(220.0));

                    if field.lost_focus() || theme::quiet(ui, &palette, "Done").clicked() {
                        let name = name.trim().to_string();

                        if !name.is_empty() {
                            ctx.holders[index].name = name;
                            *ctx.changed = true;
                        }

                        state.renaming = None;
                    }
                }
                _ => {
                    let name = ctx.holders[index].name.clone();
                    ui.scope(|ui| {
                        ui.set_max_width(ui.available_width() * 0.45);
                        ui.add(egui::Label::new(theme::strong(&name, 16.0).color(palette.ink)).truncate());
                    });
                }
            }

            if current {
                theme::pill(ui, "In use", palette.accent);
            }

            // The coin at all its addresses, once each is proven. An offline signer follows no chain,
            // so nothing is ever proven on one: it does not say it waits for what never comes.
            if ctx.settings.role() != Role::Signer {
                let proven: Vec<_> = addresses.iter().filter_map(|address| ctx.chain.account(&address.to_string())).collect();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match proven.len() == addresses.len() {
                    true => {
                        let raw = proven.iter().fold(u256::ZERO, |sum, proven| sum.saturating_add(proven.value.balance)).to_string();
                        let shown = format!("{} {}", format::compact(&raw, ctx.coin.decimals, 4), ctx.coin.symbol);
                        theme::fitted(ui, &shown, 14.5, 10.0, egui::FontFamily::Proportional, palette.soft)
                            .on_hover_text(format::amount(&raw, ctx.coin.decimals, &ctx.coin.symbol));
                    }
                    false => {
                        ui.label(RichText::new("not proven yet").small().color(palette.muted));
                    }
                });
            }
        });

        let watched = ctx.holders[index].keys.is_empty();
        let from = match (number, watched) {
            _ if ctx.holders[index].by_metamask() => "MetaMask · it signs, in the browser".to_string(),
            (_, true) => "Watched · the offline signer holds its keys".to_string(),
            (Some(number), false) if ctx.book.phrases.len() > 1 => format!("From {} · index {number}", ctx.book.name_from(ctx.holders[index].from())),
            (Some(number), false) => format!("From the words · index {number}"),
            (None, false) => "A key brought in".to_string(),
        };
        ui.label(RichText::new(from).small().color(palette.muted));
        ui.add_space(4.0);

        for address in &addresses {
            let text = address.to_string();
            ui.horizontal_wrapped(|ui| {
                ui.add_sized([64.0, 18.0], egui::Label::new(RichText::new(super::scheme(address)).small().color(palette.muted)));
                super::address(ui, &palette, &text);

                if let Some(proven) = ctx.chain.account(&text) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::fitted(ui, &ctx.brief(None, &proven.value.balance.to_string()), 12.5, 9.0, egui::FontFamily::Proportional, palette.muted);
                    });
                }
            });
        }

        if state.adding == Some(index) {
            ui.add_space(4.0);
            ui.label(RichText::new("Another address from the words, in:").small().color(palette.soft));
            ui.horizontal_wrapped(|ui| {
                for scheme in KeyType::ALL {
                    if theme::quiet(ui, &palette, keys::label(*scheme)).clicked() {
                        add_address(ctx, index, *scheme);
                        state.adding = None;
                    }
                }
            });
        }

        ui.add_space(8.0);
        // On a narrow screen the buttons go on to a second line rather than past the edge.
        ui.horizontal_wrapped(|ui| {
            if !current && theme::quiet(ui, &palette, "Use").clicked() {
                ctx.settings.set_current(ctx.holders[index].id());
            }

            if theme::quiet(ui, &palette, "Rename").clicked() {
                state.renaming = Some((index, ctx.holders[index].name.clone()));
            }

            // A further address from the words, in a scheme picked next.
            if number.is_some() && !watched && theme::quiet(ui, &palette, "Another address").clicked() {
                state.adding = (state.adding != Some(index)).then_some(index);
            }

            let revealed = state.revealing == Some(index);
            let export = match (revealed, addresses.len() > 1) {
                (true, true) => "Hide keys",
                (true, false) => "Hide key",
                (false, true) => "Export keys",
                (false, false) => "Export key",
            };

            // Hiding them needs no asking; showing them does. A watched account has none here.
            if !watched && theme::quiet(ui, &palette, export).clicked() {
                match revealed {
                    true => state.revealing = None,
                    false => state.asking_reveal = Some(index),
                }
            }

            if theme::quiet(ui, &palette, "Remove").clicked() {
                state.removing = Some(index);
            }
        });

        if state.revealing == Some(index) {
            ui.add_space(8.0);
            ui.label(RichText::new("Anyone who sees these keys can spend from the account. Keep them somewhere only you can read.").small().color(palette.warn));

            for key in &ctx.holders[index].keys {
                let file = key.to_key_file();
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    theme::caps(ui, &palette, keys::label(key.public().key_type()));
                    super::copy(ui, &palette, &file);
                });
                ui.add(egui::Label::new(theme::mono(&file).color(palette.soft)).wrap());
            }
        }

        if state.removing == Some(index) {
            ui.add_space(8.0);
            let warning = match number {
                Some(_) => "It leaves this list. Its keys still come from the words: New account brings it back first.",
                None => "Its key leaves this wallet. Without a copy of it, what the account holds is out of reach for good.",
            };
            ui.label(RichText::new(warning).color(palette.bad));
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, &palette, "Remove it", true).clicked() {
                    remove = true;
                }

                if theme::quiet(ui, &palette, "Keep it").clicked() {
                    state.removing = None;
                }
            });
        }
    });

    remove
}

/// Asks, in a box over the page, whether to show an account's private key, which anyone who sees it
/// can spend from.
fn reveal_question(ui: &mut Ui, ctx: &Ctx, state: &mut AccountsState) {
    let Some(index) = state.asking_reveal else {
        return;
    };
    let Some(holder) = ctx.holders.get(index) else {
        state.asking_reveal = None;

        return;
    };
    let palette = *ctx.palette;
    let name = holder.name.clone();
    let mut answer = None;

    let modal = egui::Modal::new(ui.make_persistent_id("reveal key"))
        .frame(egui::Frame::new().fill(palette.card).stroke(egui::Stroke::new(1.0, palette.line)).corner_radius(12.0).inner_margin(egui::Margin::same(20)))
        .show(ui.ctx(), |ui| {
            if chainv4_ui::layout::fingers(ui.ctx()) {
                chainv4_ui::layout::finger_sized(ui);
            }

            ui.set_max_width(360.0);
            theme::caps(ui, &palette, "Show the private keys?");
            ui.add_space(4.0);
            ui.label(RichText::new(format!("Anyone who sees the keys of {name} can take everything it holds, now and later.")).color(palette.ink));
            ui.label(
                RichText::new("Show it only to copy it somewhere safe, with no one watching your screen and nothing recording it.")
                    .small()
                    .color(palette.muted),
            );
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, &palette, "Show them", true).clicked() {
                    answer = Some(true);
                }

                if theme::quiet(ui, &palette, "Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });

    match answer {
        Some(true) => {
            state.revealing = Some(index);
            state.asking_reveal = None;
        }
        Some(false) => state.asking_reveal = None,
        None if modal.should_close() => state.asking_reveal = None,
        None => {}
    }
}

/// Gives a words account a further address in a scheme, the index next_index picks, and seals
/// it with the rest: what the words bring back.
pub fn add_address(ctx: &mut Ctx, index: usize, scheme: KeyType) -> Option<chainv4::structs::address::Address> {
    let (account, from) = (ctx.holders[index].number()?, ctx.holders[index].from());
    let seed = ctx.book.phrase_of(from)?.seed()?;
    let at = keys::next_index(ctx.holders, account, from, scheme);
    let address = ctx.holders[index].add_key(seed, scheme, at)?;
    *ctx.changed = true;

    Some(address)
}

/// Takes an account out, and moves the one in use to the first left when it was that one.
fn remove(holders: &mut Vec<Holder>, current: &mut String, index: usize) {
    let removed = holders.remove(index);

    if *current == removed.id() || current.is_empty() {
        *current = holders.first().map(Holder::id).unwrap_or_default();
    }
}

/// A new account: that phrase's next, with a key in every scheme, or, without words, a random key
/// in the scheme picked. Words named that are not there are refused, never replaced by a key that
/// is not theirs. Its name comes when it is added.
fn new_account(book: &Book, holders: &[Holder], scheme: KeyType, from: Option<Fingerprint>) -> Result<Holder, String> {
    match (book.phrase_of(from).and_then(Words::seed), from) {
        (Some(seed), _) => {
            let account = keys::unused(holders, from);

            Ok(Holder::signing(String::new(), Source::Phrase { account, extra: Vec::new(), from }, keys::derive(seed, account)))
        }
        (None, Some(_)) => Err("Those words are gone from this wallet.".to_string()),
        (None, None) => Keypair::generate(scheme).map(brought_in).map_err(|err| format!("no randomness for a key: {err}")),
    }
}

/// An account of one key, brought in.
fn brought_in(key: Keypair) -> Holder {
    Holder::signing(String::new(), Source::Key { key: key.to_key_file() }, vec![key])
}

/// Adds an account with the name typed, unless the wallet has one of its addresses already.
fn add(ctx: &mut Ctx, state: &mut AccountsState, made: Result<Holder, String>) {
    state.error = None;

    let mut made = match made {
        Ok(made) => made,
        Err(why) => {
            state.error = Some(why);

            return;
        }
    };

    if let Some(held) = made.addresses().into_iter().find(|address| ctx.holders.iter().any(|holder| holder.addresses().contains(address))) {
        state.error = Some(format!("{} is in this wallet already", format::short(&held.to_string())));

        return;
    }

    made.name = match state.name.trim() {
        "" => format!("Account {}", ctx.holders.len() + 1),
        name => name.to_string(),
    };
    ctx.settings.set_current(made.id());
    ctx.holders.push(made);
    *ctx.changed = true;
    state.name.clear();
    state.pasted.clear();
}

/// On a watch-only sender: an account read off the offline signer's Pair page.
fn from_signer(ui: &mut Ui, ctx: &mut Ctx, state: &mut AccountsState) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Add from the signer");
        ui.add(egui::TextEdit::singleline(&mut state.name).hint_text("name, or the signer's").desired_width(f32::INFINITY));
        ui.add_space(8.0);

        match state.scan.show(ui, &palette) {
            Some(Message::Pair(pairing)) => {
                if state.name.trim().is_empty() {
                    state.name = pairing.name.clone();
                }

                add(ctx, state, Ok(Holder::watched(String::new(), pairing.keys, pairing.origin)));
            }
            Some(other) => state.scan.refuse(&other, "an account to pair with"),
            None => {}
        }

        if let Some(error) = &state.error {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, error);
        }
    });
}

/// Asks for a key file, which arrives later: in the browser and on Android, picking one is
/// asynchronous.
fn pick(ctx: &egui::Context, picked: Picked) {
    files::pick(ctx, Pick::default(), move |files| {
        let read = match files {
            Ok(mut files) if !files.is_empty() => {
                let file = files.swap_remove(0);

                Ok((file.name, file.bytes))
            }
            Ok(_) => return,
            Err(why) => Err(why),
        };
        *picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(read);
    });
}

fn take_picked(ctx: &mut Ctx, state: &mut AccountsState) {
    let picked = state.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();

    if let Some(Err(why)) = picked {
        state.error = Some(why);

        return;
    }

    if let Some(Ok((name, bytes))) = picked {
        let read = String::from_utf8(bytes)
            .map_err(|_| "it is not text".to_string())
            .and_then(|text| Keypair::from_key_file(text.trim()))
            .map(brought_in)
            .map_err(|why| format!("{name}: {why}"));

        if state.name.trim().is_empty() {
            state.name = name.trim_end_matches(".key").to_string();
        }

        add(ctx, state, read);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(name: &str) -> Holder {
        Holder {
            name: name.to_string(),
            ..brought_in(Keypair::generate(KeyType::Ed25519).unwrap())
        }
    }

    #[test]
    fn removing_an_account_keeps_the_rest_and_moves_off_one_in_use() {
        let mut holders = vec![holder("a"), holder("b"), holder("c")];
        let (a, c) = (holders[0].id(), holders[2].id());
        let mut current = a.clone();

        remove(&mut holders, &mut current, 0);

        assert_eq!(holders.iter().map(|holder| holder.name.as_str()).collect::<Vec<_>>(), ["b", "c"]);
        assert_eq!(current, holders[0].id(), "the one in use went, so the first left is");

        current = c.clone();
        remove(&mut holders, &mut current, 0);

        assert_eq!(current, c, "another went: the one in use stays");

        remove(&mut holders, &mut current, 0);

        assert!(holders.is_empty());
        assert_eq!(current, "", "none left to use");
    }

    #[test]
    fn a_new_account_comes_from_the_words_when_there_are_some() {
        let words = Book {
            phrases: vec![Words::new("test test test test test test test test test test test junk".to_string(), String::new())],
            accounts: Vec::new(),
            watch: false,
            swaps: Vec::new(),
            ..Book::default()
        };
        let first = new_account(&words, &[], KeyType::Ed25519, None).unwrap();
        let second = new_account(&words, std::slice::from_ref(&first), KeyType::Ed25519, None).unwrap();

        assert_eq!((first.number(), second.number()), (Some(0), Some(1)));
        assert_eq!(first.addresses().len(), 4, "an address in every scheme");
        assert_eq!(first.addresses()[2].to_string(), "secp256k1:0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");

        let without = new_account(&Book::default(), &[], KeyType::Secp256k1, None).unwrap();

        assert_eq!(without.number(), None);
        assert_eq!(without.addresses().iter().map(|address| address.kind()).collect::<Vec<_>>(), ["secp256k1"]);
    }

    const TEST: &str = "test test test test test test test test test test test junk";
    const ABANDON: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    /// A wallet of the test words and their first account, as the lock screen makes one.
    fn wallet() -> (Book, Vec<Holder>) {
        let book = Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            accounts: vec![crate::vault::StoredAccount { name: "Main".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from: None } }],
            ..Book::default()
        };
        let holders = keys::open(&book).unwrap();

        (book, holders)
    }

    /// What Wallet::store does before it seals.
    fn stored(book: &mut Book, holders: &[Holder]) {
        book.accounts = holders.iter().map(|holder| crate::vault::StoredAccount { name: holder.name.clone(), source: holder.source.clone() }).collect();
    }

    #[test]
    fn a_phrase_added_starts_at_its_own_account_0_as_a_restore_of_it_elsewhere_does() {
        let (mut book, mut holders) = wallet();
        let abandon = Phrase::parse(ABANDON).unwrap();
        let elsewhere = keys::derive(&abandon.seed(""), 0).iter().map(|key| key.public().address()).collect::<Vec<_>>();

        let id = add_words(&mut book, &mut holders, &abandon, "", "Savings", "Saved").unwrap();

        assert_eq!(holders[1].id(), id);
        assert_eq!((holders[1].name.as_str(), holders[1].number()), ("Saved", Some(0)), "number 0, though the first words hold a 0 too");
        assert_eq!(holders[1].addresses(), elsewhere, "the same addresses as those words restored alone");
        assert_eq!(book.name_from(holders[1].from()), "Savings");

        // A new account of each counts its own.
        let main = new_account(&book, &holders, KeyType::Ed25519, None).unwrap();
        let saved = new_account(&book, &holders, KeyType::Ed25519, holders[1].from()).unwrap();

        assert_eq!((main.number(), saved.number()), (Some(1), Some(1)));
        assert_ne!(main.addresses(), saved.addresses());
        assert_eq!(saved.addresses(), keys::derive(&abandon.seed(""), 1).iter().map(|key| key.public().address()).collect::<Vec<_>>());

        // Named words that are gone make no account, and never one from others in their place.
        assert_eq!(new_account(&book, &holders, KeyType::Ed25519, Some([9, 9, 9, 9])).err().as_deref(), Some("Those words are gone from this wallet."));

        // Sealed and opened again, all as it was.
        stored(&mut book, &holders);
        let vault = crate::vault::Vault::create("correct horse").unwrap();
        let (_, opened) = crate::vault::Vault::open(&vault.seal(&book).unwrap(), "correct horse").unwrap();

        assert_eq!(opened, book);
        assert_eq!(keys::open(&opened).unwrap()[1].addresses(), elsewhere);
    }

    #[test]
    fn the_same_words_twice_are_refused_with_nothing_changed() {
        let (mut book, mut holders) = wallet();
        let test = Phrase::parse(TEST).unwrap();
        let before = book.clone();

        assert_eq!(add_words(&mut book, &mut holders, &test, "", "", "").unwrap_err(), "These words are in this wallet already.");
        assert_eq!((&book, holders.len()), (&before, 1));

        // The same words with a passphrase are another seed, as anywhere else.
        add_words(&mut book, &mut holders, &test, "a passphrase", "", "").unwrap();

        assert_eq!(holders.len(), 2);
        assert_eq!(book.name_of(1), "Words 2", "a plain name, when none is given");
        assert_eq!(add_words(&mut book, &mut holders, &test, "a passphrase", "", "").unwrap_err(), "These words are in this wallet already.");

        // A key brought in that is the new words' first account: nothing half added.
        let (mut book, mut holders) = wallet();
        let abandon = Phrase::parse(ABANDON).unwrap();
        holders.push(brought_in(abandon.seed("").key(KeyType::Ed25519, 0)));
        let before = book.clone();

        assert!(add_words(&mut book, &mut holders, &abandon, "", "", "").unwrap_err().contains("in this wallet already"));
        assert_eq!((&book, holders.len()), (&before, 2));

        // A watch-only wallet holds no words.
        let mut watching = Book { watch: true, ..Book::default() };

        assert!(add_words(&mut watching, &mut Vec::new(), &abandon, "", "", "").unwrap_err().contains("watch-only"));
        assert!(watching.phrases.is_empty());
    }

    #[test]
    fn the_first_words_of_a_wallet_without_any_stay_in_the_form_older_builds_read() {
        let mut book = Book::default();
        let mut holders = vec![brought_in(Keypair::development())];

        add_words(&mut book, &mut holders, &Phrase::parse(TEST).unwrap(), "", "ignored", "Main").unwrap();

        assert_eq!(holders[1].from(), None, "the first words: named by nothing");
        stored(&mut book, &holders);
        let json = serde_json::to_string(&book).unwrap();

        assert!(json.starts_with(&format!(r#"{{"phrase":{{"words":"{TEST}","passphrase":""}},"accounts":"#)), "as one phrase always was");
    }

    #[test]
    fn the_accounts_page_draws_every_stage_of_adding_words_on_every_screen() {
        let stages = || {
            vec![
                None,
                Some(MoreWords::default()),
                Some(MoreWords { stage: Stage::Making, ..MoreWords::default() }),
                Some(MoreWords { stage: Stage::Writing { phrase: Phrase::parse(ABANDON).unwrap(), writing: Writing::default() }, ..MoreWords::default() }),
                Some(MoreWords {
                    stage: Stage::Writing {
                        phrase: Phrase::parse(ABANDON).unwrap(),
                        writing: Writing::Asked { asked: [0, 5, 11], answers: Default::default(), error: Some("not the words listed: look again".to_string()) },
                    },
                    error: Some("These words are in this wallet already.".to_string()),
                    ..MoreWords::default()
                }),
            ]
        };

        for size in crate::views::drawn::SCREENS {
            for more in stages() {
                let mut book = crate::views::drawn::two_phrases();
                let mut holders = keys::open(&book).unwrap();
                let before = book.clone();
                let mut state = AccountsState { more, seen: holders.len(), ..AccountsState::default() };
                let (width, changed) = crate::views::drawn::page(&mut book, &mut holders, size, |ui, ctx| show(ui, ctx, &mut state));

                assert!(width <= size.0 + 0.5, "{width} on a screen {} wide", size.0);
                assert!(!changed, "with no tap, nothing changes");
                assert_eq!((&book, holders.len()), (&before, 2));
            }
        }

        // A watch-only wallet has no such thing to draw, and its book takes no words.
        let mut watching = Book { watch: true, ..Book::default() };
        let mut state = AccountsState { more: Some(MoreWords::default()), ..AccountsState::default() };
        crate::views::drawn::page(&mut watching, &mut Vec::new(), (360.0, 740.0), |ui, ctx| show(ui, ctx, &mut state));

        assert!(watching.phrases.is_empty());
    }

    #[test]
    fn places_asked_about_are_forgotten_once_the_accounts_change_and_a_phrase_gone_is_not_picked() {
        let mut book = crate::views::drawn::two_phrases();
        let mut holders = keys::open(&book).unwrap();
        let mut state = AccountsState {
            revealing: Some(1),
            asking_reveal: Some(1),
            removing: Some(1),
            adding: Some(1),
            renaming: Some((1, "Saved".to_string())),
            from: Some([9, 9, 9, 9]),
            seen: 3,
            ..AccountsState::default()
        };

        // Settings removed a phrase and its accounts while this page was away, say: what these
        // indices meant has moved, and no other account's keys may show in their place.
        crate::views::drawn::page(&mut book, &mut holders, (360.0, 740.0), |ui, ctx| show(ui, ctx, &mut state));

        assert_eq!((state.revealing, state.asking_reveal, state.removing, state.adding), (None, None, None, None));
        assert!(state.renaming.is_none());
        assert_eq!(state.seen, 2);
        assert_eq!(state.from, None, "the first again, and shown as picked");

        state.more = Some(MoreWords::default());
        state.hide();

        assert!(state.more.is_none(), "words being added go as the page does");
    }

    /// A screen with room for the whole page, as what is tapped must be on it.
    const TALL: (f32, f32) = (1_200.0, 3_000.0);

    #[test]
    fn what_a_failed_import_said_goes_once_words_are_added() {
        let (mut book, mut holders) = wallet();
        let mut state = AccountsState { seen: holders.len(), ..AccountsState::default() };
        state.pasted.push_str("not a key");

        let said = crate::views::drawn::tap(&mut book, &mut holders, TALL, "Import", |ui, ctx| show(ui, ctx, &mut state));
        let why = state.error.clone().expect("it says why it could not");

        assert!(said.contains(&why), "{said}");

        // It goes as soon as words are asked for, before anything is added.
        crate::views::drawn::tap(&mut book, &mut holders, TALL, "Add words you have", |ui, ctx| show(ui, ctx, &mut state));

        assert!(state.more.is_some());
        assert_eq!(state.error, None);

        // And should it be back by then, a success leaves nothing of it.
        state.error = Some(why.clone());
        state.more.as_mut().unwrap().words.push_str(ABANDON);

        let said = crate::views::drawn::tap(&mut book, &mut holders, TALL, "Add them", |ui, ctx| show(ui, ctx, &mut state));

        assert_eq!(holders.len(), 2, "the words came in");
        assert!(state.more.is_none());
        assert_eq!(state.error, None);
        assert!(!said.contains(&why), "{said}");
    }

    #[test]
    fn words_that_do_not_read_say_so_where_they_were_typed_and_a_key_added_clears_what_it_said() {
        let (mut book, mut holders) = wallet();
        let mut state = AccountsState { seen: holders.len(), more: Some(MoreWords::default()), error: Some("not a key file".to_string()), ..AccountsState::default() };
        state.more.as_mut().unwrap().words.push_str("not words");

        crate::views::drawn::tap(&mut book, &mut holders, TALL, "Add them", |ui, ctx| show(ui, ctx, &mut state));

        assert_eq!(state.error, None, "an add started, so what the last one said is past");
        assert!(state.more.as_ref().unwrap().error.is_some(), "and this one's says why, by its own fields");

        // A failure says its own, in place of what came before, and a success nothing.
        state.more = None;
        state.error = Some("not a key file".to_string());
        state.pasted.push_str(&Keypair::development().to_key_file());

        crate::views::drawn::tap(&mut book, &mut holders, TALL, "Import", |ui, ctx| show(ui, ctx, &mut state));

        assert_eq!((holders.len(), state.error.clone()), (2, None));

        state.pasted.push_str(&Keypair::development().to_key_file());
        crate::views::drawn::tap(&mut book, &mut holders, TALL, "Import", |ui, ctx| show(ui, ctx, &mut state));

        assert!(state.error.as_deref().is_some_and(|why| why.contains("in this wallet already")), "{:?}", state.error);
    }

    /// What the Accounts page says, of a wallet whose accounts nothing has proven, as this role.
    fn page_as(book: &mut Book, holders: &mut Vec<Holder>, role: Role) -> String {
        let mut state = AccountsState { seen: holders.len(), ..AccountsState::default() };

        crate::views::drawn::said(book, holders, (360.0, 740.0), |ui, ctx| {
            ctx.settings.role = Some(role);
            show(ui, ctx, &mut state);
        })
    }

    #[test]
    fn a_signer_does_not_say_its_accounts_wait_for_a_proof_that_never_comes() {
        let (mut book, mut holders) = wallet();

        assert!(!page_as(&mut book, &mut holders, Role::Signer).contains("not proven yet"));
        assert!(page_as(&mut book, &mut holders, Role::Online).contains("not proven yet"), "an online wallet is waiting");

        // A watch-only wallet is online, whoever holds its keys.
        let key = Keypair::from_seed([7; 32]).public();
        let (mut watching, mut watched) = (Book { watch: true, ..Book::default() }, vec![Holder::watched("Watched".to_string(), vec![key], None)]);

        assert!(page_as(&mut watching, &mut watched, Role::Online).contains("not proven yet"));
    }

    #[test]
    fn no_page_of_a_signer_waits_on_a_chain_or_asks_a_node_anything() {
        use crate::views::{Page, States, nav};

        for size in crate::views::drawn::SCREENS {
            for (page, name) in nav(Role::Signer) {
                let (mut book, mut holders) = wallet();
                let mut states = States::default();
                let mut sent = Vec::new();
                let said = crate::views::drawn::said(&mut book, &mut holders, size, |ui, ctx| {
                    ctx.settings.role = Some(Role::Signer);
                    *ctx.page = *page;
                    crate::views::show(ui, ctx, &mut states);
                    assert_eq!(*ctx.page, *page, "{name} stays");
                    sent.extend(ctx.node.sent());
                });

                assert!(sent.is_empty(), "{name} asked {sent:?}");

                for word in ["proven", "Catching up", "catches up", "Light client", "Starting", "Following", "Asking the node", "Waiting for a block"] {
                    assert!(!said.contains(word), "{name} says {word}: {said}");
                }
            }
        }

        // The page it might be left on from before its role changed takes it home.
        let (mut book, mut holders) = wallet();
        let mut states = States::default();
        crate::views::drawn::said(&mut book, &mut holders, (360.0, 740.0), |ui, ctx| {
            ctx.settings.role = Some(Role::Signer);
            *ctx.page = Page::Activity;
            crate::views::show(ui, ctx, &mut states);
            assert_eq!(*ctx.page, Page::Sign);
        });
    }

    #[test]
    fn removing_the_last_account_of_a_phrase_leaves_its_words_and_it_comes_back_first() {
        let (mut book, mut holders) = wallet();
        add_words(&mut book, &mut holders, &Phrase::parse(ABANDON).unwrap(), "", "", "").unwrap();
        let from = holders[1].from();
        let mut current = holders[1].id();
        let gone = holders[1].addresses();

        remove(&mut holders, &mut current, 1);
        stored(&mut book, &holders);

        assert_eq!(book.phrases.len(), 2, "its words stay");
        book.check().unwrap();

        let back = new_account(&book, &holders, KeyType::Ed25519, from).unwrap();

        assert_eq!((back.number(), back.addresses()), (Some(0), gone));
    }
}
