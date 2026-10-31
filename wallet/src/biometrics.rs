//! Biometrics in the wallet: secrets kept behind a fingerprint or face, Face ID or a passkey (see
//! chainv4_ui::biometric), and what the wallet asks of them. What a secret is depends on the
//! wallet's guard (see crate::guard): the vault's key, or half of what makes it, for a wallet that
//! needs them to open; and the key its password stretched to, for one left from when they were an
//! alternative to it. Either way, every send it signs waits for one more check.
//!
//! What a wallet needing them does when they fail is the point of this file. A key gone is never
//! forgotten or written over for such a wallet, and a prompt turned down changes nothing: only
//! `turn_off` touches what is kept, and it does nothing for such a wallet.

use {
    crate::app::{Settings, WalletEntry},
    crate::guard::{self, Guard},
    crate::lock::{self, Typed},
    crate::theme::Palette,
    crate::vault::{Book, Vault},
    chainv4_ui::biometric::{Availability, Enrolled, Failure, Kind, Secret},
    egui::Ui,
    std::sync::{Arc, Mutex, PoisonError},
};

#[cfg(not(test))]
use chainv4_ui::biometric as platform;

#[cfg(test)]
use self::fake as platform;

/// A biometric check under way, and what it came to, once it did.
#[derive(Clone, Default)]
pub struct Check(Arc<Mutex<Option<Result<Secret, Failure>>>>);

impl Check {
    /// Shows the prompt for a wallet's biometrics.
    pub fn start(ctx: &egui::Context, enrolled: &Enrolled, reason: &str) -> Check {
        let check = Check::default();
        let slot = check.0.clone();
        platform::open(ctx, enrolled, reason, move |came| *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(came));

        check
    }

    /// What it came to, taken, once it did.
    pub fn take(&self) -> Option<Result<Secret, Failure>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// Keeping a secret behind biometrics: what it came to, once it did.
#[derive(Clone, Default)]
pub struct Enrolling(Arc<Mutex<Option<Result<Enrolled, Failure>>>>);

impl Enrolling {
    pub fn start(ctx: &egui::Context, name: &str, secret: Secret) -> Enrolling {
        let enrolling = Enrolling::default();
        let slot = enrolling.0.clone();
        platform::enroll(ctx, name, secret, move |came| *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(came));

        enrolling
    }

    pub fn take(&self) -> Option<Result<Enrolled, Failure>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// The biometrics of the wallet picked, when it has any and they can be asked for, and what they are
/// called. Those a wallet needs are always asked for: should they be missing now, the prompt fails
/// and nothing is signed, rather than a check being skipped. Those it could do without are left out
/// while they cannot be used, as they always were.
pub fn enrolled(ctx: &egui::Context, settings: &Settings) -> Option<(Enrolled, Kind)> {
    let entry = settings.wallets.get(settings.wallet)?;
    let kept = entry.biometric.as_ref()?;

    if entry.guard().needs_hardware() {
        let enrolled = guard::untagged(kept);
        let kind = platform::kind_of(&enrolled);

        return Some((enrolled, kind));
    }

    match platform::availability(ctx) {
        Availability::Ready(kind) => Some((Enrolled(kept.clone()), kind)),
        Availability::Unavailable(_) => None,
    }
}

/// Whether what a wallet needs can be kept here, and what it is called, or why not. Asked every time,
/// as a finger may be enrolled or removed at any moment.
pub fn bound(ctx: &egui::Context) -> Availability {
    platform::bound(ctx)
}

/// What biometrics are here, today's kind, for naming them in a wallet left with them.
pub fn availability(ctx: &egui::Context) -> Availability {
    platform::availability(ctx)
}

/// What the biometrics a wallet's record names are called.
pub fn kind_of(entry: &WalletEntry) -> Option<Kind> {
    entry.biometric.as_ref().map(|kept| platform::kind_of(&guard::untagged(kept)))
}

/// Whether the wallet's record was made on this kind of device: from anywhere else it never opens.
pub fn kept_here(entry: &WalletEntry) -> bool {
    entry.biometric.as_ref().is_some_and(|kept| platform::kept_here(&guard::untagged(kept)))
}

/// Forgets a secret kept, whose record nothing names: one just made and not used.
pub fn forget(ctx: &egui::Context, enrolled: &Enrolled) {
    platform::forget(ctx, enrolled);
}

/// Forgets the keys of records settings read from disk say are no longer used. As settings were read
/// back saying so, no crash can leave a wallet relying on one.
pub fn retire_all(ctx: &egui::Context, settings: &mut Settings) {
    for enrolled in guard::retired_taken(settings) {
        platform::forget(ctx, &enrolled);
    }
}

/// Turns biometrics off for the wallet picked, forgetting the key they kept: only for a wallet that
/// opens without them. For one that needs them it does nothing, as nothing else would open it.
pub fn turn_off(ctx: &egui::Context, settings: &mut Settings) {
    let Some(entry) = settings.wallets.get_mut(settings.wallet).filter(|entry| !entry.guard().needs_hardware()) else {
        return;
    };

    if let Some(kept) = entry.biometric.take() {
        platform::forget(ctx, &Enrolled(kept));
    }
}

/// What to do when a check finds the key gone, wherever it was asked for: what to tell the person.
/// For a wallet that opens without biometrics, they turn off, as they always did, and its password
/// is what opens it. For one that needs them nothing is written, and the words are the way back.
pub fn invalidated(ctx: &egui::Context, settings: &mut Settings) -> String {
    match settings.wallets.get(settings.wallet) {
        Some(entry) if entry.guard().needs_hardware() => guard::gone(kind_of(entry).unwrap_or(Kind::Biometrics)),
        _ => {
            turn_off(ctx, settings);

            CHANGED.to_string()
        }
    }
}

/// What to tell the person when the fingers or faces enrolled changed, and biometrics went off, on
/// a wallet that opens without them.
pub const CHANGED: &str = "The fingerprints or faces on this device changed, so biometrics are off for this wallet: unlock it with its password. Settings offers modes that use them.";

/// Opens a wallet with what its check gave back, and the password it may need besides.
pub fn open_entry(entry: &WalletEntry, password: &str, secret: &[u8; 32]) -> Result<(Vault, Book), String> {
    match entry.guard() {
        Guard::Both => Vault::open_both(&entry.vault, password, secret),
        _ => Vault::open_with_key(&entry.vault, secret),
    }
}

/// A wallet opened again to prove the person holds what opens it, with the secret it took, if any.
pub struct Proven {
    pub secret: Option<Secret>,
}

/// Asking for what opens the wallet in use before something that needs it: its password, its
/// biometrics, or both, by its guard. In every case it opens the vault as stored, so it proves what
/// the vault itself takes, and never just that a prompt was passed.
#[derive(Default)]
pub struct Proof {
    pub password: Typed,
    check: Option<Check>,
    pub error: Option<String>,
    proven: Option<Proven>,
}

impl Proof {
    /// The password's field, if its guard asks for one. Whether Enter was pressed in it.
    pub fn field(&mut self, ui: &mut Ui, settings: &Settings, hint: &str) -> bool {
        if settings.wallets.get(settings.wallet).is_some_and(|entry| entry.guard() == Guard::Biometric) {
            return false;
        }

        let field = lock::secret(ui, egui::TextEdit::singleline(&mut *self.password).password(true).hint_text(hint).desired_width(f32::INFINITY));

        field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))
    }

    /// Why it failed, if it did, under the field.
    pub fn trouble(&self, ui: &mut Ui, palette: &Palette) {
        if let Some(error) = &self.error {
            ui.colored_label(palette.bad, error.as_str());
        }
    }

    /// Whether a tap has what it needs, and no check is under way.
    pub fn ready(&self, settings: &Settings) -> bool {
        let Some(entry) = settings.wallets.get(settings.wallet) else {
            return false;
        };

        self.check.is_none()
            && match entry.guard() {
                Guard::Biometric => true,
                _ => entry.opens_with_a_tap() || !self.password.is_empty(),
            }
    }

    /// Whether a prompt is waiting for the person.
    pub fn waiting(&self) -> bool {
        self.check.is_some()
    }

    /// Asks, on a tap: at once by the password, or by the prompt shown, its answer coming to `take`.
    pub fn start(&mut self, ctx: &egui::Context, settings: &Settings, reason: &str) {
        self.error = None;
        let Some(entry) = settings.wallets.get(settings.wallet) else {
            self.error = Some("there is no such wallet".to_string());

            return;
        };

        if !entry.guard().needs_hardware() {
            // With no password there is nothing to get wrong: what is typed is not asked for.
            let password = if entry.opens_with_a_tap() { "" } else { self.password.as_str() };

            match Vault::open(&entry.vault, password) {
                Ok(_) => self.proven = Some(Proven { secret: None }),
                Err(why) => self.error = Some(plain(why)),
            }

            return;
        }

        match enrolled(ctx, settings) {
            Some((enrolled, _)) => self.check = Some(Check::start(ctx, &enrolled, reason)),
            None => self.error = Some("this wallet's biometrics are not there".to_string()),
        }
    }

    /// What it came to, once it did. A check turned down or failing leaves all as it was, to try
    /// again, and a key gone writes nothing either.
    pub fn take(&mut self, settings: &Settings) -> Option<Proven> {
        if let Some(came) = self.check.as_ref().and_then(Check::take) {
            self.check = None;
            let entry = settings.wallets.get(settings.wallet)?;

            match came {
                Ok(secret) => match open_entry(entry, &self.password, &secret) {
                    Ok(_) => self.proven = Some(Proven { secret: Some(secret) }),
                    Err(why) => self.error = Some(plain(why)),
                },
                Err(Failure::Canceled) => {}
                Err(Failure::Invalidated) => self.error = Some(guard::gone(kind_of(entry).unwrap_or(Kind::Biometrics))),
                Err(Failure::Failed(why)) => self.error = Some(why),
            }
        }

        self.proven.take()
    }
}

/// What a wrong password is called wherever a wallet's own is being asked for.
fn plain(why: String) -> String {
    match why.as_str() {
        "wrong password" => "not the wallet's password".to_string(),
        _ => why,
    }
}

/// A device that keeps secrets as a test says, for what the wallet does around them: its keys and
/// what they answer, on this thread alone.
#[cfg(test)]
pub(crate) mod fake {
    use {super::*, std::cell::RefCell, std::collections::HashMap};

    /// What the next prompt comes to.
    #[derive(Clone, Debug)]
    pub enum Answer {
        Canceled,
        Failed(&'static str),
        /// Something else than was kept, as a broken key might give.
        Wrong,
    }

    pub struct Device {
        /// Whether a secret can be kept here, or why not.
        pub bound: Availability,
        /// Whether biometrics of today's kind can be used here.
        pub availability: Availability,
        /// The keys kept, by name.
        pub keys: HashMap<String, [u8; 32]>,
        pub made: usize,
        /// What the next prompts come to, before going by the keys.
        pub answers: Vec<Answer>,
        /// How many prompts were shown.
        pub prompts: usize,
        pub forgotten: Vec<String>,
    }

    impl Default for Device {
        fn default() -> Device {
            Device {
                bound: Availability::Ready(Kind::Biometrics),
                availability: Availability::Ready(Kind::Biometrics),
                keys: HashMap::new(),
                made: 0,
                answers: Vec::new(),
                prompts: 0,
                forgotten: Vec::new(),
            }
        }
    }

    thread_local! {
        static DEVICE: RefCell<Device> = RefCell::default();
    }

    /// Does something with this thread's device.
    pub fn device<T>(with: impl FnOnce(&mut Device) -> T) -> T {
        DEVICE.with(|device| with(&mut device.borrow_mut()))
    }

    /// A new device, with nothing kept.
    pub fn fresh() {
        device(|device| *device = Device::default());
    }

    /// Keeps a secret, as enrolling it would, without a prompt: its record.
    pub fn keep(secret: &[u8; 32]) -> Enrolled {
        device(|device| {
            device.made += 1;
            let alias = format!("key{}", device.made);
            device.keys.insert(alias.clone(), *secret);

            Enrolled(format!("fake:{alias}"))
        })
    }

    /// Every key is gone, as a new fingerprint or another phone would make it.
    pub fn lose_every_key() {
        device(|device| device.keys.clear());
    }

    fn answer(device: &mut Device) -> Option<Answer> {
        device.prompts += 1;

        (!device.answers.is_empty()).then(|| device.answers.remove(0))
    }

    pub fn availability(_: &egui::Context) -> Availability {
        device(|device| device.availability.clone())
    }

    pub fn bound(_: &egui::Context) -> Availability {
        device(|device| device.bound.clone())
    }

    pub fn kind_of(_: &Enrolled) -> Kind {
        Kind::Biometrics
    }

    pub fn kept_here(enrolled: &Enrolled) -> bool {
        enrolled.0.starts_with("fake:")
    }

    pub fn enroll(_: &egui::Context, _: &str, secret: Secret, done: impl FnOnce(Result<Enrolled, Failure>) + Send + 'static) {
        match device(answer) {
            Some(Answer::Canceled) => done(Err(Failure::Canceled)),
            Some(Answer::Failed(why)) => done(Err(Failure::Failed(why.to_string()))),
            _ => done(Ok(keep(&secret))),
        }
    }

    pub fn open(_: &egui::Context, enrolled: &Enrolled, _: &str, done: impl FnOnce(Result<Secret, Failure>) + Send + 'static) {
        let alias = enrolled.0.strip_prefix("fake:").unwrap_or_default().to_string();
        let (answered, kept) = device(|device| (answer(device), device.keys.get(&alias).copied()));

        done(match (answered, kept) {
            (Some(Answer::Canceled), _) => Err(Failure::Canceled),
            (Some(Answer::Failed(why)), _) => Err(Failure::Failed(why.to_string())),
            (_, None) => Err(Failure::Invalidated),
            (Some(Answer::Wrong), Some(_)) => Ok(zeroize::Zeroizing::new([0xee; 32])),
            (None, Some(secret)) => Ok(zeroize::Zeroizing::new(secret)),
        });
    }

    pub fn forget(_: &egui::Context, enrolled: &Enrolled) {
        let alias = enrolled.0.strip_prefix("fake:").unwrap_or_default().to_string();
        device(|device| {
            device.keys.remove(&alias);
            device.forgotten.push(alias);
        });
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::guard::{Factors, sealed_anew, tagged},
        crate::vault::{Sealed, Words, fresh_secret},
    };

    fn book() -> Book {
        Book { phrases: vec![Words::new("test test test test test test test test test test test junk".to_string(), String::new())], ..Book::default() }
    }

    /// Settings of one wallet needing hardware, and the secret in the fake device.
    fn needing(guard: Guard) -> (Settings, Secret) {
        fake::fresh();
        let secret = fresh_secret().unwrap();
        let sealed = match guard {
            Guard::Both => sealed_anew(&Factors::Both("correct horse", &secret), &book()).unwrap().1,
            _ => sealed_anew(&Factors::Biometric(&secret), &book()).unwrap().1,
        };
        let enrolled = fake::keep(&secret);
        let settings = Settings::with_wallets(vec![entry(sealed, tagged(guard, &enrolled), false)]);

        (settings, secret)
    }

    fn entry(vault: Sealed, biometric: Option<String>, unprotected: bool) -> WalletEntry {
        WalletEntry { id: String::new(), name: "Mine".to_string(), vault, current: String::new(), biometric, unprotected }
    }

    #[test]
    fn a_key_gone_writes_nothing_for_a_wallet_that_needs_it_and_says_how_to_get_back() {
        let ctx = egui::Context::default();

        for guard in [Guard::Biometric, Guard::Both] {
            let (mut settings, _) = needing(guard);
            let before = settings.wallets.clone();
            fake::lose_every_key();
            let said = invalidated(&ctx, &mut settings);

            assert_eq!(settings.wallets, before, "{guard:?}: nothing written");
            assert!(settings.retired.is_empty() && !settings.persist_now, "{guard:?}");
            assert!(fake::device(|device| device.forgotten.is_empty()), "{guard:?}: nothing forgotten");
            assert!(said.contains("gone from this device") && said.contains("Restore from words"), "{said}");
            assert!(!said.contains("password"), "no password can help: {said}");

            // Nor does anything that turns biometrics off.
            turn_off(&ctx, &mut settings);

            assert_eq!(settings.wallets, before, "{guard:?}");
        }
    }

    #[test]
    fn a_key_gone_turns_biometrics_off_for_a_wallet_with_a_password_as_it_always_did() {
        let ctx = egui::Context::default();
        fake::fresh();
        let vault = Vault::create("correct horse").unwrap();
        let enrolled = fake::keep(&vault.key());
        let sealed = vault.seal(&book()).unwrap();
        let mut settings = Settings::with_wallets(vec![entry(sealed.clone(), Some(enrolled.0.clone()), false)]);

        assert_eq!(settings.wallets[0].guard(), Guard::Either);
        assert_eq!(enrolled_of(&ctx, &settings), Some(enrolled.clone()));

        let said = invalidated(&ctx, &mut settings);

        assert_eq!(said, CHANGED);
        assert_eq!(settings.wallets[0].biometric, None);
        assert_eq!(settings.wallets[0].guard(), Guard::Password);
        assert_eq!(settings.wallets[0].vault, sealed, "and its vault as it was");
        assert_eq!(Vault::open(&settings.wallets[0].vault, "correct horse").unwrap().1, book());
    }

    fn enrolled_of(ctx: &egui::Context, settings: &Settings) -> Option<Enrolled> {
        enrolled(ctx, settings).map(|(enrolled, _)| enrolled)
    }

    #[test]
    fn a_wallet_that_needs_biometrics_is_asked_for_them_even_when_they_read_as_unavailable() {
        let ctx = egui::Context::default();
        let (settings, _) = needing(Guard::Biometric);
        fake::device(|device| device.availability = Availability::Unavailable("no finger enrolled".to_string()));

        assert!(enrolled_of(&ctx, &settings).is_some(), "the check is made, and fails, rather than being skipped");

        // Whereas today's kind goes without, as it always did.
        let either = Settings::with_wallets(vec![entry(settings.wallets[0].vault.clone(), Some("fake:key1".to_string()), false)]);

        assert_eq!(enrolled_of(&ctx, &either), None);
    }

    #[test]
    fn each_guard_proves_by_opening_the_vault_as_stored_and_any_factor_missing_proves_nothing() {
        let ctx = egui::Context::default();

        // Its password alone.
        fake::fresh();
        let sealed = Vault::create("correct horse").unwrap().seal(&book()).unwrap();
        let settings = Settings::with_wallets(vec![entry(sealed, None, false)]);
        let mut proof = Proof::default();
        assert!(!proof.ready(&settings), "nothing typed");
        proof.password.push_str("wrong");
        proof.start(&ctx, &settings, "Prove");
        assert!(proof.take(&settings).is_none());
        assert_eq!(proof.error.as_deref(), Some("not the wallet's password"));
        proof.password.clear();
        proof.password.push_str("correct horse");
        proof.start(&ctx, &settings, "Prove");
        assert!(proof.take(&settings).unwrap().secret.is_none());
        assert_eq!(fake::device(|device| device.prompts), 0, "no prompt for a password");

        // Biometrics alone: passed, turned down, failing, and gone.
        let (settings, secret) = needing(Guard::Biometric);
        let mut proof = Proof::default();
        assert!(proof.ready(&settings), "no password to wait for");
        fake::device(|device| device.answers.push(fake::Answer::Canceled));
        proof.start(&ctx, &settings, "Prove");
        assert!(proof.take(&settings).is_none());
        assert_eq!(proof.error, None, "turned down is not a failure, and leaves all as it was");
        fake::device(|device| device.answers.push(fake::Answer::Failed("the sensor is busy")));
        proof.start(&ctx, &settings, "Prove");
        assert!(proof.take(&settings).is_none());
        assert_eq!(proof.error.as_deref(), Some("the sensor is busy"));
        proof.start(&ctx, &settings, "Prove");
        assert_eq!(*proof.take(&settings).unwrap().secret.unwrap(), *secret);
        // A key that answers with something else proves nothing: the vault would not open.
        fake::device(|device| device.answers.push(fake::Answer::Wrong));
        proof.start(&ctx, &settings, "Prove");
        assert!(proof.take(&settings).is_none());
        fake::lose_every_key();
        proof.start(&ctx, &settings, "Prove");
        assert!(proof.take(&settings).is_none());
        assert!(proof.error.as_deref().unwrap().contains("gone from this device"));

        // Both: the check passing with the wrong password proves nothing, nor the right one without
        // the check, and only the two together do.
        let (settings, secret) = needing(Guard::Both);
        let mut proof = Proof::default();
        assert!(!proof.ready(&settings), "the password first");
        proof.password.push_str("wrong");
        assert!(proof.ready(&settings));
        proof.start(&ctx, &settings, "Prove");
        assert!(proof.take(&settings).is_none());
        assert_eq!(proof.error.as_deref(), Some("not the wallet's password"));
        proof.password.clear();
        proof.password.push_str("correct horse");
        fake::device(|device| device.answers.push(fake::Answer::Canceled));
        proof.start(&ctx, &settings, "Prove");
        assert!(proof.take(&settings).is_none());
        proof.start(&ctx, &settings, "Prove");
        assert_eq!(*proof.take(&settings).unwrap().secret.unwrap(), *secret);
    }

    #[test]
    fn no_wallet_is_opened_from_the_empty_password_when_its_record_was_stripped() {
        let (mut settings, _) = needing(Guard::Biometric);
        settings.wallets[0].biometric = None;
        settings.wallets[0].unprotected = true;
        let ctx = egui::Context::default();
        let mut proof = Proof::default();
        proof.start(&ctx, &settings, "Prove");

        assert!(proof.take(&settings).is_none());
        assert_eq!(proof.error.as_deref(), Some("not the wallet's password"));
    }

    #[test]
    fn what_settings_say_is_retired_is_forgotten_as_the_app_starts() {
        let ctx = egui::Context::default();
        let (mut settings, _) = needing(Guard::Biometric);
        let other = fake::keep(&[9; 32]);
        settings.retired = vec![settings.wallets[0].biometric.clone().unwrap(), format!("both-v1:{}", other.0)];
        retire_all(&ctx, &mut settings);

        assert_eq!(fake::device(|device| device.forgotten.clone()), ["key2"], "the one in use stays");
        assert!(fake::device(|device| device.keys.contains_key("key1")));
        assert!(settings.retired.is_empty());
    }

    #[test]
    fn no_page_turns_biometrics_off_for_itself_when_a_key_is_gone() {
        // What keeps a wallet that needs biometrics from being stranded: every page goes through
        // `invalidated`, so none forgets anything on its own.
        for (page, source, checks) in [
            ("send", include_str!("views/send.rs"), 1),
            ("sign", include_str!("views/sign.rs"), 4),
            ("dapp", include_str!("views/dapp.rs"), 1),
            ("evm", include_str!("views/evm.rs"), 1),
        ] {
            assert!(!source.contains("biometrics::turn_off"), "{page}");
            assert!(!source.contains("biometrics::CHANGED"), "{page}");
            assert_eq!(source.matches("biometrics::invalidated(").count(), checks, "{page}");
        }

        assert!(!include_str!("lock.rs").contains("biometrics::turn_off"));
    }
}
