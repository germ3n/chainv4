//! Accounts, unlocked. An account from the seed phrase has a key in every scheme, and so an address
//! in each: Ed25519, EdBlake, Ethereum's secp256k1 and Bitcoin's, all from the same words. An account brought
//! in as a key has that one. An account watched from an offline signer has its public keys alone.

use {
    crate::vault::{Book, Source},
    chainv4::crypto::keypair::{KeyType, Keypair, PublicKey},
    chainv4::crypto::mnemonic::{Phrase, Seed},
    chainv4::structs::address::Address,
};

/// An unlocked account.
pub struct Holder {
    pub name: String,
    pub source: Source,
    /// Its public keys, in the order schemes are listed: Ed25519, EdBlake, secp256k1, bitcoin.
    pub publics: Vec<PublicKey>,
    /// The keys to them, in the same order; none for an account watched from an offline signer.
    pub keys: Vec<Keypair>,
}

impl Holder {
    /// An account this wallet signs for, with these keys.
    pub fn signing(name: String, source: Source, keys: Vec<Keypair>) -> Holder {
        Holder {
            name,
            source,
            publics: keys.iter().map(Keypair::public).collect(),
            keys,
        }
    }

    /// An account an offline signer signs for, from the public keys it paired.
    pub fn watched(name: String, publics: Vec<PublicKey>) -> Holder {
        Holder {
            name,
            source: Source::Watch { keys: publics.iter().map(PublicKey::to_string).collect() },
            publics,
            keys: Vec::new(),
        }
    }

    /// An account MetaMask signs for, by the key it connected with.
    pub fn metamask(name: String, key: PublicKey) -> Holder {
        Holder {
            name,
            source: Source::MetaMask { key: key.to_string() },
            publics: vec![key],
            keys: Vec::new(),
        }
    }

    /// Whether MetaMask signs for it.
    pub fn by_metamask(&self) -> bool {
        matches!(self.source, Source::MetaMask { .. })
    }

    /// What names the account across runs: its first address.
    pub fn id(&self) -> String {
        self.publics.first().map(|public| public.address().to_string()).unwrap_or_default()
    }

    /// Its addresses, one per key.
    pub fn addresses(&self) -> Vec<Address> {
        self.publics.iter().map(PublicKey::address).collect()
    }

    /// The key of one of its addresses, when this wallet holds it.
    pub fn key(&self, address: &Address) -> Option<&Keypair> {
        self.keys.iter().find(|key| key.public().address() == *address)
    }

    /// The public key of one of its addresses.
    pub fn public(&self, address: &Address) -> Option<PublicKey> {
        self.publics.iter().copied().find(|public| public.address() == *address)
    }

    /// The phrase's number for it, if it comes from the phrase.
    pub fn number(&self) -> Option<u32> {
        match self.source {
            Source::Phrase { account } => Some(account),
            Source::Key { .. } | Source::Watch { .. } | Source::MetaMask { .. } => None,
        }
    }
}

/// What a scheme is called on a page: Ethereum, for secp256k1, and Bitcoin, as people know their
/// addresses.
pub fn label(scheme: KeyType) -> &'static str {
    match scheme {
        KeyType::Ed25519 => "Ed25519",
        KeyType::EdBlake => "EdBlake",
        KeyType::Secp256k1 => "Ethereum",
        KeyType::Bitcoin => "Bitcoin",
    }
}

/// The scheme an address belongs to; None for a contract's.
pub fn scheme_of(address: &Address) -> Option<KeyType> {
    match address {
        Address::Ed25519(_) => Some(KeyType::Ed25519),
        Address::EdBlake(_) => Some(KeyType::EdBlake),
        Address::Secp256k1(_) => Some(KeyType::Secp256k1),
        Address::Bitcoin(_) => Some(KeyType::Bitcoin),
        Address::Contract(_) | Address::Evm(_) => None,
    }
}

/// An account's keys from the seed: one in every scheme.
pub fn derive(seed: &Seed, account: u32) -> Vec<Keypair> {
    KeyType::ALL.iter().map(|scheme| seed.key(*scheme, account)).collect()
}

/// The book's seed, when it has a phrase that reads.
pub fn seed(book: &Book) -> Option<Seed> {
    let words = book.phrase.as_ref()?;

    Phrase::parse(&words.words).ok().map(|phrase| phrase.seed(&words.passphrase))
}

/// The number a new account takes from the phrase: the lowest none of these has, so one removed
/// comes back first.
pub fn unused(holders: &[Holder]) -> u32 {
    (0..).find(|number| !holders.iter().any(|holder| holder.number() == Some(*number))).unwrap_or(0)
}

/// Every account in a book, unlocked. One whose key does not read is left out.
pub fn open(book: &Book) -> Vec<Holder> {
    let seed = seed(book);

    book.accounts
        .iter()
        .filter_map(|stored| {
            let keys = match &stored.source {
                Source::Phrase { account } => derive(seed.as_ref()?, *account),
                Source::Key { key } => vec![Keypair::from_key_file(key).ok()?],
                Source::Watch { keys } => {
                    let publics = keys.iter().map(|key| key.parse().ok()).collect::<Option<Vec<PublicKey>>>()?;

                    return Some(Holder::watched(stored.name.clone(), publics));
                }
                Source::MetaMask { key } => return Some(Holder::metamask(stored.name.clone(), key.parse().ok()?)),
            };

            Some(Holder::signing(stored.name.clone(), stored.source.clone(), keys))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::vault::{StoredAccount, Words},
    };

    #[test]
    fn an_account_from_the_phrase_has_an_address_in_every_scheme() {
        let book = Book {
            phrase: Some(Words {
                words: "test test test test test test test test test test test junk".to_string(),
                passphrase: String::new(),
            }),
            accounts: vec![
                StoredAccount {
                    name: "Main".to_string(),
                    source: Source::Phrase { account: 0 },
                },
                StoredAccount {
                    name: "Second".to_string(),
                    source: Source::Phrase { account: 1 },
                },
                StoredAccount {
                    name: "Brought in".to_string(),
                    source: Source::Key { key: Keypair::development().to_key_file() },
                },
            ],
            watch: false,
        };
        let holders = open(&book);
        let kinds = |holder: &Holder| holder.addresses().iter().map(|address| address.kind()).collect::<Vec<_>>();

        assert_eq!(kinds(&holders[0]), ["ed25519", "edblake", "secp256k1", "bitcoin"]);
        assert!(holders[0].addresses()[3].to_string().starts_with("bc1q"), "Bitcoin's, as BIP84 derives it");
        assert_eq!(holders[0].addresses()[2].to_string(), "secp256k1:0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266", "Ethereum's, for these words");
        assert_eq!(holders[1].addresses()[2].to_string(), "secp256k1:0x70997970c51812dc3a010c7d01b50e0d17dc79c8");
        assert_eq!((holders[0].number(), holders[2].number()), (Some(0), None));
        assert_eq!(holders[2].addresses(), vec![Keypair::development().public().address()]);
        assert_eq!(holders[0].id(), holders[0].addresses()[0].to_string());
        assert!(holders[0].key(&holders[0].addresses()[1]).is_some());
        assert!(holders[0].key(&holders[1].addresses()[1]).is_none());
        assert_eq!(unused(&holders), 2);
        assert_eq!(unused(&holders[1..]), 0, "the first, removed, comes back first");
    }

    #[test]
    fn an_account_watched_has_its_public_keys_and_no_key_to_sign_with() {
        let signer = open(&Book {
            phrase: Some(Words {
                words: "test test test test test test test test test test test junk".to_string(),
                passphrase: String::new(),
            }),
            accounts: vec![StoredAccount {
                name: "Main".to_string(),
                source: Source::Phrase { account: 0 },
            }],
            watch: false,
        });
        let watched = Holder::watched("Main".to_string(), signer[0].publics.clone());
        let book = Book {
            phrase: None,
            accounts: vec![StoredAccount {
                name: watched.name.clone(),
                source: watched.source.clone(),
            }],
            watch: true,
        };
        let reopened = open(&serde_json::from_value(serde_json::to_value(&book).unwrap()).unwrap());
        let ethereum = signer[0].addresses()[2];

        assert_eq!(reopened.len(), 1, "it reads back from the vault");
        assert_eq!(reopened[0].addresses(), signer[0].addresses(), "the signer's addresses, every scheme");
        assert_eq!(reopened[0].id(), signer[0].id());
        assert!(reopened[0].keys.is_empty() && reopened[0].key(&ethereum).is_none(), "nothing to sign with");
        assert_eq!(reopened[0].public(&ethereum), Some(signer[0].keys[2].public()));
        assert_eq!(reopened[0].number(), None);

        let broken = Book {
            phrase: None,
            accounts: vec![StoredAccount {
                name: "Broken".to_string(),
                source: Source::Watch { keys: vec!["not a key".to_string()] },
            }],
            watch: true,
        };

        assert!(open(&broken).is_empty(), "one whose keys do not read is left out");
    }
}
