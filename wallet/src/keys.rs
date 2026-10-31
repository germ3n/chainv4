//! Accounts, unlocked. An account from a seed phrase has a key in every scheme, and so an address
//! in each: Ed25519, EdBlake, Ethereum's secp256k1 and Bitcoin's, all from the same words. An account brought
//! in as a key has that one. An account watched from an offline signer has its public keys alone.
//!
//! A wallet may hold several phrases. An account's number, and its further keys' indices, count
//! within its own phrase alone, so the first account of each is number 0, as a restore of that
//! phrase elsewhere makes it.

use {
    crate::airgap::Origin,
    crate::vault::{Book, Fingerprint, Source, Words, unreadable},
    chainv4::crypto::keypair::{KeyType, Keypair, PublicKey},
    chainv4::crypto::mnemonic::Seed,
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
    /// its key, and the record that brings it back from the words. The seed is its own phrase's.
    /// Its address.
    pub fn add_key(&mut self, seed: &Seed, scheme: KeyType, index: u32) -> Option<Address> {
        let Source::Phrase { account, extra, .. } = &mut self.source else {
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

    /// Its phrase's number for it, if it comes from a phrase. Numbers count within a phrase, so one
    /// alone says nothing of which: see from.
    pub fn number(&self) -> Option<u32> {
        match self.source {
            Source::Phrase { account, .. } => Some(account),
            Source::Key { .. } | Source::Watch { .. } | Source::MetaMask { .. } => None,
        }
    }

    /// The reference to the phrase it comes from, if it comes from one that is not the first.
    pub fn from(&self) -> Option<Fingerprint> {
        match self.source {
            Source::Phrase { from, .. } => from,
            Source::Key { .. } | Source::Watch { .. } | Source::MetaMask { .. } => None,
        }
    }

    /// Whether it comes from the phrase this reference names.
    pub fn is_of(&self, words: Option<Fingerprint>) -> bool {
        matches!(self.source, Source::Phrase { from, .. } if from == words)
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

/// The scheme an address belongs to; None for a contract's or a multisig's.
pub fn scheme_of(address: &Address) -> Option<KeyType> {
    match address {
        Address::Ed25519(_) => Some(KeyType::Ed25519),
        Address::EdBlake(_) => Some(KeyType::EdBlake),
        Address::Secp256k1(_) => Some(KeyType::Secp256k1),
        Address::Bitcoin(_) => Some(KeyType::Bitcoin),
        Address::Contract(_) | Address::Evm(_) | Address::Multisig(_) => None,
    }
}

/// An account's keys from the seed: one in every scheme.
pub fn derive(seed: &Seed, account: u32) -> Vec<Keypair> {
    KeyType::ALL.iter().map(|scheme| seed.key(*scheme, account)).collect()
}

/// The seed of the phrase this account comes from; none for one that comes from no phrase.
pub fn seed_of<'a>(book: &'a Book, holder: &Holder) -> Option<&'a Seed> {
    holder.number()?;

    book.phrase_of(holder.from()).and_then(Words::seed)
}

/// The seed of every phrase in the book that reads, the first first.
pub fn seeds(book: &Book) -> Vec<&Seed> {
    book.phrases.iter().filter_map(Words::seed).collect()
}

/// The phrase a page about the wallet's words shows for the account in use: its own, and for an
/// account from no phrase, the first.
pub fn words_in_use<'a>(book: &'a Book, holder: &Holder) -> Option<(Option<Fingerprint>, &'a Seed)> {
    let from = holder.from();

    Some((from, book.phrase_of(from)?.seed()?))
}

/// Where the words derive an account, for a request in Ethereum's codes to name its key by: its
/// number, and its own phrase's fingerprint.
pub fn origin(book: &Book, holder: &Holder) -> Option<Origin> {
    Some(Origin {
        account: holder.number()?,
        fingerprint: book.phrase_of(holder.from())?.fingerprint()?,
    })
}

/// The number a new account takes from a phrase: the lowest none of that phrase's accounts has, nor
/// any further Ethereum or Bitcoin key on the path its first keys of those take, so one removed
/// comes back first and no key is two accounts'. Other phrases' accounts count for nothing: their
/// paths are their own seeds'.
pub fn unused(holders: &[Holder], from: Option<Fingerprint>) -> u32 {
    (0..)
        .find(|number| !holders.iter().filter(|holder| holder.is_of(from)).any(|holder| holder.number() == Some(*number) || holder.shared_indices().contains(number)))
        .unwrap_or(0)
}

/// The index a further key in a scheme takes for an account of a phrase: below it, the next after
/// its own for the Edwards schemes; on the shared path, the lowest no account's number and no
/// further key of that phrase takes, for secp256k1 and bitcoin.
pub fn next_index(holders: &[Holder], account: u32, from: Option<Fingerprint>, scheme: KeyType) -> u32 {
    let extras = |holder: &Holder| holder.extras().into_iter().filter(move |(kind, _)| *kind == scheme).map(|(_, index)| index);
    let mine = || holders.iter().filter(|holder| holder.is_of(from));

    match scheme.on_secp256k1() {
        true => (0..).find(|index| !mine().any(|holder| holder.number() == Some(*index) || extras(holder).any(|taken| taken == *index))).unwrap_or(0),
        false => 1 + mine().filter(|holder| holder.number() == Some(account)).flat_map(extras).max().unwrap_or(0),
    }
}

/// What removing a phrase takes with it, to show before it goes.
#[derive(Debug, PartialEq, Eq)]
pub struct Removal {
    /// The accounts that go: each one's name and first address.
    pub accounts: Vec<(String, String)>,
    /// How many swaps those accounts have kept, left behind without their keys.
    pub swaps: usize,
}

/// What removing the phrase at this place would take. The first cannot go this way: it is what an
/// account naming no phrase comes from, and what an older build would read.
pub fn removal(book: &Book, holders: &[Holder], index: usize) -> Result<Removal, String> {
    let from = removable(book, index)?;
    let going: Vec<&Holder> = holders.iter().filter(|holder| holder.is_of(Some(from))).collect();

    Ok(Removal {
        accounts: going.iter().map(|holder| (holder.name.clone(), holder.id())).collect(),
        swaps: book.swaps.iter().filter(|record| going.iter().any(|holder| holder.id() == record.account)).count(),
    })
}

/// Removes the phrase at this place, and every account from it, together, so no account is ever
/// left naming words that are gone.
pub fn remove_phrase(book: &mut Book, holders: &mut Vec<Holder>, index: usize) -> Result<(), String> {
    let from = removable(book, index)?;
    holders.retain(|holder| !holder.is_of(Some(from)));
    book.phrases.remove(index);

    Ok(())
}

fn removable(book: &Book, index: usize) -> Result<Fingerprint, String> {
    if index == 0 {
        return Err("The first words stay: only removing the wallet takes them.".to_string());
    }

    book.phrases.get(index).and_then(Words::fingerprint).ok_or_else(|| "Those words are gone from this wallet.".to_string())
}

/// Every account in a book, unlocked, or why the wallet stays shut: none is ever left out, as
/// Wallet::store writes back only what is here, so one left out would go for good with the next
/// save. A key damaged, or in a scheme this build lacks, refuses like a phrase that is not there.
pub fn open(book: &Book) -> Result<Vec<Holder>, String> {
    book.check()?;

    book.accounts
        .iter()
        .map(|stored| {
            let unread = || unreadable(&stored.name);
            let keys = match &stored.source {
                Source::Phrase { account, extra, from } => {
                    let seed = book.phrase_of(*from).and_then(Words::seed).ok_or_else(unread)?;
                    let mut keys = derive(seed, *account);

                    for extra in extra {
                        keys.push(seed.key_at(extra.scheme.parse().map_err(|_| unread())?, *account, extra.index));
                    }

                    keys
                }
                Source::Key { key } => vec![Keypair::from_key_file(key).map_err(|_| unread())?],
                Source::Watch { keys, origin } => {
                    let publics = keys.iter().map(|key| key.parse().map_err(|_| unread())).collect::<Result<Vec<PublicKey>, String>>()?;

                    return Ok(Holder::watched(stored.name.clone(), publics, *origin));
                }
                Source::MetaMask { key } => return Ok(Holder::metamask(stored.name.clone(), key.parse().map_err(|_| unread())?)),
            };

            Ok(Holder::signing(stored.name.clone(), stored.source.clone(), keys))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::vault::StoredAccount,
        chainv4::crypto::mnemonic::Phrase,
    };

    #[test]
    fn an_account_from_the_phrase_has_an_address_in_every_scheme() {
        let book = Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            accounts: vec![
                StoredAccount {
                    name: "Main".to_string(),
                    source: Source::Phrase { account: 0, extra: Vec::new(), from: None },
                },
                StoredAccount {
                    name: "Second".to_string(),
                    source: Source::Phrase { account: 1, extra: Vec::new(), from: None },
                },
                StoredAccount {
                    name: "Brought in".to_string(),
                    source: Source::Key { key: Keypair::development().to_key_file() },
                },
            ],
            watch: false,
            swaps: Vec::new(),
            ..Book::default()
        };
        let holders = open(&book).unwrap();
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
        assert_eq!(unused(&holders, None), 2);
        assert_eq!(unused(&holders[1..], None), 0, "the first, removed, comes back first");
    }

    #[test]
    fn an_account_watched_has_its_public_keys_and_no_key_to_sign_with() {
        let signer = open(&Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            accounts: vec![StoredAccount {
                name: "Main".to_string(),
                source: Source::Phrase { account: 0, extra: Vec::new(), from: None },
            }],
            watch: false,
            swaps: Vec::new(),
            ..Book::default()
        })
        .unwrap();
        let origin = Origin { account: 0, fingerprint: [1, 2, 3, 4] };
        let watched = Holder::watched("Main".to_string(), signer[0].publics.clone(), Some(origin));
        let book = Book {
            phrases: Vec::new(),
            accounts: vec![StoredAccount {
                name: watched.name.clone(),
                source: watched.source.clone(),
            }],
            watch: true,
            swaps: Vec::new(),
            ..Book::default()
        };
        let reopened = open(&serde_json::from_value(serde_json::to_value(&book).unwrap()).unwrap()).unwrap();
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
        assert_eq!(open(&older).unwrap()[0].origin(), None);

        let broken = Book {
            phrases: Vec::new(),
            accounts: vec![StoredAccount {
                name: "Broken".to_string(),
                source: Source::Watch { keys: vec!["not a key".to_string()], origin: None },
            }],
            watch: true,
            swaps: Vec::new(),
            ..Book::default()
        };

        assert!(open(&broken).err().unwrap().contains("cannot read the account Broken"), "one whose keys do not read is never left out: the wallet stays shut");
    }

    #[test]
    fn further_keys_take_free_indices_and_come_back_from_the_words() {
        let book = Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            accounts: vec![
                StoredAccount { name: "Main".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from: None } },
                StoredAccount { name: "Second".to_string(), source: Source::Phrase { account: 1, extra: Vec::new(), from: None } },
            ],
            ..Book::default()
        };
        let seed = book.phrase_of(None).unwrap().seed().unwrap();
        let mut holders = open(&book).unwrap();

        // Ethereum's next free index is past both accounts' own; the Edwards schemes' are below each.
        assert_eq!(next_index(&holders, 0, None, KeyType::Secp256k1), 2);
        assert_eq!(next_index(&holders, 0, None, KeyType::Ed25519), 1);
        let ethereum = holders[0].add_key(seed, KeyType::Secp256k1, 2).unwrap();
        assert_eq!(ethereum.to_string(), "secp256k1:0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc", "MetaMask's third account");
        holders[0].add_key(seed, KeyType::Ed25519, 1).unwrap();
        assert_eq!(next_index(&holders, 1, None, KeyType::Secp256k1), 3, "index 2 is taken now, by an account's further key");
        assert_eq!(next_index(&holders, 0, None, KeyType::Ed25519), 2);
        assert_eq!(next_index(&holders, 1, None, KeyType::Ed25519), 1, "each account counts its own");

        // A new account never takes an index a further key holds.
        assert_eq!(unused(&holders, None), 3);

        // Sealed and opened again, the words bring every key back, in order.
        let kept = Book {
            accounts: holders.iter().map(|holder| StoredAccount { name: holder.name.clone(), source: holder.source.clone() }).collect(),
            ..book.clone()
        };
        let reopened = open(&serde_json::from_value(serde_json::to_value(&kept).unwrap()).unwrap()).unwrap();
        assert_eq!(reopened[0].addresses(), holders[0].addresses());
        assert_eq!(reopened[0].addresses().len(), 6);
        assert!(reopened[0].key(&ethereum).is_some(), "and signs with it");

        // A vault from before further keys reads as before.
        let older: Book = serde_json::from_value(serde_json::json!({ "phrase": null, "accounts": [{ "name": "Old", "source": { "phrase": { "account": 4 } } }], "watch": false })).unwrap();
        assert_eq!(older.accounts[0].source, Source::Phrase { account: 4, extra: Vec::new(), from: None });
    }

    /// A book of the test words and the abandon ones, with accounts of each.
    fn two_phrases(accounts: &[(u32, bool)]) -> (Book, Fingerprint) {
        let mut book = Book {
            phrases: vec![Words::new(TEST.to_string(), String::new())],
            ..Book::default()
        };
        let other = book.add_phrase(Words::new(ABANDON.to_string(), String::new())).unwrap();
        book.accounts = accounts
            .iter()
            .map(|(account, second)| StoredAccount {
                name: format!("{account}"),
                source: Source::Phrase { account: *account, extra: Vec::new(), from: if *second { other } else { None } },
            })
            .collect();

        (book, other.unwrap())
    }

    /// The addresses a phrase restored alone gives its account of a number.
    fn restored(words: &str, account: u32) -> Vec<Address> {
        derive(&Phrase::parse(words).unwrap().seed(""), account).iter().map(|key| key.public().address()).collect()
    }

    #[test]
    fn each_phrases_accounts_count_on_their_own_and_match_a_restore_of_it_elsewhere() {
        let (book, other) = two_phrases(&[(0, false), (1, false), (0, true)]);
        let holders = open(&book).unwrap();

        assert_eq!(other, [0x73, 0xc5, 0xda, 0x0a], "BIP32's fingerprint for those words");
        assert_eq!(holders[0].addresses(), restored(TEST, 0));
        assert_eq!(holders[1].addresses(), restored(TEST, 1));
        assert_eq!(holders[2].addresses(), restored(ABANDON, 0), "account 0 of its own words");
        assert_eq!((holders[0].from(), holders[2].from()), (None, Some(other)));
        assert!(holders[2].is_of(Some(other)) && !holders[2].is_of(None) && holders[0].is_of(None));

        assert_eq!(unused(&holders, None), 2);
        assert_eq!(unused(&holders, Some(other)), 1, "its own next, whatever the first words hold");
        assert_eq!(unused(&holders[..2], Some(other)), 0, "and its 0, when it has no account");
        assert_eq!(unused(&holders[2..], None), 0);

        // A key brought in counts for no phrase.
        let brought = Holder::signing("Key".to_string(), Source::Key { key: Keypair::development().to_key_file() }, vec![Keypair::development()]);

        assert!(!brought.is_of(None) && !brought.is_of(Some(other)));
        assert_eq!(unused(std::slice::from_ref(&brought), None), 0);
    }

    #[test]
    fn further_keys_and_the_shared_paths_do_not_collide_across_phrases() {
        let (book, other) = two_phrases(&[(0, false), (1, false), (0, true)]);
        let mut holders = open(&book).unwrap();
        let seed_first = book.phrase_of(None).unwrap().seed().unwrap();
        let seed_other = book.holding(other).unwrap().seed().unwrap();

        // The first words hold 0 and 1 on the shared path; the other holds its 0 alone.
        assert_eq!(next_index(&holders, 0, None, KeyType::Secp256k1), 2);
        assert_eq!(next_index(&holders, 0, Some(other), KeyType::Secp256k1), 1, "a path of its own seed");
        assert_eq!(next_index(&holders, 0, Some(other), KeyType::Bitcoin), 1);

        let at = next_index(&holders, 0, Some(other), KeyType::Secp256k1);
        let further = holders[2].add_key(seed_other, KeyType::Secp256k1, at).unwrap();

        // m/44'/60'/0'/0/1 of the abandon words, as a restore of them lists it, and not the test
        // words' second account, which has the same index.
        assert_eq!(further, restored(ABANDON, 1)[2]);
        assert_ne!(further, restored(TEST, 1)[2]);
        assert_eq!(unused(&holders, Some(other)), 2, "its further key's index is taken, in its own words");
        assert_eq!(unused(&holders, None), 2, "and none of the first words'");

        // Below an account, each phrase's account 0 counts its own keys.
        holders[0].add_key(seed_first, KeyType::Ed25519, 1).unwrap();
        assert_eq!(next_index(&holders, 0, None, KeyType::Ed25519), 2);
        assert_eq!(next_index(&holders, 0, Some(other), KeyType::Ed25519), 1, "number 0 of the other words has none yet");

        // Kept and reopened, every key comes back from its own words.
        let kept = Book {
            accounts: holders.iter().map(|holder| StoredAccount { name: holder.name.clone(), source: holder.source.clone() }).collect(),
            ..book.clone()
        };
        let reopened = open(&serde_json::from_value(serde_json::to_value(&kept).unwrap()).unwrap()).unwrap();

        assert_eq!(reopened[2].addresses(), holders[2].addresses());
        assert!(reopened[2].key(&further).is_some());
    }

    #[test]
    fn what_comes_from_an_accounts_words_comes_from_its_own() {
        let (book, other) = two_phrases(&[(0, false), (3, true)]);
        let holders = open(&book).unwrap();
        let brought = Holder::signing("Key".to_string(), Source::Key { key: Keypair::development().to_key_file() }, vec![Keypair::development()]);
        let abandon = Phrase::parse(ABANDON).unwrap().seed("");

        // Its pairing's origin, for a site's requests to name its Ethereum key by.
        assert_eq!(origin(&book, &holders[1]), Some(Origin { account: 3, fingerprint: other }));
        assert_eq!(origin(&book, &holders[0]), Some(Origin { account: 0, fingerprint: book.phrases[0].fingerprint().unwrap() }));
        assert_eq!(origin(&book, &brought), None);

        // Its seed, where a page derives from one for the account.
        assert_eq!(seed_of(&book, &holders[1]).unwrap().ethereum_account().master_fingerprint, other);
        assert_eq!(seed_of(&book, &holders[1]).unwrap().key(KeyType::Bitcoin, 3).public(), abandon.key(KeyType::Bitcoin, 3).public());
        assert!(seed_of(&book, &brought).is_none(), "a key brought in has no words");
        assert_eq!(seeds(&book).len(), 2);

        // The pages about the wallet's words follow the account in use; a key brought in, the first.
        assert_eq!(words_in_use(&book, &holders[1]).unwrap().0, Some(other));
        assert_eq!(words_in_use(&book, &brought).unwrap().0, None);
        assert_eq!(words_in_use(&book, &brought).unwrap().1.ethereum_account().master_fingerprint, book.phrases[0].fingerprint().unwrap());
    }

    #[test]
    fn a_seed_is_stretched_once_however_often_it_is_asked_for() {
        let words = Words::new(TEST.to_string(), String::new());
        let (first, second) = (words.seed().unwrap() as *const Seed, words.seed().unwrap() as *const Seed);

        assert_eq!(first, second, "the one kept, not another copy");
        assert!(Words::new("not words".to_string(), String::new()).seed().is_none());
        assert!(Words::new("not words".to_string(), String::new()).fingerprint().is_none());
        assert_ne!(Words::new(TEST.to_string(), "another".to_string()).fingerprint(), words.fingerprint(), "another passphrase is another seed");
    }

    #[test]
    fn a_dangling_account_fails_closed_rather_than_going_quietly() {
        let (mut book, _) = two_phrases(&[(0, false), (0, true)]);

        assert_eq!(open(&book).unwrap().len(), 2);

        // Its words are taken out from under it: it opens no holders at all, so nothing saves it away.
        book.phrases.pop();
        assert!(open(&book).err().unwrap().contains("does not hold"));

        // Nor does a name for words that were never here, or an account with no phrase at all.
        let (mut book, _) = two_phrases(&[(0, false)]);
        book.accounts.push(StoredAccount { name: "Lost".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from: Some([1, 2, 3, 4]) } });
        assert!(open(&book).is_err());

        let none = Book {
            accounts: vec![StoredAccount { name: "Lost".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from: None } }],
            ..Book::default()
        };
        assert!(open(&none).is_err());

        // And words that do not read, with an account from them.
        let unread = Book {
            phrases: vec![Words::new("not words".to_string(), String::new())],
            accounts: none.accounts.clone(),
            ..Book::default()
        };
        assert!(open(&unread).is_err());
    }

    #[test]
    fn a_phrase_removed_takes_its_accounts_with_it_and_the_first_cannot_go() {
        let (mut book, other) = two_phrases(&[(0, false), (0, true), (1, true)]);
        let mut holders = open(&book).unwrap();
        let brought = Holder::signing("Key".to_string(), Source::Key { key: Keypair::development().to_key_file() }, vec![Keypair::development()]);
        holders.push(brought);
        book.swaps.push(crate::vault::SwapRecord {
            terms: String::new(),
            secret: None,
            account: holders[1].id(),
            made: 0,
            started: false,
            funding: None,
            multisig: None,
        });
        let preview = removal(&book, &holders, 1).unwrap();

        assert_eq!(preview.accounts, vec![("0".to_string(), holders[1].id()), ("1".to_string(), holders[2].id())], "exactly those, and no key brought in");
        assert_eq!(preview.swaps, 1);
        assert_eq!(removal(&book, &holders, 0).unwrap_err(), "The first words stay: only removing the wallet takes them.");
        assert!(remove_phrase(&mut book, &mut holders, 0).is_err());
        assert_eq!((book.phrases.len(), holders.len()), (2, 4), "nothing changed");
        assert!(removal(&book, &holders, 7).is_err());

        remove_phrase(&mut book, &mut holders, 1).unwrap();

        assert_eq!(book.phrases.len(), 1);
        assert!(book.holding(other).is_none());
        assert_eq!(holders.iter().map(|holder| holder.name.as_str()).collect::<Vec<_>>(), ["0", "Key"]);

        // As Wallet::store does it: and what is sealed then is the shape older builds read again.
        book.accounts = holders.iter().map(|holder| StoredAccount { name: holder.name.clone(), source: holder.source.clone() }).collect();
        book.check().unwrap();

        assert!(serde_json::to_string(&book).unwrap().starts_with(r#"{"phrase":{"words":"#));
    }

    #[test]
    fn a_phrase_with_no_accounts_left_keeps_its_words_and_reads_as_several() {
        let (book, other) = two_phrases(&[(0, false)]);
        let holders = open(&book).unwrap();

        assert_eq!(removal(&book, &holders, 1).unwrap().accounts, Vec::new(), "a phrase with no accounts previews none");
        book.check().unwrap();
        assert_eq!(serde_json::from_slice::<Book>(&serde_json::to_vec(&book).unwrap()).unwrap().holding(other).map(|words| words.words().to_string()).as_deref(), Some(ABANDON));
    }

    const TEST: &str = "test test test test test test test test test test test junk";
    const ABANDON: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
}
