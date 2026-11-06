//! What opens a wallet: its guard. A wallet is opened by its password, or by biometrics alone, or by
//! both together; or, as it may have been left before any of those were on offer, by its password
//! or biometrics either way, or by nothing at all.
//!
//! A guard is read from what `WalletEntry` always held, so an older build's save keeps it: a record
//! in `biometric` that starts with a tag of this file's is one of the two guards that need hardware,
//! and says how its key is made. An older build in a browser, where wallets stay where it reads them,
//! takes such a record for one from elsewhere, says so and writes nothing, and its password says wrong
//! password, whichever it is given. A new field instead would be dropped by its first save, and with
//! it the only way into the vault.
//!
//! What needs hardware can be locked out for good, as hardware can lose its key, so a wallet may
//! need it only when its words bring back everything it holds: `refusal` says when not, and `hint`
//! says, before it is chosen, what a lockout means.

use {
    crate::app::{Settings, WalletEntry},
    crate::vault::{Book, Sealed, Source, Vault},
    chainv4_ui::biometric::{Enrolled, Kind},
};

/// How a record for a wallet with biometrics alone starts: its vault's key is the secret kept.
const BIOMETRIC_TAG: &str = "biometric-v1:";
/// How a record for a wallet needing both starts: its vault's key is the password stretched and the
/// secret kept, joined by `Vault::both`.
const BOTH_TAG: &str = "both-v1:";

/// What opens a wallet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guard {
    /// Nothing: it is keyed from the empty password, so a copy of storage opens it.
    Bare,
    Password,
    /// Its password, or biometrics, which keep the key the password stretched to. Offered no more,
    /// and kept for the wallets that have it. Such a wallet may have no password either, when it is
    /// marked unprotected.
    Either,
    /// Biometrics alone: its key is random, and hardware or a passkey alone keeps it.
    Biometric,
    /// Its password and the secret hardware keeps, the two joined into its key.
    Both,
}

impl Guard {
    /// Whether it cannot open without hardware, and so is locked out when hardware loses its key.
    pub fn needs_hardware(self) -> bool {
        matches!(self, Guard::Biometric | Guard::Both)
    }

    pub fn name(self) -> &'static str {
        match self {
            Guard::Bare => "No password",
            Guard::Password => "Password only",
            Guard::Either => "Password or biometrics",
            Guard::Biometric => "Biometric only",
            Guard::Both => "Password and biometrics, both required",
        }
    }

    /// What it gives, in terms of who can do what with a copy of this app's storage. kind names the
    /// biometrics here, when they can be named.
    pub fn says(self, kind: Option<Kind>, unprotected: bool) -> String {
        let check = kind.map_or("biometrics", Kind::name);
        let kept = kind.map_or("hardware", kept_by);

        match self {
            Guard::Bare => "It opens with a tap. Anyone who copies this app's storage, from a phone's backup or with malware, can open it too. Set a password, or choose a mode below that needs biometrics, to stop that.".to_string(),
            Guard::Password => "Its password is stretched into the key that seals it. A copy of this app's storage holds no key, but whoever has one can go on guessing at the password on their own computer: a long password is what stops them. Your words bring it back on any device.".to_string(),
            Guard::Either if unprotected => format!("It opens with {check}, or with a tap: it has no password. Anyone who copies this app's storage can open it too, with biometrics on or off, so {check} protects it from no one who has a copy. Choose a mode below to stop that."),
            Guard::Either => format!("Its password or {check} opens it. {check} is for convenience: it keeps the key the password stretches to, so it adds nothing against a copy of this app's storage, which can be guessed at as with the password alone. Choose a mode below for more."),
            Guard::Biometric => format!("It has no password. Its key is random, and kept only wrapped by {kept}, so a copy of this app's storage opens nothing, and there is no password in it to guess. Whoever passes {check} on this device opens it, and every send asks for {check}."),
            Guard::Both => format!("Its key is made from both its password and a random secret kept by {kept}. A copy of this app's storage opens nothing, even with the password, and {check} opens nothing without the password. Every send asks for {check}."),
        }
    }
}

/// Whatever a wallet's guard, once it is unlocked its keys are in memory.
pub const IN_MEMORY: &str = "Whichever you choose: while the wallet is unlocked its keys are in this device's memory, where malware on the unlocked device can read them. These modes protect a wallet at rest.";

/// What holds the key, in words, wherever a key can be bound to hardware.
fn kept_by(kind: Kind) -> &'static str {
    match kind {
        Kind::Biometrics => "the Android Keystore, in this phone's secure hardware",
        Kind::FaceId | Kind::TouchId => "the iOS Keychain, tied by the Secure Enclave to the faces or fingers enrolled now",
        Kind::Passkey => "your passkey, held by this device or by wherever your passkey provider syncs it",
    }
}

/// What loses the key, in words.
pub fn lost_by(kind: Kind) -> &'static str {
    match kind {
        Kind::Biometrics => "Adding a fingerprint or face destroys the key that opens it, and so do removing them all, turning off the screen lock, a factory reset, a broken or lost phone, and restoring to another phone.",
        Kind::FaceId | Kind::TouchId => "Changing the faces or fingers enrolled destroys the key that opens it, and so do turning off the passcode, erasing the phone, a broken or lost phone, and restoring to another phone.",
        Kind::Passkey => "Deleting the passkey destroys what opens it, and so do losing every device that holds it, and clearing this browser's data, which takes the wallet with it.",
    }
}

impl WalletEntry {
    /// What opens it, as what it holds says.
    pub fn guard(&self) -> Guard {
        match self.biometric.as_deref() {
            Some(kept) if kept.starts_with(BIOMETRIC_TAG) => Guard::Biometric,
            Some(kept) if kept.starts_with(BOTH_TAG) => Guard::Both,
            Some(_) => Guard::Either,
            None if self.unprotected => Guard::Bare,
            None => Guard::Password,
        }
    }

    /// Whether it asks for no password at all, opening with a tap. Never one that needs hardware,
    /// whatever it was marked as: it is not keyed from an empty password.
    pub fn opens_with_a_tap(&self) -> bool {
        matches!(self.guard(), Guard::Bare | Guard::Either) && self.unprotected
    }
}

/// What a wallet needing hardware stores: the platform's record, with what says how its key is made.
pub fn tagged(guard: Guard, enrolled: &Enrolled) -> Option<String> {
    match guard {
        Guard::Biometric => Some(format!("{BIOMETRIC_TAG}{}", enrolled.0)),
        Guard::Both => Some(format!("{BOTH_TAG}{}", enrolled.0)),
        _ => None,
    }
}

/// The platform's record out of what a wallet stores, tagged or not.
pub fn untagged(kept: &str) -> Enrolled {
    let record = kept.strip_prefix(BIOMETRIC_TAG).or_else(|| kept.strip_prefix(BOTH_TAG)).unwrap_or(kept);

    Enrolled(record.to_string())
}

/// What a new guard takes, held only while a vault is sealed and checked under it.
pub enum Factors<'a> {
    Password(&'a str),
    /// The secret that is the vault's key.
    Biometric(&'a [u8; 32]),
    /// The password, and the secret hardware keeps.
    Both(&'a str, &'a [u8; 32]),
}

impl Factors<'_> {
    fn vault(&self) -> Result<Vault, String> {
        match self {
            Factors::Password(password) => Vault::create(password),
            Factors::Biometric(secret) => Vault::keyed(secret),
            Factors::Both(password, secret) => Vault::both(password, secret),
        }
    }

    /// Opens what is sealed with them.
    pub fn open(&self, sealed: &Sealed) -> Result<(Vault, Book), String> {
        match self {
            Factors::Password(password) => Vault::open(sealed, password),
            Factors::Biometric(secret) => Vault::open_with_key(sealed, secret),
            Factors::Both(password, secret) => Vault::open_both(sealed, password, secret),
        }
    }
}

/// A book sealed under new factors, opened again with them before anything relies on it. It changes
/// nothing kept: what to do with what comes back is `adopt`'s.
pub fn sealed_anew(factors: &Factors, book: &Book) -> Result<(Vault, Sealed), String> {
    let vault = factors.vault()?;
    let sealed = vault.seal(book)?;
    verified(factors, book, &vault, &sealed)?;

    Ok((vault, sealed))
}

/// Whether what was sealed opens with these factors, as the same key and the same book.
fn verified(factors: &Factors, book: &Book, vault: &Vault, sealed: &Sealed) -> Result<(), String> {
    let (opened, read) = factors.open(sealed).map_err(|why| format!("the vault sealed anew did not open, so nothing changed: {why}"))?;

    if read != *book || opened.key() != vault.key() {
        return Err("the vault sealed anew did not read back the same, so nothing changed".to_string());
    }

    Ok(())
}

/// Makes a vault sealed and checked by `sealed_anew` the wallet's, with what now opens it, and has
/// settings written at once. A record no longer used is retired and not forgotten yet: settings on
/// disk still name it until they are written, and forgetting it first would strand a wallet killed
/// in between. `retire_all` forgets it once settings that say so have been read back.
pub fn adopt(settings: &mut Settings, sealed: Sealed, record: Option<String>) -> Result<(), String> {
    let entry = settings.wallets.get_mut(settings.wallet).ok_or("the wallet is gone")?;
    let old = entry.biometric.take().filter(|old| Some(old) != record.as_ref());
    entry.vault = sealed;
    entry.biometric = record;
    entry.unprotected = false;
    settings.retired.extend(old);
    settings.persist_now = true;

    Ok(())
}

/// Takes a wallet out of the list, its record retired as `adopt`'s is, and has settings written.
pub fn remove_wallet(settings: &mut Settings, index: usize) {
    if index >= settings.wallets.len() {
        return;
    }

    let entry = settings.wallets.remove(index);
    settings.forget_indexes.push(entry.id.clone());
    settings.retired.extend(entry.biometric);
    settings.wallet = 0;
    settings.persist_now = true;
}

/// What is retired, now forgetting it can strand nothing: these settings were read back, so they
/// are what the disk says. Never a record a wallet still names, however it came to be listed.
pub fn retired_taken(settings: &mut Settings) -> Vec<Enrolled> {
    let retired = std::mem::take(&mut settings.retired);

    retired.iter().filter(|record| !settings.wallets.iter().any(|entry| entry.biometric.as_ref() == Some(record))).map(|record| untagged(record)).collect()
}

/// What a wallet holds that matters if it is locked out, and comes back from its words or not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Backup {
    /// How many seed phrases to have written down.
    pub phrases: usize,
    /// How many of them have a passphrase, without which the words bring back nothing.
    pub passphrases: usize,
    /// Accounts an offline signer holds, which come back by pairing it again.
    pub watched: Vec<String>,
    /// Accounts MetaMask holds, which come back by connecting it again.
    pub metamask: Vec<String>,
    /// How many lines of what this signer signed go with it.
    pub signed: usize,
    /// How many swaps with no secret here go with it, which it no longer lists.
    pub swaps: usize,
}

/// Why this wallet may not need hardware, each thing that words would not bring back named; or what
/// it holds that would. It stays as it was until each is dealt with.
pub fn refusal(book: &Book) -> Result<Backup, Vec<String>> {
    let mut refused = Vec::new();
    let keys: Vec<&str> = book.accounts.iter().filter(|account| matches!(account.source, Source::Key { .. })).map(|account| account.name.as_str()).collect();

    if !keys.is_empty() {
        refused.push(format!("Keys brought in, which no words bring back: {}. Move what they hold to an account from words, and remove them on the Accounts page.", keys.join(", ")));
    }

    let secrets = book.swaps.iter().filter(|swap| swap.secret.is_some()).count();

    if secrets > 0 {
        let swaps = if secrets == 1 { "1 swap keeps its secret".to_string() } else { format!("{secrets} swaps keep their secrets") };
        refused.push(format!("{swaps} here, which no words bring back. Finish each, then forget it on the Swaps page."));
    }

    if book.phrases.iter().any(|words| words.fingerprint().is_none()) {
        refused.push("Words that do not read, which could bring nothing back.".to_string());
    }

    if !refused.is_empty() {
        return Err(refused);
    }

    let named = |of: fn(&Source) -> bool| book.accounts.iter().filter(|account| of(&account.source)).map(|account| account.name.clone()).collect();

    Ok(Backup {
        phrases: book.phrases.len(),
        passphrases: book.phrases.iter().filter(|words| !words.passphrase().is_empty()).count(),
        watched: named(|source| matches!(source, Source::Watch { .. })),
        metamask: named(|source| matches!(source, Source::MetaMask { .. })),
        signed: book.signed.len(),
        swaps: book.swaps.len(),
    })
}

/// What is said before a wallet comes to need hardware, in plain words, and what is typed to accept
/// it follows it: what a lockout is, what causes one, and what is lost by it.
pub fn hint(kind: Kind, backup: &Backup) -> Vec<String> {
    let mut hint = vec![
        format!("This wallet will open only with {} on this device. If that is ever lost, it cannot be opened again by anything, and no one can help.", kind.name()),
        lost_by(kind).to_string(),
        "Then the only way back is to restore from your words, on this or any other device.".to_string(),
    ];

    hint.push(match backup.phrases {
        0 => "This wallet holds no words of its own.".to_string(),
        1 => "Be sure this wallet's seed phrase is written down.".to_string(),
        count => format!("Be sure all {count} of this wallet's seed phrases are written down: each brings back only its own accounts."),
    });

    match backup.passphrases {
        0 => {}
        1 => hint.push("One of them has a passphrase: without it, the words bring back nothing.".to_string()),
        count => hint.push(format!("{count} of them have a passphrase: without it, their words bring back nothing.")),
    }

    if !backup.watched.is_empty() {
        hint.push(format!("{}: the offline signer holds their keys, so they come back by pairing it again, not from words.", backup.watched.join(", ")));
    }

    if !backup.metamask.is_empty() {
        hint.push(format!("{}: MetaMask holds their keys, so they come back by connecting it again, not from words.", backup.metamask.join(", ")));
    }

    if backup.signed > 0 {
        hint.push(format!("The {} lines of what this signer signed are lost with the wallet: no words bring them back.", backup.signed));
    }

    if backup.swaps > 0 {
        hint.push(format!("The {} swaps listed here would have to be read in again from their terms.", backup.swaps));
    }

    hint
}

/// What to type a yes for, in as many words as it has phrases.
pub fn confirm(backup: &Backup) -> String {
    match backup.phrases {
        0 => "I can pair or connect what this wallet watches again. Type yes:".to_string(),
        1 => "I have this wallet's seed phrase written down. Type yes:".to_string(),
        count => format!("I have all {count} seed phrases of this wallet written down. Type yes:"),
    }
}

/// Whether what was typed confirms something that cannot be undone: yes, however it is written, and
/// nothing else.
pub fn confirmed(typed: &str) -> bool {
    typed.trim().eq_ignore_ascii_case("yes")
}

/// What the lock screen says when the hardware key is gone.
pub fn gone(kind: Kind) -> String {
    format!(
        "The key that opens this wallet is gone from this device, so it cannot open here again. {} Restoring from your words is the way back: Add a wallet, then Restore from words. Nothing was deleted: this wallet stays listed until you remove it.",
        lost_by(kind),
    )
}

/// What the lock screen says of a wallet that will not open, when it has not been told the key is
/// gone, as a browser never says which passkeys are left: it may just be a prompt turned down.
pub fn will_not_open(kind: Kind) -> String {
    format!("A prompt turned down or a failed try changes nothing: try again. If it never works, the key may be gone. {} Restoring from your words is the way back: Add a wallet, then Restore from words.", lost_by(kind))
}

/// What removing a wallet from the lock screen says first, no factor being possible by then. Sealed,
/// nothing in it can be listed.
pub fn removal_preview(name: &str) -> String {
    format!("{name}, and every account and seed phrase sealed in it, leave this device for good. It is locked, so what it holds cannot be listed here. Without its words, what its accounts hold is out of reach for good.")
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::vault::{Extra, Sealed, StoredAccount, SwapRecord, Words, fresh_secret},
        chainv4::crypto::keypair::Keypair,
    };

    const TEST: &str = "test test test test test test test test test test test junk";
    const ABANDON: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn book() -> Book {
        Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            accounts: vec![StoredAccount { name: "Main".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from: None } }],
            ..Book::default()
        }
    }

    fn entry(biometric: Option<&str>, unprotected: bool) -> WalletEntry {
        WalletEntry {
            id: String::new(),
            name: "Mine".to_string(),
            vault: Sealed { salt: String::new(), nonce: String::new(), sealed: String::new() },
            current: String::new(),
            biometric: biometric.map(str::to_string),
            unprotected,
            born: None,
        }
    }

    #[test]
    fn what_a_wallet_holds_says_what_opens_it_and_only_a_tag_of_ours_needs_hardware() {
        assert_eq!(entry(None, false).guard(), Guard::Password);
        assert_eq!(entry(None, true).guard(), Guard::Bare);

        for platform in ["android:chainv4-00:0a:0b", "passkey:00:01:02:03", "macos:00:01"] {
            assert_eq!(entry(Some(platform), false).guard(), Guard::Either, "{platform}");
            assert!(entry(Some(platform), true).opens_with_a_tap(), "{platform}");
        }

        let enrolled = Enrolled("android:chainv4-00:0a:0b".to_string());
        let biometric = tagged(Guard::Biometric, &enrolled).unwrap();
        let both = tagged(Guard::Both, &enrolled).unwrap();

        assert_eq!(entry(Some(&biometric), false).guard(), Guard::Biometric);
        assert_eq!(entry(Some(&both), false).guard(), Guard::Both);
        assert_eq!(untagged(&biometric), enrolled);
        assert_eq!(untagged(&both), enrolled);
        assert_eq!(untagged(&enrolled.0), enrolled, "one of today's has no tag to take off");
        assert_eq!(tagged(Guard::Either, &enrolled), None, "offered no more");

        // Marked unprotected by anything at all, one that needs hardware is still no tap away.
        assert!(!entry(Some(&biometric), true).opens_with_a_tap());
        assert_eq!(entry(Some(&both), true).guard(), Guard::Both);
    }

    #[test]
    fn a_vault_sealed_anew_opens_with_its_new_factors_alone() {
        let book = book();
        let secret = fresh_secret().unwrap();

        for factors in [Factors::Password("correct horse"), Factors::Biometric(&secret), Factors::Both("correct horse", &secret)] {
            let (vault, sealed) = sealed_anew(&factors, &book).unwrap();

            assert_eq!(factors.open(&sealed).unwrap().1, book);
            assert_eq!(factors.open(&sealed).unwrap().0.key(), vault.key());
        }

        // What will not seal is not written, so no vault comes of it.
        let mut dangling = book.clone();
        dangling.phrases.clear();

        assert!(sealed_anew(&Factors::Biometric(&secret), &dangling).is_err());
        assert!(sealed_anew(&Factors::Both("", &secret), &book).is_err(), "no password to go with the secret");
    }

    #[test]
    fn a_vault_that_does_not_read_back_is_refused_before_anything_changes() {
        let book = book();
        let secret = fresh_secret().unwrap();
        let factors = Factors::Biometric(&secret);
        let (vault, sealed) = sealed_anew(&factors, &book).unwrap();

        // A byte turned on the way, another book, and another key.
        let mut turned = sealed.clone();
        turned.sealed.replace_range(4..6, if &turned.sealed[4..6] == "00" { "ff" } else { "00" });

        assert!(verified(&factors, &book, &vault, &turned).is_err());

        let other = Book { phrases: vec![Words::new(ABANDON.to_string(), String::new())], ..Book::default() };
        assert!(verified(&factors, &other, &vault, &sealed).is_err());

        let elsewhere = Vault::keyed(&fresh_secret().unwrap()).unwrap();
        assert!(verified(&factors, &book, &elsewhere, &sealed).is_err());
    }

    #[test]
    fn adopting_a_vault_retires_the_old_record_and_keeps_one_still_in_use() {
        let mut settings = Settings::default();
        settings.wallets.push(entry(Some("android:old:00:01"), true));
        let sealed = Sealed { salt: "new".to_string(), nonce: "new".to_string(), sealed: "new".to_string() };
        adopt(&mut settings, sealed.clone(), Some("both-v1:android:new:00:01".to_string())).unwrap();

        assert_eq!(settings.wallets[0].guard(), Guard::Both);
        assert!(!settings.wallets[0].unprotected);
        assert_eq!(settings.retired, ["android:old:00:01"]);
        assert!(settings.persist_now, "written at once");

        // A new password under the same record retires nothing.
        settings.retired.clear();
        adopt(&mut settings, sealed, Some("both-v1:android:new:00:01".to_string())).unwrap();

        assert!(settings.retired.is_empty());
    }

    #[test]
    fn what_is_retired_is_forgotten_only_when_no_wallet_names_it() {
        let mut settings = Settings::default();
        settings.wallets.push(entry(Some("both-v1:android:mine:00:01"), false));
        settings.retired = vec!["both-v1:android:mine:00:01".to_string(), "biometric-v1:android:old:00:01".to_string(), "macos:older:01".to_string()];

        assert_eq!(retired_taken(&mut settings), vec![Enrolled("android:old:00:01".to_string()), Enrolled("macos:older:01".to_string())]);
        assert!(settings.retired.is_empty());
    }

    #[test]
    fn removing_a_wallet_retires_its_record_and_leaves_the_others() {
        let mut settings = Settings::default();
        settings.wallets = vec![entry(None, false), entry(Some("both-v1:android:mine:00:01"), false)];
        settings.wallet = 1;
        remove_wallet(&mut settings, 1);

        assert_eq!((settings.wallets.len(), settings.wallet), (1, 0));
        assert_eq!(settings.retired, ["both-v1:android:mine:00:01"]);
        assert!(settings.persist_now);

        remove_wallet(&mut settings, 9);

        assert_eq!(settings.wallets.len(), 1, "no such place, nothing goes");
    }

    #[test]
    fn a_wallet_holding_what_words_cannot_bring_back_is_refused_and_each_thing_named() {
        let mut keyed = book();
        keyed.accounts.push(StoredAccount { name: "Brought in".to_string(), source: Source::Key { key: Keypair::development().to_key_file() } });
        let why = refusal(&keyed).unwrap_err();

        assert_eq!(why.len(), 1);
        assert!(why[0].contains("Brought in") && why[0].contains("no words bring back"), "{why:?}");
        assert!(!why[0].contains(&Keypair::development().to_key_file()), "and never the key");

        let swap = |secret: Option<String>| SwapRecord { terms: "t".to_string(), secret, account: "a".to_string(), made: 1, started: true, funding: None, multisig: None };
        let mut swapping = book();
        swapping.swaps = vec![swap(Some(format!("0x{}", "ab".repeat(32)))), swap(Some(format!("0x{}", "cd".repeat(32)))), swap(None)];
        let why = refusal(&swapping).unwrap_err();

        assert!(why[0].contains("2 swaps keep their secrets") && why[0].contains("forget it on the Swaps page"), "{why:?}");
        assert!(!why[0].contains("abab"));

        // Both at once, both named.
        keyed.swaps = vec![swap(Some(format!("0x{}", "ab".repeat(32))))];
        let why = refusal(&keyed).unwrap_err();

        assert_eq!(why.len(), 2);
        assert!(why[1].contains("1 swap keeps its secret"), "{why:?}");

        let unread = Book { phrases: vec![Words::new("not words".to_string(), String::new())], ..Book::default() };
        assert!(refusal(&unread).unwrap_err()[0].contains("do not read"));
    }

    #[test]
    fn what_comes_back_some_other_way_is_allowed_and_said_plainly() {
        let key = Keypair::development().public().to_string();
        let mut mixed = book();
        mixed.accounts.push(StoredAccount { name: "Cold".to_string(), source: Source::Watch { keys: vec![key.clone()], origin: None } });
        mixed.accounts.push(StoredAccount { name: "Fox".to_string(), source: Source::MetaMask { key } });
        mixed.accounts.push(StoredAccount { name: "Further".to_string(), source: Source::Phrase { account: 1, extra: vec![Extra { scheme: "bitcoin".to_string(), index: 2 }], from: None } });
        mixed.swaps.push(SwapRecord { terms: "t".to_string(), secret: None, account: "a".to_string(), made: 1, started: false, funding: None, multisig: None });
        mixed.note_signed(1, "Send", "1 CV4", "ed25519:0x01");
        let backup = refusal(&mixed).unwrap();

        assert_eq!(backup.phrases, 1);
        assert_eq!(backup.watched, ["Cold"]);
        assert_eq!(backup.metamask, ["Fox"]);
        assert_eq!((backup.signed, backup.swaps), (1, 1));

        let said = hint(Kind::Biometrics, &backup).join("\n");

        for words in ["Adding a fingerprint or face destroys", "factory reset", "broken or lost phone", "restoring to another phone", "restore from your words", "seed phrase is written down", "Cold: the offline signer", "pairing it again", "Fox: MetaMask", "connecting it again", "signed are lost with the wallet", "swaps listed here"] {
            assert!(said.contains(words), "{words}: {said}");
        }

        assert_eq!(confirm(&backup), "I have this wallet's seed phrase written down. Type yes:");
    }

    #[test]
    fn the_hint_names_how_many_phrases_and_their_passphrases() {
        let mut two = book();
        two.add_phrase(Words::new(ABANDON.to_string(), "and a passphrase".to_string())).unwrap();
        let backup = refusal(&two).unwrap();
        let said = hint(Kind::Biometrics, &backup).join("\n");

        assert_eq!((backup.phrases, backup.passphrases), (2, 1));
        assert!(said.contains("all 2 of this wallet's seed phrases"), "{said}");
        assert!(said.contains("has a passphrase"), "{said}");
        assert_eq!(confirm(&backup), "I have all 2 seed phrases of this wallet written down. Type yes:");

        let watching = refusal(&Book { watch: true, ..Book::default() }).unwrap();

        assert_eq!(confirm(&watching), "I can pair or connect what this wallet watches again. Type yes:");

        // Said for what holds the key here, and never a word of a platform it is not.
        assert!(hint(Kind::Passkey, &backup).join("\n").contains("Deleting the passkey"));
        assert!(hint(Kind::FaceId, &backup).join("\n").contains("faces or fingers enrolled"));
        assert!(!hint(Kind::Passkey, &backup).join("\n").contains("fingerprint"));
    }

    #[test]
    fn what_each_guard_says_is_true_of_a_copy_of_storage() {
        let says = |guard: Guard, unprotected| guard.says(Some(Kind::Biometrics), unprotected);

        assert!(says(Guard::Bare, true).contains("Anyone who copies"));
        assert!(says(Guard::Password, false).contains("guessing at the password"));
        assert!(says(Guard::Either, true).contains("Anyone who copies"), "biometrics on save no wallet with no password");
        assert!(says(Guard::Either, false).contains("adds nothing against a copy"));
        assert!(says(Guard::Biometric, false).contains("a copy of this app's storage opens nothing"));
        assert!(says(Guard::Both, false).contains("opens nothing, even with the password"));
        assert!(Guard::Both.says(Some(Kind::Passkey), false).contains("syncs"), "a passkey may not stay on this device");
        assert!(IN_MEMORY.contains("malware"));
    }

    /// Settings as a build before these modes reads them, copied as it stood. What it parses is what
    /// such a build holds, and what it writes is what it saves.
    mod older {
        use serde::{Deserialize, Serialize};

        #[derive(Clone, Debug, Default, Serialize, Deserialize)]
        #[serde(default)]
        pub struct Settings {
            pub wallets: Vec<WalletEntry>,
            pub wallet: usize,
        }

        #[derive(Clone, Debug, Serialize, Deserialize)]
        pub struct WalletEntry {
            pub name: String,
            pub vault: Sealed,
            pub current: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub biometric: Option<String>,
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            pub unprotected: bool,
        }

        #[derive(Clone, Debug, Serialize, Deserialize)]
        pub struct Sealed {
            pub salt: String,
            pub nonce: String,
            pub sealed: String,
        }

        /// How an older build opens with a password: stretched as it stretched, and sealed as it
        /// sealed.
        pub fn open(sealed: &Sealed, password: &str) -> Result<Vec<u8>, String> {
            use {
                argon2::Argon2,
                chacha20poly1305::aead::{Aead, KeyInit},
                chacha20poly1305::{ChaCha20Poly1305, Key, Nonce},
                chainv4::structs::hex::from_hex,
            };

            let salt = from_hex(&sealed.salt).filter(|salt| salt.len() == 16).ok_or("the vault is damaged")?;
            let nonce = from_hex(&sealed.nonce).filter(|nonce| nonce.len() == 12).ok_or("the vault is damaged")?;
            let mut key = [0; 32];
            Argon2::default().hash_password_into(password.as_bytes(), &salt, &mut key).map_err(|err| err.to_string())?;

            ChaCha20Poly1305::new(Key::from_slice(&key))
                .decrypt(Nonce::from_slice(&nonce), from_hex(&sealed.sealed).ok_or("the vault is damaged")?.as_slice())
                .map_err(|_| "wrong password".to_string())
        }

        /// How an older build reads a biometric record: its platform's tag first, or it says the
        /// record is from elsewhere, and writes nothing. No platform's tag is one of the new ones.
        pub fn reads_record(kept: &str, tag: &str, fields: usize) -> bool {
            let parts: Vec<&str> = kept.split(':').collect();

            parts.first() == Some(&tag) && parts.len() == fields + 1
        }
    }

    /// A store as eframe's reads and writes: settings go through RON, as they do on a device.
    #[derive(Default)]
    struct Storage(std::collections::HashMap<String, String>);

    impl eframe::Storage for Storage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }

        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }

        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }

        fn flush(&mut self) {}
    }

    /// A wallet of each guard that needs hardware, with what opens it.
    fn guarded(book: &Book) -> Vec<(Guard, WalletEntry, zeroize::Zeroizing<[u8; 32]>)> {
        let enrolled = Enrolled("android:chainv4-00ff:0a0b:0c0d".to_string());
        let secret = fresh_secret().unwrap();
        let (_, biometric) = sealed_anew(&Factors::Biometric(&secret), book).unwrap();
        let (_, both) = sealed_anew(&Factors::Both("correct horse", &secret), book).unwrap();
        let entry = |vault: Sealed, guard: Guard| WalletEntry { id: String::new(), name: "Mine".to_string(), vault, current: "ed25519:0x01".to_string(), biometric: tagged(guard, &enrolled), unprotected: false, born: None };

        vec![(Guard::Biometric, entry(biometric, Guard::Biometric), secret.clone()), (Guard::Both, entry(both, Guard::Both), secret)]
    }

    #[test]
    fn an_older_build_fails_closed_on_a_wallet_needing_hardware_and_its_save_keeps_it_whole() {
        let book = book();

        for (guard, kept, secret) in guarded(&book) {
            let mut storage = Storage::default();
            eframe::set_value(&mut storage, "settings", &Settings::with_wallets(vec![kept.clone()]));
            let read: older::Settings = eframe::get_value(&storage, "settings").expect("settings decode in an older build");
            let entry = &read.wallets[0];

            // Not marked as having no password, so it asks for one, and no password opens it.
            assert!(!entry.unprotected, "{guard:?} is never bare to an older build");

            // Nothing opens it: a vault at the stronger stretch, as Both is now, reads to it as
            // damaged, its salt being past what it reads, and is kept as it was.
            for password in ["", "correct horse", "a guess"] {
                let refused = older::open(&entry.vault, password).err();
                assert!(matches!(refused.as_deref(), Some("wrong password" | "the vault is damaged")), "{guard:?} with {password:?}: {refused:?}");
            }

            // Its biometrics are a record from elsewhere to it, and that is not what turns them off.
            let record = entry.biometric.as_deref().unwrap();

            for (platform, fields) in [("android", 3), ("passkey", 4), ("macos", 2)] {
                assert!(!older::reads_record(record, platform, fields), "{guard:?} on {platform}");
            }

            // What it saves, having changed nothing, reads back as the same wallet, opening as it did.
            let mut saved = Storage::default();
            eframe::set_value(&mut saved, "settings", &read);
            let back: Settings = eframe::get_value(&saved, "settings").expect("settings decode after an older build's save");

            assert_eq!(back.wallets, std::slice::from_ref(&kept), "{guard:?}: nothing lost to a downgrade and back");
            assert_eq!(back.wallets[0].guard(), guard);

            let opened = match guard {
                Guard::Both => Vault::open_both(&back.wallets[0].vault, "correct horse", &secret),
                _ => Vault::open_with_key(&back.wallets[0].vault, &secret),
            };

            assert_eq!(opened.unwrap().1, book);
        }
    }

    #[test]
    fn an_entry_needing_hardware_tampered_into_looking_bare_still_opens_nothing() {
        let book = book();

        for (guard, mut kept, _) in guarded(&book) {
            // Its record stripped and it marked as having no password: the tap tries the empty one.
            kept.biometric = None;
            kept.unprotected = true;

            assert_eq!(kept.guard(), Guard::Bare, "{guard:?} reads as told");
            assert_eq!(Vault::open(&kept.vault, "").err(), Some("wrong password".to_string()), "{guard:?}, and stays shut");
        }
    }

    #[test]
    fn entries_of_every_older_kind_read_and_write_as_they_always_did() {
        let sealed = serde_json::json!({ "salt": "0x01", "nonce": "0x02", "sealed": "0x03" });
        let forms = [
            (serde_json::json!({ "name": "A", "vault": sealed, "current": "" }), Guard::Password),
            (serde_json::json!({ "name": "A", "vault": sealed, "current": "", "unprotected": true }), Guard::Bare),
            (serde_json::json!({ "name": "A", "vault": sealed, "current": "", "biometric": "android:a:00:01" }), Guard::Either),
            (serde_json::json!({ "name": "A", "vault": sealed, "current": "", "biometric": "macos:a:01", "unprotected": true }), Guard::Either),
        ];

        for (form, guard) in forms {
            let read: WalletEntry = serde_json::from_value(form.clone()).unwrap();

            assert_eq!(read.guard(), guard);
            assert_eq!(serde_json::to_value(&read).unwrap(), form, "written back as it came");
        }

        // Settings with nothing retired or asked to be written, written as they were.
        let written = serde_json::to_value(Settings::default()).unwrap();

        assert!(written.get("retired").is_none() && written.get("persist_now").is_none(), "{written}");

        // Their list decodes for a build that has it, and an older build ignores it.
        let mut listed = Settings::default();
        listed.retired.push("android:a:00:01".to_string());
        let mut storage = Storage::default();
        eframe::set_value(&mut storage, "settings", &listed);

        assert_eq!(eframe::get_value::<Settings>(&storage, "settings").unwrap().retired, listed.retired);
        assert!(eframe::get_value::<older::Settings>(&storage, "settings").is_some(), "an older build reads past it");
        assert!(!eframe::get_value::<Settings>(&storage, "settings").unwrap().persist_now, "never read back as asked");
    }

    #[test]
    fn storage_killed_at_any_step_of_a_switch_still_opens_with_what_it_names() {
        // What the disk holds is what `save` last wrote. Each step of leaving Both required for
        // Biometric only, the disk as it stands if the app is killed right there.
        let book = book();
        let old_secret = fresh_secret().unwrap();
        let (_, old_sealed) = sealed_anew(&Factors::Both("correct horse", &old_secret), &book).unwrap();
        let old = WalletEntry { id: String::new(), name: "Mine".to_string(), vault: old_sealed, current: String::new(), biometric: Some("both-v1:fake:old".to_string()), unprotected: false, born: None };
        let mut settings = Settings::with_wallets(vec![old.clone()]);
        let disk = |settings: &Settings| {
            let mut storage = Storage::default();
            eframe::set_value(&mut storage, "settings", settings);

            eframe::get_value::<Settings>(&storage, "settings").unwrap()
        };
        let old_opens = |entry: &WalletEntry| assert_eq!(Vault::open_both(&entry.vault, "correct horse", &old_secret).unwrap().1, book);
        let mut written = disk(&settings);

        // Enrolled and sealed, in memory alone: killed now, the old wallet and its key are as they
        // were, and only an orphan key is left behind, which nothing names and nothing opens with.
        let new_secret = fresh_secret().unwrap();
        let (_, new_sealed) = sealed_anew(&Factors::Biometric(&new_secret), &book).unwrap();
        old_opens(&written.wallets[0]);
        assert!(written.retired.is_empty());

        // Adopted, in memory, the disk not yet written: the same, as nothing was forgotten.
        adopt(&mut settings, new_sealed, Some("biometric-v1:fake:new".to_string())).unwrap();
        assert_eq!(settings.retired, ["both-v1:fake:old"]);
        assert_eq!(written.wallets[0], old);
        old_opens(&written.wallets[0]);
        assert!(retired_taken(&mut written.clone()).is_empty(), "and nothing to forget when it starts again");

        // Written: killed now, the new wallet opens, and starting again forgets the old key alone.
        written = disk(&settings);
        assert_eq!(written.wallets[0].guard(), Guard::Biometric);
        assert_eq!(Vault::open_with_key(&written.wallets[0].vault, &new_secret).unwrap().1, book);
        assert_eq!(retired_taken(&mut written), [Enrolled("fake:old".to_string())]);
        assert!(written.retired.is_empty());
        assert_eq!(Vault::open_with_key(&written.wallets[0].vault, &new_secret).unwrap().1, book, "and it still opens");
    }
}
