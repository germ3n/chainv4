//! Keys at rest. What app storage holds is sealed with a password: Argon2id stretches the password
//! with a salt of its own into a key, and ChaCha20-Poly1305 seals the accounts with it under a
//! fresh nonce each time they are saved. Without the password, the storage says nothing but how
//! long the sealed accounts are. The key the password stretches to, the accounts opened, and the
//! words are wiped from memory as they drop.
//!
//! A vault holds a book: the wallet's seed phrases, when it has any, and its accounts, each one
//! phrase's account of some number or a key brought in.
//!
//! A wallet with one phrase is written as it always was, so older builds keep reading it. Once it
//! has a second, `phrase` is written as a list, which older builds cannot read as the one phrase
//! they expect: they say the vault is damaged and write nothing, rather than open half of it and
//! save the rest away. An account of any phrase but the first names it by its fingerprint, and a
//! book with an account whose phrase is not there neither opens nor seals.

use {
    argon2::Argon2,
    chacha20poly1305::aead::{Aead, KeyInit},
    chacha20poly1305::{ChaCha20Poly1305, Key, Nonce},
    chainv4::crypto::keypair::{KeyType, Keypair, PublicKey},
    chainv4::crypto::mnemonic::{Phrase, Seed},
    chainv4::structs::hex::{from_hex, to_hex},
    serde::{Deserialize, Serialize},
    std::sync::OnceLock,
    zeroize::{Zeroize, Zeroizing},
};

/// Accounts as app storage keeps them: sealed, with what opening them takes besides the password.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    /// The salt the password is stretched with, hex.
    pub salt: String,
    /// The nonce the accounts were sealed under, hex.
    pub nonce: String,
    /// The sealed accounts, hex.
    pub sealed: String,
}

/// What names a seed phrase across runs: the fingerprint of its master key, BIP32's. Never its
/// place in a list, which shifts as phrases go.
pub type Fingerprint = [u8; 4];

/// What a wallet's vault holds: its seed phrases, when it has any, the first the one an account
/// comes from when it names none, and its accounts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Book {
    /// Written under the name it always had, in the form `phrase_form` says.
    #[serde(rename = "phrase", with = "phrase_form")]
    pub phrases: Vec<Words>,
    pub accounts: Vec<StoredAccount>,
    /// A watch-only wallet's: its accounts are an offline signer's, each watched, and it holds no
    /// key and no words.
    pub watch: bool,
    /// Its atomic swaps, with the secrets of those it started: sealed with the rest, as whoever
    /// learns a secret early can take the other side's coins.
    pub swaps: Vec<SwapRecord>,
    /// What this wallet signed on an offline signer, oldest first. Written only when there is any,
    /// so a wallet without any stays as it was; an older build ignores it, and loses only this.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub signed: Vec<SignedLine>,
    /// Whether a signer keeps no such history.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub signed_off: bool,
}

/// One thing a signer signed, as it read then: for looking back, never to act on. It holds no key
/// or signature, and what a request said is cut short, as a request may say anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedLine {
    /// When, in milliseconds since 1970, by this device's clock.
    pub at: u64,
    /// What kind of thing it was.
    pub kind: String,
    /// What it did.
    pub what: String,
    /// Who it was signed as.
    pub signer: String,
}

/// How many lines a signer keeps: past these, the oldest go.
pub const SIGNED_KEPT: usize = 200;
/// How long a line's text may be.
const SIGNED_TEXT: usize = 200;

impl Book {
    /// Notes something just signed, unless history is off. Whether it noted it.
    pub fn note_signed(&mut self, at: u64, kind: &str, what: &str, signer: &str) -> bool {
        if self.signed_off {
            return false;
        }

        // Nor anything that breaks a line, or turns or hides text so a line reads as what it is not.
        let hides = |one: char| matches!(one, '\u{ad}' | '\u{61c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}');
        let cut = |text: &str| text.chars().filter(|one| !one.is_control() && !hides(*one)).take(SIGNED_TEXT).collect::<String>();
        self.signed.push(SignedLine { at, kind: cut(kind), what: cut(what), signer: cut(signer) });

        if self.signed.len() > SIGNED_KEPT {
            self.signed.drain(..self.signed.len() - SIGNED_KEPT);
        }

        true
    }

    /// The phrase an account's reference names: the first when it names none, and otherwise the
    /// one with that fingerprint, or none at all: never another in its place.
    pub fn phrase_of(&self, from: Option<Fingerprint>) -> Option<&Words> {
        match from {
            None => self.phrases.first(),
            Some(fingerprint) => self.holding(fingerprint),
        }
    }

    /// The phrase with this fingerprint.
    pub fn holding(&self, fingerprint: Fingerprint) -> Option<&Words> {
        self.phrases.iter().find(|words| words.fingerprint() == Some(fingerprint))
    }

    /// The phrases that read, each by the reference an account of it takes, and what it is called.
    pub fn references(&self) -> Vec<(Option<Fingerprint>, String)> {
        self.phrases
            .iter()
            .enumerate()
            .filter_map(|(index, words)| {
                let fingerprint = words.fingerprint()?;

                Some(((index > 0).then_some(fingerprint), self.name_of(index)))
            })
            .collect()
    }

    /// What the phrase at this place is called: the name it was given, or a plain one.
    pub fn name_of(&self, index: usize) -> String {
        match self.phrases.get(index).map(Words::name) {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => format!("Words {}", index + 1),
        }
    }

    /// What the phrase this reference names is called.
    pub fn name_from(&self, from: Option<Fingerprint>) -> String {
        let index = match from {
            None => 0,
            Some(fingerprint) => self.phrases.iter().position(|words| words.fingerprint() == Some(fingerprint)).unwrap_or(0),
        };

        self.name_of(index)
    }

    /// Why these words cannot join this book, if they cannot: a watch-only wallet holds none, the
    /// same phrase goes in once, and what names phrases must read for every phrase there.
    pub fn refuses(&self, words: &Words) -> Result<(), String> {
        if self.watch {
            return Err("A watch-only wallet holds no words: they stay on the signer.".to_string());
        }

        let fingerprint = words.fingerprint().ok_or("These words do not read.")?;

        if self.phrases.iter().any(|held| held.fingerprint().is_none()) {
            return Err("This wallet's words do not read, so no others can join them.".to_string());
        }

        match self.holding(fingerprint) {
            Some(_) => Err("These words are in this wallet already.".to_string()),
            None => Ok(()),
        }
    }

    /// Adds a phrase, refused as `refuses` says, with a plain name if it has none and is not the
    /// first. The reference its accounts take: none for the first, its fingerprint for any other.
    pub fn add_phrase(&mut self, mut words: Words) -> Result<Option<Fingerprint>, String> {
        self.refuses(&words)?;

        if words.name.is_empty() && !self.phrases.is_empty() {
            let names: Vec<String> = (0..self.phrases.len()).map(|index| self.name_of(index)).collect();
            let plain = (self.phrases.len() + 1..).map(|number| format!("Words {number}")).find(|name| !names.contains(name));
            words.name = plain.unwrap_or_default();
        }

        let from = if self.phrases.is_empty() { None } else { words.fingerprint() };
        self.phrases.push(words);

        Ok(from)
    }

    /// Why this book must not open or seal, if it must not. An account whose phrase is not there
    /// is never left out, or the next save would delete it for good; and one naming a phrase that
    /// a one-phrase book lacks would read, in an older build, as the only phrase's account of that
    /// number: other keys than its own. A list of phrases must all read and none be there twice,
    /// or nothing could tell which one a fingerprint names. One phrase that does not read, with
    /// no account from it, opens as it always did. An account this build cannot read at all, a key
    /// damaged or in a scheme it was built without, is refused the same way: `Wallet::store` writes
    /// back only the accounts that opened. What this says never holds the words or a key.
    pub fn check(&self) -> Result<(), String> {
        if self.phrases.len() > 1 {
            let mut seen = Vec::with_capacity(self.phrases.len());

            for words in &self.phrases {
                let fingerprint = words.fingerprint().ok_or("the vault is damaged: words in it do not read")?;

                if seen.contains(&fingerprint) {
                    return Err("the vault is damaged: the same words are in it twice".to_string());
                }

                seen.push(fingerprint);
            }
        }

        let first = self.phrases.first().and_then(Words::fingerprint);

        for account in &self.accounts {
            let readable = match &account.source {
                Source::Phrase { from, extra, .. } => {
                    // Only an account of another phrase names one: the first, named, would count
                    // apart from the accounts that name none, and take their numbers.
                    if from.is_some() && *from == first {
                        return Err(format!("the vault is damaged: the account {} names the first words, as none should", account.name));
                    }

                    if self.phrase_of(*from).and_then(Words::seed).is_none() {
                        return Err(format!("the vault is damaged: the account {} comes from words it does not hold", account.name));
                    }

                    extra.iter().all(|extra| extra.scheme.parse::<KeyType>().is_ok())
                }
                Source::Key { key } => Keypair::from_key_file(key).is_ok(),
                Source::Watch { keys, .. } => keys.iter().all(|key| key.parse::<PublicKey>().is_ok()),
                Source::MetaMask { key } => key.parse::<PublicKey>().is_ok(),
            };

            if !readable {
                return Err(unreadable(&account.name));
            }
        }

        Ok(())
    }
}

/// Why a wallet stays shut for an account this build cannot read. Parsing's own errors are left out
/// of it, as they may quote what they were given.
pub fn unreadable(name: &str) -> String {
    format!("This build cannot read the account {name}: its key is damaged, or in a scheme this build does not have. The wallet stays closed and nothing is written: open it with a build that reads it.")
}

/// How `phrase` is written. None is null, and one is an object of its words and passphrase alone,
/// byte for byte as before there could be more. Several are a list of objects, which an older
/// build refuses: it reads a list as one Words by position, whose first must be a string, and the
/// list's first is an object, so it says the vault is damaged and writes nothing. Reading never
/// goes through a value in between, which would leave an unwiped copy of the words behind.
mod phrase_form {
    use {
        super::Words,
        serde::de::{Deserialize, Deserializer, Error, MapAccess, SeqAccess, Visitor, value::MapAccessDeserializer},
        serde::ser::{Serialize, Serializer},
    };

    /// One phrase, as it was always written: without the name older builds never had.
    #[derive(serde::Serialize)]
    struct Alone<'a> {
        words: &'a str,
        passphrase: &'a str,
    }

    pub fn serialize<S: Serializer>(phrases: &[Words], serializer: S) -> Result<S::Ok, S::Error> {
        match phrases {
            [] => serializer.serialize_none(),
            [words] => Alone { words: words.words(), passphrase: words.passphrase() }.serialize(serializer),
            several => serializer.collect_seq(several),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Words>, D::Error> {
        struct Form;

        impl<'de> Visitor<'de> for Form {
            type Value = Vec<Words>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("null, a phrase, or a list of at least two")
            }

            fn visit_none<E: Error>(self) -> Result<Vec<Words>, E> {
                Ok(Vec::new())
            }

            fn visit_unit<E: Error>(self) -> Result<Vec<Words>, E> {
                Ok(Vec::new())
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Vec<Words>, A::Error> {
                Ok(vec![Words::deserialize(MapAccessDeserializer::new(map))?])
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut list: A) -> Result<Vec<Words>, A::Error> {
                let mut phrases = Vec::new();

                while let Some(words) = list.next_element()? {
                    phrases.push(words);
                }

                // Nothing writes a shorter list, so one is not this wallet's.
                match phrases.len() {
                    0 | 1 => Err(A::Error::custom("a list of phrases holds at least two")),
                    _ => Ok(phrases),
                }
            }
        }

        deserializer.deserialize_any(Form)
    }
}

/// An atomic swap as the wallet keeps it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapRecord {
    /// Its terms, as their code: the offer until answered, then the agreed terms.
    pub terms: String,
    /// The secret, 0x and 64 hex digits, when this wallet started the swap.
    pub secret: Option<String>,
    /// Which account's keys it uses, by its first address.
    pub account: String,
    /// When it was made, in milliseconds since 1970.
    pub made: u64,
    /// Whether this wallet started it, and so made its secret and locks first.
    #[serde(default)]
    pub started: bool,
    /// The HTLC's coins on Bitcoin, once known: txid:vout:sats.
    #[serde(default)]
    pub funding: Option<String>,
    /// The native multisig it swaps as, by address, when not the account itself: its chainv4
    /// steps go to the Multisig page, and its Bitcoin spends are signed by its members.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multisig: Option<String>,
}

/// Never its secret, whoever learns it early can take the other side's coins, so it can end up in
/// no log or failed test.
impl std::fmt::Debug for SwapRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwapRecord")
            .field("terms", &self.terms)
            .field("secret", &self.secret.as_ref().map(|_| "hidden"))
            .field("account", &self.account)
            .field("made", &self.made)
            .field("started", &self.started)
            .field("funding", &self.funding)
            .field("multisig", &self.multisig)
            .finish()
    }
}

impl Drop for SwapRecord {
    fn drop(&mut self) {
        if let Some(secret) = &mut self.secret {
            secret.zeroize();
        }
    }
}

/// A seed phrase, the BIP39 passphrase it is used with, empty for none, and what the wallet calls
/// it. Wiped as it drops, along with the seed it stretched to, kept so that asking for it every
/// frame stretches it once. Its fields stay private, so that seed is always these words'.
#[derive(Serialize, Deserialize)]
pub struct Words {
    words: String,
    passphrase: String,
    /// Written only beside other phrases: one alone is written as it was before names.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    name: String,
    #[serde(skip)]
    derived: OnceLock<Option<Derived>>,
}

/// What a phrase's words stretch to, and what names it. The seed wipes itself as it drops.
struct Derived {
    seed: Seed,
    fingerprint: Fingerprint,
}

impl Words {
    /// A phrase's words, as Phrase writes them, and its passphrase, empty for none.
    pub fn new(words: String, passphrase: String) -> Words {
        Words {
            words,
            passphrase,
            name: String::new(),
            derived: OnceLock::new(),
        }
    }

    pub fn words(&self) -> &str {
        &self.words
    }

    pub fn passphrase(&self) -> &str {
        &self.passphrase
    }

    /// What it was named, if it was.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = name.trim().to_string();
    }

    /// Its seed, when its words read, stretched the first time it is asked for.
    pub fn seed(&self) -> Option<&Seed> {
        self.derived().map(|derived| &derived.seed)
    }

    /// The fingerprint of its master key, when its words read.
    pub fn fingerprint(&self) -> Option<Fingerprint> {
        self.derived().map(|derived| derived.fingerprint)
    }

    fn derived(&self) -> Option<&Derived> {
        self.derived
            .get_or_init(|| {
                let seed = Phrase::parse(&self.words).ok()?.seed(&self.passphrase);
                let fingerprint = seed.ethereum_account().master_fingerprint;

                Some(Derived { seed, fingerprint })
            })
            .as_ref()
    }
}

/// A copy stretches its own seed if it ever needs one, so none is copied here.
impl Clone for Words {
    fn clone(&self) -> Words {
        Words {
            words: self.words.clone(),
            passphrase: self.passphrase.clone(),
            name: self.name.clone(),
            derived: OnceLock::new(),
        }
    }
}

impl PartialEq for Words {
    fn eq(&self, other: &Words) -> bool {
        self.words == other.words && self.passphrase == other.passphrase && self.name == other.name
    }
}

impl Eq for Words {}

/// Never the words or the passphrase, so neither can end up in a log or a failed test.
impl std::fmt::Debug for Words {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Words({} words, named {:?})", self.words.split_whitespace().count(), self.name)
    }
}

impl Drop for Words {
    fn drop(&mut self) {
        self.words.zeroize();
        self.passphrase.zeroize();
    }
}

/// A further key of a words account's: its scheme, by name, and its index, which Seed::key_at
/// derives it at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extra {
    pub scheme: String,
    pub index: u32,
}

/// One account, as the vault holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAccount {
    pub name: String,
    pub source: Source,
}

/// Where an account's keys come from.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A phrase's account of this number: an address in every scheme, and the further keys it was
    /// given, each by its scheme and index. Of the first phrase when it names none, and otherwise
    /// of the one with this fingerprint.
    Phrase {
        account: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extra: Vec<Extra>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<Fingerprint>,
    },
    /// One key brought in, as a key file holds it.
    Key { key: String },
    /// An account an offline signer holds the keys of, watched: its public keys, as the signer
    /// paired it, and nothing to sign with; and where the signer's words derive it, when it said.
    Watch {
        keys: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<crate::airgap::Origin>,
    },
    /// An Ethereum account MetaMask holds, or a hardware wallet through it: its public key, and
    /// nothing to sign with here. MetaMask signs what it sends, in the browser.
    #[serde(rename = "metamask")]
    MetaMask { key: String },
}

/// Never a key brought in: it spends all the account holds, so it can end up in no log or failed
/// test. What is public shows.
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Phrase { account, extra, from } => f.debug_struct("Phrase").field("account", account).field("extra", extra).field("from", from).finish(),
            Source::Key { .. } => f.debug_struct("Key").field("key", &"hidden").finish(),
            Source::Watch { keys, origin } => f.debug_struct("Watch").field("keys", keys).field("origin", origin).finish(),
            Source::MetaMask { key } => f.debug_struct("MetaMask").field("key", key).finish(),
        }
    }
}

/// One account, as vaults from before seed phrases held it. No Debug, as it holds a key.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stored {
    name: String,
    key: String,
}

/// An open vault: the key the password stretched to, kept while the wallet is unlocked, so it
/// seals again after a change without asking for the password. Wiped as it drops.
pub struct Vault {
    salt: [u8; 16],
    key: Zeroizing<[u8; 32]>,
    /// Whether it was sealed with no password at all, as known when it was made or opened by one.
    bare: bool,
}

impl Vault {
    /// A new vault, sealed with this password.
    pub fn create(password: &str) -> Result<Vault, String> {
        let mut salt = [0; 16];
        getrandom::fill(&mut salt).map_err(|err| format!("no randomness for a salt: {err}"))?;

        Ok(Vault {
            key: stretch(password, &salt)?,
            salt,
            bare: password.is_empty(),
        })
    }

    /// Whether it is sealed with no password: anyone who reads app storage opens it.
    pub fn bare(&self) -> bool {
        self.bare
    }

    /// Opens sealed accounts with a password.
    pub fn open(sealed: &Sealed, password: &str) -> Result<(Vault, Book), String> {
        let salt: [u8; 16] = bytes(&sealed.salt)?;

        let (mut vault, book) = Vault::open_with_key(sealed, &*stretch(password, &salt)?)?;
        vault.bare = password.is_empty();

        Ok((vault, book))
    }

    /// The key the password stretched to: what biometrics keep, to open the vault without it.
    pub fn key(&self) -> Zeroizing<[u8; 32]> {
        self.key.clone()
    }

    /// Opens sealed accounts with the key a password stretched to, as biometrics keep it.
    pub fn open_with_key(sealed: &Sealed, key: &[u8; 32]) -> Result<(Vault, Book), String> {
        let salt: [u8; 16] = bytes(&sealed.salt)?;
        let nonce: [u8; 12] = bytes(&sealed.nonce)?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
        let plain = Zeroizing::new(
            cipher
                .decrypt(Nonce::from_slice(&nonce), from_hex(&sealed.sealed).ok_or("the vault is damaged")?.as_slice())
                .map_err(|_| "wrong password".to_string())?,
        );
        // A vault from before seed phrases holds a list of keys: its accounts, all brought in.
        let book = serde_json::from_slice::<Book>(&plain)
            .or_else(|_| {
                serde_json::from_slice::<Vec<Stored>>(&plain).map(|keys| Book {
                    accounts: keys
                        .into_iter()
                        .map(|stored| StoredAccount {
                            name: stored.name,
                            source: Source::Key { key: stored.key },
                        })
                        .collect(),
                    ..Book::default()
                })
            })
            .map_err(|_| "the vault is damaged".to_string())?;
        book.check()?;

        Ok((
            Vault {
                salt,
                key: Zeroizing::new(*key),
                bare: false,
            },
            book,
        ))
    }

    /// Seals accounts again, under a fresh nonce. A book that would not open is not written either,
    /// so what storage holds stays as it was.
    pub fn seal(&self, book: &Book) -> Result<Sealed, String> {
        book.check()?;
        let mut nonce = [0; 12];
        getrandom::fill(&mut nonce).map_err(|err| format!("no randomness for a nonce: {err}"))?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&*self.key));
        let plain = plain(book)?;
        let sealed = cipher.encrypt(Nonce::from_slice(&nonce), plain.as_slice()).map_err(|_| "sealing failed".to_string())?;

        Ok(Sealed {
            salt: to_hex(&self.salt),
            nonce: to_hex(&nonce),
            sealed: to_hex(&sealed),
        })
    }
}

/// A book as the JSON sealed: written into room made to its size first, so it never grows and
/// leaves an unwiped copy of the words behind, and wiped as it drops.
fn plain(book: &Book) -> Result<Zeroizing<Vec<u8>>, String> {
    /// Counts what would be written, keeping none of it.
    struct Count(usize);

    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();

            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut count = Count(0);
    serde_json::to_writer(&mut count, book).map_err(|err| err.to_string())?;
    let mut plain = Zeroizing::new(Vec::with_capacity(count.0));
    serde_json::to_writer(&mut *plain, book).map_err(|err| err.to_string())?;

    debug_assert_eq!(plain.capacity(), count.0, "never grown");

    Ok(plain)
}

fn stretch(password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let mut key = Zeroizing::new([0; 32]);
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut *key)
        .map_err(|err| format!("could not stretch the password: {err}"))?;

    Ok(key)
}

fn bytes<const N: usize>(text: &str) -> Result<[u8; N], String> {
    from_hex(text).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| "the vault is damaged".to_string())
}

#[cfg(test)]
mod tests {
    use {super::*, chainv4::crypto::keypair::Keypair};

    #[test]
    fn a_signer_notes_what_it_signed_oldest_going_past_what_it_keeps_and_nothing_when_off() {
        let mut book = Book::default();
        assert!(book.note_signed(1, "Send", "1 CV4 to bob", "ed25519:0x01"));
        assert_eq!(book.signed[0], SignedLine { at: 1, kind: "Send".to_string(), what: "1 CV4 to bob".to_string(), signer: "ed25519:0x01".to_string() });

        for at in 2..=(SIGNED_KEPT as u64 + 5) {
            book.note_signed(at, "Send", "again", "ed25519:0x01");
        }

        assert_eq!(book.signed.len(), SIGNED_KEPT);
        assert_eq!(book.signed[0].at, 6, "the oldest went");
        assert_eq!(book.signed.last().unwrap().at, SIGNED_KEPT as u64 + 5);

        // Off, nothing more is noted, and what is there stays until cleared.
        book.signed_off = true;
        assert!(!book.note_signed(999, "Send", "not this", "ed25519:0x01"));
        assert_eq!(book.signed.len(), SIGNED_KEPT);
        assert!(book.signed.iter().all(|line| line.what != "not this"));
    }

    #[test]
    fn what_a_request_says_is_cut_short_and_holds_no_line_breaks_to_fake_a_row() {
        let mut book = Book::default();
        book.note_signed(1, "Send\nReally", &"x".repeat(5_000), "a\u{202e}b");

        assert_eq!(book.signed[0].kind, "SendReally");
        assert_eq!(book.signed[0].what.chars().count(), SIGNED_TEXT);
        assert_eq!(book.signed[0].signer, "ab", "nor what turns text around");
    }

    #[test]
    fn a_history_seals_opens_and_clears_and_a_wallet_with_none_is_written_as_before() {
        let vault = Vault::create("pw").unwrap();
        let mut book = Book::default();
        let before = serde_json::to_string(&book).unwrap();
        assert!(!before.contains("signed"), "nothing new on disk until there is something: {before}");

        book.note_signed(7, "Send", "1 CV4 to bob", "ed25519:0x01");
        book.signed_off = true;
        let (_, opened) = Vault::open(&vault.seal(&book).unwrap(), "pw").unwrap();
        assert_eq!((opened.signed.len(), opened.signed[0].at, opened.signed_off), (1, 7, true));
        assert!(Vault::open(&vault.seal(&book).unwrap(), "not it").is_err(), "sealed with the rest");

        book.signed.clear();
        book.signed_off = false;
        assert_eq!(serde_json::to_string(&book).unwrap(), before, "cleared and on again, as it was");
    }


    #[test]
    fn what_is_sealed_is_written_into_room_made_for_it() {
        let book = Book {
            phrases: vec![Words::new(TEST.to_string(), "and a passphrase".to_string())],
            ..Book::default()
        };
        let plain = plain(&book).unwrap();

        assert_eq!(plain.len(), plain.capacity(), "never grown, so never copied");
        assert_eq!(serde_json::from_slice::<Book>(&plain).unwrap(), book);
    }

    #[test]
    fn accounts_open_with_their_password_alone() {
        let book = Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            accounts: vec![
                StoredAccount {
                    name: "Spending".to_string(),
                    source: Source::Phrase { account: 0, extra: Vec::new(), from: None },
                },
                StoredAccount {
                    name: "Development".to_string(),
                    source: Source::Key { key: Keypair::development().to_key_file() },
                },
            ],
            watch: false,
            swaps: Vec::new(),
            ..Book::default()
        };
        let vault = Vault::create("correct horse").unwrap();
        let sealed = vault.seal(&book).unwrap();
        let again = vault.seal(&book).unwrap();

        assert_ne!(sealed.nonce, again.nonce, "a fresh nonce every time");
        assert!(!sealed.sealed.contains(&to_hex(b"test test")[2..]), "nothing readable");
        assert_eq!(Vault::open(&sealed, "correct horse").unwrap().1, book);
        assert_eq!(Vault::open(&sealed, "wrong").err(), Some("wrong password".to_string()));
    }

    #[test]
    fn the_key_biometrics_keep_opens_the_vault_as_its_password_does() {
        let book = Book::default();
        let vault = Vault::create("correct horse").unwrap();
        let sealed = vault.seal(&book).unwrap();
        let (opened, read) = Vault::open_with_key(&sealed, &vault.key()).unwrap();

        assert_eq!(read, book);
        assert_eq!(opened.key(), vault.key(), "it seals again as the password would");
        assert_eq!(Vault::open(&opened.seal(&book).unwrap(), "correct horse").unwrap().1, book);
        assert!(Vault::open_with_key(&sealed, &[0; 32]).is_err());

        // After the password changes, the key kept before opens nothing.
        let changed = Vault::create("battery staple").unwrap().seal(&book).unwrap();

        assert!(Vault::open_with_key(&changed, &vault.key()).is_err());
    }

    #[test]
    fn a_vault_from_before_seed_phrases_opens_with_its_keys_brought_in() {
        let vault = Vault::create("correct horse").unwrap();
        let keys = vec![Stored {
            name: "Development".to_string(),
            key: Keypair::development().to_key_file(),
        }];
        // Sealed as the wallet sealed its list of keys before seed phrases.
        let nonce = [7; 12];
        let plain = serde_json::to_vec(&keys).unwrap();
        let sealed = ChaCha20Poly1305::new(Key::from_slice(&*vault.key))
            .encrypt(Nonce::from_slice(&nonce), plain.as_slice())
            .unwrap();
        let old = Sealed {
            salt: to_hex(&vault.salt),
            nonce: to_hex(&nonce),
            sealed: to_hex(&sealed),
        };
        let (_, book) = Vault::open(&old, "correct horse").unwrap();

        assert_eq!(book.phrases, Vec::new());
        assert_eq!(book.accounts, vec![StoredAccount {
            name: "Development".to_string(),
            source: Source::Key { key: Keypair::development().to_key_file() },
        }]);
    }

    const TEST: &str = "test test test test test test test test test test test junk";
    const ABANDON: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    /// What the abandon words' BIP32 fingerprint is.
    const ABANDON_PRINT: Fingerprint = [0x73, 0xc5, 0xda, 0x0a];

    /// The shapes an older build reads a vault into, copied as they stood before a wallet could hold
    /// more than one phrase. What they parse is what such a build would open, and what they write is
    /// what it would save.
    mod older {
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Default, Serialize, Deserialize)]
        #[serde(default)]
        pub struct Book {
            pub phrase: Option<Words>,
            pub accounts: Vec<StoredAccount>,
            pub watch: bool,
            pub swaps: Vec<SwapRecord>,
        }

        #[derive(Debug, Serialize, Deserialize)]
        pub struct SwapRecord {
            pub terms: String,
            pub secret: Option<String>,
            pub account: String,
            pub made: u64,
            #[serde(default)]
            pub started: bool,
            #[serde(default)]
            pub funding: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub multisig: Option<String>,
        }

        #[derive(Debug, Serialize, Deserialize)]
        pub struct Words {
            pub words: String,
            pub passphrase: String,
        }

        #[derive(Debug, Serialize, Deserialize)]
        pub struct Extra {
            pub scheme: String,
            pub index: u32,
        }

        #[derive(Debug, Serialize, Deserialize)]
        pub struct StoredAccount {
            pub name: String,
            pub source: Source,
        }

        #[derive(Debug, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum Source {
            Phrase {
                account: u32,
                #[serde(default, skip_serializing_if = "Vec::is_empty")]
                extra: Vec<Extra>,
            },
            Key { key: String },
            Watch {
                keys: Vec<String>,
                #[serde(default, skip_serializing_if = "Option::is_none")]
                origin: Option<crate::airgap::Origin>,
            },
            #[serde(rename = "metamask")]
            MetaMask { key: String },
        }

        #[derive(Debug, Serialize, Deserialize)]
        pub struct Stored {
            pub name: String,
            pub key: String,
        }

        /// Opens as an older build opened: a book, or else a bare list of keys, or damaged.
        pub fn opens(plain: &[u8]) -> bool {
            serde_json::from_slice::<Book>(plain).is_ok() || serde_json::from_slice::<Vec<Stored>>(plain).is_ok()
        }
    }

    /// Seals these bytes as the wallet would, whatever they are.
    fn sealed_as(vault: &Vault, plain: &[u8]) -> Sealed {
        let nonce = [7; 12];
        let sealed = ChaCha20Poly1305::new(Key::from_slice(&*vault.key)).encrypt(Nonce::from_slice(&nonce), plain).unwrap();

        Sealed {
            salt: to_hex(&vault.salt),
            nonce: to_hex(&nonce),
            sealed: to_hex(&sealed),
        }
    }

    /// What a vault seals, read back out of it.
    fn unsealed(vault: &Vault, sealed: &Sealed) -> Vec<u8> {
        let nonce = from_hex(&sealed.nonce).unwrap();

        ChaCha20Poly1305::new(Key::from_slice(&*vault.key)).decrypt(Nonce::from_slice(&nonce), from_hex(&sealed.sealed).unwrap().as_slice()).unwrap()
    }

    /// A one-phrase book holding every kind of account and field an older build wrote, in JSON as
    /// it wrote it.
    fn full_older_form() -> String {
        let key = Keypair::development().to_key_file();
        let public = Keypair::development().public();

        format!(
            concat!(
                r#"{{"phrase":{{"words":"{test}","passphrase":"a passphrase"}},"#,
                r#""accounts":["#,
                r#"{{"name":"Plain","source":{{"phrase":{{"account":0}}}}}},"#,
                r#"{{"name":"Further","source":{{"phrase":{{"account":1,"extra":[{{"scheme":"secp256k1","index":2}}]}}}}}},"#,
                r#"{{"name":"Brought","source":{{"key":{{"key":"{key}"}}}}}},"#,
                r#"{{"name":"Watched","source":{{"watch":{{"keys":["{public}"]}}}}}},"#,
                r#"{{"name":"Paired","source":{{"watch":{{"keys":["{public}"],"origin":{{"account":3,"fingerprint":[1,2,3,4]}}}}}}}},"#,
                r#"{{"name":"MetaMask","source":{{"metamask":{{"key":"{public}"}}}}}}"#,
                r#"],"watch":false,"#,
                r#""swaps":[{{"terms":"t","secret":null,"account":"a","made":5,"started":true,"funding":"f:0:1","multisig":"m"}}]}}"#,
            ),
            test = TEST,
            key = key,
            public = public,
        )
    }

    #[test]
    fn every_older_form_opens_and_saves_back_as_it_was() {
        let vault = Vault::create("correct horse").unwrap();
        let development = Keypair::development();
        let full = full_older_form();
        let forms: Vec<(String, usize, usize)> = vec![
            (full.clone(), 1, 6),
            // A phrase as null, and no phrase at all: a wallet of keys, or watching.
            (format!(r#"{{"phrase":null,"accounts":[{{"name":"Key","source":{{"key":{{"key":"{}"}}}}}}],"watch":false,"swaps":[]}}"#, development.to_key_file()), 0, 1),
            (format!(r#"{{"phrase":null,"accounts":[{{"name":"W","source":{{"watch":{{"keys":["{}"]}}}}}}],"watch":true,"swaps":[]}}"#, development.public()), 0, 1),
            // Before swaps, watching or further keys.
            (format!(r#"{{"phrase":{{"words":"{TEST}","passphrase":""}},"accounts":[{{"name":"Old","source":{{"phrase":{{"account":0}}}}}}]}}"#), 1, 1),
            (r#"{"accounts":[]}"#.to_string(), 0, 0),
            ("{}".to_string(), 0, 0),
            // A bare list of keys from before seed phrases, and the empty one.
            (format!(r#"[{{"name":"Dev","key":"{}"}}]"#, development.to_key_file()), 0, 1),
            ("[]".to_string(), 0, 0),
        ];

        for (form, phrases, accounts) in forms {
            assert!(older::opens(form.as_bytes()), "an older build opens this form: {form}");

            let (opened, book) = Vault::open(&sealed_as(&vault, form.as_bytes()), "correct horse").unwrap();

            assert_eq!((book.phrases.len(), book.accounts.len()), (phrases, accounts), "{form}");

            // Saved again, and read by an older build, with what it would have written itself.
            let saved = unsealed(&vault, &opened.seal(&book).unwrap());
            let as_older = match serde_json::from_slice::<older::Book>(form.as_bytes()) {
                Ok(book) => serde_json::to_vec(&book).unwrap(),
                Err(_) => {
                    let keys: Vec<older::Stored> = serde_json::from_slice(form.as_bytes()).unwrap();

                    serde_json::to_vec(&older::Book {
                        accounts: keys.into_iter().map(|stored| older::StoredAccount { name: stored.name, source: older::Source::Key { key: stored.key } }).collect(),
                        ..older::Book::default()
                    })
                    .unwrap()
                }
            };

            assert_eq!(String::from_utf8(saved.clone()).unwrap(), String::from_utf8(as_older).unwrap(), "byte for byte what an older build saves");
            assert!(older::opens(&saved), "and an older build opens it after");
        }

        // The fullest of them, unchanged to the byte.
        let (opened, book) = Vault::open(&sealed_as(&vault, full.as_bytes()), "correct horse").unwrap();

        assert_eq!(String::from_utf8(unsealed(&vault, &opened.seal(&book).unwrap())).unwrap(), full);
    }

    #[test]
    fn a_one_phrase_wallets_bytes_are_as_they_always_were() {
        let mut book = Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            accounts: vec![StoredAccount {
                name: "Main".to_string(),
                source: Source::Phrase { account: 0, extra: Vec::new(), from: None },
            }],
            ..Book::default()
        };
        let expected = format!(r#"{{"phrase":{{"words":"{TEST}","passphrase":""}},"accounts":[{{"name":"Main","source":{{"phrase":{{"account":0}}}}}}],"watch":false,"swaps":[]}}"#);

        assert_eq!(String::from_utf8(plain(&book).unwrap().to_vec()).unwrap(), expected);

        // Its seed looked at, and even a name given, change nothing written: a name is kept only
        // beside other phrases, as an older build has no place for it.
        book.phrases[0].fingerprint().unwrap();
        book.phrases[0].set_name("Mine");

        assert_eq!(String::from_utf8(plain(&book).unwrap().to_vec()).unwrap(), expected);
        assert_eq!(serde_json::to_string(&Book::default()).unwrap(), r#"{"phrase":null,"accounts":[],"watch":false,"swaps":[]}"#);
    }

    /// A book of two phrases, with an account of each.
    fn two_phrases() -> Book {
        let mut book = Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            accounts: vec![StoredAccount {
                name: "Main".to_string(),
                source: Source::Phrase { account: 0, extra: Vec::new(), from: None },
            }],
            ..Book::default()
        };
        let mut words = Words::new(ABANDON.to_string(), "with a passphrase".to_string());
        words.set_name("Savings");
        let from = book.add_phrase(words).unwrap();
        book.accounts.push(StoredAccount {
            name: "Saved".to_string(),
            source: Source::Phrase { account: 0, extra: vec![Extra { scheme: "bitcoin".to_string(), index: 2 }], from },
        });

        book
    }

    #[test]
    fn a_wallet_of_two_phrases_seals_and_opens_as_it_was() {
        let book = two_phrases();
        let vault = Vault::create("correct horse").unwrap();
        let sealed = vault.seal(&book).unwrap();
        let (opened, read) = Vault::open(&sealed, "correct horse").unwrap();

        assert_eq!(read, book);
        assert_eq!(read.phrases[1].name(), "Savings");
        assert_eq!(read.phrases[1].passphrase(), "with a passphrase");
        assert_eq!(Vault::open(&opened.seal(&read).unwrap(), "correct horse").unwrap().1, book, "and again");
        assert_eq!(Vault::open(&sealed, "wrong").err(), Some("wrong password".to_string()), "all under one password");

        // The reference is the fingerprint, written for that account alone, and never a place.
        let json: serde_json::Value = serde_json::to_value(&book).unwrap();

        assert!(json["phrase"].is_array());
        assert_eq!(json["accounts"][0]["source"]["phrase"].get("from"), None, "the first phrase's accounts name none");
        assert_eq!(json["accounts"][1]["source"]["phrase"]["from"], serde_json::to_value(read.phrases[1].fingerprint().unwrap()).unwrap());
    }

    #[test]
    fn biometrics_keep_opening_a_wallet_of_two_phrases_as_they_keep_the_stretched_key_alone() {
        let book = two_phrases();
        let vault = Vault::create("correct horse").unwrap();
        let sealed = vault.seal(&book).unwrap();
        let (opened, read) = Vault::open_with_key(&sealed, &vault.key()).unwrap();

        assert_eq!(read, book);
        assert_eq!(opened.key(), vault.key());

        // A phrase added while it stays open changes nothing they keep.
        let mut grown = read;
        grown.add_phrase(Words::new(TEST.to_string(), "a third".to_string())).unwrap();

        assert_eq!(Vault::open_with_key(&opened.seal(&grown).unwrap(), &vault.key()).unwrap().1, grown);
    }

    #[test]
    fn an_older_build_refuses_a_wallet_of_two_phrases_rather_than_half_read_it() {
        // Every shape of one: with accounts of each, a second with none, with names and passphrases.
        let mut no_accounts = Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            ..Book::default()
        };
        no_accounts.add_phrase(Words::new(ABANDON.to_string(), String::new())).unwrap();

        let mut watch_field = no_accounts.clone();
        watch_field.swaps.push(SwapRecord { terms: "t".to_string(), secret: None, account: "a".to_string(), made: 1, started: false, funding: None, multisig: None });

        let mut third = two_phrases();
        third.add_phrase(Words::new(TEST.to_string(), "another".to_string())).unwrap();

        for book in [two_phrases(), no_accounts, watch_field, third] {
            let plain = plain(&book).unwrap();
            let why = serde_json::from_slice::<older::Book>(&plain).unwrap_err().to_string();

            assert!(!why.contains("test") && !why.contains("abandon"), "and says none of the words: {why}");
            assert!(serde_json::from_slice::<Vec<older::Stored>>(&plain).is_err(), "nor as the bare list it tries next");
            assert!(!older::opens(&plain), "so it says damaged, and writes nothing");
        }
    }

    #[test]
    fn a_list_of_fewer_than_two_or_of_what_is_not_phrases_is_not_a_wallets() {
        for form in [
            r#"{"phrase":[]}"#.to_string(),
            format!(r#"{{"phrase":[{{"words":"{TEST}","passphrase":""}}]}}"#),
            format!(r#"{{"phrase":["{TEST}",""]}}"#),
            r#"{"phrase":"words"}"#.to_string(),
        ] {
            assert!(serde_json::from_str::<Book>(&form).is_err(), "{form}");
        }
    }

    #[test]
    fn an_account_naming_words_that_are_not_there_neither_opens_nor_seals() {
        let vault = Vault::create("correct horse").unwrap();

        // A wallet of one phrase with an account of another: an older build would read that
        // account as the first phrase's, with other keys, so this build never writes one.
        let mut dangling = Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            accounts: vec![StoredAccount {
                name: "Saved".to_string(),
                source: Source::Phrase { account: 0, extra: Vec::new(), from: Some(ABANDON_PRINT) },
            }],
            ..Book::default()
        };

        assert!(vault.seal(&dangling).unwrap_err().contains("does not hold"));

        let form = serde_json::to_vec(&dangling).unwrap();
        let sealed = sealed_as(&vault, &form);

        assert!(Vault::open(&sealed, "correct horse").err().unwrap().contains("does not hold"), "it does not open");
        assert!(Vault::open_with_key(&sealed, &vault.key()).is_err(), "by biometrics either");

        // In a list too, and with no words at all.
        let mut listed = two_phrases();
        let Source::Phrase { from, .. } = &mut listed.accounts[1].source else { unreachable!() };
        *from = Some([9, 9, 9, 9]);

        assert!(vault.seal(&listed).is_err());
        assert!(Vault::open(&sealed_as(&vault, &serde_json::to_vec(&listed).unwrap()), "correct horse").is_err());

        dangling.phrases.clear();
        assert!(vault.seal(&dangling).is_err());

        let accountless = br#"{"phrase":null,"accounts":[{"name":"Lost","source":{"phrase":{"account":4}}}]}"#;

        assert!(Vault::open(&sealed_as(&vault, accountless), "correct horse").is_err());

        // Words that do not read, with an account from them, likewise; with none, it opens as it did.
        let unread = br#"{"phrase":{"words":"not words","passphrase":""},"accounts":[{"name":"Lost","source":{"phrase":{"account":0}}}]}"#;

        assert!(Vault::open(&sealed_as(&vault, unread), "correct horse").is_err());
        assert!(Vault::open(&sealed_as(&vault, br#"{"phrase":{"words":"not words","passphrase":""},"accounts":[]}"#), "correct horse").is_ok());

        // Naming the first, which nothing writes, would count apart from the accounts naming none.
        let first = Words::new(TEST.to_string(), String::new()).fingerprint();
        let named = Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            accounts: vec![StoredAccount { name: "Named".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from: first } }],
            ..Book::default()
        };

        assert!(vault.seal(&named).is_err());

        // What the list holds must all read, and once each.
        let twice = format!(r#"{{"phrase":[{{"words":"{TEST}","passphrase":""}},{{"words":"{TEST}","passphrase":""}}]}}"#);
        let bad = format!(r#"{{"phrase":[{{"words":"{TEST}","passphrase":""}},{{"words":"not words","passphrase":""}}]}}"#);

        assert!(Vault::open(&sealed_as(&vault, twice.as_bytes()), "correct horse").err().unwrap().contains("twice"));
        assert!(Vault::open(&sealed_as(&vault, bad.as_bytes()), "correct horse").err().unwrap().contains("do not read"));
    }

    #[test]
    fn the_words_go_into_no_error_and_no_debug_output() {
        let book = two_phrases();
        let shown = format!("{book:?}");

        assert!(!shown.contains("test") && !shown.contains("abandon") && !shown.contains("with a passphrase"), "{shown}");
        assert!(format!("{:?}", book.phrases[0]).contains("12 words"));

        let mut dangling = book.clone();
        let Source::Phrase { from, .. } = &mut dangling.accounts[1].source else { unreachable!() };
        *from = Some([9, 9, 9, 9]);
        let why = dangling.check().unwrap_err();

        assert!(!why.contains("abandon") && !why.contains("with a passphrase"), "{why}");
        assert!(serde_json::from_str::<Book>(&format!(r#"{{"phrase":[{{"words":"{TEST}"}}]}}"#)).is_err());
    }

    #[test]
    fn no_secret_in_a_book_goes_into_its_debug_output() {
        let key = Keypair::from_seed([9; 32]).to_key_file();
        let secret = format!("0x{}", "ab".repeat(32));
        let mut book = two_phrases();
        book.accounts.push(StoredAccount { name: "Brought".to_string(), source: Source::Key { key: key.clone() } });
        book.swaps.push(SwapRecord {
            terms: "the terms".to_string(),
            secret: Some(secret.clone()),
            account: "the account".to_string(),
            made: 1,
            started: true,
            funding: None,
            multisig: None,
        });
        let shown = format!("{book:?}");
        let hex = key.split_once(':').unwrap().1.trim_start_matches("0x").to_string();

        assert!(!shown.contains(&key) && !shown.contains(&hex), "the key brought in: {shown}");
        assert!(!shown.contains(&secret) && !shown.contains("abab"), "the swap's secret: {shown}");
        assert!(!shown.contains(TEST) && !shown.contains(ABANDON), "the words: {shown}");

        // Pretty-printed too, and what is public still shows, to tell accounts apart.
        let pretty = format!("{book:#?}");

        assert!(!pretty.contains(&hex) && !pretty.contains("abab"));
        assert!(shown.contains("Brought") && shown.contains("the account") && shown.contains("hidden"));

        let watched = format!("{:?}", Source::Watch { keys: vec!["ed25519:0x01".to_string()], origin: None });

        assert!(watched.contains("ed25519:0x01"), "a public key is no secret");
    }

    /// An account of each kind, in a key text or scheme a build without its algorithm cannot read,
    /// and one that is plain damaged: to a build, the two look alike.
    fn unreadable_accounts() -> Vec<(&'static str, Source)> {
        let scheme = "dilithium:0x0101010101010101010101010101010101010101010101010101010101010101".to_string();
        let good = Keypair::development().public().to_string();

        vec![
            ("a key in a scheme it lacks", Source::Key { key: scheme.clone() }),
            ("a key that does not read", Source::Key { key: "ed25519:not hex".to_string() }),
            ("a watched key in a scheme it lacks", Source::Watch { keys: vec![scheme.clone()], origin: None }),
            ("one watched key of several", Source::Watch { keys: vec![good, scheme.clone()], origin: None }),
            ("MetaMask's key in a scheme it lacks", Source::MetaMask { key: scheme }),
            ("a further key in a scheme it lacks", Source::Phrase { account: 0, extra: vec![Extra { scheme: "dilithium".to_string(), index: 1 }], from: None }),
        ]
    }

    #[test]
    fn an_account_this_build_cannot_read_keeps_the_wallet_shut_and_nothing_is_written() {
        let vault = Vault::create("correct horse").unwrap();

        for (what, source) in unreadable_accounts() {
            let book = Book {
                phrases: vec![Words::new(TEST.to_string(), String::new())],
                accounts: vec![
                    StoredAccount { name: "Main".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from: None } },
                    StoredAccount { name: "Unread".to_string(), source },
                ],
                ..Book::default()
            };
            let form = serde_json::to_vec(&book).unwrap();
            let kept = sealed_as(&vault, &form);

            // It says so in words, naming the account, by password and by biometrics alike.
            for why in [Vault::open(&kept, "correct horse").err().unwrap(), Vault::open_with_key(&kept, &vault.key()).err().unwrap()] {
                assert!(why.contains("cannot read the account Unread") && why.contains("nothing is written"), "{what}: {why}");
                assert!(!why.contains("dilithium:") && !why.contains("0101") && !why.contains("not hex"), "{what}, and quotes no key: {why}");
                assert_ne!(why, "wrong password", "{what}: the password was right");
            }

            // No holders to save from, and no way to seal it over what is kept.
            assert!(crate::keys::open(&book).is_err(), "{what}");
            assert!(vault.seal(&book).is_err(), "{what}");

            // What is in storage still holds it whole, for a build that reads it.
            assert_eq!(unsealed(&vault, &kept), form, "{what}");
            assert_eq!(serde_json::from_slice::<Book>(&unsealed(&vault, &kept)).unwrap(), book, "{what}");
        }
    }

    #[test]
    fn every_kind_of_account_a_build_reads_survives_opening_and_saving() {
        let development = Keypair::development();
        let public = development.public().to_string();
        let origin = crate::airgap::Origin { account: 3, fingerprint: [1, 2, 3, 4] };
        let mut book = two_phrases();
        let every_scheme: Vec<String> = crate::keys::derive(Words::new(TEST.to_string(), String::new()).seed().unwrap(), 5).iter().map(|key| key.public().to_string()).collect();
        let further = KeyType::ALL.iter().map(|scheme| Extra { scheme: scheme.name().to_string(), index: 7 }).collect();
        let mut accounts = vec![StoredAccount { name: "Further".to_string(), source: Source::Phrase { account: 1, extra: further, from: None } }];

        for scheme in KeyType::ALL {
            accounts.push(StoredAccount { name: format!("Key {}", scheme.name()), source: Source::Key { key: Keypair::generate(*scheme).unwrap().to_key_file() } });
        }

        accounts.push(StoredAccount { name: "Watched".to_string(), source: Source::Watch { keys: every_scheme, origin: Some(origin) } });
        accounts.push(StoredAccount { name: "Watched, older".to_string(), source: Source::Watch { keys: vec![public.clone()], origin: None } });
        accounts.push(StoredAccount { name: "MetaMask".to_string(), source: Source::MetaMask { key: public } });
        book.accounts.extend(accounts);

        // Opened, and saved back as Wallet::store saves it: from the holders alone.
        let holders = crate::keys::open(&book).unwrap();

        assert_eq!(holders.len(), book.accounts.len(), "none left out");

        let saved = Book {
            accounts: holders.iter().map(|holder| StoredAccount { name: holder.name.clone(), source: holder.source.clone() }).collect(),
            ..book.clone()
        };

        assert_eq!(saved, book, "every account, of every kind, as it was");

        let vault = Vault::create("correct horse").unwrap();

        assert_eq!(Vault::open(&vault.seal(&saved).unwrap(), "correct horse").unwrap().1, book);
    }

    #[test]
    fn a_phrase_joins_a_book_once_named_plainly_and_a_watch_only_wallet_takes_none() {
        let mut book = Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            ..Book::default()
        };

        assert_eq!(book.add_phrase(Words::new(TEST.to_string(), String::new())).unwrap_err(), "These words are in this wallet already.");
        assert_eq!(book.add_phrase(Words::new("not words".to_string(), String::new())).unwrap_err(), "These words do not read.");

        let from = book.add_phrase(Words::new(ABANDON.to_string(), String::new())).unwrap();

        assert_eq!(from, Some(ABANDON_PRINT));
        assert_eq!((book.name_of(0), book.name_of(1)), ("Words 1".to_string(), "Words 2".to_string()));
        assert_eq!(book.references(), vec![(None, "Words 1".to_string()), (from, "Words 2".to_string())]);
        assert_eq!(book.phrase_of(None).unwrap().words(), TEST);
        assert_eq!(book.phrase_of(from).unwrap().words(), ABANDON);
        assert_eq!(book.phrase_of(Some([9, 9, 9, 9])), None, "never another in its place");

        let mut empty = Book::default();

        assert_eq!(empty.add_phrase(Words::new(TEST.to_string(), String::new())).unwrap(), None, "the first is named by nothing");

        let mut watching = Book { watch: true, ..Book::default() };

        assert!(watching.add_phrase(Words::new(TEST.to_string(), String::new())).is_err());

        let mut unread = Book { phrases: vec![Words::new("not words".to_string(), String::new())], ..Book::default() };

        assert!(unread.add_phrase(Words::new(TEST.to_string(), String::new())).is_err(), "a list of phrases that would not open");
    }
}
