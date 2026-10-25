//! Keys at rest. What app storage holds is sealed with a password: Argon2id stretches the password
//! with a salt of its own into a key, and ChaCha20-Poly1305 seals the accounts with it under a
//! fresh nonce each time they are saved. Without the password, the storage says nothing but how
//! long the sealed accounts are.

use {
    argon2::Argon2,
    chacha20poly1305::aead::{Aead, KeyInit},
    chacha20poly1305::{ChaCha20Poly1305, Key, Nonce},
    chainv4::crypto::keypair::Keypair,
    chainv4::structs::hex::{from_hex, to_hex},
    serde::{Deserialize, Serialize},
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

/// One account, as the vault holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub name: String,
    /// The key, as a key file holds it.
    pub key: String,
}

/// An open vault: the key the password stretched to, kept while the wallet is unlocked, so it
/// seals again after a change without asking for the password.
pub struct Vault {
    salt: [u8; 16],
    key: [u8; 32],
}

impl Vault {
    /// A new vault, sealed with this password.
    pub fn create(password: &str) -> Result<Vault, String> {
        let mut salt = [0; 16];
        getrandom::fill(&mut salt).map_err(|err| format!("no randomness for a salt: {err}"))?;

        Ok(Vault {
            key: stretch(password, &salt)?,
            salt,
        })
    }

    /// Opens sealed accounts with a password.
    pub fn open(sealed: &Sealed, password: &str) -> Result<(Vault, Vec<Stored>), String> {
        let salt: [u8; 16] = bytes(&sealed.salt)?;
        let nonce: [u8; 12] = bytes(&sealed.nonce)?;
        let key = stretch(password, &salt)?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        let plain = cipher
            .decrypt(Nonce::from_slice(&nonce), from_hex(&sealed.sealed).ok_or("the vault is damaged")?.as_slice())
            .map_err(|_| "wrong password".to_string())?;
        let accounts = serde_json::from_slice(&plain).map_err(|_| "the vault is damaged".to_string())?;

        Ok((Vault { salt, key }, accounts))
    }

    /// Seals accounts again, under a fresh nonce.
    pub fn seal(&self, accounts: &[Stored]) -> Result<Sealed, String> {
        let mut nonce = [0; 12];
        getrandom::fill(&mut nonce).map_err(|err| format!("no randomness for a nonce: {err}"))?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let plain = serde_json::to_vec(accounts).map_err(|err| err.to_string())?;
        let sealed = cipher.encrypt(Nonce::from_slice(&nonce), plain.as_slice()).map_err(|_| "sealing failed".to_string())?;

        Ok(Sealed {
            salt: to_hex(&self.salt),
            nonce: to_hex(&nonce),
            sealed: to_hex(&sealed),
        })
    }
}

/// An account's key from what the vault holds.
pub fn key_of(stored: &Stored) -> Result<Keypair, String> {
    Keypair::from_key_file(&stored.key)
}

fn stretch(password: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    let mut key = [0; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|err| format!("could not stretch the password: {err}"))?;

    Ok(key)
}

fn bytes<const N: usize>(text: &str) -> Result<[u8; N], String> {
    from_hex(text).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| "the vault is damaged".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_open_with_their_password_alone() {
        let accounts = vec![Stored {
            name: "spending".to_string(),
            key: Keypair::development().to_key_file(),
        }];
        let vault = Vault::create("correct horse").unwrap();
        let sealed = vault.seal(&accounts).unwrap();
        let again = vault.seal(&accounts).unwrap();

        assert_ne!(sealed.nonce, again.nonce, "a fresh nonce every time");
        assert!(!sealed.sealed.contains(&to_hex(accounts[0].key.as_bytes())), "nothing readable");
        assert_eq!(Vault::open(&sealed, "correct horse").unwrap().1, accounts);
        assert_eq!(Vault::open(&sealed, "wrong").err(), Some("wrong password".to_string()));
        assert_eq!(key_of(&accounts[0]).unwrap().public(), Keypair::development().public());
    }
}
