//! Biometrics, alike wherever an app runs: a secret kept so that only the person's fingerprint,
//! face or device unlock gives it back, and a check that they are there before something happens.
//!
//! - **Android:** an AES key in the Android Keystore wraps the secret. It opens only after a strong
//!   biometric check, BiometricPrompt, and a fingerprint or face enrolled after it was made
//!   invalidates it for good. The prompt is shown by a Java class the APK carries,
//!   `ui/android/Biometric.java`, which `ui/build-android.sh` compiles in with the file picker.
//! - **The browser:** a passkey, through WebAuthn. Its PRF extension gives a key that only the
//!   passkey's user verification releases, Touch ID, Windows Hello or a phone's unlock, and that
//!   key wraps the secret. A browser or passkey without PRF cannot keep one.
//! - **macOS:** Touch ID, through LocalAuthentication, before the secret is read from the login
//!   Keychain. The fingers enrolled are remembered as they were, and once they change the secret is
//!   forgotten, as on Android. The Keychain item itself is guarded by the login, not by Touch ID: an
//!   unsigned app cannot ask the Keychain for that.
//! - **Elsewhere:** none, and [`availability`] says so.
//!
//! What a check comes to arrives later, through a callback, as the prompt is the system's own;
//! the app is repainted when it does.

/// A secret biometrics keep: 32 bytes, such as the key a password stretches to.
pub type Secret = [u8; 32];

/// What biometrics are called here, for the app to name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Android's fingerprint or face.
    Biometrics,
    TouchId,
    Passkey,
}

impl Kind {
    /// How the app names it: "fingerprint or face", "Touch ID" or "a passkey".
    pub fn name(self) -> &'static str {
        match self {
            Kind::Biometrics => "fingerprint or face",
            Kind::TouchId => "Touch ID",
            Kind::Passkey => "a passkey",
        }
    }
}

/// Whether biometrics can keep a secret here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Ready(Kind),
    /// Why not: no such hardware, nothing enrolled, or a platform without them.
    Unavailable(String),
}

/// A secret kept, as the app stores it to have it back: the platform's own words, which name a key
/// and hold the secret wrapped, never the secret itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enrolled(pub String);

/// Why a check did not give the secret back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The person turned the prompt down, or asked for the password instead.
    Canceled,
    /// The fingers or faces enrolled changed since, or the key is gone: the secret is lost, and
    /// the password is needed to keep it again.
    Invalidated,
    Failed(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Canceled => write!(formatter, "canceled"),
            Failure::Invalidated => write!(formatter, "the biometrics enrolled changed, so the password is needed again"),
            Failure::Failed(why) => write!(formatter, "{why}"),
        }
    }
}

/// Whether biometrics can keep a secret here, and what they are called.
pub fn availability(ctx: &egui::Context) -> Availability {
    #[cfg(target_os = "android")]
    return android::availability(ctx);

    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;

        web::availability()
    }

    #[cfg(target_os = "macos")]
    {
        let _ = ctx;

        mac::availability()
    }

    #[cfg(not(any(target_os = "android", target_arch = "wasm32", target_os = "macos")))]
    {
        let _ = ctx;

        Availability::Unavailable("this system has no biometrics the app can use".to_string())
    }
}

/// Keeps a secret behind biometrics, after a check that the person is there: what to store comes
/// to done. name says what it is for, in the prompt and, for a passkey, as its name.
pub fn enroll(ctx: &egui::Context, name: &str, secret: Secret, done: impl FnOnce(Result<Enrolled, Failure>) + Send + 'static) {
    let done = waking(ctx, done);

    #[cfg(target_os = "android")]
    android::enroll(ctx, name, secret, Box::new(done));

    #[cfg(target_arch = "wasm32")]
    {
        let name = name.to_string();
        wasm_bindgen_futures::spawn_local(async move { done(web::enroll(&name, secret).await) });
    }

    #[cfg(target_os = "macos")]
    mac::enroll(name, secret, done);

    #[cfg(not(any(target_os = "android", target_arch = "wasm32", target_os = "macos")))]
    {
        let _ = (name, secret);
        done(Err(Failure::Failed("this system has no biometrics the app can use".to_string())));
    }
}

/// Gives a secret kept back, after a check: also how to confirm the person is there before
/// something, the secret then unused. reason says why, in the prompt.
pub fn open(ctx: &egui::Context, enrolled: &Enrolled, reason: &str, done: impl FnOnce(Result<Secret, Failure>) + Send + 'static) {
    let done = waking(ctx, done);

    #[cfg(target_os = "android")]
    android::open(ctx, enrolled, reason, Box::new(done));

    #[cfg(target_arch = "wasm32")]
    {
        let (enrolled, _) = (enrolled.clone(), reason);
        wasm_bindgen_futures::spawn_local(async move { done(web::open(&enrolled).await) });
    }

    #[cfg(target_os = "macos")]
    mac::open(enrolled, reason, done);

    #[cfg(not(any(target_os = "android", target_arch = "wasm32", target_os = "macos")))]
    {
        let _ = (enrolled, reason);
        done(Err(Failure::Failed("this system has no biometrics the app can use".to_string())));
    }
}

/// Forgets a secret kept: its Android Keystore key or its Keychain item goes. A passkey stays with
/// the browser or the device, which offer to remove it; without the wrapped secret it opens nothing.
pub fn forget(ctx: &egui::Context, enrolled: &Enrolled) {
    #[cfg(target_os = "android")]
    android::forget(ctx, enrolled);

    #[cfg(target_os = "macos")]
    {
        let _ = ctx;
        mac::forget(enrolled);
    }

    #[cfg(not(any(target_os = "android", target_os = "macos")))]
    let _ = (ctx, enrolled);
}

/// done, repainting the app once it ran.
fn waking<T>(ctx: &egui::Context, done: impl FnOnce(T) + Send + 'static) -> impl FnOnce(T) + Send + 'static {
    let ctx = ctx.clone();

    move |result| {
        done(result);
        ctx.request_repaint();
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }

    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok()).collect()
}

/// An enrollment's parts: its platform's tag, then its fields, as hex where they are bytes.
fn parts<'a>(enrolled: &'a Enrolled, tag: &str, count: usize) -> Result<Vec<&'a str>, Failure> {
    let parts: Vec<&str> = enrolled.0.split(':').collect();

    match parts.split_first() {
        Some((first, rest)) if *first == tag && rest.len() == count => Ok(rest.to_vec()),
        _ => Err(Failure::Failed("biometrics were turned on elsewhere, or with an older app: turn them on again here".to_string())),
    }
}

/// Fresh random bytes, as hex: a name for a key no other has.
#[cfg_attr(not(any(target_os = "android", target_os = "macos")), allow(dead_code))]
fn random_name() -> Result<String, Failure> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|err| Failure::Failed(format!("no randomness: {err}")))?;

    Ok(hex(&bytes))
}

/// On Android, a check is asked for through the egui context, which the app's Android glue owns
/// and checks every frame: it starts the prompt, waits for it, and hands over what came of it.
#[cfg(any(target_os = "android", test))]
pub(crate) mod android {
    use {
        super::{Availability, Enrolled, Failure, Kind, Secret},
        std::sync::{Arc, Mutex, PoisonError},
    };

    /// What the Java side does: keep a secret under a new key, or open one with its key.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Mode {
        Enroll = 0,
        Open = 1,
    }

    /// What a check came to: the bytes out and, for an enrollment, the IV they were sealed with.
    pub(crate) type Came = Result<(Vec<u8>, Vec<u8>), Failure>;

    /// A check asked for, for the glue to start, or a key to forget, at once.
    pub(crate) enum Asked {
        Check {
            alias: String,
            mode: Mode,
            data: Vec<u8>,
            iv: Vec<u8>,
            title: String,
            subtitle: String,
            done: Box<dyn FnOnce(Came) + Send>,
        },
        Forget {
            alias: String,
        },
    }

    /// What was asked and not yet taken, in order.
    #[derive(Clone, Default)]
    struct Asks(Arc<Mutex<Vec<Asked>>>);

    fn asks(ctx: &egui::Context) -> Asks {
        ctx.data_mut(|data| data.get_temp_mut_or_default::<Asks>(egui::Id::new("chainv4-ui biometric")).clone())
    }

    fn ask(ctx: &egui::Context, asked: Asked) {
        asks(ctx).0.lock().unwrap_or_else(PoisonError::into_inner).push(asked);
        ctx.request_repaint();
    }

    /// What was asked since last time, taken, oldest first.
    pub(crate) fn asked(ctx: &egui::Context) -> Vec<Asked> {
        std::mem::take(&mut *asks(ctx).0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// What `Biometric.available` last said: 2 ready, 1 nothing enrolled, 0 no strong biometrics or
    /// Android older than 9; and -1 for an APK without the class.
    pub(crate) fn set_level(ctx: &egui::Context, level: i32) {
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("chainv4-ui biometric level"), level));
    }

    pub(crate) fn availability(ctx: &egui::Context) -> Availability {
        match ctx.data(|data| data.get_temp::<i32>(egui::Id::new("chainv4-ui biometric level"))).unwrap_or(-1) {
            2 => Availability::Ready(Kind::Biometrics),
            1 => Availability::Unavailable("no fingerprint or face is enrolled: add one in Android's settings".to_string()),
            0 => Availability::Unavailable("this phone has no strong biometrics, or is older than Android 9".to_string()),
            _ => Availability::Unavailable("this app was built without biometrics".to_string()),
        }
    }

    pub(crate) fn enroll(ctx: &egui::Context, name: &str, secret: Secret, done: Box<dyn FnOnce(Result<Enrolled, Failure>) + Send>) {
        let alias = match super::random_name() {
            Ok(name) => format!("chainv4-{name}"),
            Err(failure) => return done(Err(failure)),
        };
        let kept = alias.clone();

        ask(ctx, Asked::Check {
            alias,
            mode: Mode::Enroll,
            data: secret.to_vec(),
            iv: Vec::new(),
            title: format!("Unlock {name} with your fingerprint or face"),
            subtitle: "Confirm it is you to turn this on".to_string(),
            done: Box::new(move |came: Came| done(came.map(|(sealed, iv)| Enrolled(format!("android:{kept}:{}:{}", super::hex(&iv), super::hex(&sealed)))))),
        });
    }

    pub(crate) fn open(ctx: &egui::Context, enrolled: &Enrolled, reason: &str, done: Box<dyn FnOnce(Result<Secret, Failure>) + Send>) {
        let parts = match super::parts(enrolled, "android", 3) {
            Ok(parts) => parts,
            Err(failure) => return done(Err(failure)),
        };
        let (Some(iv), Some(sealed)) = (super::unhex(parts[1]), super::unhex(parts[2])) else {
            return done(Err(Failure::Failed("the biometric key's record is damaged".to_string())));
        };

        ask(ctx, Asked::Check {
            alias: parts[0].to_string(),
            mode: Mode::Open,
            data: sealed,
            iv,
            title: reason.to_string(),
            subtitle: String::new(),
            done: Box::new(move |came: Came| {
                done(came.and_then(|(secret, _)| secret.try_into().map_err(|_| Failure::Failed("the biometric key gave back something else".to_string()))))
            }),
        });
    }

    pub(crate) fn forget(ctx: &egui::Context, enrolled: &Enrolled) {
        if let Ok(parts) = super::parts(enrolled, "android", 3) {
            ask(ctx, Asked::Forget { alias: parts[0].to_string() });
        }
    }
}

/// A passkey, through WebAuthn: its PRF extension's output, released only by the passkey's user
/// verification, wraps the secret.
#[cfg(target_arch = "wasm32")]
mod web {
    use {
        super::{Availability, Enrolled, Failure, Kind, Secret},
        chacha20poly1305::aead::{Aead, KeyInit},
        chacha20poly1305::{ChaCha20Poly1305, Key, Nonce},
        js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array},
        wasm_bindgen::{JsCast, JsValue},
        wasm_bindgen_futures::JsFuture,
    };

    /// How long the browser waits for the person.
    const TIMEOUT_MS: f64 = 120_000.0;

    fn get(target: &JsValue, key: &str) -> JsValue {
        Reflect::get(target, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    fn object(fields: &[(&str, JsValue)]) -> JsValue {
        let object = Object::new();

        for (key, value) in fields {
            let _ = Reflect::set(&object, &JsValue::from_str(key), value);
        }

        object.into()
    }

    fn bytes(data: &[u8]) -> JsValue {
        Uint8Array::from(data).into()
    }

    fn random<const N: usize>() -> Result<[u8; N], Failure> {
        let mut bytes = [0; N];
        getrandom::fill(&mut bytes).map_err(|err| Failure::Failed(format!("no randomness: {err}")))?;

        Ok(bytes)
    }

    /// navigator.credentials, where the browser has passkeys.
    fn credentials() -> Option<JsValue> {
        let window = web_sys::window()?;

        if get(&window, "PublicKeyCredential").is_undefined() {
            return None;
        }

        Some(get(&window.navigator(), "credentials")).filter(|credentials| !credentials.is_undefined())
    }

    pub(super) fn availability() -> Availability {
        match credentials() {
            Some(_) => Availability::Ready(Kind::Passkey),
            None => Availability::Unavailable("this browser has no passkeys, or the page is not served over HTTPS".to_string()),
        }
    }

    /// What a WebAuthn call came to as a failure: turned down, or something else.
    fn failure(err: JsValue) -> Failure {
        match get(&err, "name").as_string().as_deref() {
            Some("NotAllowedError" | "AbortError") => Failure::Canceled,
            _ => Failure::Failed(get(&err, "message").as_string().unwrap_or_else(|| format!("{err:?}"))),
        }
    }

    /// navigator.credentials.create or get, with these publicKey options.
    async fn call(method: &str, public_key: JsValue) -> Result<JsValue, Failure> {
        let credentials = credentials().ok_or_else(|| Failure::Failed("this browser has no passkeys".to_string()))?;
        let function: Function = get(&credentials, method).dyn_into().map_err(failure)?;
        let promise: Promise = function.call1(&credentials, &object(&[("publicKey", public_key)])).map_err(failure)?.dyn_into().map_err(failure)?;

        JsFuture::from(promise).await.map_err(failure)
    }

    /// The PRF extension's output for the salt asked about, when the credential gave one.
    fn prf_output(credential: &JsValue) -> Result<Option<Vec<u8>>, Failure> {
        let results: Function = get(credential, "getClientExtensionResults").dyn_into().map_err(failure)?;
        let results = results.call0(credential).map_err(failure)?;
        let prf = get(&results, "prf");
        let first = get(&get(&prf, "results"), "first");

        if !first.is_undefined() {
            return Ok(Some(Uint8Array::new(&first).to_vec()));
        }

        if get(&prf, "enabled").as_bool() == Some(false) || prf.is_undefined() {
            return Err(Failure::Failed(
                "this browser or passkey cannot release a key (WebAuthn's PRF extension): use a recent Chrome, Edge or Safari, with the device's own passkeys".to_string(),
            ));
        }

        Ok(None)
    }

    fn prf(salt: &[u8]) -> JsValue {
        object(&[("prf", object(&[("eval", object(&[("first", bytes(salt))]))]))])
    }

    /// Asks the passkey for its PRF output for a salt: what wraps the secret.
    async fn assert(id: &[u8], salt: &[u8]) -> Result<Vec<u8>, Failure> {
        let allowed = Array::of1(&object(&[("type", "public-key".into()), ("id", bytes(id))]));
        let credential = call(
            "get",
            object(&[
                ("challenge", bytes(&random::<32>()?)),
                ("allowCredentials", allowed.into()),
                ("userVerification", "required".into()),
                ("timeout", TIMEOUT_MS.into()),
                ("extensions", prf(salt)),
            ]),
        )
        .await?;

        prf_output(&credential)?.ok_or_else(|| Failure::Failed("the passkey released no key".to_string()))
    }

    fn cipher(key: &[u8]) -> Result<ChaCha20Poly1305, Failure> {
        let key: [u8; 32] = key.get(..32).and_then(|key| key.try_into().ok()).ok_or_else(|| Failure::Failed("the passkey released too short a key".to_string()))?;

        Ok(ChaCha20Poly1305::new(Key::from_slice(&key)))
    }

    pub(super) async fn enroll(name: &str, secret: Secret) -> Result<Enrolled, Failure> {
        let salt = random::<32>()?;
        let credential = call(
            "create",
            object(&[
                ("rp", object(&[("name", "chainv4 wallet".into())])),
                ("user", object(&[("id", bytes(&random::<16>()?)), ("name", name.into()), ("displayName", name.into())])),
                ("challenge", bytes(&random::<32>()?)),
                (
                    "pubKeyCredParams",
                    Array::of2(&object(&[("type", "public-key".into()), ("alg", (-7).into())]), &object(&[("type", "public-key".into()), ("alg", (-257).into())])).into(),
                ),
                ("authenticatorSelection", object(&[("userVerification", "required".into()), ("residentKey", "preferred".into())])),
                ("timeout", TIMEOUT_MS.into()),
                ("extensions", prf(&salt)),
            ]),
        )
        .await?;
        let id = Uint8Array::new(&get(&credential, "rawId")).to_vec();
        // Some passkeys give the output as they are made; the rest, once asked.
        let output = match prf_output(&credential)? {
            Some(output) => output,
            None => assert(&id, &salt).await?,
        };
        let nonce = random::<12>()?;
        let wrapped = cipher(&output)?
            .encrypt(Nonce::from_slice(&nonce), secret.as_slice())
            .map_err(|_| Failure::Failed("wrapping the secret failed".to_string()))?;

        Ok(Enrolled(format!("passkey:{}:{}:{}:{}", super::hex(&id), super::hex(&salt), super::hex(&nonce), super::hex(&wrapped))))
    }

    pub(super) async fn open(enrolled: &Enrolled) -> Result<Secret, Failure> {
        let parts = super::parts(enrolled, "passkey", 4)?;
        let read: Vec<Vec<u8>> = parts
            .iter()
            .map(|part| super::unhex(part))
            .collect::<Option<_>>()
            .ok_or_else(|| Failure::Failed("the passkey's record is damaged".to_string()))?;
        let [id, salt, nonce, wrapped] = &read[..] else {
            unreachable!("four parts, as parts checked");
        };
        let output = assert(id, salt).await?;
        let nonce: [u8; 12] = nonce.as_slice().try_into().map_err(|_| Failure::Failed("the passkey's record is damaged".to_string()))?;
        let secret = cipher(&output)?
            .decrypt(Nonce::from_slice(&nonce), wrapped.as_slice())
            .map_err(|_| Failure::Failed("that passkey is not the one this was kept with".to_string()))?;

        secret.try_into().map_err(|_| Failure::Failed("the passkey's record is damaged".to_string()))
    }
}

/// Touch ID, through LocalAuthentication, and the secret in the login Keychain.
#[cfg(target_os = "macos")]
mod mac {
    use {
        super::{Availability, Enrolled, Failure, Kind, Secret},
        block2::RcBlock,
        objc2::runtime::Bool,
        objc2_foundation::{NSError, NSString},
        objc2_local_authentication::{LAContext, LAError, LAPolicy},
        std::sync::Mutex,
    };

    /// The Keychain service the secrets are kept under, each its own account.
    const SERVICE: &str = "org.chainv4.ui biometric";
    const POLICY: LAPolicy = LAPolicy::DeviceOwnerAuthenticationWithBiometrics;

    pub(super) fn availability() -> Availability {
        // SAFETY: a fresh context, asked about a policy; nothing else holds it.
        let context = unsafe { LAContext::new() };

        match unsafe { context.canEvaluatePolicy_error(POLICY) } {
            Ok(()) => Availability::Ready(Kind::TouchId),
            Err(err) => Availability::Unavailable(err.localizedDescription().to_string()),
        }
    }

    /// Asks for Touch ID; then has the fingers enrolled as they are now, an opaque value that
    /// changes when they do.
    fn check(reason: &str, then: impl FnOnce(Result<Vec<u8>, Failure>) + Send + 'static) {
        // SAFETY: a fresh context, kept alive by the reply until it runs.
        let context = unsafe { LAContext::new() };
        let held = context.clone();
        let then = Mutex::new(Some(then));
        let reply = RcBlock::new(move |ok: Bool, error: *mut NSError| {
            let Some(then) = then.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() else {
                return;
            };

            if ok.as_bool() {
                // SAFETY: the context was evaluated, which is when it has a domain state.
                let state = unsafe { held.evaluatedPolicyDomainState() }.map(|data| data.to_vec()).unwrap_or_default();

                return then(Ok(state));
            }

            // SAFETY: LocalAuthentication hands a valid error, or none, to a reply that failed.
            let failure = match unsafe { error.as_ref() } {
                Some(error) if [LAError::UserCancel, LAError::UserFallback, LAError::SystemCancel, LAError::AppCancel].contains(&LAError(error.code())) => Failure::Canceled,
                Some(error) if LAError(error.code()) == LAError::BiometryNotEnrolled => Failure::Invalidated,
                Some(error) => Failure::Failed(error.localizedDescription().to_string()),
                None => Failure::Failed("Touch ID failed".to_string()),
            };
            then(Err(failure));
        });

        // SAFETY: the reason is a valid string, and the reply block lives as long as the call needs.
        unsafe { context.evaluatePolicy_localizedReason_reply(POLICY, &NSString::from_str(reason), &reply) };
    }

    pub(super) fn enroll(name: &str, secret: Secret, done: impl FnOnce(Result<Enrolled, Failure>) + Send + 'static) {
        check(&format!("turn on Touch ID for {name}"), move |checked| {
            done(checked.and_then(|state| {
                let account = super::random_name()?;
                security_framework::passwords::set_generic_password(SERVICE, &account, super::hex(&secret).as_bytes())
                    .map_err(|err| Failure::Failed(format!("the Keychain refused the secret: {err}")))?;

                Ok(Enrolled(format!("macos:{account}:{}", super::hex(&state))))
            }))
        });
    }

    pub(super) fn open(enrolled: &Enrolled, reason: &str, done: impl FnOnce(Result<Secret, Failure>) + Send + 'static) {
        let parts = match super::parts(enrolled, "macos", 2) {
            Ok(parts) => parts,
            Err(failure) => return done(Err(failure)),
        };
        let (account, enrolled_state) = (parts[0].to_string(), parts[1].to_string());

        check(reason, move |checked| {
            done(checked.and_then(|state| {
                // Fingers added or removed since: as Android does, the secret is forgotten.
                if super::hex(&state) != enrolled_state {
                    let _ = security_framework::passwords::delete_generic_password(SERVICE, &account);

                    return Err(Failure::Invalidated);
                }

                let kept = security_framework::passwords::get_generic_password(SERVICE, &account).map_err(|_| Failure::Invalidated)?;

                std::str::from_utf8(&kept)
                    .ok()
                    .and_then(super::unhex)
                    .and_then(|secret| secret.try_into().ok())
                    .ok_or_else(|| Failure::Failed("the Keychain item is damaged".to_string()))
            }))
        });
    }

    pub(super) fn forget(enrolled: &Enrolled) {
        if let Ok(parts) = super::parts(enrolled, "macos", 2) {
            let _ = security_framework::passwords::delete_generic_password(SERVICE, parts[0]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_read_back_and_refuse_another_platforms() {
        assert_eq!(unhex(&hex(&[0, 1, 0xab, 0xff])), Some(vec![0, 1, 0xab, 0xff]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);

        let enrolled = Enrolled("android:chainv4-00ff:0a0b:0c0d".to_string());

        assert_eq!(parts(&enrolled, "android", 3), Ok(vec!["chainv4-00ff", "0a0b", "0c0d"]));
        assert!(parts(&enrolled, "macos", 2).is_err(), "a record from another platform is no use here");
        assert!(parts(&enrolled, "android", 2).is_err());
    }

    #[test]
    fn on_android_checks_wait_in_the_context_for_the_glue() {
        use std::sync::mpsc;

        let ctx = egui::Context::default();

        assert!(matches!(android::availability(&ctx), Availability::Unavailable(_)), "until the glue says");

        android::set_level(&ctx, 2);

        assert_eq!(android::availability(&ctx), Availability::Ready(Kind::Biometrics));

        let (sent, got) = mpsc::channel();
        android::enroll(&ctx, "Main", [7; 32], Box::new(move |enrolled| sent.send(enrolled).unwrap()));
        let mut asked = android::asked(&ctx);

        assert!(android::asked(&ctx).is_empty(), "taken once");

        let Some(android::Asked::Check { alias, mode, data, done, .. }) = asked.pop() else {
            panic!("an enrollment to start");
        };

        assert_eq!((mode, data), (android::Mode::Enroll, vec![7; 32]));

        done(Ok((vec![1, 2], vec![3])));
        let enrolled = got.recv().unwrap().unwrap();

        assert_eq!(enrolled.0, format!("android:{alias}:03:0102"));

        let (sent, got) = mpsc::channel();
        android::open(&ctx, &enrolled, "Unlock", Box::new(move |secret| sent.send(secret).unwrap()));
        let Some(android::Asked::Check { mode, data, iv, done, .. }) = android::asked(&ctx).pop() else {
            panic!("an opening to start");
        };

        assert_eq!((mode, data, iv), (android::Mode::Open, vec![1, 2], vec![3]));

        done(Err(Failure::Invalidated));

        assert_eq!(got.recv().unwrap(), Err(Failure::Invalidated));

        android::forget(&ctx, &enrolled);

        assert!(matches!(android::asked(&ctx).pop(), Some(android::Asked::Forget { alias: forgotten }) if forgotten == alias));
    }
}
