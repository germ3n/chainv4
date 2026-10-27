//! Accounts, unlocked. An account from the seed phrase has a key in every scheme, and so an address
//! in each: Ed25519, EdBlake, Ethereum's secp256k1 and Bitcoin's, all from the same words. An account brought
//! in as a key has that one. An account watched from an offline signer has its public keys alone.

use {
    crate::airgap::Origin,
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

    /// An account an offline signer signs for, from the public keys it paired, and where its words
    /// derive it.
    pub fn watched(name: String, publics: Vec<PublicKey>, origin: Option<Origin>) -> Holder {
        Holder {
            name,
            source: Source::Watch { keys: publics.iter().map(PublicKey::to_string).collect(), origin },
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

    /// Its further keys, each by scheme and index, for an account from the words.
    pub fn extras(&self) -> Vec<(KeyType, u32)> {
        match &self.source {
            Source::Phrase { extra, .. } => extra.iter().filter_map(|extra| Some((extra.scheme.parse().ok()?, extra.index))).collect(),
            _ => Vec::new(),
        }
    }

    /// The indices its further Ethereum and Bitcoin keys take on the paths every account shares.
    fn shared_indices(&self) -> Vec<u32> {
        self.extras().into_iter().filter(|(scheme, _)| scheme.on_secp256k1()).map(|(_, index)| index).collect()
    }

    /// Gives an account from the words a further key in a scheme, at the index next_index gives:
    /// its key, and the record that brings it back from the words. Its address.
    pub fn add_key(&mut self, seed: &Seed, scheme: KeyType, index: u32) -> Option<Address> {
        let Source::Phrase { account, extra } = &mut self.source else {
            return None;
        };
        let key = seed.key_at(scheme, *account, index);
        extra.push(crate::vault::Extra { scheme: scheme.name().to_string(), index });
        let address = key.public().address();
        self.publics.push(key.public());
        self.keys.push(key);

        Some(address)
    }

    /// Where an offline signer's words derive it, for an account watched from one that said.
    pub fn origin(&self) -> Option<Origin> {
        match self.source {
            Source::Watch { origin, .. } => origin,
            _ => None,
        }
    }

    /// The phrase's number for it, if it comes from the phrase.
    pub fn number(&self) -> Option<u32> {
        match self.source {
            Source::Phrase { account, .. } => Some(account),
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

/// The number a new account takes from the phrase: the lowest none of these has, nor any further
/// Ethereum or Bitcoin key on the path its first keys of those take, so one removed comes back
/// first and no key is two accounts'.
pub fn unused(holders: &[Holder]) -> u32 {
    (0..).find(|number| !holders.iter().any(|holder| holder.number() == Some(*number) || holder.shared_indices().contains(number))).unwrap_or(0)
}

/// The index a further key in a scheme takes for an account: below it, the next after its own for
/// the Edwards schemes; on the shared path, the lowest no account's number and no further key
/// takes, for secp256k1 and bitcoin.
pub fn next_index(holders: &[Holder], account: u32, scheme: KeyType) -> u32 {
    let extras = |holder: &Holder| holder.extras().into_iter().filter(move |(kind, _)| *kind == scheme).map(|(_, index)| index);

    match scheme.on_secp256k1() {
        true => (0..).find(|index| !holders.iter().any(|holder| holder.number() == Some(*index) || extras(holder).any(|taken| taken == *index))).unwrap_or(0),
        false => 1 + holders.iter().filter(|holder| holder.number() == Some(account)).flat_map(extras).max().unwrap_or(0),
    }
}

/// Every account in a book, unlocked. One whose key does not read is left out.
pub fn open(book: &Book) -> Vec<Holder> {
    let seed = seed(book);

    book.accounts
        .iter()
        .filter_map(|stored| {
            let keys = match &stored.source {
                Source::Phrase { account, extra } => {
                    let seed = seed.as_ref()?;
                    let mut keys = derive(seed, *account);
                    keys.extend(extra.iter().filter_map(|extra| Some(seed.key_at(extra.scheme.parse().ok()?, *account, extra.index))));

                    keys
                }
                Source::Key { key } => vec![Keypair::from_key_file(key).ok()?],
                Source::Watch { keys, origin } => {
                    let publics = keys.iter().map(|key| key.parse().ok()).collect::<Option<Vec<PublicKey>>>()?;

                    return Some(Holder::watched(stored.name.clone(), publics, *origin));
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
                    source: Source::Phrase { account: 0, extra: Vec::new() },
                },
                StoredAccount {
                    name: "Second".to_string(),
                    source: Source::Phrase { account: 1, extra: Vec::new() },
                },
                StoredAccount {
                    name: "Brought in".to_string(),
                    source: Source::Key { key: Keypair::development().to_key_file() },
                },
            ],
            watch: false,
            swaps: Vec::new(),
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
                source: Source::Phrase { account: 0, extra: Vec::new() },
            }],
            watch: false,
            swaps: Vec::new(),
        });
        let origin = Origin { account: 0, fingerprint: [1, 2, 3, 4] };
        let watched = Holder::watched("Main".to_string(), signer[0].publics.clone(), Some(origin));
        let book = Book {
            phrase: None,
            accounts: vec![StoredAccount {
                name: watched.name.clone(),
                source: watched.source.clone(),
            }],
            watch: true,
            swaps: Vec::new(),
        };
        let reopened = open(&serde_json::from_value(serde_json::to_value(&book).unwrap()).unwrap());
        let ethereum = signer[0].addresses()[2];

        assert_eq!(reopened.len(), 1, "it reads back from the vault");
        assert_eq!(reopened[0].addresses(), signer[0].addresses(), "the signer's addresses, every scheme");
        assert_eq!(reopened[0].id(), signer[0].id());
        assert!(reopened[0].keys.is_empty() && reopened[0].key(&ethereum).is_none(), "nothing to sign with");
        assert_eq!(reopened[0].public(&ethereum), Some(signer[0].keys[2].public()));
        assert_eq!(reopened[0].number(), None);
        assert_eq!(reopened[0].origin(), Some(origin), "where the signer derives it");

        // One kept before pairings said where.
        let older: Book = serde_json::from_value(serde_json::json!({ "phrase": null, "watch": true, "accounts": [{ "name": "Old", "source": { "watch": { "keys": [watched.publics[0].to_string()] } } }] })).unwrap();
        assert_eq!(open(&older)[0].origin(), None);

        let broken = Book {
            phrase: None,
            accounts: vec![StoredAccount {
                name: "Broken".to_string(),
                source: Source::Watch { keys: vec!["not a key".to_string()], origin: None },
            }],
            watch: true,
            swaps: Vec::new(),
        };

        assert!(open(&broken).is_empty(), "one whose keys do not read is left out");
    }

    #[test]
    fn further_keys_take_free_indices_and_come_back_from_the_words() {
        let words = crate::vault::Words { words: "test test test test test test test test test test test junk".to_string(), passphrase: String::new() };
        let book = Book {
            phrase: Some(words),
            accounts: vec![
                StoredAccount { name: "Main".to_string(), source: Source::Phrase { account: 0, extra: Vec::new() } },
                StoredAccount { name: "Second".to_string(), source: Source::Phrase { account: 1, extra: Vec::new() } },
            ],
            ..Book::default()
        };
        let seed = seed(&book).unwrap();
        let mut holders = open(&book);

        // Ethereum's next free index is past both accounts' own; the Edwards schemes' are below each.
        assert_eq!(next_index(&holders, 0, KeyType::Secp256k1), 2);
        assert_eq!(next_index(&holders, 0, KeyType::Ed25519), 1);
        let ethereum = holders[0].add_key(&seed, KeyType::Secp256k1, 2).unwrap();
        assert_eq!(ethereum.to_string(), "secp256k1:0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc", "MetaMask's third account");
        holders[0].add_key(&seed, KeyType::Ed25519, 1).unwrap();
        assert_eq!(next_index(&holders, 1, KeyType::Secp256k1), 3, "index 2 is taken now, by an account's further key");
        assert_eq!(next_index(&holders, 0, KeyType::Ed25519), 2);
        assert_eq!(next_index(&holders, 1, KeyType::Ed25519), 1, "each account counts its own");

        // A new account never takes an index a further key holds.
        assert_eq!(unused(&holders), 3);

        // Sealed and opened again, the words bring every key back, in order.
        let kept = Book {
            accounts: holders.iter().map(|holder| StoredAccount { name: holder.name.clone(), source: holder.source.clone() }).collect(),
            ..book.clone()
        };
        let reopened = open(&serde_json::from_value(serde_json::to_value(&kept).unwrap()).unwrap());
        assert_eq!(reopened[0].addresses(), holders[0].addresses());
        assert_eq!(reopened[0].addresses().len(), 6);
        assert!(reopened[0].key(&ethereum).is_some(), "and signs with it");

        // A vault from before further keys reads as before.
        let older: Book = serde_json::from_value(serde_json::json!({ "phrase": null, "accounts": [{ "name": "Old", "source": { "phrase": { "account": 4 } } }], "watch": false })).unwrap();
        assert_eq!(older.accounts[0].source, Source::Phrase { account: 4, extra: Vec::new() });
    }
}
