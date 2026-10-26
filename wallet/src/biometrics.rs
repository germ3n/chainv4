//! Biometrics in the wallet. A wallet can keep its vault's key behind a fingerprint or face, Touch
//! ID or a passkey (see chainv4_ui::biometric), which then opens it in place of the password, and
//! every send it signs waits for one more check. The password stays: it opens the wallet when
//! biometrics are turned down or gone, and turns them back on once the fingers enrolled change.

use {
    crate::app::Settings,
    chainv4_ui::biometric::{self, Availability, Enrolled, Failure, Kind, Secret},
    std::sync::{Arc, Mutex, PoisonError},
};

/// A biometric check under way, and what it came to, once it did.
#[derive(Clone, Default)]
pub struct Check(Arc<Mutex<Option<Result<Secret, Failure>>>>);

impl Check {
    /// Shows the prompt for a wallet's biometrics.
    pub fn start(ctx: &egui::Context, enrolled: &Enrolled, reason: &str) -> Check {
        let check = Check::default();
        let slot = check.0.clone();
        biometric::open(ctx, enrolled, reason, move |came| *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(came));

        check
    }

    /// What it came to, taken, once it did.
    pub fn take(&self) -> Option<Result<Secret, Failure>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// Turning biometrics on: what it came to, once it did.
#[derive(Clone, Default)]
pub struct Enrolling(Arc<Mutex<Option<Result<Enrolled, Failure>>>>);

impl Enrolling {
    pub fn start(ctx: &egui::Context, name: &str, secret: Secret) -> Enrolling {
        let enrolling = Enrolling::default();
        let slot = enrolling.0.clone();
        biometric::enroll(ctx, name, secret, move |came| *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(came));

        enrolling
    }

    pub fn take(&self) -> Option<Result<Enrolled, Failure>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// The biometrics of the wallet picked, when they are on and can be used here, and what they are
/// called.
pub fn enrolled(ctx: &egui::Context, settings: &Settings) -> Option<(Enrolled, Kind)> {
    let kept = settings.wallets.get(settings.wallet)?.biometric.clone()?;

    match biometric::availability(ctx) {
        Availability::Ready(kind) => Some((Enrolled(kept), kind)),
        Availability::Unavailable(_) => None,
    }
}

/// Turns biometrics off for the wallet picked, forgetting the key they kept.
pub fn turn_off(ctx: &egui::Context, settings: &mut Settings) {
    if let Some(kept) = settings.wallets.get_mut(settings.wallet).and_then(|entry| entry.biometric.take()) {
        biometric::forget(ctx, &Enrolled(kept));
    }
}

/// What to tell the person when the fingers or faces enrolled changed, and biometrics went off.
pub const CHANGED: &str = "The fingerprints or faces on this device changed, so biometrics are off for this wallet: unlock it with its password, and turn them on again in Settings.";
