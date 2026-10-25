//! The lock screen: which wallet, and its password. Or a wallet more: from new words, shown once and
//! three of them asked back, or from words written down before. Each wallet is a vault of its own,
//! with a password of its own.

use {
    crate::app::{Settings, WalletEntry},
    crate::theme::{self, Palette},
    crate::vault::{Book, Source, StoredAccount, Vault, Words},
    chainv4::crypto::mnemonic::Phrase,
    egui::{RichText, Ui},
};

/// Where the lock screen is.
pub enum Lock {
    /// Asking for the password of the wallet picked.
    Locked { password: String, error: Option<String> },
    /// Making a wallet: from new words, or from words written down before.
    Choose,
    New(Making),
    /// New words, to write down.
    Words(Made),
    /// Three of them, asked back.
    Confirm { made: Made, asked: [usize; 3], answers: [String; 3], error: Option<String> },
    Restore { making: Making, words: String },
    Open,
}

/// A wallet being made: its name, password twice, and BIP39's optional passphrase.
#[derive(Default)]
pub struct Making {
    name: String,
    first: String,
    second: String,
    passphrase: String,
    error: Option<String>,
}

/// A wallet made, its words not yet confirmed.
pub struct Made {
    name: String,
    vault: Vault,
    phrase: Phrase,
    passphrase: String,
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
    /// Where to start: the password of the wallet last used, or making the first.
    pub fn start(settings: &Settings) -> Lock {
        match settings.wallets.is_empty() {
            true => Lock::Choose,
            false => Lock::Locked { password: String::new(), error: None },
        }
    }
}

/// Draws the lock screen's card. Returns the wallet it opened.
pub fn show(ui: &mut Ui, palette: &Palette, settings: &mut Settings, lock: &mut Lock) -> Option<Opened> {
    let mut opened = None;
    let mut next = None;
    let mut step = None;

    theme::card(ui, palette, |ui| match lock {
        Lock::Locked { password, error } => {
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
            let field = ui.add(egui::TextEdit::singleline(password).password(true).hint_text("password").desired_width(f32::INFINITY));
            let enter = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));

            if let Some(error) = error {
                ui.colored_label(palette.bad, error.as_str());
            }

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, palette, "Unlock", !password.is_empty()).clicked() || enter {
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
            theme::caps(ui, palette, "A wallet");
            ui.label(RichText::new("Its accounts all come from 12 words. Make new ones, or bring a wallet back from words you wrote down.").color(palette.soft));
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, palette, "New wallet", true).clicked() {
                    next = Some(Lock::New(Making::default()));
                }

                if theme::quiet(ui, palette, "Restore from words").clicked() {
                    next = Some(Lock::Restore {
                        making: Making::default(),
                        words: String::new(),
                    });
                }

                if !settings.wallets.is_empty() && theme::quiet(ui, palette, "Back").clicked() {
                    next = Some(Lock::Locked { password: String::new(), error: None });
                }
            });
        }
        Lock::New(making) => {
            theme::caps(ui, palette, "New wallet");
            making_fields(ui, palette, making, settings.wallets.len());
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
                    ui.add(egui::TextEdit::singleline(answer).desired_width(f32::INFINITY));
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
            ui.add(egui::TextEdit::multiline(words).desired_rows(3).desired_width(f32::INFINITY).hint_text("word word word …"));
            ui.add_space(6.0);
            making_fields(ui, palette, making, settings.wallets.len());
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, palette, "Restore", true).clicked() {
                    let restored = Phrase::parse(words)
                        .map_err(|err| err.to_string())
                        .and_then(|phrase| Ok((phrase, Vault::create(&password(making)?)?)));

                    match restored {
                        Ok((phrase, vault)) => opened = Some(keep(settings, name(making, settings.wallets.len()), vault, &phrase, making.passphrase.clone())),
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
                opened = Some(keep(settings, made.name, made.vault, &made.phrase, made.passphrase));

                Lock::Open
            }
            (_, unchanged) => unchanged,
        };
    }

    opened
}

/// A name, a password twice, and, put away, BIP39's passphrase.
fn making_fields(ui: &mut Ui, palette: &Palette, making: &mut Making, wallets: usize) {
    ui.add(egui::TextEdit::singleline(&mut making.name).hint_text(format!("name, such as Wallet {}", wallets + 1)).desired_width(f32::INFINITY));
    ui.add_space(4.0);
    ui.label(RichText::new("A password seals the wallet on this device. Nothing opens it without it.").small().color(palette.muted));
    ui.add(egui::TextEdit::singleline(&mut making.first).password(true).hint_text("password").desired_width(f32::INFINITY));
    ui.add(egui::TextEdit::singleline(&mut making.second).password(true).hint_text("the same again").desired_width(f32::INFINITY));
    ui.add_space(4.0);
    egui::CollapsingHeader::new(RichText::new("Advanced").color(palette.soft)).id_salt("passphrase").show(ui, |ui| {
        ui.label(
            RichText::new("A BIP39 passphrase, kept with the words: the same words with another passphrase are another wallet. Lose it, and the words alone bring back nothing.")
                .small()
                .color(palette.warn),
        );
        ui.add(egui::TextEdit::singleline(&mut making.passphrase).password(true).hint_text("passphrase, or none").desired_width(f32::INFINITY));
    });

    if let Some(error) = &making.error {
        ui.colored_label(palette.bad, error.as_str());
    }
}

/// The password chosen, once it is long enough and typed the same twice.
fn password(making: &Making) -> Result<String, String> {
    match (making.first.len() >= 8, making.first == making.second) {
        (false, _) => Err("use a password of at least 8 characters".to_string()),
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
fn right(phrase: &Phrase, asked: &[usize; 3], answers: &[String; 3]) -> bool {
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
    };

    match vault.seal(&book) {
        Ok(sealed) => {
            settings.wallets.push(WalletEntry {
                name,
                vault: sealed,
                current: String::new(),
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

        assert!(right(&phrase, &[0, 5, 11], &["test".into(), " TEST ".into(), "junk".into()]));
        assert!(!right(&phrase, &[0, 5, 11], &["test".into(), "test".into(), "test".into()]));
    }

    #[test]
    fn a_kept_wallet_starts_with_the_phrases_first_account() {
        let mut settings = Settings::default();
        let phrase = Phrase::generate(12).unwrap();
        let opened = keep(&mut settings, "Mine".to_string(), Vault::create("correct horse").unwrap(), &phrase, String::new());

        assert_eq!((settings.wallets.len(), settings.wallet, settings.wallets[0].name.as_str()), (1, 0, "Mine"));
        assert_eq!(opened.book.accounts[0].source, Source::Phrase { account: 0 });
        assert_eq!(Vault::open(&settings.wallets[0].vault, "correct horse").unwrap().1, opened.book);

        let short = Making {
            first: "short".into(),
            second: "short".into(),
            ..Making::default()
        };
        let mismatched = Making {
            first: "long enough".into(),
            second: "long enougH".into(),
            ..Making::default()
        };

        assert_eq!(password(&short), Err("use a password of at least 8 characters".to_string()));
        assert_eq!(password(&mismatched), Err("the two passwords do not match".to_string()));
    }
}
