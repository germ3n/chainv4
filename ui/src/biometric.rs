//! Biometrics, alike wherever an app runs: a secret kept so that only the person's fingerprint,
//! face or device unlock gives it back, and a check that they are there before something happens.
//!
//! - **Android:** an AES key in the Android Keystore wraps the secret. It opens only after a strong
//!   biometric check, BiometricPrompt, and a fingerprint or face enrolled after it was made
//!   invalidates it for good. It must be inside secure hardware, or none is made. The prompt is
//!   shown by a Java class the APK carries, `ui/android/Biometric.java`, which
//!   `ui/build-android.sh` compiles in with the file picker.
//! - **The browser:** a passkey, through WebAuthn. Its PRF extension gives a key that only the
//!   passkey's user verification releases, Touch ID, Windows Hello or a phone's unlock, and that
//!   key wraps the secret. A browser or passkey without PRF cannot keep one.
//! - **iOS:** a Keychain item of this device alone, whose access control ties it to the faces or
//!   fingers enrolled now, as `kSecAccessControlBiometryCurrentSet` does. The Secure Enclave enforces
//!   that when it is read, the system showing the prompt itself: nothing here decides.
//! - **macOS:** nothing new is kept, as an unsigned app cannot tie a Keychain item to Touch ID. What
//!   was kept before still opens: Touch ID, through LocalAuthentication, before it is read from the
//!   login Keychain, which is a check and no lock, and iOS reads what it kept that way too.
//! - **Elsewhere:** none, and [`availability`] and [`bound`] say so.
//!
//! [`bound`] says whether a secret can be kept here so that only hardware or a passkey gives it back,
//! and [`enroll`] keeps one only then, for wallets that depend on it.
//!
//! What a check comes to arrives later, through a callback, as the prompt is the system's own;
//! the app is repainted when it does. The secret, and every copy of it on the way, is wiped from
//! memory once done with: in Rust as it drops, in Java once it was read across.

use zeroize::Zeroizing;

/// A secret biometrics keep: 32 bytes, such as the key a password stretches to, wiped as it drops.
pub type Secret = Zeroizing<[u8; 32]>;

/// A secret read back as bytes, if it is 32 of them.
fn secret(bytes: &[u8]) -> Option<Secret> {
    let mut secret = Zeroizing::new([0; 32]);

    if bytes.len() != secret.len() {
        return None;
    }

    secret.copy_from_slice(bytes);

    Some(secret)
}

/// What biometrics are called here, for the app to name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Android's fingerprint or face.
    Biometrics,
    TouchId,
    FaceId,
    Passkey,
}

impl Kind {
    /// How the app names it: "fingerprint or face", "Touch ID", "Face ID" or "a passkey".
    pub fn name(self) -> &'static str {
        match self {
            Kind::Biometrics => "fingerprint or face",
            Kind::TouchId => "Touch ID",
            Kind::FaceId => "Face ID",
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
    /// The fingers or faces enrolled changed since, or the key is gone: the secret is lost for good.
    Invalidated,
    Failed(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Canceled => write!(formatter, "canceled"),
            Failure::Invalidated => write!(formatter, "the key biometrics kept is gone, as the fingers or faces enrolled changed"),
            Failure::Failed(why) => write!(formatter, "{why}"),
        }
    }
}

/// Why a Mac keeps no new secrets: said plainly to whoever looks for them there.
#[cfg(target_os = "macos")]
const NOT_BOUND_HERE: &str = "on macOS this app is unsigned, so it cannot tie a key to Touch ID in hardware: Touch ID here would be a check and not a lock";

/// Why a system without biometrics keeps none.
#[cfg(not(any(target_os = "android", target_arch = "wasm32", target_os = "macos", target_os = "ios")))]
const NOT_BOUND_HERE: &str = "this system has no secure hardware the app can tie a key to";

/// Whether a secret can be kept here so that only hardware or a passkey gives it back, and what the
/// check is called. Asked at run time: a phone with no strong biometrics, or a browser whose passkeys
/// have no PRF, says no. Only this may be depended on for a wallet to open at all.
pub fn bound(ctx: &egui::Context) -> Availability {
    #[cfg(target_os = "android")]
    return android::availability(ctx);

    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;

        web::bound()
    }

    #[cfg(target_os = "ios")]
    {
        let _ = ctx;

        mac::availability()
    }

    #[cfg(not(any(target_os = "android", target_arch = "wasm32", target_os = "ios")))]
    {
        let _ = ctx;

        Availability::Unavailable(NOT_BOUND_HERE.to_string())
    }
}

/// What the biometrics of a record are called, even where they cannot be asked for now.
pub fn kind_of(enrolled: &Enrolled) -> Kind {
    match enrolled.0.split(':').next() {
        Some("passkey") => Kind::Passkey,
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        Some("ios" | "macos") => match mac::availability() {
            Availability::Ready(kind) => kind,
            Availability::Unavailable(_) => Kind::Biometrics,
        },
        _ => Kind::Biometrics,
    }
}

/// Whether a record could have been made on this kind of device: one from any other never opens.
pub fn kept_here(enrolled: &Enrolled) -> bool {
    let tag = enrolled.0.split(':').next().unwrap_or_default();

    if cfg!(target_os = "android") {
        tag == "android"
    } else if cfg!(target_arch = "wasm32") {
        tag == "passkey"
    } else if cfg!(target_os = "ios") {
        tag == "ios" || tag == "macos"
    } else if cfg!(target_os = "macos") {
        tag == "macos"
    } else {
        false
    }
}

/// Whether biometrics can be asked for here, and what they are called: of use to what was kept
/// before [`bound`], as a Mac's Touch ID was.
pub fn availability(ctx: &egui::Context) -> Availability {
    #[cfg(target_os = "android")]
    return android::availability(ctx);

    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;

        web::availability()
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        let _ = ctx;

        mac::availability()
    }

    #[cfg(not(any(target_os = "android", target_arch = "wasm32", target_os = "macos", target_os = "ios")))]
    {
        let _ = ctx;

        Availability::Unavailable("this system has no biometrics the app can use".to_string())
    }
}

/// Keeps a secret so that only hardware or a passkey gives it back, after a check that the person is
/// there where one can be made first: what to store comes to done. Refused wherever [`bound`] says
/// no. name says what it is for, in the prompt and, for a passkey, as its name.
pub fn enroll(ctx: &egui::Context, name: &str, secret: Secret, done: impl FnOnce(Result<Enrolled, Failure>) + Send + 'static) {
    let done = waking(ctx, done);

    #[cfg(target_os = "android")]
    android::enroll(ctx, name, secret, Box::new(done));

    #[cfg(target_arch = "wasm32")]
    {
        let name = name.to_string();
        wasm_bindgen_futures::spawn_local(async move { done(web::enroll(&name, secret).await) });
    }

    #[cfg(target_os = "ios")]
    {
        let _ = name;
        ios::enroll(secret, done);
    }

    #[cfg(not(any(target_os = "android", target_arch = "wasm32", target_os = "ios")))]
    {
        let _ = (name, secret);
        done(Err(Failure::Failed(NOT_BOUND_HERE.to_string())));
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

    #[cfg(target_os = "ios")]
    match enrolled.0.starts_with("ios:") {
        true => ios::open(enrolled, reason, done),
        false => mac::open(enrolled, reason, done),
    }

    #[cfg(target_os = "macos")]
    mac::open(enrolled, reason, done);

    #[cfg(not(any(target_os = "android", target_arch = "wasm32", target_os = "macos", target_os = "ios")))]
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

    #[cfg(target_os = "ios")]
    {
        let _ = ctx;

        match enrolled.0.starts_with("ios:") {
            true => ios::forget(enrolled),
            false => mac::forget(enrolled),
        }
    }

    #[cfg(target_os = "macos")]
    {
        let _ = ctx;
        mac::forget(enrolled);
    }

    #[cfg(not(any(target_os = "android", target_os = "macos", target_os = "ios")))]
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
#[cfg_attr(not(any(target_os = "android", target_os = "ios", test)), allow(dead_code))]
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
        zeroize::Zeroizing,
    };

    /// What the Java side does: keep a secret under a new key, or open one with its key.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Mode {
        Enroll = 0,
        Open = 1,
    }

    /// What a check came to: the bytes out, wiped as they drop, as they may be the secret; and,
    /// for an enrollment, the IV they were sealed with.
    pub(crate) type Came = Result<(Zeroizing<Vec<u8>>, Vec<u8>), Failure>;

    /// A check asked for, for the glue to start, or a key to forget, at once.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) enum Asked {
        Check {
            alias: String,
            mode: Mode,
            /// The secret to keep, or what it was sealed to: wiped as it drops.
            data: Zeroizing<Vec<u8>>,
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
        ctx.data_mut(|data| data.get_temp_mut_or_default::<Asks>(egui::Id::unique("chainv4-ui biometric")).clone())
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
        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("chainv4-ui biometric level"), level));
    }

    pub(crate) fn availability(ctx: &egui::Context) -> Availability {
        match ctx.data(|data| data.get_temp::<i32>(egui::Id::unique("chainv4-ui biometric level"))).unwrap_or(-1) {
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
            data: Zeroizing::new(secret.to_vec()),
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
            data: Zeroizing::new(sealed),
            iv,
            title: reason.to_string(),
            subtitle: String::new(),
            done: Box::new(move |came: Came| {
                done(came.and_then(|(opened, _)| super::secret(&opened).ok_or_else(|| Failure::Failed("the biometric key gave back something else".to_string()))))
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
        zeroize::Zeroizing,
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

    thread_local! {
        /// What the browser says of PRF, once it has: None until then, or if it will not say.
        static PRF: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
        static ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Asks once whether this browser's passkeys have PRF. Older browsers have no way of saying, and
    /// are offered the choice: making the passkey then finds out, and leaves the wallet as it was.
    fn ask_about_prf() {
        if ASKED.with(|asked| asked.replace(true)) {
            return;
        }

        let Some(window) = web_sys::window() else {
            return;
        };
        let class = get(&window, "PublicKeyCredential");
        let Ok(ask) = get(&class, "getClientCapabilities").dyn_into::<Function>() else {
            return;
        };
        let Some(promise) = ask.call0(&class).ok().and_then(|promise| promise.dyn_into::<Promise>().ok()) else {
            return;
        };

        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(said) = JsFuture::from(promise).await {
                PRF.with(|prf| prf.set(get(&said, "extension:prf").as_bool()));
            }
        });
    }

    pub(super) fn bound() -> Availability {
        ask_about_prf();

        match (availability(), PRF.with(std::cell::Cell::get)) {
            (Availability::Ready(_), Some(false)) => Availability::Unavailable("this browser's passkeys cannot release a key (WebAuthn's PRF extension): use a recent Chrome, Edge or Safari, with the device's own passkeys".to_string()),
            (said, _) => said,
        }
    }

    pub(super) fn availability() -> Availability {
        // Firefox refuses WebAuthn to extension pages, as a SecurityError.
        let protocol = web_sys::window().and_then(|window| window.location().protocol().ok()).unwrap_or_default();

        if protocol == "moz-extension:" {
            return Availability::Unavailable("Firefox does not let extensions use passkeys".to_string());
        }

        match credentials() {
            Some(_) => Availability::Ready(Kind::Passkey),
            None => Availability::Unavailable("this browser has no passkeys, or the page is not served over HTTPS".to_string()),
        }
    }

    /// What a WebAuthn call came to as a failure: turned down, or something else.
    fn failure(err: JsValue) -> Failure {
        match get(&err, "name").as_string().as_deref() {
            Some("NotAllowedError" | "AbortError") => Failure::Canceled,
            Some("SecurityError") => Failure::Failed("this browser does not allow passkeys here: the page must be served over HTTPS, and some browsers refuse them to extensions".to_string()),
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
    fn prf_output(credential: &JsValue) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
        let results: Function = get(credential, "getClientExtensionResults").dyn_into().map_err(failure)?;
        let results = results.call0(credential).map_err(failure)?;
        let prf = get(&results, "prf");
        let first = get(&get(&prf, "results"), "first");

        if !first.is_undefined() {
            let output = Uint8Array::new(&first);
            let read = Zeroizing::new(output.to_vec());
            // The browser's own copy, overwritten once read.
            output.fill(0, 0, output.length());

            return Ok(Some(read));
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
    async fn assert(id: &[u8], salt: &[u8]) -> Result<Zeroizing<Vec<u8>>, Failure> {
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
        let key = key.get(..32).ok_or_else(|| Failure::Failed("the passkey released too short a key".to_string()))?;

        Ok(ChaCha20Poly1305::new(Key::from_slice(key)))
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
        let opened = Zeroizing::new(
            cipher(&output)?
                .decrypt(Nonce::from_slice(&nonce), wrapped.as_slice())
                .map_err(|_| Failure::Failed("that passkey is not the one this was kept with".to_string()))?,
        );

        super::secret(&opened).ok_or_else(|| Failure::Failed("the passkey's record is damaged".to_string()))
    }
}

/// What a Keychain answer means for a check of what iOS keeps: a prompt turned down is no failure,
/// and an item not there is gone for good, as one tied to the faces or fingers enrolled reads as not
/// found once they change. Anything else may pass, so it is said and tried again.
#[cfg(any(target_os = "ios", test))]
fn keychain_failure(code: i32, said: String) -> Failure {
    const USER_CANCELED: i32 = -128;
    const ITEM_NOT_FOUND: i32 = -25300;

    match code {
        USER_CANCELED => Failure::Canceled,
        ITEM_NOT_FOUND => Failure::Invalidated,
        _ => Failure::Failed(said),
    }
}

/// A secret in the iOS Keychain, tied to the faces or fingers enrolled now, on this device alone. The
/// Secure Enclave enforces the tie as the item is read, showing the prompt itself, so nothing is
/// checked first here as on macOS. Written and read off the main thread, as reading waits for the
/// person. Each secret is its own item, under a name no other has.
#[cfg(target_os = "ios")]
mod ios {
    use {
        super::{Enrolled, Failure, Secret},
        core_foundation::base::TCFType,
        core_foundation::string::{CFString, CFStringRef},
        security_framework::access_control::{ProtectionMode, SecAccessControl},
        security_framework::passwords::{self, AccessControlOptions, PasswordOptions},
        zeroize::Zeroizing,
    };

    /// The Keychain service they are kept under, apart from what iOS kept the older way.
    const SERVICE: &str = "org.chainv4.ui bound";

    unsafe extern "C" {
        /// What the system's prompt says. security-framework does not name it.
        static kSecUseOperationPrompt: CFStringRef;
    }

    fn options(account: &str) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(SERVICE, account);
        options.use_protected_keychain();

        options
    }

    pub(super) fn enroll(secret: Secret, done: impl FnOnce(Result<Enrolled, Failure>) + Send + 'static) {
        std::thread::spawn(move || {
            done((|| {
                let account = super::random_name()?;
                let control = SecAccessControl::create_with_protection(Some(ProtectionMode::AccessibleWhenPasscodeSetThisDeviceOnly), AccessControlOptions::BIOMETRY_CURRENT_SET.bits())
                    .map_err(|err| Failure::Failed(format!("the Keychain cannot tie a secret to biometrics: {err}")))?;
                let mut options = options(&account);
                options.set_access_control(control);
                passwords::set_generic_password_options(&*secret, options).map_err(|err| Failure::Failed(format!("the Keychain refused the secret: {err}")))?;

                Ok(Enrolled(format!("ios:{account}")))
            })())
        });
    }

    pub(super) fn open(enrolled: &Enrolled, reason: &str, done: impl FnOnce(Result<Secret, Failure>) + Send + 'static) {
        let account = match super::parts(enrolled, "ios", 1) {
            Ok(parts) => parts[0].to_string(),
            Err(failure) => return done(Err(failure)),
        };
        let reason = reason.to_string();

        std::thread::spawn(move || {
            let mut options = options(&account);

            // SAFETY: a constant the Security framework defines for as long as the app runs. The
            // field is deprecated only for how it exposes its types, and has no setter for this.
            #[allow(deprecated)]
            options.query.push((unsafe { CFString::wrap_under_get_rule(kSecUseOperationPrompt) }, CFString::new(&reason).into_CFType()));

            done(match passwords::generic_password(options) {
                Ok(bytes) => {
                    let bytes = Zeroizing::new(bytes);

                    super::secret(&bytes).ok_or_else(|| Failure::Failed("the Keychain item is damaged".to_string()))
                }
                Err(err) => Err(super::keychain_failure(err.code(), err.to_string())),
            });
        });
    }

    pub(super) fn forget(enrolled: &Enrolled) {
        if let Ok(parts) = super::parts(enrolled, "ios", 1) {
            let _ = passwords::delete_generic_password_options(options(parts[0]));
        }
    }
}

/// Touch ID or Face ID, through LocalAuthentication, and what was kept in the Keychain before
/// `ios` and `bound`: the login Keychain on macOS, the app's own on iOS. It keeps nothing new.
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod mac {
    use {
        super::{Availability, Enrolled, Failure, Kind, Secret},
        block2::RcBlock,
        objc2::runtime::Bool,
        objc2_foundation::{NSError, NSString},
        objc2_local_authentication::{LABiometryType, LAContext, LAError, LAPolicy},
        std::sync::Mutex,
        zeroize::Zeroizing,
    };

    /// The Keychain service the secrets are kept under, each its own account.
    const SERVICE: &str = "org.chainv4.ui biometric";
    const POLICY: LAPolicy = LAPolicy::DeviceOwnerAuthenticationWithBiometrics;

    /// What the device's biometry is called: Face ID or Touch ID.
    fn named() -> &'static str {
        match availability() {
            Availability::Ready(kind) => kind.name(),
            Availability::Unavailable(_) => "biometrics",
        }
    }

    pub(super) fn availability() -> Availability {
        // SAFETY: a fresh context, asked about a policy; nothing else holds it.
        let context = unsafe { LAContext::new() };

        match unsafe { context.canEvaluatePolicy_error(POLICY) } {
            // Which the device has is known once the policy was asked about.
            Ok(()) => Availability::Ready(match unsafe { context.biometryType() } {
                LABiometryType::FaceID => Kind::FaceId,
                _ => Kind::TouchId,
            }),
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
                // SAFETY: the context was evaluated, which is when it has a domain state. Its
                // successor, domainState, is macOS 15's alone; this one reads on every macOS.
                #[allow(deprecated)]
                let state = unsafe { held.evaluatedPolicyDomainState() }.map(|data| data.to_vec()).unwrap_or_default();

                return then(Ok(state));
            }

            // SAFETY: LocalAuthentication hands a valid error, or none, to a reply that failed.
            let failure = match unsafe { error.as_ref() } {
                Some(error) if [LAError::UserCancel, LAError::UserFallback, LAError::SystemCancel, LAError::AppCancel].contains(&LAError(error.code())) => Failure::Canceled,
                Some(error) if LAError(error.code()) == LAError::BiometryNotEnrolled => Failure::Invalidated,
                Some(error) => Failure::Failed(error.localizedDescription().to_string()),
                None => Failure::Failed(format!("{} failed", named())),
            };
            then(Err(failure));
        });

        // SAFETY: the reason is a valid string, and the reply block lives as long as the call needs.
        unsafe { context.evaluatePolicy_localizedReason_reply(POLICY, &NSString::from_str(reason), &reply) };
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

                let kept = Zeroizing::new(security_framework::passwords::get_generic_password(SERVICE, &account).map_err(|_| Failure::Invalidated)?);
                let read = std::str::from_utf8(&kept).ok().and_then(super::unhex).map(Zeroizing::new);

                read.and_then(|read| super::secret(&read)).ok_or_else(|| Failure::Failed("the Keychain item is damaged".to_string()))
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
    fn what_ios_answers_is_told_apart_as_gone_turned_down_or_worth_another_try() {
        assert_eq!(keychain_failure(-128, "canceled".to_string()), Failure::Canceled);
        assert_eq!(keychain_failure(-25300, "not found".to_string()), Failure::Invalidated);

        for code in [-25293, -25308, -34018, -4] {
            assert_eq!(keychain_failure(code, "said".to_string()), Failure::Failed("said".to_string()), "{code} may pass");
        }
    }

    /// Only platforms that bind a key to hardware keep new secrets, and this test's platform says
    /// why not in words, where it is one that does not.
    #[cfg(not(any(target_os = "android", target_arch = "wasm32", target_os = "ios")))]
    #[test]
    fn a_platform_that_cannot_bind_a_key_offers_nothing_and_says_why() {
        use std::sync::mpsc;

        let ctx = egui::Context::default();
        let Availability::Unavailable(why) = bound(&ctx) else {
            panic!("no new modes here");
        };

        assert!(!why.is_empty());
        #[cfg(target_os = "macos")]
        assert!(why.contains("unsigned") && why.contains("check and not a lock"), "{why}");

        let (sent, got) = mpsc::channel();
        enroll(&ctx, "Mine", Zeroizing::new([7; 32]), move |came| sent.send(came).unwrap());

        assert!(matches!(got.recv().unwrap(), Err(Failure::Failed(said)) if said == why), "and enrolling is refused the same way");
    }

    #[test]
    fn a_record_says_where_it_was_made_and_what_it_is_called() {
        let passkey = Enrolled("passkey:00:01:02:03".to_string());
        let android = Enrolled("android:a:00:01".to_string());
        let ios = Enrolled("ios:00ff".to_string());

        assert_eq!(kind_of(&passkey), Kind::Passkey);
        assert_eq!(kind_of(&android), Kind::Biometrics);
        assert!(parts(&ios, "ios", 1).is_ok());
        assert!(parts(&ios, "macos", 2).is_err(), "and iOS's own is not read as the older kind");

        // On this machine.
        #[cfg(target_os = "macos")]
        {
            assert!(!kept_here(&passkey) && !kept_here(&android) && !kept_here(&ios));
            assert!(kept_here(&Enrolled("macos:00:01".to_string())));
        }
    }

    #[test]
    fn a_record_tagged_for_a_wallet_that_needs_it_reads_as_from_elsewhere_to_any_older_build() {
        // What an older build runs: it says the record is from elsewhere, and writes nothing.
        for tag in ["biometric-v1", "both-v1"] {
            for (platform, record, fields) in [("android", "android:a:00:01", 3), ("passkey", "passkey:00:01:02:03", 4), ("macos", "macos:00:01", 2)] {
                assert!(parts(&Enrolled(format!("{tag}:{record}")), platform, fields).is_err());
            }
        }
    }

    /// LocalAuthentication answers whether Touch ID can be used, without a prompt: ready on a Mac
    /// with a finger enrolled, and why not on one without, or on CI.
    #[cfg(target_os = "macos")]
    #[test]
    fn touch_id_says_whether_it_can_without_asking() {
        match mac::availability() {
            Availability::Ready(kind) => assert_eq!(kind, Kind::TouchId),
            Availability::Unavailable(why) => assert!(!why.is_empty()),
        }
    }

    #[test]
    fn a_secret_reads_back_from_exactly_32_bytes() {
        assert_eq!(secret(&[5; 32]).map(|secret| *secret), Some([5; 32]));
        assert!(secret(&[5; 31]).is_none() && secret(&[5; 33]).is_none());
    }

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
        android::enroll(&ctx, "Main", Zeroizing::new([7; 32]), Box::new(move |enrolled| sent.send(enrolled).unwrap()));
        let mut asked = android::asked(&ctx);

        assert!(android::asked(&ctx).is_empty(), "taken once");

        let Some(android::Asked::Check { alias, mode, data, done, .. }) = asked.pop() else {
            panic!("an enrollment to start");
        };

        assert_eq!((mode, data.to_vec()), (android::Mode::Enroll, vec![7; 32]));

        done(Ok((Zeroizing::new(vec![1, 2]), vec![3])));
        let enrolled = got.recv().unwrap().unwrap();

        assert_eq!(enrolled.0, format!("android:{alias}:03:0102"));

        let (sent, got) = mpsc::channel();
        android::open(&ctx, &enrolled, "Unlock", Box::new(move |secret| sent.send(secret).unwrap()));
        let Some(android::Asked::Check { mode, data, iv, done, .. }) = android::asked(&ctx).pop() else {
            panic!("an opening to start");
        };

        assert_eq!((mode, data.to_vec(), iv), (android::Mode::Open, vec![1, 2], vec![3]));

        done(Ok((Zeroizing::new(vec![9; 32]), Vec::new())));

        assert_eq!(got.recv().unwrap().map(|secret| *secret), Ok([9; 32]));

        android::forget(&ctx, &enrolled);

        assert!(matches!(android::asked(&ctx).pop(), Some(android::Asked::Forget { alias: forgotten }) if forgotten == alias));
    }
}
