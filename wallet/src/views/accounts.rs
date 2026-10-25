//! The accounts this wallet holds keys for: making a new one, from the seed phrase, with an address
//! in every scheme, or bringing one in from a key file or its text, and naming, using, exporting or
//! removing each. On a watch-only sender, the accounts are an offline signer's, each read off its
//! Pair page. Every change seals the vault again.

use {
    super::Ctx,
    super::scan::Scan,
    crate::airgap::Message,
    crate::app::Role,
    crate::format,
    crate::keys::{self, Holder},
    crate::theme,
    crate::vault::{Book, Source},
    chainv4::crypto::keypair::{KeyType, Keypair},
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
    /// A key's text, pasted, and a key file picked, once it is read.
    pasted: String,
    picked: Picked,
    error: Option<String>,
    renaming: Option<(usize, String)>,
    /// The account whose key it asks about showing, the one whose key shows, and the one asked to
    /// be removed.
    asking_reveal: Option<usize>,
    revealing: Option<usize>,
    removing: Option<usize>,
    /// Reading a signer's account, on a watch-only sender.
    scan: Scan,
}

impl AccountsState {
    /// Puts away any keys shown.
    pub fn hide(&mut self) {
        self.revealing = None;
        self.asking_reveal = None;
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut AccountsState) {
    let palette = *ctx.palette;
    take_picked(ctx, state);

    theme::title(ui, &palette, "Accounts", "The keys this wallet holds, sealed with your password on this device.");

    if ctx.holders.is_empty() {
        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, "Start here");
            let text = match (ctx.settings.role(), &ctx.book.phrase) {
                (Role::Sender, _) => "Add an account from the offline signer: on it, open Pair, and read its code below.",
                (_, Some(_)) => "Make a new account from the wallet's words, or bring in a key you have. Either way the keys stay on this device, sealed.",
                (_, None) => "Make a new account, or bring in a key you have. Either way the key stays on this device, sealed.",
            };
            ui.label(RichText::new(text).color(palette.soft));
        });
        ui.add_space(14.0);
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
        *ctx.changed = true;
    }

    ui.add_space(4.0);

    if ctx.settings.role() == Role::Sender {
        from_signer(ui, ctx, state);

        return;
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Add an account");
        ui.add_space(4.0);
        ui.add(egui::TextEdit::singleline(&mut state.name).hint_text("name, such as Spending").desired_width(f32::INFINITY));
        ui.add_space(6.0);

        match ctx.book.phrase {
            Some(_) => {
                let number = keys::unused(ctx.holders);
                ui.label(
                    RichText::new(format!("A new account is the words' index {number}: an Ed25519, an EdBlake and an Ethereum address, all from the same words."))
                        .small()
                        .color(palette.muted),
                );
            }
            // Without words, a new account is one key, in the scheme picked.
            None => {
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
                let made = new_account(ctx.book, ctx.holders, state.scheme.unwrap_or(KeyType::Ed25519));
                add(ctx, state, made);
            }

            // On Android, only when the APK carries the picker.
            if files::can_pick(ui.ctx()) && theme::quiet(ui, &palette, "Open key file…").clicked() {
                pick(ctx.wake, state.picked.clone());
            }
        });

        ui.add_space(10.0);
        ui.label(RichText::new("Or paste a key, as chainv4 keygen writes it:").small().color(palette.muted));
        let import = super::beside(
            ui,
            |ui| theme::quiet(ui, &palette, "Import").clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(&mut state.pasted).password(true).hint_text("ed25519:0x…, secp256k1:0x…").desired_width(width));
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
}

/// One account's card. Returns whether it was asked to go.
fn holder(ui: &mut Ui, ctx: &mut Ctx, state: &mut AccountsState, index: usize) -> bool {
    let palette = *ctx.palette;
    let addresses = ctx.holders[index].addresses();
    let number = ctx.holders[index].number();
    let current = index == ctx.current;
    let mut remove = false;

    theme::card(ui, &palette, |ui| {
        ui.horizontal(|ui| {
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

            // The coin at all its addresses, once each is proven.
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
        });

        let watched = ctx.holders[index].keys.is_empty();
        let from = match (number, watched) {
            (_, true) => "Watched · the offline signer holds its keys".to_string(),
            (Some(number), false) => format!("From the words · index {number}"),
            (None, false) => "A key brought in".to_string(),
        };
        ui.label(RichText::new(from).small().color(palette.muted));
        ui.add_space(4.0);

        for address in &addresses {
            let text = address.to_string();
            ui.horizontal(|ui| {
                ui.add_sized([64.0, 18.0], egui::Label::new(RichText::new(super::scheme(address)).small().color(palette.muted)));
                super::address(ui, &palette, &text);

                if let Some(proven) = ctx.chain.account(&text) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::fitted(ui, &ctx.brief(None, &proven.value.balance.to_string()), 12.5, 9.0, egui::FontFamily::Proportional, palette.muted);
                    });
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
                ui.horizontal(|ui| {
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
            ui.horizontal(|ui| {
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
            ui.horizontal(|ui| {
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

/// Takes an account out, and moves the one in use to the first left when it was that one.
fn remove(holders: &mut Vec<Holder>, current: &mut String, index: usize) {
    let removed = holders.remove(index);

    if *current == removed.id() || current.is_empty() {
        *current = holders.first().map(Holder::id).unwrap_or_default();
    }
}

/// A new account: the words' next, with a key in every scheme, or, without words, a random key in
/// the scheme picked. Its name comes when it is added.
fn new_account(book: &Book, holders: &[Holder], scheme: KeyType) -> Result<Holder, String> {
    match keys::seed(book) {
        Some(seed) => {
            let account = keys::unused(holders);

            Ok(Holder::signing(String::new(), Source::Phrase { account }, keys::derive(&seed, account)))
        }
        None => Keypair::generate(scheme).map(brought_in).map_err(|err| format!("no randomness for a key: {err}")),
    }
}

/// An account of one key, brought in.
fn brought_in(key: Keypair) -> Holder {
    Holder::signing(String::new(), Source::Key { key: key.to_key_file() }, vec![key])
}

/// Adds an account with the name typed, unless the wallet has one of its addresses already.
fn add(ctx: &mut Ctx, state: &mut AccountsState, made: Result<Holder, String>) {
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
    state.error = None;
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

                add(ctx, state, Ok(Holder::watched(String::new(), pairing.keys)));
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
    use {super::*, crate::vault::Words};

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
            phrase: Some(Words {
                words: "test test test test test test test test test test test junk".to_string(),
                passphrase: String::new(),
            }),
            accounts: Vec::new(),
        };
        let first = new_account(&words, &[], KeyType::Ed25519).unwrap();
        let second = new_account(&words, std::slice::from_ref(&first), KeyType::Ed25519).unwrap();

        assert_eq!((first.number(), second.number()), (Some(0), Some(1)));
        assert_eq!(first.addresses().len(), 3, "an address in every scheme");
        assert_eq!(first.addresses()[2].to_string(), "secp256k1:0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");

        let without = new_account(&Book::default(), &[], KeyType::Secp256k1).unwrap();

        assert_eq!(without.number(), None);
        assert_eq!(without.addresses().iter().map(|address| address.kind()).collect::<Vec<_>>(), ["secp256k1"]);
    }
}
