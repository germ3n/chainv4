//! The lock screen: which wallet, and its password. Or a wallet more: from new words, shown once and
//! three of them asked back, or from words written down before; or, online, a watch-only one, from
//! an offline signer's account read off its screen. Each wallet is a vault of its own, with a
//! password of its own. On its first launch, it asks whether this install is an offline signer. A
//! wallet with biometrics on opens with them too: on Android and macOS the prompt shows as the
//! screen does, and in the browser, which shows a passkey's only for a tap, at the button.

use {
    crate::airgap::{Message, Pairing},
    crate::biometrics::{self, Check},
    chainv4_ui::biometric::Failure,
    crate::app::{Role, Settings, WalletEntry},
    crate::theme::{self, Palette},
    crate::vault::{Book, Source, StoredAccount, Vault, Words},
    crate::views::scan::Scan,
    chainv4::crypto::mnemonic::Phrase,
    egui::{RichText, Ui},
    zeroize::Zeroizing,
};

/// What is typed into a password or words field: wiped from memory as it drops.
pub type Typed = Zeroizing<String>;

/// Where the lock screen is.
pub enum Lock {
    /// What this install is, asked once.
    Role,
    /// Asking for the password of the wallet picked, or its biometrics: the check under way, and
    /// the wallet the prompt showed for by itself, so it shows once.
    Locked { password: Typed, error: Option<String>, check: Option<Check>, prompted: Option<usize> },
    /// Making a wallet: from new words, or from words written down before.
    Choose,
    New(Making),
    /// New words, to write down.
    Words(Made),
    /// Three of them, asked back.
    Confirm { made: Made, asked: [usize; 3], answers: [Typed; 3], error: Option<String> },
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

/// Where a step leads that takes what the screen holds along with it.
enum Step {
    /// From the words to asking three of them back.
    Check,
    /// From asking back to the words again.
    Reread,
    /// The words asked back were right: the wallet is kept, and opens.
    Keep,
}

impl Lock {
    /// Asking for a password, or biometrics, afresh.
    pub fn locked() -> Lock {
        Lock::Locked {
            password: Typed::default(),
            error: None,
            check: None,
            prompted: None,
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
    let mut step = None;

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
        Lock::Locked { password, error, check, prompted } => {
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
            let biometric = biometrics::enrolled(ui.ctx(), settings);

            // What a check came to: the vault's key, opening it; or why not.
            if let Some(came) = check.as_ref().and_then(Check::take) {
                *check = None;

                match came {
                    Ok(key) => match settings.wallets.get(settings.wallet).map(|entry| Vault::open_with_key(&entry.vault, &key)) {
                        Some(Ok((vault, book))) => opened = Some(Opened { vault, book }),
                        Some(Err(why)) => *error = Some(why),
                        None => *error = Some("there is no such wallet".to_string()),
                    },
                    Err(Failure::Canceled) => {}
                    Err(Failure::Invalidated) => {
                        biometrics::turn_off(ui.ctx(), settings);
                        *error = Some(biometrics::CHANGED.to_string());
                    }
                    Err(Failure::Failed(why)) => *error = Some(why),
                }
            }

            if let Some((enrolled, kind)) = &biometric {
                let name = settings.wallets.get(settings.wallet).map(|entry| entry.name.clone()).unwrap_or_default();
                // The browser shows a passkey's prompt only right after a tap.
                let by_itself = !cfg!(target_arch = "wasm32") && *prompted != Some(settings.wallet) && opened.is_none();
                let tapped = theme::primary(ui, palette, &format!("Unlock with {}", kind.name()), check.is_none()).clicked();

                if (by_itself || tapped) && check.is_none() {
                    *prompted = Some(settings.wallet);
                    *check = Some(Check::start(ui.ctx(), enrolled, &format!("Unlock {name}")));
                }

                if check.is_some() {
                    ui.label(RichText::new(format!("Waiting for {}…", kind.name())).small().color(palette.muted));
                }

                ui.add_space(6.0);
                ui.label(RichText::new("Or its password:").small().color(palette.muted));
            }

            let unprotected = settings.wallets.get(settings.wallet).is_some_and(|entry| entry.unprotected);
            let enter = match unprotected {
                // No password to type: it opens with a tap.
                true => {
                    ui.label(RichText::new("This wallet has no password: anyone who can read this app's storage can open it. Set one in Settings.").small().color(palette.warn));
                    false
                }
                false => {
                    let field = ui.add(egui::TextEdit::singleline(&mut **password).password(true).hint_text("password").desired_width(f32::INFINITY));

                    field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))
                }
            };

            if let Some(error) = error {
                ui.colored_label(palette.bad, error.as_str());
            }

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
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

                if theme::quiet(ui, palette, "Add a wallet").clicked() {
                    next = Some(Lock::Choose);
                }
            });
        }
        Lock::Choose => {
            let role = settings.role();
            theme::caps(ui, palette, "A wallet");

            match role {
                Role::Online => ui.label(RichText::new("From 12 words: new ones, or ones you wrote down. Or watch-only: an offline signer's account, read off its Pair page.").color(palette.soft)),
                Role::Signer => ui.label(RichText::new("Its accounts all come from 12 words. Make new ones, or bring a wallet back from words you wrote down.").color(palette.soft)),
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
                        Ok(made) => next = Some(Lock::Words(made)),
                        Err(why) => making.error = Some(why),
                    }
                }

                if theme::quiet(ui, palette, "Back").clicked() {
                    next = Some(Lock::Choose);
                }
            });
        }
        Lock::Words(made) => {
            theme::caps(ui, palette, "Your words");
            ui.label(
                RichText::new("Write these down, in order, and keep them where only you can read them. They are every account's keys: anyone with them has it all, and without them a lost device is a lost wallet.")
                    .color(palette.soft),
            );
            ui.add_space(10.0);
            words_grid(ui, palette, &made.phrase);
            ui.add_space(10.0);

            if theme::primary(ui, palette, "I wrote them down", true).clicked() {
                step = Some(Step::Check);
            }
        }
        Lock::Confirm { made, asked, answers, error } => {
            theme::caps(ui, palette, "Check them");
            ui.label(RichText::new("To be sure they are written down right: these words, from your list.").color(palette.soft));
            ui.add_space(6.0);

            for (position, answer) in asked.iter().zip(answers.iter_mut()) {
                ui.horizontal(|ui| {
                    ui.add_sized([72.0, 24.0], egui::Label::new(RichText::new(format!("Word {}", position + 1)).color(palette.muted)));
                    ui.add(egui::TextEdit::singleline(&mut **answer).desired_width(f32::INFINITY));
                });
            }

            if let Some(error) = error {
                ui.colored_label(palette.bad, error.as_str());
            }

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, palette, "Open the wallet", true).clicked() {
                    match right(&made.phrase, asked, answers) {
                        true => step = Some(Step::Keep),
                        false => *error = Some("not the words listed: look again".to_string()),
                    }
                }

                if theme::quiet(ui, palette, "Show them again").clicked() {
                    step = Some(Step::Reread);
                }
            });
        }
        Lock::Restore { making, words } => {
            theme::caps(ui, palette, "Restore from words");
            ui.label(RichText::new("The 12 or 24 words, in order, with spaces between.").small().color(palette.muted));
            ui.add(egui::TextEdit::multiline(&mut **words).desired_rows(3).desired_width(f32::INFINITY).hint_text("word word word …"));
            ui.add_space(6.0);
            making_fields(ui, palette, making, settings.wallets.len(), true);
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, palette, "Restore", true).clicked() {
                    let restored = Phrase::parse(words)
                        .map_err(|err| err.to_string())
                        .and_then(|phrase| Ok((phrase, Vault::create(&password(making)?)?)));

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

    // The steps that take the wallet made along.
    if let Some(step) = step {
        *lock = match (step, std::mem::replace(lock, Lock::Open)) {
            (Step::Check, Lock::Words(made)) => Lock::Confirm {
                asked: three_of(made.phrase.words().len()),
                made,
                answers: Default::default(),
                error: None,
            },
            (Step::Reread, Lock::Confirm { made, .. }) => Lock::Words(made),
            (Step::Keep, Lock::Confirm { made, .. }) => {
                opened = Some(keep(settings, made.name, made.vault, &made.phrase, made.passphrase.to_string()));

                Lock::Open
            }
            (_, unchanged) => unchanged,
        };
    }

    opened
}

/// A name, a password twice, and, put away, BIP39's passphrase, for a wallet with words.
fn making_fields(ui: &mut Ui, palette: &Palette, making: &mut Making, wallets: usize, words: bool) {
    ui.add(egui::TextEdit::singleline(&mut making.name).hint_text(format!("name, such as Wallet {}", wallets + 1)).desired_width(f32::INFINITY));
    ui.add_space(4.0);
    ui.label(RichText::new("A password seals the wallet on this device. Nothing opens it without it.").small().color(palette.muted));
    ui.add(egui::TextEdit::singleline(&mut *making.first).password(true).hint_text("password, or none").desired_width(f32::INFINITY));
    ui.add(egui::TextEdit::singleline(&mut *making.second).password(true).hint_text("the same again").desired_width(f32::INFINITY));

    if making.first.is_empty() && making.second.is_empty() {
        ui.label(
            RichText::new("With none, it opens with a tap, and so does it for anyone who can read this app's storage: a phone's backup, or malware. Fine for small sums; turn biometrics on, or set one later in Settings.")
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

    egui::CollapsingHeader::new(RichText::new("Advanced").color(palette.soft)).id_salt("passphrase").show(ui, |ui| {
        ui.label(
            RichText::new("A BIP39 passphrase, kept with the words: the same words with another passphrase are another wallet. Lose it, and the words alone bring back nothing.")
                .small()
                .color(palette.warn),
        );
        ui.add(egui::TextEdit::singleline(&mut *making.passphrase).password(true).hint_text("passphrase, or none").desired_width(f32::INFINITY));
    });

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
        phrase: Some(Words {
            words: phrase.to_string(),
            passphrase,
        }),
        accounts: vec![StoredAccount {
            name: "Account 1".to_string(),
            source: Source::Phrase { account: 0 },
        }],
        watch: false,
        swaps: Vec::new(),
    };

    match vault.seal(&book) {
        Ok(sealed) => {
            settings.wallets.push(WalletEntry {
                name,
                vault: sealed,
                current: String::new(),
                biometric: None,
                unprotected: vault.bare(),
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
        phrase: None,
        accounts: vec![StoredAccount {
            name: pairing.name.clone(),
            source: Source::Watch { keys: pairing.keys.iter().map(ToString::to_string).collect(), origin: pairing.origin },
        }],
        watch: true,
        swaps: Vec::new(),
    };

    match vault.seal(&book) {
        Ok(sealed) => {
            settings.wallets.push(WalletEntry {
                name,
                vault: sealed,
                current: String::new(),
                biometric: None,
                unprotected: vault.bare(),
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

    egui::Grid::new("words").num_columns(2 * columns).spacing([10.0, 8.0]).show(ui, |ui| {
        for row in 0..rows {
            for column in 0..columns {
                let position = column * rows + row;

                if let Some(word) = words.get(position) {
                    ui.label(RichText::new(format!("{:>2}", position + 1)).monospace().color(palette.muted));
                    ui.label(RichText::new(*word).monospace().color(palette.ink));
                }
            }

            ui.end_row();
        }
    });
}

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
    use super::*;

    #[test]
    fn three_different_words_are_asked_back_and_checked() {
        let phrase = Phrase::parse("test test test test test test test test test test test junk").unwrap();

        for _ in 0..50 {
            let asked = three_of(12);

            assert!(asked[0] < asked[1] && asked[1] < asked[2] && asked[2] < 12, "{asked:?}");
        }

        assert!(right(&phrase, &[0, 5, 11], &[Typed::new("test".to_string()), Typed::new(" TEST ".to_string()), Typed::new("junk".to_string())]));
        assert!(!right(&phrase, &[0, 5, 11], &[Typed::new("test".to_string()), Typed::new("test".to_string()), Typed::new("test".to_string())]));
    }

    #[test]
    fn a_kept_wallet_starts_with_the_phrases_first_account() {
        let mut settings = Settings::default();
        let phrase = Phrase::generate(12).unwrap();
        let opened = keep(&mut settings, "Mine".to_string(), Vault::create("correct horse").unwrap(), &phrase, String::new());

        assert_eq!((settings.wallets.len(), settings.wallet, settings.wallets[0].name.as_str()), (1, 0, "Mine"));
        assert_eq!(opened.book.accounts[0].source, Source::Phrase { account: 0 });
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

        assert!(!mine.watch && mine.phrase.is_some());
        assert_eq!(cold, watched.book);
        assert!(cold.watch && cold.phrase.is_none());

        let holders = crate::keys::open(&cold);

        assert!(holders[0].keys.is_empty(), "nothing to sign with");
        assert_eq!(holders[0].addresses(), signer.iter().map(|key| key.public().address()).collect::<Vec<_>>());
    }
}
