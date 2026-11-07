//! What the wallet knows about the chain, all of it checked. The light client follows the node
//! from the genesis the wallet trusts, and every balance, token and receipt the wallet shows is
//! one it proved against a header the client checked. The node only ever says where to look:
//! which tokens exist, which transactions touch an address. What is there is proven: each
//! transaction against its block's tx_root, and what it did against its receipts root.

use {
    crate::node::{Ask, Failure},
    chainv4::db::{Column, StateProof, address_key},
    chainv4::light::{
        Assurance, CheckpointStart, FinalityStep, LightClient, LightError, LightParams, LightRequest, LightState, MAX_LIGHT_HEADERS, Resume, may_resume, multi_balance_key, resume,
        nft_owner, nft_owner_key, token_balance, token_balance_key,
    },
    chainv4::registry::names::{FoundName, NameRecord, NameStatus, NamesConfig, PrimaryName, lineage, name_problem, primary_key, record_key, standing},
    chainv4::registry::directory::{DIRECTORY_KEY, Directory},
    chainv4::registry::federation::{DecidedFederation, FEDERATION_KEY},
    chainv4::registry::collections::{Collection, collection_key},
    chainv4::registry::tokens::{Token, token_key},
    chainv4::structs::account::Account,
    chainv4::structs::address::Address,
    chainv4::structs::block::SealedHeader,
    chainv4::structs::hash::Hash,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::receipt::CommittedReceipt,
    chainv4::structs::tx::SignedTx,
    chainv4::structs::u256::u256,
    chainv4_ui::filters::Answered,
    serde_json::Value,
    std::collections::HashMap,
};

/// Seconds between following the chain again once caught up, and proving balances again.
const FOLLOW_EVERY: f64 = 3.0;
/// How often it asks while the node's stream says when there is more: in case an event is missed.
const FOLLOW_STREAMED: f64 = 20.0;
const PROVE_EVERY: f64 = 4.0;
/// Proofs a second it goes on asking for, over a wallet's whole round of them: nginx lets an address
/// have 30 requests a second, and this leaves room for all else the apps ask.
const PROOFS_A_SECOND: f64 = 15.0;
/// Seconds before asking again what failed to come.
const RETRY_AFTER: f64 = 3.0;
/// A value the wallet proved, and how far it trusts it.
#[derive(Clone, Debug)]
pub struct Proven<T> {
    pub value: T,
    pub assurance: Assurance,
}

/// What a proven key is the key of.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Account(String),
    Token(String),
    /// A collection's registry entry, by address text.
    Collection(String),
    Balance { token: String, holder: String },
    /// A name's record, by name without the suffix.
    Name(String),
    /// An address's primary name.
    Primary(String),
    /// Who owns an NFT, by collection and id.
    NftOwner { collection: String, id: String },
    /// How much of a multi-token id a holder holds.
    MultiBalance { collection: String, id: String, holder: String },
    /// The contracts the chain announces.
    Directory,
    /// The bridges' federation in force.
    Federation,
    /// A multisig's policy, by its address as text.
    Policy(String),
    /// Governance's locks.
    Locks,
    /// The root a pool's block committed to, by its height.
    PoolRoot(u64),
    /// What a miner has claimed from a pool, by the pool's and the miner's addresses as text.
    PoolClaimed { pool: String, miner: String },
    /// An Ethereum account's EIP-7702 delegation, by its address as text.
    Delegation(String),
}

/// An NFT a page shows, for its ownership to be proven: a token of an NFT collection, or a
/// holder's amount of a multi-token id.
#[cfg_attr(not(feature = "nft"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Held {
    pub collection: String,
    /// The id, in decimal.
    pub id: String,
    pub multi: bool,
    pub holder: String,
}

/// What a contract is, by the registry entry proven for it.
pub enum Known<'a> {
    Token(&'a Token),
    Collection(&'a Collection),
}

/// Keeps what was proven at the higher block: proofs answer out of order, and one asked before an
/// NFT moved must not bring back who held it then.
fn newest<K: std::hash::Hash + Eq, T>(proven: &mut HashMap<K, Proven<T>>, key: K, new: Proven<T>) {
    if proven.get(&key).is_none_or(|old| old.assurance.height <= new.assurance.height) {
        proven.insert(key, new);
    }
}

/// A name looked up from proven records: where it stands at the tip, and how far that is trusted.
#[derive(Clone, Debug)]
pub struct Resolved {
    /// The name, or None when it is available: nobody holds it, or it is past its grace period.
    pub found: Option<FoundName>,
    /// The least of its records' assurances.
    pub assurance: Assurance,
}

impl Resolved {
    /// The address it pays: its target while it is active.
    pub fn target(&self) -> Option<Address> {
        self.found.as_ref().filter(|found| found.status == NameStatus::Active).and_then(|found| found.record.target)
    }
}

/// A transaction the node said touches an address: where to look, not what is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    pub tx: String,
    pub height: u64,
}

/// What the node listed of an address's transactions under a filter: where to look, as ever, with
/// what it said of its answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilteredTxs {
    /// Newest first.
    pub mentions: Vec<Mention>,
    /// Where the next page starts, or None at the end.
    pub next: Option<String>,
    pub answered: Answered,
    /// How many pages were read.
    pub pages: u32,
}

impl FilteredTxs {
    /// Takes a page: the newest, which stands for the list while that is all that was read and
    /// otherwise only adds what is new in front, or an older one, which goes after what is held.
    pub fn take(&mut self, found: Vec<Mention>, next: Option<String>, answered: Answered, older: bool) {
        match (older, self.pages) {
            (true, _) => {
                let fresh: Vec<Mention> = found.into_iter().filter(|mention| !self.mentions.contains(mention)).collect();
                self.mentions.extend(fresh);
                self.next = next;
                self.answered = answered;
                self.pages += 1;
            }
            (false, 0 | 1) => {
                self.mentions = found;
                self.next = next;
                self.answered = answered;
                self.pages = 1;
            }
            (false, _) => {
                let fresh: Vec<Mention> = found.into_iter().filter(|mention| !self.mentions.contains(mention)).collect();
                self.mentions.splice(0..0, fresh);
            }
        }
    }
}

/// A transaction that landed, as proven: who sent what, and what it did.
#[derive(Clone, Debug)]
pub struct Landed {
    pub signed: SignedTx,
    pub receipt: CommittedReceipt,
    /// When its block was made, in milliseconds since the Unix epoch.
    pub time: u64,
}

/// How often a client still walking is written out, for settings to keep: little to lose, as it
/// goes on from there, and much to gain, as writing it out costs more the further it got.
const SAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

/// What the wallet trusts at a node: the genesis pinned there, a checkpoint pasted to start from,
/// and what it saved of the chain it followed from each.
pub struct Trust<'a> {
    pub node: &'a str,
    /// Genesis ids by node.
    pub pinned: &'a mut HashMap<String, String>,
    /// Checkpoint ids by node.
    pub checkpoints: &'a HashMap<String, String>,
    /// Saved light client states by genesis id.
    pub saved: &'a HashMap<String, String>,
    /// Whether this node is trusted to resume from its own finalized checkpoint.
    pub fast_resume: bool,
}

/// A client on a node trusted for fast resume, until the node says how far it has finalized.
struct Resuming {
    params: LightParams,
    header: SealedHeader,
    genesis: Hash,
    /// What it saved, to walk on from.
    restored: Option<LightClient>,
}

/// Where a client fast-resumed, for the wallet to say so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FastResumed {
    /// The finalized height it saved, none if it saved nothing.
    pub from: Option<u64>,
    /// The checkpoint's height it took up the chain at.
    pub at: u64,
}

/// A light client about to start at a checkpoint, from what the node said of its chain.
struct Starting {
    params: LightParams,
    header: SealedHeader,
    genesis: Hash,
    /// The checkpoint pasted; None for the node's own.
    checkpoint: Option<Hash>,
    /// For a fast resume, what to walk on from should the node have no checkpoint it can start
    /// from, or none that holds: what it saved, or from genesis.
    resuming: Option<Option<LightClient>>,
}

#[derive(Default)]
pub struct Chain {
    pub client: Option<LightClient>,
    /// Waiting for what starts the client at a checkpoint.
    starting: Option<Starting>,
    /// Waiting for the node to say how far it has finalized.
    resuming: Option<Resuming>,
    /// Where it fast-resumed this time, if it did: what the sidebar and Settings say.
    pub fast_resumed: Option<FastResumed>,
    /// When it first asked anything, for how long it took to catch up.
    began: Option<f64>,
    /// What stopped the light client: a node on another chain than the one trusted, or one that
    /// sent something that does not check.
    pub trouble: Option<String>,
    /// Whether the client caught up with the node at least once since it started: pages show
    /// what it proved from then on, rather than going back to waiting each time it asks the node
    /// what came since.
    caught_up_once: bool,
    /// The genesis the client follows, once it started.
    pub genesis: Option<Hash>,
    /// Whether this node answered that it has no GET /light/bundle.
    no_bundle: bool,
    /// Whether what the client checked changed since it was last saved, and when it last was.
    pub dirty: bool,
    last_saved: Option<web_time::Instant>,
    followed: f64,
    /// Whether the node's stream is open, to say when there is more to follow.
    pub streamed: bool,
    /// How often it follows and proves again, by Low data: the app sets it from the setting.
    pub pace: chainv4_ui::pace::Pace,
    proved: f64,
    /// Seconds until the next round of proofs, from the size of the last.
    prove_gap: f64,
    /// What is proven already at the tip and the final block, by key and block: a block's state does
    /// not change, so neither does its proof.
    proven_at: std::collections::HashSet<(String, String)>,
    retry_at: f64,
    /// Accounts by address text: what was proven at the final block and at the tip.
    pub accounts: HashMap<String, Vec<Proven<Account>>>,
    /// Each Ethereum account's EIP-7702 delegation, proven at the tip: the contract whose code it
    /// runs as its own, or None.
    pub delegations: HashMap<String, Proven<Option<Address>>>,
    /// The chain's bonded validators, as GET /validators lists them: the node's word, for showing
    /// an account that bonded one its bond and where its rewards go.
    pub validators: Vec<serde_json::Value>,
    /// The tokens the registry lists, by address text: the node says which, the proof what.
    pub listed: Vec<String>,
    /// The registry's pages so far, while a listing of it comes in a page at a time: what listed
    /// becomes once the last arrives, so no token drops out while the rest are on their way.
    listing: Vec<String>,
    /// The pages of a listing still to ask, after the one that came.
    more: Vec<Ask>,
    pub tokens: HashMap<String, Proven<Token>>,
    /// The collections the registry lists, by address text, as GET /collections said: the node
    /// says which, the proof of each one's entry what it is.
    pub listed_collections: Vec<String>,
    /// Each collection's entry as the registry's last whole listing wrote it, its kind, name and
    /// symbol: what the NFTs page looks through. And the pages so far while one comes in.
    pub collection_entries: Vec<Value>,
    collections_listing: Vec<Value>,
    pub collections: HashMap<String, Proven<Collection>>,
    /// Token balances by token and holder.
    pub balances: HashMap<(String, String), Vec<Proven<u256>>>,
    /// Transactions proven with their receipts since the app last took them, by transaction id.
    pub receipts: HashMap<String, Proven<Landed>>,
    /// Transactions the node says touch an address, by address, newest first.
    pub mentions: HashMap<String, Vec<Mention>>,
    /// What each of the node's indexes said of an address, the newest page and the older ones read
    /// since, which mentions puts together.
    by_address_index: HashMap<String, FilteredTxs>,
    by_event_index: HashMap<String, FilteredTxs>,
    /// The chain's names, as the node said them, once it did; whether it has any, once known.
    pub names: Option<NamesConfig>,
    pub has_names: Option<bool>,
    /// The names the node says each address owns, by address text: where to look.
    pub owned: HashMap<String, Vec<String>>,
    /// An address's owned names so far, while they come a page at a time.
    owning: HashMap<String, Vec<String>>,
    /// Name records proven, by name without the suffix: None where there is none.
    pub records: HashMap<String, Proven<Option<NameRecord>>>,
    /// Primary names proven, by address text, as chosen: whether each still resolves to its
    /// address is for resolve to say.
    pub primaries: HashMap<String, Proven<Option<String>>>,
    /// When each name's records were last asked for.
    named: HashMap<String, f64>,
    /// NFT owners proven, by collection and id: None where nobody owns it.
    pub nft_owners: HashMap<(String, String), Proven<Option<Address>>>,
    /// Multi-token amounts proven, by collection, id and holder.
    pub multi_balances: HashMap<(String, String, String), Proven<u256>>,
    /// When each NFT's proof was last asked for, by its key.
    nfts_asked: HashMap<String, f64>,
    /// Whether the node keeps an address index, or else an event index, to list them from.
    pub indexes_addresses: Option<bool>,
    pub indexes_events: Option<bool>,
    /// What each proof asked for is the proof of.
    targets: HashMap<String, Target>,
    /// Why a proof did not check, by what it was for.
    pub refused: HashMap<String, String>,
    /// The contracts the chain announces, proven at the tip: None where it announces none. What
    /// the pages use where Settings gives none, before what the node names.
    pub directory: Option<Proven<Option<Directory>>>,
    /// The bridges' federation in force, proven at the tip: None on a chain without one. What
    /// deposit addresses to Bitcoin are computed from, never taken from a node.
    pub federation: Option<Proven<Option<DecidedFederation>>>,
    /// Multisigs' policies, proven, by address as text: None for an address that holds none.
    pub policies: HashMap<String, Proven<Option<chainv4::registry::multisig::MultisigPolicy>>>,
    /// The multisigs whose policies a page wants proven, as the app last said.
    pub wanted_policies: Vec<String>,
    /// Pools' roots, proven, by height: None for a height whose block holds none.
    pub pool_roots: HashMap<u64, Proven<Option<chainv4::registry::pools::PoolRoot>>>,
    /// What miners have claimed from pools, proven, by pool and miner as text.
    pub pool_claimed: HashMap<(String, String), Proven<u256>>,
    /// The claims a page wants proven, as the app last said: the pool, the miner and the height of
    /// the root the claim is under.
    pub wanted_claims: Vec<(String, String, u64)>,
    /// Governance's locks, proven: none at all on a chain that set none.
    pub locks: Option<Proven<Vec<String>>>,
    /// The lowest height a receipt landed at below every header the client holds: what it takes
    /// ancestors down to, a stretch at a time.
    wanted_below: Option<u64>,
    /// What the node listed under filters, by address and the filter's query.
    pub filtered: HashMap<(String, String), FilteredTxs>,
}

impl Chain {
    /// The federation in force, proven, if the chain has one.
    #[cfg_attr(not(feature = "bridge"), allow(dead_code))]
    pub fn federation_in_force(&self) -> Option<&DecidedFederation> {
        self.federation.as_ref().and_then(|proven| proven.value.as_ref())
    }

    /// The directory proven, if the chain announces one.
    #[cfg_attr(not(any(feature = "swap", feature = "dex")), allow(dead_code))]
    pub fn announced(&self) -> Option<&Directory> {
        self.directory.as_ref().and_then(|proven| proven.value.as_ref())
    }

    /// Starts the client waiting to start at a checkpoint another way: on from what it saved, for
    /// a fast resume that saved something, else from genesis.
    fn start_from_genesis(&mut self) {
        if let Some(mut starting) = self.starting.take() {
            if let Some(Some(saved)) = starting.resuming.take() {
                self.client = Some(saved);

                return;
            }

            match LightClient::new(starting.params, starting.header, starting.genesis) {
                Ok(client) => self.client = Some(client),
                Err(err) => self.trouble = Some(err.to_string()),
            }
        }
    }

    /// The node's stream says there is more: it asks now rather than at its next turn, proving what
    /// comes as ever. The stream is trusted for nothing else.
    pub fn nudge(&mut self) {
        self.followed = f64::NEG_INFINITY;
    }

    /// Whether to ask the node for more of the chain now.
    fn follow_due(&self, time: f64) -> bool {
        time - self.followed >= self.pace.every(if self.streamed { FOLLOW_STREAMED } else { FOLLOW_EVERY })
    }

    /// Asks again for all it follows and proves by, at once, whatever it is waiting for: for a
    /// reload. What it already proved stands until the answers come.
    pub fn reload(&mut self) {
        self.retry_at = 0.0;
        self.followed = f64::NEG_INFINITY;
        self.proved = f64::NEG_INFINITY;
        self.named.clear();
        self.nfts_asked.clear();
    }

    /// The tip's height, and the finalized height on a chain with finality, None on one without:
    /// what a swap's lock here is reckoned final by.
    #[cfg_attr(not(feature = "swap"), allow(dead_code))]
    pub fn finality(&self) -> Option<(u64, Option<u64>)> {
        let client = self.client.as_ref()?;

        Some((client.tip().height, client.params().finality.is_some().then(|| client.final_height())))
    }

    /// How far it follows: the tip's height and the height up to which blocks are final.
    pub fn following(&self) -> Option<(u64, u64)> {
        self.client.as_ref().map(|client| (client.tip().height, client.final_height()))
    }

    /// Whether it has every header and justification the node had when last asked.
    pub fn caught_up(&self) -> bool {
        self.client.as_ref().is_some_and(|client| client.next().is_none())
    }

    /// Whether it caught up at least once since it started: for pages, which keep showing what
    /// was proven while it asks after new blocks, as it does every few seconds.
    pub fn has_followed(&self) -> bool {
        self.caught_up_once || self.caught_up()
    }

    /// What the light client needs next, if anything is due.
    pub fn next_ask(&mut self, time: f64) -> Option<Ask> {
        if self.trouble.is_some() || time < self.retry_at {
            return None;
        }

        let began = *self.began.get_or_insert(time);

        if !self.caught_up_once && self.client.as_ref().is_some_and(|client| client.next().is_none()) {
            self.caught_up_once = true;
            log::info!("the light client caught up in {:.1} s{}", time - began, if self.fast_resumed.is_some() { ", fast-resumed" } else { "" });
        }

        let due = self.follow_due(time);

        let Some(client) = &mut self.client else {
            return Some(match (&self.starting, &self.resuming) {
                (Some(starting), _) => Ask::Checkpoint(starting.checkpoint.map(|block| block.to_string())),
                (None, Some(_)) => Ask::Finalized,
                (None, None) => Ask::Light,
            });
        };

        if client.next().is_none() && due {
            self.followed = time;
            client.refresh();
        }

        // What it wants next all at once, from a node that can: a walk's time is its round trips.
        if !self.no_bundle
            && let Some((headers, epoch)) = client.next_bundle()
        {
            return Some(Ask::Bundle { headers, epoch });
        }

        if let Some(request) = client.next() {
            return Some(match request {
                LightRequest::Headers { from, count } => Ask::Headers { from, count },
                LightRequest::Finality { from, count } => Ask::Finality { from, count },
            });
        }

        // Caught up: the headers below it that older receipts need, the nearest stretch first.
        let lowest = client.lowest();
        let wanted = self.wanted_below.filter(|wanted| *wanted < lowest)?;
        let from = wanted.max(lowest.saturating_sub(u64::from(MAX_LIGHT_HEADERS)));

        Some(Ask::Ancestors {
            from,
            count: (lowest - from) as u32,
        })
    }

    /// The proofs due for these holders: each account, every listed token's entry, and each
    /// holder's balance of it, at the tip and at the final block.
    pub fn proofs_due(&mut self, holders: &[Address], time: f64) -> Vec<Ask> {
        // The next pages of a listing go out as soon as the page before them came.
        let more = std::mem::take(&mut self.more);

        if !self.caught_up() || time - self.proved < self.pace.every(PROVE_EVERY).max(self.prove_gap) {
            return more;
        }

        self.proved = time;
        let client = self.client.as_ref().expect("caught up");
        let tip = client.tip().id();
        let mut blocks = vec![tip];

        if let Some(final_block) = client.header_at(client.final_height()).map(|header| header.id())
            && client.final_height() > 0
            && final_block != tip
        {
            blocks.insert(0, final_block);
        }

        let mut wanted = vec![Ask::Tokens(None)];
        let mut targets = Vec::new();

        if self.has_names != Some(false) {
            wanted.push(Ask::Names);
        }

        for holder in holders {
            targets.push((Column::Accounts, address_key(holder), Target::Account(holder.to_string()), blocks.clone()));

            // An Ethereum key's account may run a contract's code, by EIP-7702: proven at the tip.
            if let Address::Secp256k1(bytes) = holder {
                targets.push((Column::Storage, chainv4::structs::account::delegation_key(bytes), Target::Delegation(holder.to_string()), vec![tip]));
            }

            if self.has_names == Some(true) {
                wanted.push(Ask::OwnedNames { address: holder.to_string(), cursor: None });
                targets.push((Column::Names, primary_key(holder), Target::Primary(holder.to_string()), vec![tip]));
            }

            for token in &self.listed {
                let Ok(parsed) = token.parse::<Address>() else {
                    continue;
                };
                let balance = Target::Balance {
                    token: token.clone(),
                    holder: holder.to_string(),
                };
                targets.push((Column::Namespaces, token_balance_key(&parsed, holder), balance, blocks.clone()));
            }
        }

        for token in &self.listed {
            if let Ok(parsed) = token.parse::<Address>() {
                targets.push((Column::Tokens, token_key(&parsed), Target::Token(token.clone()), vec![tip]));
            }
        }

        // What each collection is, so a review reads an NFT's transfer as one rather than as a
        // token's: an id is no amount.
        wanted.push(Ask::Collections(None));

        for collection in &self.listed_collections {
            if let Ok(parsed) = collection.parse::<Address>() {
                targets.push((Column::Tokens, collection_key(&parsed), Target::Collection(collection.clone()), vec![tip]));
            }
        }

        targets.push((Column::Validators, DIRECTORY_KEY.to_vec(), Target::Directory, vec![tip]));
        targets.push((Column::Validators, chainv4::registry::changes::Changes::locks_key().to_vec(), Target::Locks, vec![tip]));

        // The multisigs a page shows, so their members and threshold rest on no node's word.
        for multisig in &self.wanted_policies {
            if let Ok(parsed) = multisig.parse::<Address>() {
                targets.push((Column::Accounts, chainv4::registry::Multisigs::policy_key(&parsed), Target::Policy(multisig.clone()), vec![tip]));
            }
        }
        targets.push((Column::Validators, FEDERATION_KEY.to_vec(), Target::Federation, vec![tip]));

        // A pool's claim a page shows: the root it is under, and what its miner claimed, so what
        // it pays rests on no node's word.
        for (pool, miner, height) in &self.wanted_claims {
            use chainv4::registry::pools::Pools;

            targets.push((Column::Pools, Pools::root_key(*height), Target::PoolRoot(*height), vec![tip]));

            if let (Ok(parsed_pool), Ok(parsed_miner)) = (pool.parse::<Address>(), miner.parse::<Address>()) {
                targets.push((Column::Pools, Pools::claimed_key(&parsed_pool, &parsed_miner), Target::PoolClaimed { pool: pool.clone(), miner: miner.clone() }, vec![tip]));
            }
        }

        wanted.extend(self.not_proven_yet(&blocks, targets));
        wanted.extend(more);
        self.prove_gap = wanted.len() as f64 / PROOFS_A_SECOND;

        wanted
    }

    /// The proofs these targets still need at the blocks they name, of those current: what is proven
    /// at a block already stays so, and what was proven at any other is let go of.
    fn not_proven_yet(&mut self, current: &[Hash], targets: Vec<(Column, Vec<u8>, Target, Vec<Hash>)>) -> Vec<Ask> {
        let current: Vec<String> = current.iter().map(ToString::to_string).collect();
        self.proven_at.retain(|(_, block)| current.contains(block));
        let mut asks = Vec::new();

        for (column, key, target, blocks) in targets {
            let key = to_hex(&key);
            self.targets.insert(key.clone(), target);

            for block in blocks {
                let block = block.to_string();

                if !self.proven_at.contains(&(key.clone(), block.clone())) {
                    asks.push(Ask::Proof {
                        column: column.name(),
                        key: key.clone(),
                        block,
                    });
                }
            }
        }

        asks
    }

    /// The proofs of names asked for, as the pages want them: each name's records, and every one
    /// above it, at the tip, asked again once they are PROVE_EVERY old. Names the account owns, and
    /// every account's primary name, are wanted too.
    pub fn names_due(&mut self, wanted: &[String], time: f64) -> Vec<Ask> {
        let (Some(names), Some(client)) = (&self.names, &self.client) else {
            return Vec::new();
        };

        if !self.caught_up() {
            return Vec::new();
        }

        let tip = client.tip().id().to_string();
        let mut asks = Vec::new();
        let primaries = self.primaries.values().filter_map(|proven| proven.value.clone());
        let mut every: Vec<String> = wanted.iter().map(|text| names.strip(text)).chain(self.owned.values().flatten().cloned()).chain(primaries).collect();
        every.sort();
        every.dedup();

        for name in every.into_iter().filter(|name| name_problem(name).is_none()) {
            if self.named.get(&name).is_some_and(|asked| time - asked < self.pace.every(PROVE_EVERY)) {
                continue;
            }

            self.named.insert(name.clone(), time);

            for above in lineage(&name) {
                let key = to_hex(&record_key(&above));
                self.targets.insert(key.clone(), Target::Name(above));
                asks.push(Ask::Proof {
                    column: Column::Names.name(),
                    key,
                    block: tip.clone(),
                });
            }
        }

        asks
    }

    /// The proofs of NFTs a page shows, at the tip: each NFT's owner, or each holder's amount of a
    /// multi-token id, asked again once PROVE_EVERY old.
    #[cfg_attr(not(feature = "nft"), allow(dead_code))]
    pub fn nfts_due(&mut self, wanted: &[Held], time: f64) -> Vec<Ask> {
        let Some(client) = self.client.as_ref().filter(|_| self.caught_up()) else {
            return Vec::new();
        };
        let tip = client.tip().id().to_string();
        let mut asks = Vec::new();

        for held in wanted {
            let (Ok(collection), Ok(id)) = (held.collection.parse::<Address>(), u256::from_str_radix(&held.id, 10)) else {
                continue;
            };
            let (key, target) = match held.multi {
                false => (nft_owner_key(&collection, id), Target::NftOwner { collection: held.collection.clone(), id: held.id.clone() }),
                true => {
                    let Ok(holder) = held.holder.parse::<Address>() else {
                        continue;
                    };
                    let target = Target::MultiBalance { collection: held.collection.clone(), id: held.id.clone(), holder: held.holder.clone() };

                    (multi_balance_key(&collection, id, &holder), target)
                }
            };
            let key = to_hex(&key);

            if self.nfts_asked.get(&key).is_some_and(|asked| time - asked < self.pace.every(PROVE_EVERY)) {
                continue;
            }

            self.nfts_asked.insert(key.clone(), time);
            self.targets.insert(key.clone(), target);
            asks.push(Ask::Proof { column: Column::Namespaces.name(), key, block: tip.clone() });
        }

        asks
    }

    /// A name looked up from what was proven of it, with or without the suffix: None until every
    /// record it rests on is proven. Judged at the time of the block they were proven at.
    pub fn resolve(&self, text: &str) -> Option<Resolved> {
        let names = self.names.as_ref()?;
        let client = self.client.as_ref()?;
        let name = names.strip(text);
        let mut records = Vec::new();
        let mut assurance: Option<Assurance> = None;

        for above in lineage(&name) {
            let proven = self.records.get(&above)?;
            records.push(proven.value.clone());

            if assurance.as_ref().is_none_or(|least| proven.assurance.height < least.height) {
                assurance = Some(proven.assurance);
            }

            // Nothing above a name that is not there matters.
            if proven.value.is_none() {
                break;
            }
        }

        let assurance = assurance?;
        let now = client.header_at(assurance.height)?.timestamp;

        Some(Resolved {
            found: standing(&records, now, names.grace_ms),
            assurance,
        })
    }

    /// An address's primary name with the suffix, proven, while it resolves to the address.
    pub fn primary(&self, address: &str) -> Option<String> {
        let names = self.names.as_ref()?;
        let chosen = self.primaries.get(address)?.value.clone()?;
        let resolved = self.resolve(&chosen)?;

        (resolved.target().map(|target| target.to_string()).as_deref() == Some(address)).then(|| names.display(&chosen))
    }

    /// What asks for the page after the last read of each of the node's indexes, for these
    /// addresses: what Activity's Older transactions asks. None once every list was read to its end,
    /// or before its newest page came.
    pub fn older_pages(&self, addresses: &[String]) -> Vec<Ask> {
        let next = |listed: &HashMap<String, FilteredTxs>, address: &String| listed.get(address).and_then(|listed| listed.next.clone());

        addresses
            .iter()
            .flat_map(|address| {
                let history = next(&self.by_address_index, address).map(|cursor| Ask::History { address: address.clone(), cursor: Some(cursor) });
                let events = next(&self.by_event_index, address).map(|cursor| Ask::Events { address: address.clone(), cursor: Some(cursor) });

                history.into_iter().chain(events)
            })
            .collect()
    }

    /// Takes an answer meant for the chain. Returns false for one that is not.
    pub fn take(&mut self, ask: &Ask, result: &Result<Value, Failure>, trust: Trust, now: u64, time: f64) -> bool {
        let mine = matches!(
            ask,
            Ask::Light
                | Ask::Headers { .. }
                | Ask::Ancestors { .. }
                | Ask::Finality { .. }
                | Ask::Bundle { .. }
                | Ask::Checkpoint(_)
                | Ask::Proof { .. }
                | Ask::Receipt(_)
                | Ask::SentReceipt(_)
                | Ask::Tokens(_)
                | Ask::History { .. }
                | Ask::Filtered { .. }
                | Ask::Finalized
                | Ask::Events { .. }
                | Ask::Names
                | Ask::OwnedNames { .. }
        );

        if !mine {
            return false;
        }

        let value = match (ask, result) {
            (_, Ok(value)) => value,
            // Kept in, with Read from checked blocks on: nothing is wrong with the way to the node,
            // and the light client has no cause to wait.
            (_, Err(failure)) if failure.status == crate::node::NEEDS_ASKING_STATUS => return true,
            // Not on the chain yet, or a node without the index asked of: nothing to show yet.
            (Ask::Receipt(_) | Ask::SentReceipt(_) | Ask::Proof { .. }, Err(failure)) if failure.status == 404 => return true,
            (Ask::History { .. } | Ask::Filtered { .. }, Err(failure)) if failure.status == 404 => {
                self.indexes_addresses = Some(false);

                return true;
            }
            // A node that will not say walks, as with no fast resume.
            (Ask::Finalized, Err(failure)) if failure.status != 0 => {
                self.walk();

                return true;
            }
            (Ask::Events { .. }, Err(failure)) if failure.status == 404 => {
                self.indexes_events = Some(false);

                return true;
            }
            (Ask::Names, Err(failure)) if failure.status == 404 => {
                self.has_names = Some(false);

                return true;
            }
            // The node's own checkpoint, on first use, is a shortcut: without one, as before
            // anything is final, the client follows from genesis.
            (Ask::Checkpoint(None), Err(failure)) if failure.status != 0 => {
                self.start_from_genesis();

                return true;
            }
            // A node from before the route: a request at a time does the same.
            (Ask::Bundle { .. }, Err(failure)) if failure.status != 0 => {
                self.no_bundle = true;

                return true;
            }
            (Ask::Checkpoint(Some(block)), Err(failure)) if failure.status != 0 => {
                self.trouble = Some(format!("the node cannot start the light client at checkpoint {block}: {}", failure.message));

                return true;
            }
            (_, Err(_)) => {
                self.retry_at = time + RETRY_AFTER;

                return true;
            }
        };

        match self.take_value(ask, value, trust, now) {
            Ok(()) => {
                if let Ask::Proof { key, block, .. } = ask {
                    self.proven_at.insert((key.clone(), block.clone()));
                }
            }
            Err(why) => match ask {
                Ask::Proof { key, .. } => {
                    self.refused.insert(key.clone(), why);
                }
                Ask::Receipt(tx) | Ask::SentReceipt(tx) => {
                    self.refused.insert(tx.clone(), why);
                }
                Ask::Tokens(_) | Ask::History { .. } | Ask::Filtered { .. } | Ask::Events { .. } | Ask::Names | Ask::OwnedNames { .. } => {}
                _ => self.trouble = Some(why),
            },
        }

        true
    }

    fn take_value(&mut self, ask: &Ask, value: &Value, trust: Trust, now: u64) -> Result<(), String> {
        match ask {
            Ask::Light => {
                let params: LightParams = decode(&value["params"])?;
                let header: SealedHeader = decode(&value["header"])?;
                let genesis = header.id();
                // Trust on first use: the genesis this node showed the first time, or one pasted.
                let first_use = !trust.pinned.contains_key(trust.node);
                let trusted = trust.pinned.entry(trust.node.to_string()).or_insert_with(|| genesis.to_string()).clone();

                if trusted != genesis.to_string() {
                    return Err(format!("the node is on chain {genesis}, not {trusted}, the one this wallet trusts there"));
                }

                let restored = trust
                    .saved
                    .get(&trusted)
                    .and_then(|hex| from_hex(hex))
                    .and_then(|bytes| wincode::deserialize::<LightState>(&bytes).ok())
                    .and_then(|state| LightClient::restore(state, genesis).ok());
                let pasted = trust.checkpoints.get(trust.node).and_then(|text| from_hex(text)).and_then(|bytes| Hash::from_bytes(params.hash, &bytes));
                let may_resume = may_resume(trust.fast_resume, params.finality);

                // On a node trusted for it, once it says how far it has finalized: what is decided
                // then, over what it saved, and a checkpoint pasted if it saved nothing. Otherwise
                // saved, it carries on. Else from a checkpoint pasted; else, met for the first time
                // with no genesis given, from its own finalized checkpoint, trusting it as much as
                // its genesis; else from genesis.
                match (restored, pasted, first_use) {
                    (restored, None, false) if may_resume => self.resuming = Some(Resuming { params, header, genesis, restored }),
                    (Some(restored), Some(_), false) if may_resume => self.resuming = Some(Resuming { params, header, genesis, restored: Some(restored) }),
                    (Some(client), _, _) => self.client = Some(client),
                    (None, Some(checkpoint), _) => self.starting = Some(Starting { params, header, genesis, checkpoint: Some(checkpoint), resuming: None }),
                    (None, None, true) => self.starting = Some(Starting { params, header, genesis, checkpoint: None, resuming: None }),
                    (None, None, false) => self.client = Some(LightClient::new(params, header, genesis).map_err(|err| err.to_string())?),
                }

                self.genesis = Some(genesis);
                self.dirty = true;

                Ok(())
            }
            Ask::Finalized => {
                let resuming = self.resuming.as_ref().ok_or("a finalized height nobody asked for")?;
                let saved = resuming.restored.as_ref().map(|client| client.finalized().epoch);

                match resume(true, resuming.params.finality, saved, value["finality"]["finalized"]["epoch"].as_u64()) {
                    Resume::Walk => self.walk(),
                    Resume::Checkpoint => {
                        if let Some(Resuming { params, header, genesis, restored }) = self.resuming.take() {
                            self.starting = Some(Starting { params, header, genesis, checkpoint: None, resuming: Some(restored) });
                        }
                    }
                }

                Ok(())
            }
            Ask::Checkpoint(_) => {
                let mut starting = self.starting.take().ok_or("a checkpoint nobody asked for")?;
                let block = value["checkpoint"]["block"]
                    .as_str()
                    .and_then(from_hex)
                    .and_then(|bytes| Hash::from_bytes(starting.params.hash, &bytes))
                    .ok_or("a checkpoint whose block does not read")?;
                let start: CheckpointStart = decode(&value["start"])?;

                // One pasted must be the one sent; the node's own is whichever it names.
                if starting.checkpoint.is_some_and(|checkpoint| checkpoint != block) {
                    return Err(format!("the node sent checkpoint {block}, not the one trusted"));
                }

                // Resuming goes forward only, and never against a block it proved final.
                let holds = |client: &LightClient| !matches!(&starting.resuming, Some(Some(saved)) if !goes_on_from(saved, client));

                match LightClient::from_checkpoint(starting.params.clone(), starting.header.clone(), starting.genesis, block, start).map(|client| (holds(&client), client)) {
                    Ok((false, _)) => {
                        self.starting = Some(starting);
                        self.start_from_genesis();
                    }
                    Ok((true, client)) => {
                        if let Some(saved) = starting.resuming.take() {
                            self.fast_resumed = Some(FastResumed { from: saved.map(|saved| saved.final_height()), at: client.final_height() });
                        }

                        self.client = Some(client);
                    }
                    Err(err) if starting.checkpoint.is_some() => return Err(format!("the checkpoint trusted does not check: {err}")),
                    // The node's own does not hold: following from genesis, or from what was saved,
                    // checks everything.
                    Err(_) => {
                        self.starting = Some(starting);
                        self.start_from_genesis();
                    }
                }

                self.dirty = true;

                Ok(())
            }
            Ask::Bundle { headers: from, epoch } => {
                let headers: Vec<SealedHeader> = list(&value["headers"])?;
                let steps: Vec<FinalityStep> = list(&value["steps"])?;
                let tip = value["tip"].as_u64().unwrap_or(0);
                self.dirty |= !headers.is_empty() || !steps.is_empty();

                self.with_client(|client| client.deliver_bundle(*from, &headers, *epoch, &steps, tip, now).map_err(|err| err.to_string()))
            }
            Ask::Headers { from, .. } => {
                let headers: Vec<SealedHeader> = list(&value["headers"])?;
                self.dirty |= !headers.is_empty();

                self.with_client(|client| client.deliver_headers(*from, headers, now).map(|_| ()).map_err(|err| err.to_string()))
            }
            Ask::Ancestors { from, .. } => {
                let headers: Vec<SealedHeader> = list(&value["headers"])?;

                self.with_client(|client| match client.deliver_ancestors(*from, headers) {
                    // Asked before an earlier stretch came in: the next ask starts lower.
                    Ok(_) | Err(LightError::Misplaced) => Ok(()),
                    Err(err) => Err(format!("the node's older headers do not check: {err}")),
                })
            }
            Ask::Finality { from, .. } => {
                let steps: Vec<FinalityStep> = list(&value["steps"])?;
                self.dirty |= !steps.is_empty();

                self.with_client(|client| client.deliver_finality(*from, steps).map(|_| ()).map_err(|err| err.to_string()))
            }
            Ask::Tokens(cursor) => {
                let page = value["tokens"].as_array().into_iter().flatten().filter_map(|token| token["address"].as_str().map(str::to_string));

                // A first page starts the listing over; each after it adds on, until the last.
                if cursor.is_none() {
                    self.listing.clear();
                }

                self.listing.extend(page);

                match value["next"].as_str() {
                    Some(next) => self.more.push(Ask::Tokens(Some(next.to_string()))),
                    None => self.listed = std::mem::take(&mut self.listing),
                }

                Ok(())
            }
            Ask::Names => {
                let config = NamesConfig {
                    suffix: value["suffix"].as_str().ok_or("names with no suffix")?.to_string(),
                    prices: value["prices"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|price| price.as_str().and_then(|text| u256::from_str_radix(text, 10).ok()).ok_or("a price that does not read"))
                        .collect::<Result<_, _>>()?,
                    grace_ms: value["grace_ms"].as_u64().ok_or("names with no grace_ms")?,
                    max_years: value["max_years"].as_u64().and_then(|years| u32::try_from(years).ok()).ok_or("names with no max_years")?,
                };

                if let Some(problem) = config.problem() {
                    return Err(format!("the node's names do not read: {problem}"));
                }

                self.names = Some(config);
                self.has_names = Some(true);

                Ok(())
            }
            Ask::OwnedNames { address, cursor } => {
                let page = value["names"].as_array().into_iter().flatten().filter_map(|name| name["name"].as_str().map(str::to_string));
                let owning = self.owning.entry(address.clone()).or_default();

                if cursor.is_none() {
                    owning.clear();
                }

                owning.extend(page);

                match value["next"].as_str() {
                    Some(next) => self.more.push(Ask::OwnedNames { address: address.clone(), cursor: Some(next.to_string()) }),
                    None => {
                        let owned = self.owning.remove(address).unwrap_or_default();
                        self.owned.insert(address.clone(), owned);
                    }
                }

                Ok(())
            }
            Ask::History { address, cursor } => {
                self.indexes_addresses = Some(true);
                self.by_address_index.entry(address.clone()).or_default().take(mentions(&value["txs"], "id"), value["next"].as_str().map(str::to_string), answered(value), cursor.is_some());
                self.mentions.insert(address.clone(), together(self.by_address_index.get(address), self.by_event_index.get(address)));

                Ok(())
            }
            Ask::Filtered { address, query, cursor } => {
                self.indexes_addresses = Some(true);
                // Only the filters in use: what another one found is asked again if it comes back.
                self.filtered.retain(|(_, kept), _| kept == query);
                self.filtered.entry((address.clone(), query.clone())).or_default().take(mentions(&value["txs"], "id"), value["next"].as_str().map(str::to_string), answered(value), cursor.is_some());

                Ok(())
            }
            Ask::Events { address, cursor } => {
                self.indexes_events = Some(true);
                self.by_event_index.entry(address.clone()).or_default().take(mentions(&value["events"], "tx"), value["next"].as_str().map(str::to_string), Answered::default(), cursor.is_some());
                self.mentions.insert(address.clone(), together(self.by_address_index.get(address), self.by_event_index.get(address)));

                Ok(())
            }
            Ask::Proof { column, key, .. } => {
                let target = self.targets.get(key).cloned().ok_or("a proof nobody asked for")?;
                let client = self.client.as_ref().ok_or("the light client has not started")?;
                let block = id(client, value["block"].as_str().unwrap_or_default()).ok_or("a proof at a block id that does not read")?;
                let column = [Column::Accounts, Column::Tokens, Column::Namespaces, Column::Names, Column::Validators, Column::Storage]
                    .into_iter()
                    .find(|known| known.name() == *column)
                    .ok_or("a proof of a column it never asks for")?;
                let raw_key = from_hex(key).ok_or("a key that is not hex")?;
                let held = value["value"].as_str().map(|text| from_hex(text).ok_or("a value that is not hex")).transpose()?;
                let proof: StateProof = decode(&value["proof"])?;
                let assurance = match client.check_state(&block, column, &raw_key, held.as_deref(), &proof) {
                    Ok(assurance) => assurance,
                    // At a block the client has not followed yet: proven at the next round.
                    Err(LightError::NotOnChain) => return Ok(()),
                    Err(err) => return Err(format!("the node's proof does not check: {err}")),
                };
                self.refused.remove(key);

                match target {
                    Target::Account(address) => {
                        let account: Account = match held {
                            Some(bytes) => wincode::deserialize(&bytes).map_err(|_| "an account that does not decode")?,
                            None => Account::default(),
                        };
                        keep(self.accounts.entry(address).or_default(), Proven { value: account, assurance });
                    }
                    Target::Token(token) => {
                        let Some(bytes) = held else {
                            self.tokens.remove(&token);

                            return Ok(());
                        };
                        let entry: Token = wincode::deserialize(&bytes).map_err(|_| "a token entry that does not decode")?;
                        self.tokens.insert(token, Proven { value: entry, assurance });
                    }
                    Target::Collection(collection) => {
                        let Some(bytes) = held else {
                            self.collections.remove(&collection);

                            return Ok(());
                        };
                        let entry: Collection = wincode::deserialize(&bytes).map_err(|_| "a collection entry that does not decode")?;

                        // An entry is kept by its address: one under another's key is no proof of this one.
                        if entry.address.to_string() != collection {
                            return Err("a collection entry for another address".to_string());
                        }

                        self.collections.insert(collection, Proven { value: entry, assurance });
                    }
                    Target::Balance { token, holder } => {
                        let balance = match held {
                            Some(bytes) => token_balance(&bytes).ok_or("a balance that does not read")?,
                            None => u256::ZERO,
                        };
                        keep(self.balances.entry((token, holder)).or_default(), Proven { value: balance, assurance });
                    }
                    Target::Name(name) => {
                        let record: Option<NameRecord> = held
                            .map(|bytes| wincode::deserialize(&bytes).map_err(|_| "a name record that does not decode"))
                            .transpose()?;
                        self.records.insert(name, Proven { value: record, assurance });
                    }
                    Target::Primary(address) => {
                        let chosen: Option<PrimaryName> = held
                            .map(|bytes| wincode::deserialize(&bytes).map_err(|_| "a primary name that does not decode"))
                            .transpose()?;
                        self.primaries.insert(address, Proven { value: chosen.map(|chosen| chosen.name), assurance });
                    }
                    Target::NftOwner { collection, id } => {
                        let owner = held.map(|bytes| nft_owner(&bytes).ok_or("an NFT owner that does not read")).transpose()?;
                        newest(&mut self.nft_owners, (collection, id), Proven { value: owner, assurance });
                    }
                    Target::MultiBalance { collection, id, holder } => {
                        let amount = match held {
                            Some(bytes) => token_balance(&bytes).ok_or("an amount that does not read")?,
                            None => u256::ZERO,
                        };
                        newest(&mut self.multi_balances, (collection, id, holder), Proven { value: amount, assurance });
                    }
                    Target::Delegation(address) => {
                        // 0xef0100 and the 20 bytes of the contract it runs, as Ethereum writes it.
                        let to = match held {
                            Some(code) => Some(code.strip_prefix(&[0xef, 0x01, 0x00][..]).and_then(|bytes| <[u8; 20]>::try_from(bytes).ok()).map(Address::Evm).ok_or("a delegation that does not read")?),
                            None => None,
                        };
                        self.delegations.insert(address, Proven { value: to, assurance });
                    }
                    Target::Directory => {
                        let directory: Option<Directory> = held
                            .map(|bytes| wincode::deserialize(&bytes).map_err(|_| "a directory that does not decode"))
                            .transpose()?;
                        self.directory = Some(Proven { value: directory, assurance });
                    }
                    Target::Federation => {
                        let federation: Option<DecidedFederation> = held
                            .map(|bytes| wincode::deserialize(&bytes).map_err(|_| "a federation that does not decode"))
                            .transpose()?;
                        self.federation = Some(Proven { value: federation, assurance });
                    }
                    Target::Policy(multisig) => {
                        let policy: Option<chainv4::registry::multisig::MultisigPolicy> = held
                            .map(|bytes| wincode::deserialize(&bytes).map_err(|_| "a multisig's policy that does not decode"))
                            .transpose()?;
                        self.policies.insert(multisig, Proven { value: policy, assurance });
                    }
                    Target::PoolRoot(height) => {
                        let root = held.map(|bytes| chainv4::registry::pools::Pools::decode_root(&bytes).ok_or("a pool's root that does not decode")).transpose()?;
                        self.pool_roots.insert(height, Proven { value: root, assurance });
                    }
                    Target::PoolClaimed { pool, miner } => {
                        let claimed = match held {
                            Some(bytes) => chainv4::registry::pools::Pools::decode_claimed(&bytes).ok_or("a claimed total that does not decode")?,
                            None => u256::ZERO,
                        };
                        self.pool_claimed.insert((pool, miner), Proven { value: claimed, assurance });
                    }
                    Target::Locks => {
                        let locks = match held {
                            Some(bytes) => chainv4::registry::changes::Changes::decode_locks(&bytes).ok_or("locks that do not decode")?,
                            None => Vec::new(),
                        };
                        self.locks = Some(Proven { value: locks, assurance });
                    }
                }

                Ok(())
            }
            Ask::Receipt(tx) | Ask::SentReceipt(tx) => {
                let client = self.client.as_ref().ok_or("the light client has not started")?;
                let block = id(client, value["block"].as_str().unwrap_or_default()).ok_or("a receipt at a block id that does not read")?;
                let bytes = |name: &str| from_hex(value[name].as_str().unwrap_or_default()).ok_or(format!("a {name} that is not hex"));
                let path = |name: &str| value[name].as_array().into_iter().flatten().filter_map(|sibling| id(client, sibling.as_str()?)).collect::<Vec<Hash>>();
                let index = value["index"].as_u64().ok_or("no index")? as usize;
                let count = value["count"].as_u64().ok_or("no count")? as usize;
                let (receipt, assurance) = match client.check_receipt(&block, &bytes("receipt")?, index, count, &path("path")) {
                    Ok(checked) => checked,
                    // In a block the client has not followed yet, or one below every header it
                    // holds, whose ancestors it takes first: proven at the next round.
                    Err(LightError::NotOnChain) => {
                        if let Some(height) = value["height"].as_u64().filter(|height| *height < client.lowest()) {
                            self.wanted_below = Some(self.wanted_below.map_or(height, |wanted| wanted.min(height)));
                        }

                        return Ok(());
                    }
                    Err(err) => return Err(format!("the node's receipt does not check: {err}")),
                };
                let (signed, _) = client
                    .check_tx(&block, &bytes("signed")?, index, count, &path("tx_path"))
                    .map_err(|err| format!("the node's transaction does not check: {err}"))?;

                if receipt.tx.to_string() != *tx || signed.id(client.params().hash) != receipt.tx {
                    return Err("a proof of another transaction".to_string());
                }

                let time = client.header_at(assurance.height).map_or(0, |header| header.timestamp);
                self.refused.remove(tx);
                self.receipts.insert(tx.clone(), Proven {
                    value: Landed { signed, receipt, time },
                    assurance,
                });

                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Starts the client waiting for the node's word on its checkpoint without one: on from what it
    /// saved, or from genesis.
    fn walk(&mut self) {
        let Some(Resuming { params, header, genesis, restored }) = self.resuming.take() else {
            return;
        };

        match restored {
            Some(client) => self.client = Some(client),
            None => match LightClient::new(params, header, genesis) {
                Ok(client) => self.client = Some(client),
                Err(err) => self.trouble = Some(err.to_string()),
            },
        }
    }

    /// What the client checked, to save, once it changed, and at most every SAVE_EVERY unless told
    /// now: a saved client is thousands of headers, far too much to write out with every answer.
    pub fn saved(&mut self, now: bool) -> Option<(String, String)> {
        let client = self.client.as_ref()?;

        if !self.dirty || (!now && self.last_saved.is_some_and(|saved| saved.elapsed() < SAVE_EVERY)) {
            return None;
        }

        self.dirty = false;
        self.last_saved = Some(web_time::Instant::now());
        let state = wincode::serialize(&client.save()).ok()?;

        Some((self.genesis?.to_string(), to_hex(&state)))
    }

    /// The newest proven account, and whether a final one agrees with it.
    pub fn account(&self, address: &str) -> Option<&Proven<Account>> {
        self.accounts.get(address)?.last()
    }

    /// The newest proven balance of a token.
    pub fn balance(&self, token: &str, holder: &str) -> Option<&Proven<u256>> {
        self.balances.get(&(token.to_string(), holder.to_string()))?.last()
    }

    /// Takes a page of the registry's list of collections, as GET /collections answered: which to
    /// prove, and what the NFTs page lists. The first page starts the listing over, each after it
    /// adds on, the next is asked as soon as one comes, and what was listed stands until the last.
    pub fn list_collections(&mut self, later: bool, result: &Result<Value, Failure>) {
        let Ok(value) = result else {
            return;
        };

        if !later {
            self.collections_listing.clear();
        }

        self.collections_listing.extend(value["collections"].as_array().into_iter().flatten().cloned());

        match value["next"].as_str() {
            Some(next) => self.more.push(Ask::Collections(Some(next.to_string()))),
            None => {
                self.collection_entries = std::mem::take(&mut self.collections_listing);
                self.listed_collections = self.collection_entries.iter().filter_map(|found| found["address"].as_str().map(str::to_string)).collect();
            }
        }
    }

    /// What the chain's registries say a contract is, as proven: a token, a collection, or
    /// neither as far as the wallet knows.
    pub fn known(&self, contract: &Address) -> Option<Known<'_>> {
        let text = contract.to_string();

        match (self.tokens.get(&text), self.collections.get(&text)) {
            (Some(token), _) => Some(Known::Token(&token.value)),
            (None, Some(collection)) => Some(Known::Collection(&collection.value)),
            (None, None) => None,
        }
    }

    /// The proven final account, when the newest is not final.
    pub fn final_account(&self, address: &str) -> Option<&Proven<Account>> {
        self.accounts.get(address)?.iter().rev().find(|proven| proven.assurance.final_)
    }

    fn with_client(&mut self, take: impl FnOnce(&mut LightClient) -> Result<(), String>) -> Result<(), String> {
        match &mut self.client {
            Some(client) => take(client),
            None => Err("the light client has not started".to_string()),
        }
    }
}

/// Keeps a proven value with the others of its kind: the final one and the newest, by height.
fn keep<T>(list: &mut Vec<Proven<T>>, proven: Proven<T>) {
    list.retain(|known| known.assurance.block != proven.assurance.block);
    list.push(proven);
    list.sort_by_key(|known| known.assurance.height);
    let newest_final = list.iter().rposition(|known| known.assurance.final_);
    let newest = list.len() - 1;
    let mut position = 0;
    list.retain(|_| {
        let keep = Some(position) == newest_final || position == newest;
        position += 1;
        keep
    });
}

/// Whether a client taking up the chain at a checkpoint goes on from one that saved: its checkpoint
/// further on than what was proven final, and agreeing with that block if it reaches back to it.
fn goes_on_from(saved: &LightClient, client: &LightClient) -> bool {
    let height = saved.final_height();
    let agrees = match (saved.header_at(height), client.header_at(height)) {
        (Some(proven), Some(theirs)) => proven.id() == theirs.id(),
        _ => true,
    };

    client.final_height() > height && agrees
}

/// What an answer to GET /accounts/{address}/txs says of itself: an older node says nothing.
fn answered(value: &Value) -> Answered {
    Answered {
        filtered: value["applied"].is_array(),
        scanned: value["scanned"].as_u64().unwrap_or(0),
        limited: value["limited"].as_bool().unwrap_or(false),
    }
}

/// Where the node said to look: each listed transaction's id and height.
fn mentions(list: &Value, id_field: &str) -> Vec<Mention> {
    let mut found: Vec<Mention> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(Mention {
                tx: entry[id_field].as_str()?.to_string(),
                height: entry["height"].as_u64()?,
            })
        })
        .collect();
    // An event listing names a transaction once for each of its events.
    found.dedup_by(|later, earlier| later.tx == earlier.tx);

    found
}

/// What both of a node's indexes list of an address, newest first, each transaction once.
fn together(first: Option<&FilteredTxs>, second: Option<&FilteredTxs>) -> Vec<Mention> {
    let mut all: Vec<Mention> = first.into_iter().chain(second).flat_map(|listed| &listed.mentions).cloned().collect();
    let mut seen = std::collections::HashSet::new();

    all.sort_by_key(|mention| std::cmp::Reverse(mention.height));
    all.retain(|mention| seen.insert(mention.tx.clone()));

    all
}

fn id(client: &LightClient, text: &str) -> Option<Hash> {
    Hash::from_bytes(client.params().hash, &from_hex(text)?)
}

fn decode<T>(value: &Value) -> Result<T, String>
where
    T: for<'de> wincode::SchemaRead<'de, wincode::config::DefaultConfig, Dst = T>,
{
    let bytes = from_hex(value.as_str().ok_or("nothing to decode")?).ok_or("something that is not hex")?;

    wincode::deserialize(&bytes).map_err(|_| "something that does not decode".to_string())
}

fn list<T>(value: &Value) -> Result<Vec<T>, String>
where
    T: for<'de> wincode::SchemaRead<'de, wincode::config::DefaultConfig, Dst = T>,
{
    value.as_array().ok_or("no list")?.iter().map(decode).collect()
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::node::Node,
        std::time::{Duration, Instant},
    };

    use {
        chainv4::consensus::ConsensusConfig,
        chainv4::crypto::hashable::Hashable,
        chainv4::light::{FEW_EPOCHS, LightFinality},
        chainv4::crypto::keypair::Keypair,
        chainv4::structs::block::{BlockHeader, Seal},
        chainv4::structs::hash::HashType,
        serde_json::json,
    };

    /// A header of a made-up chain, linked to the one below: nothing here checks their seals, and
    /// the tests that do are the light client's own.
    fn header(height: u64, parent: Hash) -> SealedHeader {
        let zero = Hash::zero(HashType::default_chain());

        SealedHeader {
            header: BlockHeader {
                parent,
                height,
                timestamp: height * 1_000,
                producer: Address::Ed25519([0; 32]),
                difficulty: 0,
                tx_root: zero,
                proof_root: zero,
                pool_root: zero,
                receipts_root: zero,
                state_root: zero,
                gas_used: 0,
                base_price: u256::ZERO,
            },
            seal: Seal::None,
        }
    }

    const VOTES: LightFinality = LightFinality::Authority { checkpoint_interval: 8, fixed_ahead: 2 };

    fn params(finality: Option<LightFinality>) -> LightParams {
        LightParams {
            hash: HashType::default_chain(),
            consensus: ConsensusConfig::Authority { key: Keypair::from_seed([7; 32]).public(), slot_ms: 1_000 },
            finality,
            pow_window: None,
            governance: None,
        }
    }

    fn encoded<T: wincode::SchemaWrite<wincode::config::DefaultConfig, Src = T>>(value: &T) -> String {
        to_hex(&wincode::serialize(value).unwrap())
    }

    /// A made-up chain the wallet resumes on, as far as its answers go.
    #[test]
    fn nothing_is_proven_twice_at_one_block_and_a_new_tip_asks_only_for_itself() {
        let block = |byte: u8| Hash::from_bytes(HashType::default_chain(), &[byte; 32]).unwrap();
        let (last_final, tip, next) = (block(1), block(2), block(3));
        let targets = |blocks: &[Hash]| vec![(Column::Accounts, vec![9], Target::Account("a".to_string()), blocks.to_vec())];
        let mut chain = Chain::default();

        let asks = chain.not_proven_yet(&[last_final, tip], targets(&[last_final, tip]));
        assert_eq!(asks.len(), 2, "at the final block and the tip");

        // Asked again before it was proven, as a failed one would be.
        assert_eq!(chain.not_proven_yet(&[last_final, tip], targets(&[last_final, tip])).len(), 2);

        for ask in &asks {
            let Ask::Proof { key, block, .. } = ask else { unreachable!() };
            chain.proven_at.insert((key.clone(), block.clone()));
        }

        assert!(chain.not_proven_yet(&[last_final, tip], targets(&[last_final, tip])).is_empty(), "both held");

        let asks = chain.not_proven_yet(&[last_final, next], targets(&[last_final, next]));
        assert_eq!(asks.len(), 1, "the new tip alone: the final block has not moved");
        assert!(matches!(&asks[0], Ask::Proof { block, .. } if *block == next.to_string()));
        assert_eq!(chain.proven_at.len(), 1, "what the old tip held is let go of");
    }

    #[test]
    fn with_the_nodes_stream_open_it_asks_when_told_and_otherwise_seldom() {
        let mut chain = Chain { followed: 100.0, ..Chain::default() };
        assert!(chain.follow_due(100.0 + FOLLOW_EVERY), "with no stream, it asks at its turn");

        chain.streamed = true;
        assert!(!chain.follow_due(100.0 + FOLLOW_EVERY), "the stream will say");
        assert!(chain.follow_due(100.0 + FOLLOW_STREAMED), "but in case an event is missed");

        chain.nudge();
        assert!(chain.follow_due(100.0), "it said");
    }

    #[test]
    fn with_low_data_on_it_follows_a_quarter_as_often_and_still_at_once_when_the_stream_says() {
        let mut chain = Chain { followed: 100.0, pace: chainv4_ui::pace::Pace::new(true), ..Chain::default() };

        assert!(!chain.follow_due(100.0 + FOLLOW_EVERY), "not at the usual turn");
        assert!(chain.follow_due(100.0 + 4.0 * FOLLOW_EVERY), "four times later");

        chain.streamed = true;
        assert!(!chain.follow_due(100.0 + FOLLOW_STREAMED));
        assert!(chain.follow_due(100.0 + 4.0 * FOLLOW_STREAMED));

        chain.nudge();
        assert!(chain.follow_due(100.0), "what the stream tells is not held back");
    }

    struct Resumes {
        chain: Chain,
        finality: Option<LightFinality>,
        genesis: SealedHeader,
        saved: HashMap<String, String>,
        trusted: bool,
    }

    impl Resumes {
        fn new(finality: Option<LightFinality>, trusted: bool) -> Resumes {
            let mut genesis = header(0, Hash::zero(HashType::default_chain()));
            let params = params(finality);
            genesis.header.proof_root = params.hash(params.hash);

            Resumes { chain: Chain::default(), finality, genesis, saved: HashMap::new(), trusted }
        }

        /// What it saved: a client that took up the chain at the checkpoint opening this epoch.
        fn saved_at(mut self, epoch: u64) -> Resumes {
            let genesis = self.genesis.id();
            let client = match epoch {
                0 => LightClient::new(params(self.finality), self.genesis.clone(), genesis).unwrap(),
                epoch => {
                    let (block, start) = self.start(epoch);

                    LightClient::from_checkpoint(params(self.finality), self.genesis.clone(), genesis, block, start).unwrap()
                }
            };
            self.saved.insert(genesis.to_string(), to_hex(&wincode::serialize(&client.save()).unwrap()));

            self
        }

        /// What a node starts a client from at the checkpoint opening this epoch.
        fn start(&self, epoch: u64) -> (Hash, CheckpointStart) {
            let height = epoch * 8;
            let (_, from) = chainv4::light::checkpoint_start(self.finality, height).unwrap();
            let mut headers = vec![header(from, self.genesis.id())];

            for at in from + 1..=height {
                let below = headers.last().unwrap().id();
                headers.push(header(at, below));
            }

            (headers.last().unwrap().id(), CheckpointStart { updates: vec![None; ((height - from) / 8 + 1) as usize], headers })
        }

        fn answer(&mut self, ask: Ask, answer: Result<Value, Failure>) {
            let mut pinned = HashMap::from([("http://node".to_string(), self.genesis.id().to_string())]);
            let checkpoints = HashMap::new();
            let trust = Trust { node: "http://node", pinned: &mut pinned, checkpoints: &checkpoints, saved: &self.saved, fast_resume: self.trusted };

            assert!(self.chain.take(&ask, &answer, trust, u64::MAX / 2, 0.0));
        }

        /// Asks where to start, as the wallet does, and answers.
        fn light(&mut self) {
            assert_eq!(self.chain.next_ask(0.0), Some(Ask::Light));
            let answer = json!({ "params": encoded(&params(self.finality)), "header": encoded(&self.genesis) });
            self.answer(Ask::Light, Ok(answer));
        }

        fn finalized_at(&mut self, epoch: u64) {
            assert_eq!(self.chain.next_ask(0.0), Some(Ask::Finalized));
            self.answer(Ask::Finalized, Ok(json!({ "finality": { "finalized": { "epoch": epoch, "height": epoch * 8 } } })));
        }

        fn checkpoint_at(&mut self, epoch: u64) {
            assert_eq!(self.chain.next_ask(0.0), Some(Ask::Checkpoint(None)));
            let (block, start) = self.start(epoch);
            let answer = json!({ "checkpoint": { "block": block.to_string() }, "start": encoded(&start) });
            self.answer(Ask::Checkpoint(None), Ok(answer));
        }

        /// Answers what comes before a client wants the chain's headers.
        fn walk_to_a_bundle(&mut self) {
            for _ in 0..4 {
                match self.chain.next_ask(0.0) {
                    Some(Ask::Bundle { .. }) | None => return,
                    Some(Ask::Light) => self.light(),
                    Some(other) => panic!("{other:?}"),
                }
            }
        }

        fn walks_on_from(&mut self, height: u64) {
            assert_eq!(self.chain.following().map(|(tip, _)| tip), Some(height), "it took up the chain where it got to");
            assert_eq!(self.chain.fast_resumed, None);
            assert!(self.chain.trouble.is_none(), "{:?}", self.chain.trouble);
            assert!(matches!(self.chain.next_ask(0.0), Some(Ask::Bundle { headers, .. }) if headers == height + 1));
        }
    }

    fn failed(status: u16) -> Result<Value, Failure> {
        Err(Failure { status, message: "no".to_string() })
    }

    #[test]
    fn a_node_without_the_bundle_route_is_asked_a_request_at_a_time() {
        let mut resumes = Resumes::new(Some(VOTES), false).saved_at(5);
        resumes.walk_to_a_bundle();
        let ask = resumes.chain.next_ask(0.0).unwrap();

        assert!(matches!(ask, Ask::Bundle { .. }), "{ask:?}");
        resumes.answer(ask, failed(404));
        assert!(resumes.chain.trouble.is_none(), "{:?}", resumes.chain.trouble);
        assert!(matches!(resumes.chain.next_ask(0.0), Some(Ask::Headers { .. } | Ask::Finality { .. })));
    }

    #[test]
    fn the_dot_says_the_node_is_down_whatever_the_light_client_last_knew_and_its_own_state_otherwise() {
        let palette = chainv4_ui::DARK;
        let down = Failure { status: 0, message: "connection refused".to_string() };
        let mut resumes = Resumes::new(Some(VOTES), true).saved_at(5);
        resumes.light();
        resumes.finalized_at(100);
        resumes.checkpoint_at(100);

        let (color, text, _) = crate::views::following(&palette, &resumes.chain, None);
        assert_eq!((color, text.as_str()), (palette.warn, "Catching up · 800"), "still walking to the tip");

        let (color, text, detail) = crate::views::following(&palette, &resumes.chain, Some(&down));
        assert_eq!((color, text.as_str()), (palette.bad, "Node not answering"));
        assert!(detail.contains("connection refused") && detail.contains("800"), "why, and what shows: {detail}");
        assert!(detail.contains("Fast-resumed at block 800, on this node's word"), "and it still says so: {detail}");

        // Stopped by what the node sent is the graver: it stands.
        resumes.chain.trouble = Some("a header that does not check".to_string());
        let (color, text, detail) = crate::views::following(&palette, &resumes.chain, Some(&down));
        assert_eq!((color, text.as_str(), detail.as_str()), (palette.bad, "Stopped", "a header that does not check"));

        // Nothing heard of yet.
        let (color, text, _) = crate::views::following(&palette, &Chain::default(), None);
        assert_eq!((color, text.as_str()), (palette.muted, "Starting"));
    }

    #[test]
    fn with_the_switch_off_it_walks_on_from_what_it_saved_as_ever_however_far_behind() {
        let mut resumes = Resumes::new(Some(VOTES), false).saved_at(5);

        resumes.light();
        resumes.walks_on_from(40);
    }

    #[test]
    fn far_behind_on_a_trusted_node_it_takes_up_the_chain_at_the_nodes_checkpoint_and_follows_on_from_there() {
        let mut resumes = Resumes::new(Some(VOTES), true).saved_at(5);

        resumes.light();
        resumes.finalized_at(100);
        resumes.checkpoint_at(100);

        assert_eq!(resumes.chain.following(), Some((800, 800)));
        assert_eq!(resumes.chain.fast_resumed, Some(FastResumed { from: Some(40), at: 800 }));
        assert!(resumes.chain.trouble.is_none(), "{:?}", resumes.chain.trouble);

        // On from there as any client, headers first; and it saves, to resume from next time.
        assert!(matches!(resumes.chain.next_ask(0.0), Some(Ask::Bundle { headers: 801, .. })));

        let (genesis, kept) = resumes.chain.saved(true).expect("it changed");
        let state: LightState = wincode::deserialize(&from_hex(&kept).unwrap()).unwrap();

        assert_eq!(genesis, resumes.genesis.id().to_string());
        assert_eq!(state.finalized.epoch, 100);
    }

    #[test]
    fn a_few_epochs_behind_on_a_trusted_node_it_walks_with_no_checkpoint_asked_for() {
        let mut resumes = Resumes::new(Some(VOTES), true).saved_at(95);

        resumes.light();
        resumes.finalized_at(95 + FEW_EPOCHS);
        resumes.walks_on_from(95 * 8);
    }

    #[test]
    fn under_depth_finality_or_none_it_walks_and_asks_nothing_of_the_nodes_finality() {
        for finality in [Some(LightFinality::Depth { depth: 10 }), None] {
            let mut resumes = Resumes::new(finality, true).saved_at(0);

            resumes.light();

            assert!(matches!(resumes.chain.next_ask(0.0), Some(Ask::Headers { from: 1, .. })), "{finality:?}");
            assert_eq!(resumes.chain.fast_resumed, None);
        }
    }

    #[test]
    fn a_node_with_no_checkpoint_to_give_leaves_it_walking_on_from_what_it_saved() {
        // It says of no finality.
        let mut resumes = Resumes::new(Some(VOTES), true).saved_at(5);
        resumes.light();
        assert_eq!(resumes.chain.next_ask(0.0), Some(Ask::Finalized));
        resumes.answer(Ask::Finalized, Ok(json!({ "finality": null })));
        resumes.walks_on_from(40);

        // It names one, then has none to start from.
        let mut resumes = Resumes::new(Some(VOTES), true).saved_at(5);
        resumes.light();
        resumes.finalized_at(100);
        assert_eq!(resumes.chain.next_ask(0.0), Some(Ask::Checkpoint(None)));
        resumes.answer(Ask::Checkpoint(None), failed(404));
        resumes.walks_on_from(40);

        // An older node, with no such thing as this route.
        let mut resumes = Resumes::new(Some(VOTES), true).saved_at(5);
        resumes.light();
        assert_eq!(resumes.chain.next_ask(0.0), Some(Ask::Finalized));
        resumes.answer(Ask::Finalized, failed(404));
        resumes.walks_on_from(40);
    }

    #[test]
    fn with_nothing_saved_and_nothing_to_resume_from_it_follows_from_genesis() {
        let mut resumes = Resumes::new(Some(VOTES), true);

        resumes.light();
        resumes.finalized_at(100);
        assert_eq!(resumes.chain.next_ask(0.0), Some(Ask::Checkpoint(None)));
        resumes.answer(Ask::Checkpoint(None), failed(404));

        assert_eq!(resumes.chain.following().map(|(tip, _)| tip), Some(0));
        assert_eq!(resumes.chain.fast_resumed, None);
    }

    #[test]
    fn the_node_out_of_reach_is_asked_again_and_nothing_else_changes() {
        let mut resumes = Resumes::new(Some(VOTES), true).saved_at(5);

        resumes.light();
        assert_eq!(resumes.chain.next_ask(0.0), Some(Ask::Finalized));
        resumes.answer(Ask::Finalized, failed(0));

        assert!(resumes.chain.client.is_none());
        assert_eq!(resumes.chain.next_ask(0.0), None, "held for a moment");
        assert_eq!(resumes.chain.next_ask(RETRY_AFTER), Some(Ask::Finalized));

        resumes.answer(Ask::Finalized, Ok(json!({ "finality": { "finalized": { "epoch": 100 } } })));
        assert_eq!(resumes.chain.next_ask(RETRY_AFTER), Some(Ask::Checkpoint(None)));
        resumes.answer(Ask::Checkpoint(None), failed(0));
        assert!(resumes.chain.client.is_none());
        assert_eq!(resumes.chain.next_ask(RETRY_AFTER * 2.0), Some(Ask::Checkpoint(None)), "asked again, not given up on");
    }

    #[test]
    fn the_genesis_pinned_still_stops_a_trusted_node_on_another_chain() {
        let mut resumes = Resumes::new(Some(VOTES), true).saved_at(5);
        let mut pinned = HashMap::from([("http://node".to_string(), Hash::zero(HashType::default_chain()).to_string())]);
        let (checkpoints, saved) = (HashMap::new(), resumes.saved.clone());
        let answer = json!({ "params": encoded(&params(Some(VOTES))), "header": encoded(&resumes.genesis) });
        let trust = Trust { node: "http://node", pinned: &mut pinned, checkpoints: &checkpoints, saved: &saved, fast_resume: true };

        resumes.chain.take(&Ask::Light, &Ok(answer), trust, 0, 0.0);

        assert!(resumes.chain.trouble.as_deref().is_some_and(|why| why.contains("not ")), "{:?}", resumes.chain.trouble);
        assert!(resumes.chain.client.is_none());
        assert_eq!(resumes.chain.next_ask(100.0), None, "and it stops there");
    }

    #[test]
    fn a_checkpoint_behind_what_it_proved_final_is_not_gone_back_to() {
        let mut resumes = Resumes::new(Some(VOTES), true).saved_at(50);

        resumes.light();
        // The node says 100 and sends 40, by mistake or not.
        resumes.finalized_at(100);
        resumes.checkpoint_at(40);
        resumes.walks_on_from(400);
    }

    #[test]
    fn a_checkpoint_at_odds_with_a_block_it_proved_final_is_not_taken() {
        let resumes = Resumes::new(Some(VOTES), true);
        let genesis = resumes.genesis.id();
        // Headers linked from one with this parent, at these heights.
        let linked = |heights: std::ops::RangeInclusive<u64>, mut below: Hash| -> Vec<SealedHeader> {
            heights.map(|height| {
                let next = header(height, below);
                below = next.id();

                next
            }).collect()
        };
        let client = |headers: Vec<SealedHeader>| {
            let block = headers.last().unwrap().id();

            LightClient::from_checkpoint(params(Some(VOTES)), resumes.genesis.clone(), genesis, block, CheckpointStart { updates: vec![None; 2], headers }).unwrap()
        };
        let proven = linked(32..=40, genesis);
        let saved = client(proven.clone());
        let on_from_it = client(linked(40..=40, proven[7].id()).into_iter().chain(linked(41..=48, proven[8].id())).collect());
        let elsewhere = client(linked(40..=48, Hash::zero(HashType::default_chain())));

        assert_eq!((saved.final_height(), on_from_it.final_height(), elsewhere.final_height()), (40, 48, 48));
        assert!(goes_on_from(&saved, &on_from_it));
        assert!(!goes_on_from(&saved, &elsewhere), "another block at the height it proved final");
        assert!(!goes_on_from(&on_from_it, &saved), "and never back");
    }

    #[test]
    fn filtered_pages_stand_for_the_list_until_more_are_read_then_only_add_what_is_new() {
        let mention = |tx: &str, height: u64| Mention { tx: tx.to_string(), height };
        let answered = Answered { filtered: true, ..Answered::default() };
        let mut held = FilteredTxs::default();

        held.take(vec![mention("0x05", 5), mention("0x04", 4)], Some("c1".to_string()), answered.clone(), false);
        held.take(vec![mention("0x06", 6), mention("0x05", 5)], Some("c2".to_string()), answered.clone(), false);

        assert_eq!(held.mentions.iter().map(|mention| mention.height).collect::<Vec<_>>(), [6, 5], "a first page alone is the list");
        assert_eq!((held.next.as_deref(), held.pages), (Some("c2"), 1));

        held.take(vec![mention("0x03", 3), mention("0x05", 5)], None, Answered { limited: true, ..answered.clone() }, true);

        assert_eq!(held.mentions.iter().map(|mention| mention.height).collect::<Vec<_>>(), [6, 5, 3], "each once");
        assert_eq!((held.next.clone(), held.answered.limited), (None, true));

        held.take(vec![mention("0x07", 7), mention("0x06", 6)], Some("c3".to_string()), answered.clone(), false);

        assert_eq!(held.mentions.iter().map(|mention| mention.height).collect::<Vec<_>>(), [7, 6, 5, 3], "newer ones go in front");
        assert_eq!(held.next, None, "where it had read to stands");
    }

    #[test]
    fn the_token_registry_and_owned_names_are_read_a_page_at_a_time_to_the_end() {
        let mut chain = Chain::default();
        let mut pinned = HashMap::new();
        let (checkpoints, saved) = (HashMap::new(), HashMap::new());
        let mut take = |chain: &mut Chain, ask: Ask, answer: Value| {
            let trust = Trust { node: "http://node", pinned: &mut pinned, checkpoints: &checkpoints, saved: &saved, fast_resume: false };

            assert!(chain.take(&ask, &Ok(answer), trust, 0, 0.0));
        };
        let tokens = |addresses: &[&str], next: Option<&str>| json!({ "tokens": addresses.iter().map(|address| json!({ "address": address })).collect::<Vec<_>>(), "next": next });

        chain.listed = vec!["contract:0x01".to_string()];
        take(&mut chain, Ask::Tokens(None), tokens(&["contract:0x02"], Some("0xaa")));

        assert_eq!(chain.listed, ["contract:0x01"], "what was listed stands until the last page comes");
        assert_eq!(chain.proofs_due(&[], 0.0), [Ask::Tokens(Some("0xaa".to_string()))], "the next page goes out at once");

        take(&mut chain, Ask::Tokens(Some("0xaa".to_string())), tokens(&["contract:0x03"], None));
        assert_eq!(chain.listed, ["contract:0x02", "contract:0x03"], "past the first page");

        let names = |listed: &[&str], next: Option<&str>| json!({ "names": listed.iter().map(|name| json!({ "name": name })).collect::<Vec<_>>(), "next": next });
        let address = "ed25519:0x01".to_string();
        take(&mut chain, Ask::OwnedNames { address: address.clone(), cursor: None }, names(&["alice"], Some("alice")));
        assert_eq!(chain.proofs_due(&[], 0.0), [Ask::OwnedNames { address: address.clone(), cursor: Some("alice".to_string()) }]);
        take(&mut chain, Ask::OwnedNames { address: address.clone(), cursor: Some("alice".to_string()) }, names(&["bob"], None));

        assert_eq!(chain.owned[&address], ["alice", "bob"]);
        assert!(chain.more.is_empty(), "nothing after the last");

        // The collection registry the same way, for its proofs and the NFTs page both.
        let collections = |addresses: &[&str], next: Option<&str>| Ok(json!({ "collections": addresses.iter().map(|address| json!({ "address": address, "kind": "nft" })).collect::<Vec<_>>(), "next": next }));
        chain.list_collections(false, &collections(&["contract:0xaa"], None));
        chain.list_collections(false, &collections(&["contract:0xbb"], Some("0x01")));

        assert_eq!(chain.listed_collections, ["contract:0xaa"], "until the last page comes");
        assert_eq!(chain.proofs_due(&[], 0.0), [Ask::Collections(Some("0x01".to_string()))]);

        chain.list_collections(true, &collections(&["contract:0xcc"], None));
        assert_eq!(chain.listed_collections, ["contract:0xbb", "contract:0xcc"]);
        assert_eq!(chain.collection_entries.len(), 2, "each entry whole, for the NFTs page");
    }

    #[test]
    fn an_older_nodes_listing_says_nothing_of_filters_and_a_new_ones_does() {
        assert_eq!(answered(&json!({ "txs": [], "next": null, "indexed_from": 0 })), Answered::default());
        assert_eq!(answered(&json!({ "txs": [], "applied": [], "scanned": 7, "limited": false })), Answered { filtered: true, scanned: 7, limited: false });
        assert_eq!(answered(&json!({ "txs": [], "applied": ["kind"], "scanned": 1000, "limited": true })), Answered { filtered: true, scanned: 1000, limited: true });
    }

    /// Drives the chain against the node until it is caught up, answering each ask, or until it
    /// stops on trouble. Returns the asks it made, in order.
    fn follow(chain: &mut Chain, node: &str, pinned: &mut HashMap<String, String>, checkpoints: &HashMap<String, String>) -> Vec<Ask> {
        let mut link = Node::new(node, Ok(chainv4_ui::net::Client::direct()), egui::Context::default());
        let saved = HashMap::new();
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut asked = Vec::new();

        while !chain.caught_up() && chain.trouble.is_none() {
            assert!(Instant::now() < deadline, "never caught up; asked {asked:?}");
            let Some(ask) = chain.next_ask(0.0) else {
                break;
            };
            asked.push(ask.clone());
            link.ask(ask);

            let answers = loop {
                let answers = link.answers();

                if !answers.is_empty() {
                    break answers;
                }

                std::thread::sleep(Duration::from_millis(5));
            };

            for (ask, result) in answers {
                let trust = Trust { node, pinned, checkpoints, saved: &saved, fast_resume: false };
                chain.take(&ask, &result, trust, u64::MAX / 2, 0.0);
            }
        }

        asked
    }

    #[test]
    #[ignore = "needs a node with voting finality: CHAINV4_NODE=http://127.0.0.1:8080 cargo test -- --ignored"]
    fn a_light_client_starts_where_the_trust_it_was_given_says() {
        let node = std::env::var("CHAINV4_NODE").expect("CHAINV4_NODE names a node");
        let status: Value = ureq_get(&format!("{node}/status"));
        let finalized = &status["finality"]["finalized"];
        let (checkpoint, height) = (finalized["block"].as_str().unwrap().to_string(), finalized["height"].as_u64().unwrap());
        let genesis = status["genesis"].as_str().unwrap().to_string();
        assert!(height > 0, "the node has finalized nothing yet");

        // Met for the first time, with nothing given: from the node's own finalized checkpoint.
        let mut chain = Chain::default();
        let mut pinned = HashMap::new();
        let asked = follow(&mut chain, &node, &mut pinned, &HashMap::new());

        assert_eq!(asked[..2], [Ask::Light, Ask::Checkpoint(None)]);
        assert!(chain.client.as_ref().unwrap().lowest() > 0, "it never followed from genesis");
        assert!(chain.following().unwrap().0 >= height);
        assert_eq!(pinned.get(&node), Some(&genesis), "and pinned the genesis as before");

        // A genesis given, as pasted: from genesis, checking every block.
        let mut chain = Chain::default();
        let mut given = HashMap::from([(node.clone(), genesis.clone())]);
        let asked = follow(&mut chain, &node, &mut given, &HashMap::new());

        assert!(!asked.iter().any(|ask| matches!(ask, Ask::Checkpoint(_))));
        assert_eq!(chain.client.as_ref().unwrap().lowest(), 0);

        // A checkpoint given: from there, whatever else.
        let mut chain = Chain::default();
        let at = HashMap::from([(node.clone(), checkpoint.clone())]);
        let asked = follow(&mut chain, &node, &mut given, &at);

        assert_eq!(asked[1], Ask::Checkpoint(Some(checkpoint.clone())));
        assert!(chain.trouble.is_none(), "{:?}", chain.trouble);

        // And a block given that is no checkpoint stops it, rather than following from genesis.
        let past: Value = ureq_get(&format!("{node}/blocks/{}", height + 1));
        let mut chain = Chain::default();
        let wrong = HashMap::from([(node.clone(), past["id"].as_str().unwrap().to_string())]);
        follow(&mut chain, &node, &mut given, &wrong);

        assert!(chain.trouble.as_deref().is_some_and(|trouble| trouble.contains("cannot start the light client")), "{:?}", chain.trouble);
        assert!(chain.client.is_none());
    }

    /// Asks each of these, and gives the chain every answer.
    fn answer(chain: &mut Chain, node: &str, pinned: &mut HashMap<String, String>, asks: Vec<Ask>) {
        let mut link = Node::new(node, Ok(chainv4_ui::net::Client::direct()), egui::Context::default());
        let (saved, checkpoints) = (HashMap::new(), HashMap::new());
        let mut waiting = asks.len();

        for ask in asks {
            link.ask(ask);
        }

        let deadline = Instant::now() + Duration::from_secs(20);

        while waiting > 0 && Instant::now() < deadline {
            for (ask, result) in link.answers() {
                waiting -= 1;
                let trust = Trust { node, pinned, checkpoints: &checkpoints, saved: &saved, fast_resume: false };
                chain.take(&ask, &result, trust, u64::MAX / 2, 0.0);
            }

            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    #[ignore = "needs a node with a name: CHAINV4_NODE=http://127.0.0.1:8080 CHAINV4_NAME=alice.dev CHAINV4_NAME_TARGET=ed25519:0x… cargo test -- --ignored"]
    fn a_name_resolves_from_its_records_proven_against_the_tip() {
        let node = std::env::var("CHAINV4_NODE").expect("CHAINV4_NODE names a node");
        let name = std::env::var("CHAINV4_NAME").expect("CHAINV4_NAME names a name it holds");
        let target = std::env::var("CHAINV4_NAME_TARGET").expect("CHAINV4_NAME_TARGET is what it resolves to");
        let mut chain = Chain::default();
        let mut pinned = HashMap::new();
        follow(&mut chain, &node, &mut pinned, &HashMap::new());

        // The chain's names first, as the node says them; then the records, proven.
        let asks = chain.proofs_due(&[], 10.0);
        answer(&mut chain, &node, &mut pinned, asks);
        assert!(chain.names.is_some(), "the node has no names");

        let asks = chain.names_due(&[name.clone(), "nobody-at-all.dev".to_string()], 10.0);
        assert!(!asks.is_empty());
        answer(&mut chain, &node, &mut pinned, asks);

        let resolved = chain.resolve(&name).expect("every record it rests on, proven");

        assert_eq!(resolved.target().map(|target| target.to_string()), Some(target));
        assert!(chain.refused.is_empty(), "{:?}", chain.refused);
        assert!(chain.resolve("nobody-at-all.dev").expect("its absence proven").found.is_none());
        assert!(chain.names_due(&[name], 11.0).is_empty(), "asked again only once PROVE_EVERY passes");
    }

    fn ureq_get(url: &str) -> Value {
        let (sent, got) = std::sync::mpsc::channel();
        chainv4_ui::net::Client::direct().fetch(url.to_string(), None, move |answer| sent.send(answer).unwrap());

        serde_json::from_slice(&got.recv_timeout(Duration::from_secs(20)).unwrap().unwrap().bytes).unwrap()
    }
}

#[cfg(test)]
mod mention_tests {
    use super::*;

    #[test]
    fn what_both_indexes_list_of_an_address_comes_together_newest_first_each_once() {
        let mention = |tx: &str, height: u64| Mention { tx: tx.to_string(), height };
        let listed = |mentions: Vec<Mention>| FilteredTxs { mentions, ..FilteredTxs::default() };
        let by_address = listed(vec![mention("0x03", 9), mention("0x01", 2)]);
        let by_event = listed(vec![mention("0x04", 11), mention("0x03", 9), mention("0x02", 5)]);
        let heights = |list: Vec<Mention>| list.iter().map(|mention| mention.height).collect::<Vec<_>>();

        assert_eq!(heights(together(Some(&by_address), Some(&by_event))), [11, 9, 5, 2], "a token's transfer only the event index found goes in");
        assert_eq!(heights(together(Some(&by_address), None)), [9, 2], "a node with one index lists what it lists");
        assert_eq!(heights(together(None, Some(&by_event))), [11, 9, 5]);
        assert!(together(None, None).is_empty());
    }

    #[test]
    fn older_pages_of_both_indexes_go_on_from_their_cursors_and_newer_ones_go_in_front() {
        let (mut pinned, checkpoints, saved) = (HashMap::new(), HashMap::new(), HashMap::new());
        let mut chain = Chain::default();
        let address = "ed25519:0x01".to_string();
        let mut answer = |chain: &mut Chain, ask: Ask, value: Value| {
            let trust = Trust { node: "http://node", pinned: &mut pinned, checkpoints: &checkpoints, saved: &saved, fast_resume: false };

            assert!(chain.take(&ask, &Ok(value), trust, 0, 0.0));
        };
        let txs = |list: &[(&str, u64)]| list.iter().map(|(id, height)| serde_json::json!({ "id": id, "height": height })).collect::<Vec<_>>();
        let events = |list: &[(&str, u64)]| list.iter().map(|(tx, height)| serde_json::json!({ "tx": tx, "height": height })).collect::<Vec<_>>();
        let heights = |chain: &Chain| chain.mentions[&address].iter().map(|mention| mention.height).collect::<Vec<_>>();
        let newest = |cursor: Option<&str>| (Ask::History { address: address.clone(), cursor: cursor.map(str::to_string) }, Ask::Events { address: address.clone(), cursor: cursor.map(str::to_string) });

        assert!(chain.older_pages(std::slice::from_ref(&address)).is_empty(), "nothing to go on from before the newest page");

        let (history, event_list) = newest(None);
        answer(&mut chain, history, serde_json::json!({ "txs": txs(&[("0x09", 9), ("0x08", 8)]), "next": "0xaa" }));
        answer(&mut chain, event_list, serde_json::json!({ "events": events(&[("0x07", 7)]), "next": "0xbb" }));

        assert_eq!(heights(&chain), [9, 8, 7]);
        assert_eq!(chain.older_pages(std::slice::from_ref(&address)), [newest(Some("0xaa")).0, newest(Some("0xbb")).1], "each index's own cursor");

        answer(&mut chain, newest(Some("0xaa")).0, serde_json::json!({ "txs": txs(&[("0x05", 5), ("0x03", 3)]), "next": null }));
        answer(&mut chain, newest(Some("0xbb")).1, serde_json::json!({ "events": events(&[("0x04", 4)]), "next": "0xcc" }));

        assert_eq!(heights(&chain), [9, 8, 7, 5, 4, 3], "older pages go after what was read");
        assert_eq!(chain.older_pages(std::slice::from_ref(&address)), [newest(Some("0xcc")).1], "the address index is read to its end");

        // The newest page asked again, as it is every few seconds, keeps what older pages found.
        answer(&mut chain, newest(None).0, serde_json::json!({ "txs": txs(&[("0x10", 10), ("0x09", 9)]), "next": "0xaa" }));

        assert_eq!(heights(&chain), [10, 9, 8, 7, 5, 4, 3]);
        assert_eq!(chain.older_pages(std::slice::from_ref(&address)), [newest(Some("0xcc")).1], "and where they got to");
    }

    fn at(height: u64) -> Assurance {
        Assurance { block: Hash::zero(chainv4::structs::hash::HashType::default_chain()), height, final_: true }
    }

    #[test]
    fn an_nft_s_owner_proven_at_an_older_block_does_not_undo_a_newer_one() {
        let mut owners: HashMap<(String, String), Proven<Option<Address>>> = HashMap::new();
        let key = ("contract:0x01".to_string(), "9".to_string());
        let (before, after) = (Address::Ed25519([1; 32]), Address::Bitcoin([2; 20]));
        newest(&mut owners, key.clone(), Proven { value: Some(after), assurance: at(8) });
        newest(&mut owners, key.clone(), Proven { value: Some(before), assurance: at(7) });

        assert_eq!(owners[&key].value, Some(after), "the answer that came late was older");

        newest(&mut owners, key.clone(), Proven { value: None, assurance: at(9) });

        assert_eq!(owners[&key].value, None);
    }

    #[test]
    fn a_contract_is_known_by_the_registry_entry_proven_for_it() {
        use chainv4::registry::collections::CollectionKind;

        let mut chain = Chain::default();
        let (token, collection, stranger) = (Address::Contract([1; 32]), Address::Contract([2; 32]), Address::Contract([3; 32]));
        chain.list_collections(false, &Ok(serde_json::json!({ "collections": [{ "address": collection.to_string(), "kind": "nft" }] })));

        assert_eq!(chain.listed_collections, [collection.to_string()], "which to prove, as the node said");
        assert!(chain.known(&collection).is_none(), "and nothing known of it until proven");

        chain.tokens.insert(token.to_string(), Proven { value: Token { address: token, name: None, symbol: Some("GOLD".to_string()), decimals: Some(2) }, assurance: at(1) });
        chain.collections.insert(collection.to_string(), Proven { value: Collection { address: collection, kind: CollectionKind::Nft, name: None, symbol: None }, assurance: at(1) });

        assert!(matches!(chain.known(&token), Some(Known::Token(found)) if found.symbol.as_deref() == Some("GOLD")));
        assert!(matches!(chain.known(&collection), Some(Known::Collection(found)) if found.kind == CollectionKind::Nft));
        assert!(chain.known(&stranger).is_none());
    }
}
