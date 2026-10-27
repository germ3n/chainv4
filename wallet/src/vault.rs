//! Keys at rest. What app storage holds is sealed with a password: Argon2id stretches the password
//! with a salt of its own into a key, and ChaCha20-Poly1305 seals the accounts with it under a
//! fresh nonce each time they are saved. Without the password, the storage says nothing but how
//! long the sealed accounts are. The key the password stretches to, the accounts opened, and the
//! words are wiped from memory as they drop.
//!
//! A vault holds a book: the wallet's seed phrase, when it has one, and its accounts, each the
//! phrase's account of some number or a key brought in.

use {
    argon2::Argon2,
    chacha20poly1305::aead::{Aead, KeyInit},
    chacha20poly1305::{ChaCha20Poly1305, Key, Nonce},
    chainv4::structs::hex::{from_hex, to_hex},
    serde::{Deserialize, Serialize},
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

/// What a wallet's vault holds: its seed phrase, when it has one, and its accounts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Book {
    pub phrase: Option<Words>,
    pub accounts: Vec<StoredAccount>,
    /// A watch-only wallet's: its accounts are an offline signer's, each watched, and it holds no
    /// key and no words.
    pub watch: bool,
}

/// A seed phrase, and the BIP39 passphrase it is used with, empty for none. Wiped as it drops.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Words {
    pub words: String,
    pub passphrase: String,
}

impl Drop for Words {
    fn drop(&mut self) {
        self.words.zeroize();
        self.passphrase.zeroize();
    }
}

/// One account, as the vault holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAccount {
    pub name: String,
    pub source: Source,
}

/// Where an account's keys come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The phrase's account of this number: an address in every scheme.
    Phrase { account: u32 },
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

/// One account, as vaults from before seed phrases held it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

        Ok((
            Vault {
                salt,
                key: Zeroizing::new(*key),
                bare: false,
            },
            book,
        ))
    }

    /// Seals accounts again, under a fresh nonce.
    pub fn seal(&self, book: &Book) -> Result<Sealed, String> {
        let mut nonce = [0; 12];
        getrandom::fill(&mut nonce).map_err(|err| format!("no randomness for a nonce: {err}"))?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&*self.key));
        let plain = Zeroizing::new(serde_json::to_vec(book).map_err(|err| err.to_string())?);
        let sealed = cipher.encrypt(Nonce::from_slice(&nonce), plain.as_slice()).map_err(|_| "sealing failed".to_string())?;

        Ok(Sealed {
            salt: to_hex(&self.salt),
            nonce: to_hex(&nonce),
            sealed: to_hex(&sealed),
        })
    }
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
    fn accounts_open_with_their_password_alone() {
        let book = Book {
            phrase: Some(Words {
                words: "test test test test test test test test test test test junk".to_string(),
                passphrase: String::new(),
            }),
            accounts: vec![
                StoredAccount {
                    name: "Spending".to_string(),
                    source: Source::Phrase { account: 0 },
                },
                StoredAccount {
                    name: "Development".to_string(),
                    source: Source::Key { key: Keypair::development().to_key_file() },
                },
            ],
            watch: false,
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

        assert_eq!(book.phrase, None);
        assert_eq!(book.accounts, vec![StoredAccount {
            name: "Development".to_string(),
            source: Source::Key { key: Keypair::development().to_key_file() },
        }]);
    }
}
