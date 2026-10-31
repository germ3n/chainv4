//! The lock screen: which wallet, and what opens it. Or a wallet more: from new words, shown once and
//! three of them asked back, or from words written down before; or, online, a watch-only one, from
//! an offline signer's account read off its screen. Each wallet is a vault of its own, opened its
//! own way (see crate::guard). On its first launch, it asks whether this install is an offline
//! signer. A wallet that opens with biometrics shows the prompt as the screen does, on the devices
//! that can, and in the browser, which shows a passkey's only for a tap, at the button. One that
//! needs both takes its password first.
//!
//! A wallet that needs hardware can lose it for good. Nothing here is written when it does: the
//! screen says so, and that the words are the way back, and removing it stays possible with a typed
//! yes alone, as nothing else can be given by then.

use {
    crate::airgap::{Message, Pairing},
    crate::biometrics::{self, Check},
    chainv4_ui::biometric::{Availability, Failure, Kind},
    crate::app::{Role, Settings, WalletEntry},
    crate::guard::{self, Guard},
    crate::theme::{self, Palette},
    crate::vault::{Book, Source, StoredAccount, Vault, Words},
    crate::views::scan::Scan,
    chainv4::crypto::mnemonic::{MnemonicError, Phrase},
    egui::{RichText, Ui},
};

pub use chainv4_ui::secret::{Typed, secret};

/// Where the lock screen is.
pub enum Lock {
    /// What this install is, asked once.
    Role,
    /// Asking for what opens the wallet picked: the check under way, and the wallet the prompt
    /// showed for by itself, so it shows once; the wallet found to have lost its hardware key, kept
    /// here alone, as nothing is written when that happens; and, for one that needs hardware, being
    /// removed, with its place and the yes typed.
    Locked { password: Typed, error: Option<String>, check: Option<Check>, prompted: Option<usize>, gone: Option<usize>, removing: Option<(usize, String)> },
    /// Making a wallet: from new words, or from words written down before.
    Choose,
    New(Making),
    /// New words, to write down, and three of them asked back.
    Words { made: Made, writing: Writing },
    Restore { making: Making, words: Typed },
    /// A watch-only wallet from an offline signer's account: read, then named and sealed.
    Pair { making: Making, scan: Box<Scan>, paired: Option<Pairing> },
    Open,
}

/// A wallet being made: its name, password twice, and BIP39's optional passphrase.
#[derive(Default)]
pub struct Making {
    name: String,
    first: Typed,
    second: Typed,
    passphrase: Typed,
    error: Option<String>,
}

/// A wallet made, its words not yet confirmed.
pub struct Made {
    name: String,
    vault: Vault,
    phrase: Phrase,
    passphrase: Typed,
}

/// A wallet the lock screen opened.
pub struct Opened {
    pub vault: Vault,
    pub book: Book,
}

/// How far new words are from being written down: shown, or three of them asked back.
#[derive(Default)]
pub enum Writing {
    #[default]
    Shown,
    Asked { asked: [usize; 3], answers: [Typed; 3], error: Option<String> },
}

impl Lock {
    /// Asking for what opens a wallet, afresh.
    pub fn locked() -> Lock {
        Lock::Locked {
            password: Typed::default(),
            error: None,
            check: None,
            prompted: None,
            gone: None,
            removing: None,
        }
    }

    /// Where to start: what this install is, the first time; then the password of the wallet last
    /// used, or making the first.
    pub fn start(settings: &Settings) -> Lock {
        match (settings.role, settings.wallets.is_empty()) {
            (None, _) => Lock::Role,
            (Some(_), true) => Lock::Choose,
            (Some(_), false) => Lock::locked(),
        }
    }
}

/// Draws the lock screen's card. Returns the wallet it opened.
pub fn show(ui: &mut Ui, palette: &Palette, settings: &mut Settings, lock: &mut Lock) -> Option<Opened> {
    let mut opened = None;
    let mut next = None;
    let mut confirmed = false;

    theme::card(ui, palette, |ui| match lock {
        Lock::Role => {
            theme::caps(ui, palette, "This device");
            ui.label(RichText::new("What will it be? It stays that way: changing it takes removing its wallets.").color(palette.soft));
            ui.add_space(10.0);
            let choices = [
                (
                    Role::Online,
                    "Talks to a node. Its wallets hold their keys, or watch an offline signer's accounts and send what it signs, side by side.",
                ),
                (Role::Signer, "Holds the keys and never goes online. It signs what a watch-only wallet asks, by QR code. Keep the device offline."),
            ];

            for (role, what) in choices {
                if theme::quiet(ui, palette, role.name()).clicked() {
                    settings.role = Some(role);
                    next = Some(Lock::Choose);
                }

                ui.label(RichText::new(what).small().color(palette.muted));
                ui.add_space(8.0);
            }
        }
        Lock::Locked { password, error, check, prompted, gone, removing } => {
            if settings.wallets.len() > 1 {
                theme::caps(ui, palette, "Wallet");
                let shown = settings.wallets.get(settings.wallet).map(|entry| entry.name.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("wallet").selected_text(shown).width(ui.available_width()).show_ui(ui, |ui| {
                    for (index, entry) in settings.wallets.iter().enumerate() {
                        ui.selectable_value(&mut settings.wallet, index, &entry.name);
                    }
                });
                ui.add_space(8.0);
            } else if let Some(entry) = settings.wallets.first() {
                ui.label(theme::strong(&entry.name, 16.0).color(palette.ink));
                ui.add_space(4.0);
            }

            theme::caps(ui, palette, "Unlock");
            let guard = settings.wallets.get(settings.wallet).map_or(Guard::Password, WalletEntry::guard);
            let kind = settings.wallets.get(settings.wallet).and_then(biometrics::kind_of).unwrap_or(Kind::Biometrics);
            let biometric = biometrics::enrolled(ui.ctx(), settings);

            // A record made on another kind of device can never open here.
            if guard.needs_hardware() && settings.wallets.get(settings.wallet).is_some_and(|entry| !biometrics::kept_here(entry)) {
                *gone = Some(settings.wallet);
            }

            // What a check came to: what opens it, with the password if it needs that too; or why not.
            if let Some(came) = check.as_ref().and_then(Check::take) {
                *check = None;

                match came {
                    Ok(secret) => match settings.wallets.get(settings.wallet).map(|entry| biometrics::open_entry(entry, password, &secret)) {
                        Some(Ok((vault, book))) => opened = Some(Opened { vault, book }),
                        Some(Err(why)) => *error = Some(why),
                        None => *error = Some("there is no such wallet".to_string()),
                    },
                    // Turned down, it stays as it was, to try again.
                    Err(Failure::Canceled) => {}
                    // Nothing is written, and the state is only here, for one that needs hardware.
                    Err(Failure::Invalidated) if guard.needs_hardware() => *gone = Some(settings.wallet),
                    Err(Failure::Invalidated) => *error = Some(biometrics::invalidated(ui.ctx(), settings)),
                    Err(Failure::Failed(why)) => *error = Some(why),
                }
            }

            let lost = *gone == Some(settings.wallet);

            if lost {
                ui.colored_label(palette.bad, guard::gone(kind));
                ui.add_space(6.0);
            } else if guard.needs_hardware()
                && let Availability::Unavailable(why) = biometrics::bound(ui.ctx())
            {
                ui.label(RichText::new(format!("Cannot be asked for right now: {why}. Trying again changes nothing here.")).small().color(palette.warn));
                ui.add_space(4.0);
            }

            if guard != Guard::Both
                && let Some((enrolled, kind)) = &biometric
            {
                let name = settings.wallets.get(settings.wallet).map(|entry| entry.name.clone()).unwrap_or_default();
                // The browser shows a passkey's prompt only right after a tap.
                let by_itself = !cfg!(target_arch = "wasm32") && *prompted != Some(settings.wallet) && opened.is_none() && !lost;
                let tapped = theme::primary(ui, palette, &format!("Unlock with {}", kind.name()), check.is_none()).clicked();

                if (by_itself || tapped) && check.is_none() {
                    *prompted = Some(settings.wallet);
                    *error = None;
                    *check = Some(Check::start(ui.ctx(), enrolled, &format!("Unlock {name}")));
                }

                if check.is_some() {
                    ui.label(RichText::new(format!("Waiting for {}…", kind.name())).small().color(palette.muted));
                }

                ui.add_space(6.0);

                if guard == Guard::Either {
                    ui.label(RichText::new("Or its password:").small().color(palette.muted));
                }
            }

            let unprotected = settings.wallets.get(settings.wallet).is_some_and(WalletEntry::opens_with_a_tap);
            let enter = match (guard, unprotected) {
                // Nothing to type: it has no password.
                (Guard::Biometric, _) => false,
                (_, true) => {
                    ui.label(RichText::new("This wallet has no password: anyone who can read this app's storage can open it, biometrics or not. Settings can set a password, or make it need biometrics.").small().color(palette.warn));
                    false
                }
                (_, false) => {
                    let field = secret(ui, egui::TextEdit::singleline(&mut **password).password(true).hint_text("password").desired_width(f32::INFINITY));

                    field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))
                }
            };

            if let Some(error) = error {
                ui.colored_label(palette.bad, error.as_str());
            }

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                match guard {
                    Guard::Biometric => {}
                    // The check comes after the password, so both are asked for together.
                    Guard::Both => {
                        let ready = !password.is_empty() && check.is_none();
                        let tapped = theme::primary(ui, palette, &format!("Unlock with {}", kind.name()), ready).clicked();

                        if (tapped || enter) && ready
                            && let Some((enrolled, _)) = &biometric
                        {
                            let name = settings.wallets.get(settings.wallet).map(|entry| entry.name.clone()).unwrap_or_default();
                            *error = None;
                            *check = Some(Check::start(ui.ctx(), enrolled, &format!("Unlock {name}")));
                        }
                    }
                    Guard::Bare | Guard::Password | Guard::Either => {
                        let unlock = if unprotected { "Open" } else { "Unlock" };

                        if theme::primary(ui, palette, unlock, unprotected || !password.is_empty()).clicked() || enter {
                            if unprotected {
                                password.clear();
                            }

                            match settings.wallets.get(settings.wallet).map(|entry| Vault::open(&entry.vault, password)) {
                                Some(Ok((vault, book))) => opened = Some(Opened { vault, book }),
                                Some(Err(why)) => *error = Some(why),
                                None => *error = Some("there is no such wallet".to_string()),
                            }
                        }
                    }
                }

                if theme::quiet(ui, palette, "Add a wallet").clicked() {
                    next = Some(Lock::Choose);
                }
            });

            if guard == Guard::Both && check.is_some() {
                ui.label(RichText::new(format!("Waiting for {}…", kind.name())).small().color(palette.muted));
            }

            // Stuck with nothing left to give, it can still be taken off this device.
            if guard.needs_hardware() {
                if removing.as_ref().is_some_and(|(at, _)| *at != settings.wallet) {
                    *removing = None;
                }

                ui.add_space(8.0);

                match removing {
                    None => {
                        if theme::quiet(ui, palette, if lost { "Remove this wallet" } else { "It will not open" }).clicked() {
                            *removing = Some((settings.wallet, String::new()));
                        }
                    }
                    Some((at, yes)) => {
                        if !lost {
                            ui.label(RichText::new(guard::will_not_open(kind)).small().color(palette.soft));
                            ui.add_space(4.0);
                        }

                        let name = settings.wallets.get(*at).map(|entry| entry.name.clone()).unwrap_or_default();
                        ui.label(RichText::new(guard::removal_preview(&name)).color(palette.bad));
                        ui.add(egui::TextEdit::singleline(yes).hint_text("type yes").desired_width(f32::INFINITY));
                        let (mut remove, mut keep) = (false, false);
                        ui.horizontal_wrapped(|ui| {
                            remove = theme::primary(ui, palette, "Remove it", guard::confirmed(yes)).clicked();
                            keep = theme::quiet(ui, palette, "Keep it").clicked();
                        });

                        if remove && guard::confirmed(yes) {
                            guard::remove_wallet(settings, *at);
                            next = Some(Lock::start(settings));
                        } else if keep {
                            *removing = None;
                        }
                    }
                }
            }
        }
        Lock::Choose => {
            let role = settings.role();
            theme::caps(ui, palette, "A wallet");

            match role {
                Role::Online => ui.label(RichText::new("From 12 words: new ones, or ones you wrote down. Or watch-only: an offline signer's account, read off its Pair page.").color(palette.soft)),
                Role::Signer => ui.label(RichText::new("Its accounts come from 12 words, and from more once it is open. Make new ones, or bring a wallet back from words you wrote down.").color(palette.soft)),
            };
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, palette, "New wallet", true).clicked() {
                    next = Some(Lock::New(Making::default()));
                }

                if theme::quiet(ui, palette, "Restore from words").clicked() {
                    next = Some(Lock::Restore {
                        making: Making::default(),
                        words: Typed::default(),
                    });
                }

                if role == Role::Online && theme::quiet(ui, palette, "Pair with a signer").clicked() {
                    next = Some(Lock::Pair {
                        making: Making::default(),
                        scan: Box::default(),
                        paired: None,
                    });
                }

                if !settings.wallets.is_empty() && theme::quiet(ui, palette, "Back").clicked() {
                    next = Some(Lock::locked());
                }
            });

            // Only a device with no wallets yet can become something else.
            if settings.wallets.is_empty() {
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("This device: {}.", role.name())).small().color(palette.muted));

                    if theme::quiet(ui, palette, "Change").clicked() {
                        next = Some(Lock::Role);
                    }
                });
            }
        }
        Lock::Pair { making, scan, paired } => {
            theme::caps(ui, palette, "Pair with a signer");

            match paired {
                None => {
                    ui.label(RichText::new("On the signer, open Pair and show the account. Read its code here.").color(palette.soft));
                    ui.add_space(10.0);

                    match scan.show(ui, palette) {
                        Some(Message::Pair(pairing)) => *paired = Some(pairing),
                        Some(other) => scan.refuse(&other, "an account to pair with"),
                        None => {}
                    }
                }
                Some(pairing) => {
                    ui.label(theme::strong(&pairing.name, 16.0).color(palette.ink));

                    for key in &pairing.keys {
                        ui.label(theme::mono(crate::format::short(&key.address().to_string())).color(palette.soft));
                    }

                    ui.add_space(8.0);
                    ui.label(RichText::new("This wallet holds these public keys alone: it signs nothing, and asks the signer for each signature.").small().color(palette.muted));
                    ui.add_space(6.0);
                    making_fields(ui, palette, making, settings.wallets.len(), false);
                    ui.add_space(6.0);
                }
            }

            ui.horizontal_wrapped(|ui| {
                if let Some(pairing) = paired.as_ref()
                    && theme::primary(ui, palette, "Watch it", true).clicked()
                {
                    match password(making).and_then(|password| Vault::create(&password)) {
                        Ok(vault) => opened = Some(keep_watched(settings, name(making, settings.wallets.len()), vault, pairing)),
                        Err(why) => making.error = Some(why),
                    }
                }

                if theme::quiet(ui, palette, "Back").clicked() {
                    next = Some(Lock::Choose);
                }
            });
        }
        Lock::New(making) => {
            theme::caps(ui, palette, "New wallet");
            making_fields(ui, palette, making, settings.wallets.len(), true);
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, palette, "Make the words", true).clicked() {
                    match made(making, settings.wallets.len()) {
                        Ok(made) => next = Some(Lock::Words { made, writing: Writing::default() }),
                        Err(why) => making.error = Some(why),
                    }
                }

                if theme::quiet(ui, palette, "Back").clicked() {
                    next = Some(Lock::Choose);
                }
            });
        }
        Lock::Words { made, writing } => confirmed = write_down(ui, palette, &made.phrase, writing, "Open the wallet"),
        Lock::Restore { making, words } => {
            theme::caps(ui, palette, "Restore from words");
            ui.label(RichText::new("The 12 or 24 words, in order, with spaces between.").small().color(palette.muted));
            secret(ui, egui::TextEdit::multiline(&mut **words).desired_rows(3).desired_width(f32::INFINITY).hint_text("word word word …"));
            ui.add_space(6.0);
            making_fields(ui, palette, making, settings.wallets.len(), true);
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, palette, "Restore", true).clicked() {
                    let restored = read_words(words).and_then(|phrase| Ok((phrase, Vault::create(&password(making)?)?)));

                    match restored {
                        Ok((phrase, vault)) => opened = Some(keep(settings, name(making, settings.wallets.len()), vault, &phrase, making.passphrase.to_string())),
                        Err(why) => making.error = Some(why),
                    }
                }

                if theme::quiet(ui, palette, "Back").clicked() {
                    next = Some(Lock::Choose);
                }
            });
        }
        Lock::Open => {}
    });

    if let Some(next) = next {
        *lock = next;
    }

    // The words asked back were right: the wallet is kept, and opens.
    if confirmed && let Lock::Words { made, .. } = std::mem::replace(lock, Lock::Open) {
        opened = Some(keep(settings, made.name, made.vault, &made.phrase, made.passphrase.to_string()));
        settings.just_made = true;
    }

    opened
}

/// New words to write down: shown, then three of them asked back, as a new wallet's are and as a
/// further phrase's are. Whether they were right, with done on the button that says so.
pub fn write_down(ui: &mut Ui, palette: &Palette, phrase: &Phrase, writing: &mut Writing, done: &str) -> bool {
    let mut confirmed = false;
    let mut next = None;

    match writing {
        Writing::Shown => {
            theme::caps(ui, palette, "Your words");
            ui.label(
                RichText::new("Write these down, in order, and keep them where only you can read them. They are the keys of every account made from them: anyone with them has those accounts, and without them a lost device loses them.")
                    .color(palette.soft),
            );
            ui.add_space(10.0);
            words_grid(ui, palette, phrase);
            ui.add_space(10.0);

            if theme::primary(ui, palette, "I wrote them down", true).clicked() {
                next = Some(Writing::Asked {
                    asked: three_of(phrase.words().len()),
                    answers: Default::default(),
                    error: None,
                });
            }
        }
        Writing::Asked { asked, answers, error } => {
            theme::caps(ui, palette, "Check them");
            ui.label(RichText::new("To be sure they are written down right: these words, from your list.").color(palette.soft));
            ui.add_space(6.0);

            for (position, answer) in asked.iter().zip(answers.iter_mut()) {
                // The word's number beside its field, the fields lined up under each other.
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(egui::vec2(72.0, ui.spacing().interact_size.y), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_width(72.0);
                        ui.label(RichText::new(format!("Word {}", position + 1)).color(palette.muted));
                    });
                    let width = ui.available_width();
                    secret(ui, egui::TextEdit::singleline(&mut **answer).desired_width(width));
                });
            }

            if let Some(error) = error {
                ui.colored_label(palette.bad, error.as_str());
            }

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, palette, done, true).clicked() {
                    match right(phrase, asked, answers) {
                        true => confirmed = true,
                        false => *error = Some("not the words listed: look again".to_string()),
                    }
                }

                if theme::quiet(ui, palette, "Show them again").clicked() {
                    next = Some(Writing::Shown);
                }
            });
        }
    }

    if let Some(next) = next {
        *writing = next;
    }

    confirmed
}

/// Reads words as someone typed them, saying what is wrong without repeating any of them: a typo
/// of a real word is most of one, and errors end up in places words should not.
pub fn read_words(typed: &str) -> Result<Phrase, String> {
    Phrase::parse(typed).map_err(|why| match why {
        MnemonicError::UnknownWord(_) => "one of them is not a word of the list: check each one's spelling".to_string(),
        other => other.to_string(),
    })
}

/// BIP39's optional passphrase, put away under Advanced.
pub fn passphrase_field(ui: &mut Ui, palette: &Palette, passphrase: &mut Typed) {
    egui::CollapsingHeader::new(RichText::new("Advanced").color(palette.soft)).id_salt("passphrase").show(ui, |ui| {
        ui.label(
            RichText::new("A BIP39 passphrase, kept with the words: the same words with another passphrase are another wallet. Lose it, and the words alone bring back nothing.")
                .small()
                .color(palette.warn),
        );
        secret(ui, egui::TextEdit::singleline(&mut **passphrase).password(true).hint_text("passphrase, or none").desired_width(f32::INFINITY));
    });
}

/// A name, a password twice, and, put away, BIP39's passphrase, for a wallet with words.
fn making_fields(ui: &mut Ui, palette: &Palette, making: &mut Making, wallets: usize, words: bool) {
    ui.add(egui::TextEdit::singleline(&mut making.name).hint_text(format!("name, such as Wallet {}", wallets + 1)).desired_width(f32::INFINITY));
    ui.add_space(4.0);
    ui.label(RichText::new("A password seals the wallet on this device. Nothing opens it without it. Settings can make it need biometrics instead, or as well.").small().color(palette.muted));
    secret(ui, egui::TextEdit::singleline(&mut *making.first).password(true).hint_text("password, or none").desired_width(f32::INFINITY));
    secret(ui, egui::TextEdit::singleline(&mut *making.second).password(true).hint_text("the same again").desired_width(f32::INFINITY));

    if making.first.is_empty() && making.second.is_empty() {
        ui.label(
            RichText::new("With none, it opens with a tap, and so does it for anyone who can read this app's storage: a phone's backup, or malware. Fine for small sums. Settings can set one later, or make it need biometrics.")
                .small()
                .color(palette.warn),
        );
    }
    ui.add_space(4.0);

    if !words {
        if let Some(error) = &making.error {
            ui.colored_label(palette.bad, error.as_str());
        }

        return;
    }

    passphrase_field(ui, palette, &mut making.passphrase);

    if let Some(error) = &making.error {
        ui.colored_label(palette.bad, error.as_str());
    }
}

/// The password chosen: none, or one at least 8 characters long, typed the same twice.
fn password(making: &Making) -> Result<Typed, String> {
    match (making.first.is_empty() || making.first.len() >= 8, making.first == making.second) {
        (false, _) => Err("use a password of at least 8 characters, or none".to_string()),
        (_, false) => Err("the two passwords do not match".to_string()),
        (true, true) => Ok(making.first.clone()),
    }
}

fn name(making: &Making, wallets: usize) -> String {
    match making.name.trim() {
        "" => format!("Wallet {}", wallets + 1),
        name => name.to_string(),
    }
}

/// A new wallet's vault and words.
fn made(making: &Making, wallets: usize) -> Result<Made, String> {
    let vault = Vault::create(&password(making)?)?;
    let phrase = Phrase::generate(12).map_err(|err| err.to_string())?;

    Ok(Made {
        name: name(making, wallets),
        vault,
        phrase,
        passphrase: making.passphrase.clone(),
    })
}

/// Whether the words asked back are the phrase's, whatever their case and spacing.
fn right(phrase: &Phrase, asked: &[usize; 3], answers: &[Typed; 3]) -> bool {
    let words = phrase.words();

    asked.iter().zip(answers).all(|(position, answer)| words.get(*position).is_some_and(|word| answer.trim().eq_ignore_ascii_case(word)))
}

/// Seals a new wallet with its first account, the phrase's account 0, and keeps it, as the wallet
/// in use.
fn keep(settings: &mut Settings, name: String, vault: Vault, phrase: &Phrase, passphrase: String) -> Opened {
    let book = Book {
        phrases: vec![Words::new(phrase.to_string(), passphrase)],
        accounts: vec![StoredAccount {
            name: "Account 1".to_string(),
            source: Source::Phrase { account: 0, extra: Vec::new(), from: None },
        }],
        watch: false,
        swaps: Vec::new(),
        ..Book::default()
    };

    match vault.seal(&book) {
        Ok(sealed) => {
            settings.wallets.push(WalletEntry {
                id: String::new(),
                name,
                vault: sealed,
                current: String::new(),
                biometric: None,
                unprotected: vault.bare(),
                born: None,
            });
            settings.wallet = settings.wallets.len() - 1;
        }
        Err(why) => log::warn!("could not seal the new wallet: {why}"),
    }

    Opened { vault, book }
}

/// Seals a new watch-only wallet with the signer's account it read, and keeps it, as the wallet in
/// use.
fn keep_watched(settings: &mut Settings, name: String, vault: Vault, pairing: &Pairing) -> Opened {
    let book = Book {
        phrases: Vec::new(),
        accounts: vec![StoredAccount {
            name: pairing.name.clone(),
            source: Source::Watch { keys: pairing.keys.iter().map(ToString::to_string).collect(), origin: pairing.origin },
        }],
        watch: true,
        swaps: Vec::new(),
        ..Book::default()
    };

    match vault.seal(&book) {
        Ok(sealed) => {
            settings.wallets.push(WalletEntry {
                id: String::new(),
                name,
                vault: sealed,
                current: String::new(),
                biometric: None,
                unprotected: vault.bare(),
                born: None,
            });
            settings.wallet = settings.wallets.len() - 1;
        }
        Err(why) => log::warn!("could not seal the new wallet: {why}"),
    }

    Opened { vault, book }
}

/// A phrase's words, numbered, down columns: two on a narrow screen, three otherwise.
pub fn words_grid(ui: &mut Ui, palette: &Palette, phrase: &Phrase) {
    let words = phrase.words();
    let columns = if ui.available_width() < 320.0 { 2 } else { 3 };
    let rows = words.len().div_ceil(columns);

    // Its id is its place's, so a grid on one page never shares a layout with one on another.
    egui::Grid::new(ui.scope_id().with("words")).num_columns(2 * columns).spacing([6.0, 8.0]).show(ui, |ui| {
        for row in 0..rows {
            for column in 0..columns {
                let position = column * rows + row;

                // Never broken across lines: a word is at most 8 letters, and one split reads as two.
                // A number sits close to its word and further from the word before it, so the
                // eight-letter ones do not run into the next column's number.
                if let Some(word) = words.get(position) {
                    ui.horizontal(|ui| {
                        if column > 0 {
                            ui.add_space(WORD_GAP);
                        }

                        ui.add(egui::Label::new(RichText::new(format!("{:>2}", position + 1)).monospace().color(palette.muted)).extend());
                    });
                    ui.add(egui::Label::new(RichText::new(*word).monospace().color(palette.ink)).extend());
                }
            }

            ui.end_row();
        }
    });
}

/// The room before each column of words but the first, besides the grid's own spacing.
const WORD_GAP: f32 = 10.0;

/// Three positions among this many, all different, in order.
fn three_of(count: usize) -> [usize; 3] {
    let count = count.max(3);
    let mut chosen = Vec::new();

    while chosen.len() < 3 {
        let mut bytes = [0; 4];
        let _ = getrandom::fill(&mut bytes);
        let position = u32::from_le_bytes(bytes) as usize % count;

        if !chosen.contains(&position) {
            chosen.push(position);
        }
    }

    chosen.sort_unstable();

    [chosen[0], chosen[1], chosen[2]]
}

#[cfg(test)]
mod tests {
    use {super::*, crate::biometrics::fake};

    #[test]
    fn three_different_words_are_asked_back_and_checked() {
        let phrase = Phrase::parse("test test test test test test test test test test test junk").unwrap();

        for _ in 0..50 {
            let asked = three_of(12);

            assert!(asked[0] < asked[1] && asked[1] < asked[2] && asked[2] < 12, "{asked:?}");
        }

        assert!(Typed::default().capacity() >= chainv4_ui::secret::ROOM, "room for 24 words, never moved");
        assert_eq!(*Typed::new("abc".to_string()).clone(), "abc");
        assert!(right(&phrase, &[0, 5, 11], &[Typed::new("test".to_string()), Typed::new(" TEST ".to_string()), Typed::new("junk".to_string())]));
        assert!(!right(&phrase, &[0, 5, 11], &[Typed::new("test".to_string()), Typed::new("test".to_string()), Typed::new("test".to_string())]));
    }

    #[test]
    fn what_is_wrong_with_words_typed_never_repeats_them() {
        let typed = "test test test test test test test test test test test jnuk";
        let why = read_words(typed).unwrap_err();

        assert!(!why.contains("jnuk") && !why.contains("test"), "{why}");
        assert_eq!(read_words("test test").unwrap_err(), "2 words: a phrase is 12, 15, 18, 21 or 24");
        assert!(read_words("test test test test test test test test test test test junk").is_ok());
    }

    #[test]
    fn a_kept_wallet_starts_with_the_phrases_first_account() {
        let mut settings = Settings::default();
        let phrase = Phrase::generate(12).unwrap();
        let opened = keep(&mut settings, "Mine".to_string(), Vault::create("correct horse").unwrap(), &phrase, String::new());

        assert_eq!((settings.wallets.len(), settings.wallet, settings.wallets[0].name.as_str()), (1, 0, "Mine"));
        assert_eq!(opened.book.accounts[0].source, Source::Phrase { account: 0, extra: Vec::new(), from: None });
        assert_eq!(Vault::open(&settings.wallets[0].vault, "correct horse").unwrap().1, opened.book);

        assert!(!settings.wallets[0].unprotected);

        let typed = |first: &str, second: &str| Making {
            first: Typed::new(first.to_string()),
            second: Typed::new(second.to_string()),
            ..Making::default()
        };

        assert_eq!(password(&typed("short", "short")), Err("use a password of at least 8 characters, or none".to_string()));
        assert_eq!(password(&typed("long enough", "long enougH")), Err("the two passwords do not match".to_string()));
        assert_eq!(password(&typed("", "")).map(|password| password.is_empty()), Ok(true), "none at all is a choice");
        assert!(password(&typed("", "something")).is_err());
    }

    #[test]
    fn a_wallet_without_a_password_opens_with_none_and_says_so() {
        let mut settings = Settings::default();
        let phrase = Phrase::generate(12).unwrap();
        let opened = keep(&mut settings, "Open".to_string(), Vault::create("").unwrap(), &phrase, String::new());

        assert!(settings.wallets[0].unprotected);
        assert_eq!(Vault::open(&settings.wallets[0].vault, "").unwrap().1, opened.book);
        assert!(Vault::open(&settings.wallets[0].vault, "").unwrap().0.bare());
        assert!(Vault::open(&settings.wallets[0].vault, "a guess").is_err());
    }

    #[test]
    fn a_watch_only_wallet_sits_beside_one_with_keys() {
        let mut settings = Settings::default();
        settings.role = Some(Role::Online);
        let phrase = Phrase::generate(12).unwrap();
        keep(&mut settings, "Mine".to_string(), Vault::create("correct horse").unwrap(), &phrase, String::new());
        let signer = crate::keys::derive(&phrase.seed(""), 0);
        let pairing = Pairing {
            name: "Cold".to_string(),
            keys: signer.iter().map(|key| key.public()).collect(),
            origin: None,
        };
        let watched = keep_watched(&mut settings, "Cold storage".to_string(), Vault::create("battery staple").unwrap(), &pairing);

        assert_eq!(settings.wallets.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["Mine", "Cold storage"]);
        assert_eq!(settings.wallet, 1, "the one just paired is in use");

        let (_, mine) = Vault::open(&settings.wallets[0].vault, "correct horse").unwrap();
        let (_, cold) = Vault::open(&settings.wallets[1].vault, "battery staple").unwrap();

        assert!(!mine.watch && !mine.phrases.is_empty());
        assert_eq!(cold, watched.book);
        assert!(cold.watch && cold.phrases.is_empty());

        let holders = crate::keys::open(&cold).unwrap();

        assert!(holders[0].keys.is_empty(), "nothing to sign with");
        assert_eq!(holders[0].addresses(), signer.iter().map(|key| key.public().address()).collect::<Vec<_>>());
    }

    /// A wallet of each guard that needs hardware on a fresh fake device, with what opens it.
    fn needing(guard: Guard) -> (Settings, Book) {
        use crate::vault::fresh_secret;

        fake::fresh();
        let book = Book { phrases: vec![Words::new("test test test test test test test test test test test junk".to_string(), String::new())], ..Book::default() };
        let secret = fresh_secret().unwrap();
        let vault = match guard {
            Guard::Both => Vault::both("correct horse", &secret).unwrap(),
            _ => Vault::keyed(&secret).unwrap(),
        };
        let sealed = vault.seal(&book).unwrap();
        let entry = WalletEntry { id: String::new(), name: "Mine".to_string(), vault: sealed, current: String::new(), biometric: guard::tagged(guard, &fake::keep(&secret)), unprotected: false, born: None };

        (Settings::with_wallets(vec![entry]), book)
    }

    /// Draws the lock screen, a finger tapping this text on the way if one is named: what it said,
    /// how wide it came out, and the wallet it opened.
    fn drawn_lock(size: (f32, f32), tap: Option<&str>, settings: &mut Settings, lock: &mut Lock) -> (String, f32, Option<Opened>) {
        let mut opened = None;
        let palette = theme::DARK;
        let (width, said) = crate::views::drawn::screen(size, tap, |ui| {
            if let Some(came) = show(ui, &palette, settings, lock) {
                opened = Some(came);
            }
        });

        (said, width, opened)
    }

    const PHONE: (f32, f32) = (360.0, 740.0);

    #[test]
    fn a_wallet_with_biometrics_alone_opens_with_them_and_asks_for_no_password() {
        let (mut settings, book) = needing(Guard::Biometric);
        let mut lock = Lock::locked();
        let (said, _, opened) = drawn_lock(PHONE, None, &mut settings, &mut lock);

        // The prompt showed by itself, and nothing is asked for that is not there.
        assert!(!said.contains("password"), "{said}");
        assert!(opened.is_some_and(|opened| opened.book == book));
        assert_eq!(fake::device(|device| device.prompts), 1);
    }

    #[test]
    fn a_wallet_needing_both_waits_for_its_password_and_a_wrong_one_is_wrong_even_past_the_check() {
        let (mut settings, book) = needing(Guard::Both);
        let mut lock = Lock::locked();
        let (said, _, opened) = drawn_lock(PHONE, None, &mut settings, &mut lock);

        assert!(said.contains("password"), "{said}");
        assert!(opened.is_none());
        assert_eq!(fake::device(|device| device.prompts), 0, "the check comes after the password");

        let Lock::Locked { password, .. } = &mut lock else { unreachable!() };
        *password = Typed::new("not it at all".to_string());
        let (said, _, opened) = drawn_lock(PHONE, Some("Unlock with fingerprint or face"), &mut settings, &mut lock);

        assert!(opened.is_none());
        assert!(said.contains("wrong password"), "{said}");
        assert_eq!(fake::device(|device| device.prompts), 1);

        let Lock::Locked { password, .. } = &mut lock else { unreachable!() };
        *password = Typed::new("correct horse".to_string());
        let (_, _, opened) = drawn_lock(PHONE, Some("Unlock with fingerprint or face"), &mut settings, &mut lock);

        assert!(opened.is_some_and(|opened| opened.book == book));
    }

    #[test]
    fn a_prompt_turned_down_or_failing_is_no_lockout_and_writes_nothing() {
        let (mut settings, _) = needing(Guard::Biometric);
        let before = settings.wallets.clone();
        let mut lock = Lock::locked();
        fake::device(|device| device.answers.push(fake::Answer::Canceled));
        let (said, _, opened) = drawn_lock(PHONE, None, &mut settings, &mut lock);

        assert!(opened.is_none());
        assert!(!said.contains("gone from this device"), "{said}");
        assert!(said.contains("Unlock with fingerprint or face"), "there to try again: {said}");
        assert!(matches!(&lock, Lock::Locked { gone: None, .. }));

        fake::device(|device| device.answers.push(fake::Answer::Failed("too many tries")));
        let (said, _, opened) = drawn_lock(PHONE, Some("Unlock with fingerprint or face"), &mut settings, &mut lock);

        assert!(opened.is_none());
        assert!(said.contains("too many tries") && !said.contains("gone from this device"), "{said}");
        assert_eq!(settings.wallets, before);

        // And it opens once it passes.
        let (_, _, opened) = drawn_lock(PHONE, Some("Unlock with fingerprint or face"), &mut settings, &mut lock);

        assert!(opened.is_some());
    }

    #[test]
    fn a_lost_key_keeps_the_wallet_writes_nothing_says_why_and_removal_takes_a_typed_yes_alone() {
        for guard in [Guard::Biometric, Guard::Both] {
            let (mut settings, _) = needing(guard);
            let before = settings.wallets.clone();
            fake::lose_every_key();
            let mut lock = Lock::locked();

            if guard == Guard::Both {
                let Lock::Locked { password, .. } = &mut lock else { unreachable!() };
                *password = Typed::new("correct horse".to_string());
                drawn_lock(PHONE, Some("Unlock with fingerprint or face"), &mut settings, &mut lock);
            }

            let (said, _, opened) = drawn_lock(PHONE, None, &mut settings, &mut lock);

            assert!(opened.is_none(), "{guard:?}");
            assert!(said.contains("gone from this device") && said.contains("Restore from words"), "{guard:?}: {said}");
            assert!(said.contains("Adding a fingerprint or face"), "{guard:?}: and why: {said}");
            assert_eq!(settings.wallets, before, "{guard:?}: nothing written");
            assert!(settings.retired.is_empty() && !settings.persist_now, "{guard:?}");
            assert!(fake::device(|device| device.forgotten.is_empty()), "{guard:?}");

            // The way to restore stays there, and so does the way out.
            assert!(said.contains("Add a wallet") && said.contains("Remove this wallet"), "{guard:?}: {said}");

            let (said, _, _) = drawn_lock(PHONE, Some("Remove this wallet"), &mut settings, &mut lock);

            assert!(said.contains("Mine, and every account and seed phrase sealed in it") && said.contains("out of reach for good"), "{guard:?}: {said}");

            // A tap without a yes takes nothing; nor does anything else typed.
            for typed in ["", "y", "yes please"] {
                let Lock::Locked { removing: Some((_, yes)), .. } = &mut lock else { unreachable!() };
                *yes = typed.to_string();
                drawn_lock(PHONE, Some("Remove it"), &mut settings, &mut lock);

                assert_eq!(settings.wallets.len(), 1, "{guard:?}: {typed:?}");
            }

            let Lock::Locked { removing: Some((_, yes)), .. } = &mut lock else { unreachable!() };
            *yes = "yes".to_string();
            drawn_lock(PHONE, Some("Remove it"), &mut settings, &mut lock);

            assert!(settings.wallets.is_empty(), "{guard:?}");
            assert_eq!(settings.retired, before[0].biometric.iter().cloned().collect::<Vec<_>>(), "{guard:?}");
            assert!(settings.persist_now, "{guard:?}");
            assert!(matches!(lock, Lock::Choose), "{guard:?}: on to making or restoring one");
        }
    }

    #[test]
    fn a_wallet_with_a_password_or_none_has_nothing_new_on_its_lock_screen() {
        let phrase = Phrase::generate(12).unwrap();
        let mut settings = Settings::with_wallets(Vec::new());
        keep(&mut settings, "Mine".to_string(), Vault::create("correct horse").unwrap(), &phrase, String::new());
        let mut lock = Lock::locked();
        let (said, _, _) = drawn_lock(PHONE, None, &mut settings, &mut lock);

        assert!(!said.contains("It will not open") && !said.contains("Remove"), "{said}");

        let mut settings = Settings::with_wallets(Vec::new());
        keep(&mut settings, "Open".to_string(), Vault::create("").unwrap(), &phrase, String::new());
        let (said, _, _) = drawn_lock(PHONE, None, &mut settings, &mut Lock::locked());

        assert!(said.contains("no password") && said.contains("need biometrics"), "the warning stays, and points on: {said}");
    }

    #[test]
    fn a_wallet_left_with_either_keeps_opening_either_way_and_a_lost_key_falls_back_to_its_password() {
        fake::fresh();
        let phrase = Phrase::generate(12).unwrap();
        let vault = Vault::create("correct horse").unwrap();
        let enrolled = fake::keep(&vault.key());
        let mut settings = Settings::with_wallets(Vec::new());
        keep(&mut settings, "Mine".to_string(), vault, &phrase, String::new());
        settings.wallets[0].biometric = Some(enrolled.0);

        let (said, _, opened) = drawn_lock(PHONE, None, &mut settings, &mut Lock::locked());

        assert!(opened.is_some(), "by biometrics, by itself");
        assert!(said.contains("password"), "{said}");

        let mut lock = Lock::locked();
        let Lock::Locked { prompted, password, .. } = &mut lock else { unreachable!() };
        *prompted = Some(0);
        *password = Typed::new("correct horse".to_string());
        assert!(drawn_lock(PHONE, Some("Unlock"), &mut settings, &mut lock).2.is_some(), "by its password");

        fake::lose_every_key();
        let (said, _, opened) = drawn_lock(PHONE, None, &mut settings, &mut Lock::locked());

        assert!(opened.is_none());
        assert!(said.contains("biometrics are off"), "{said}");
        assert_eq!(settings.wallets[0].guard(), Guard::Password, "as it always did");
        assert!(!said.contains("Remove this wallet"), "and it has its password, so nothing is stranded: {said}");
    }

    #[test]
    fn where_biometrics_read_as_unavailable_a_wallet_that_needs_them_says_so_and_stays_as_it_was() {
        let (mut settings, _) = needing(Guard::Biometric);
        let before = settings.wallets.clone();
        fake::device(|device| {
            device.bound = chainv4_ui::biometric::Availability::Unavailable("no finger is enrolled".to_string());
            device.answers.push(fake::Answer::Failed("no finger is enrolled"));
        });
        let (said, _, opened) = drawn_lock(PHONE, None, &mut settings, &mut Lock::locked());

        assert!(opened.is_none());
        assert!(said.contains("no finger is enrolled"), "{said}");
        assert_eq!(settings.wallets, before);
    }

    #[test]
    fn the_lock_screen_of_each_way_of_opening_draws_on_every_screen() {
        for size in crate::views::drawn::SCREENS {
            for guard in [Guard::Biometric, Guard::Both] {
                let (mut settings, _) = needing(guard);
                fake::lose_every_key();
                let mut lock = Lock::locked();
                let Lock::Locked { gone, removing, error, .. } = &mut lock else { unreachable!() };
                *gone = Some(0);
                *removing = Some((0, String::new()));
                *error = Some("Something went wrong.".to_string());
                let (_, width, _) = drawn_lock(size, None, &mut settings, &mut lock);

                assert!(width <= size.0 + 0.5, "{guard:?}: {width} on a screen {} wide", size.0);

                let (mut settings, _) = needing(guard);
                let mut lock = Lock::locked();
                let Lock::Locked { prompted, removing, .. } = &mut lock else { unreachable!() };
                *prompted = Some(0);
                *removing = Some((0, String::new()));
                let (_, width, _) = drawn_lock(size, None, &mut settings, &mut lock);

                assert!(width <= size.0 + 0.5, "{guard:?}: {width} on a screen {} wide", size.0);
            }
        }
    }
}
