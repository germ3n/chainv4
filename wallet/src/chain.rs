//! What the wallet knows about the chain, all of it checked. The light client follows the node
//! from the genesis the wallet trusts, and every balance, token and receipt the wallet shows is
//! one it proved against a header the client checked. The node only ever says where to look:
//! which tokens exist, which transactions touch an address. What is there is proven: each
//! transaction against its block's tx_root, and what it did against its receipts root.

use {
    crate::node::{Ask, Failure},
    chainv4::db::{Column, StateProof, address_key},
    chainv4::light::{
        Assurance, CheckpointStart, FinalityStep, LightClient, LightError, LightParams, LightRequest, LightState, MAX_LIGHT_HEADERS, token_balance,
        token_balance_key,
    },
    chainv4::registry::names::{FoundName, NameRecord, NameStatus, NamesConfig, PrimaryName, lineage, name_problem, primary_key, record_key, standing},
    chainv4::registry::tokens::{Token, token_key},
    chainv4::structs::account::Account,
    chainv4::structs::address::Address,
    chainv4::structs::block::SealedHeader,
    chainv4::structs::hash::Hash,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::receipt::CommittedReceipt,
    chainv4::structs::tx::SignedTx,
    chainv4::structs::u256::u256,
    serde_json::Value,
    std::collections::HashMap,
};

/// Seconds between following the chain again once caught up, and proving balances again.
const FOLLOW_EVERY: f64 = 3.0;
const PROVE_EVERY: f64 = 4.0;
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
    Balance { token: String, holder: String },
    /// A name's record, by name without the suffix.
    Name(String),
    /// An address's primary name.
    Primary(String),
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

/// A transaction that landed, as proven: who sent what, and what it did.
#[derive(Clone, Debug)]
pub struct Landed {
    pub signed: SignedTx,
    pub receipt: CommittedReceipt,
    /// When its block was made, in milliseconds since the Unix epoch.
    pub time: u64,
}

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
}

/// A light client about to start at a checkpoint, from what the node said of its chain.
struct Starting {
    params: LightParams,
    header: SealedHeader,
    genesis: Hash,
    /// The checkpoint pasted; None for the node's own, on first use.
    checkpoint: Option<Hash>,
}

#[derive(Default)]
pub struct Chain {
    pub client: Option<LightClient>,
    /// Waiting for what starts the client at a checkpoint.
    starting: Option<Starting>,
    /// What stopped the light client: a node on another chain than the one trusted, or one that
    /// sent something that does not check.
    pub trouble: Option<String>,
    /// The genesis the client follows, once it started.
    pub genesis: Option<Hash>,
    /// Whether what the client checked changed since it was last saved.
    pub dirty: bool,
    followed: f64,
    proved: f64,
    retry_at: f64,
    /// Accounts by address text: what was proven at the final block and at the tip.
    pub accounts: HashMap<String, Vec<Proven<Account>>>,
    /// The tokens the registry lists, by address text: the node says which, the proof what.
    pub listed: Vec<String>,
    pub tokens: HashMap<String, Proven<Token>>,
    /// Token balances by token and holder.
    pub balances: HashMap<(String, String), Vec<Proven<u256>>>,
    /// Transactions proven with their receipts since the app last took them, by transaction id.
    pub receipts: HashMap<String, Proven<Landed>>,
    /// Transactions the node says touch an address, by address, newest first.
    pub mentions: HashMap<String, Vec<Mention>>,
    /// The chain's names, as the node said them, once it did; whether it has any, once known.
    pub names: Option<NamesConfig>,
    pub has_names: Option<bool>,
    /// The names the node says each address owns, by address text: where to look.
    pub owned: HashMap<String, Vec<String>>,
    /// Name records proven, by name without the suffix: None where there is none.
    pub records: HashMap<String, Proven<Option<NameRecord>>>,
    /// Primary names proven, by address text, as chosen: whether each still resolves to its
    /// address is for resolve to say.
    pub primaries: HashMap<String, Proven<Option<String>>>,
    /// When each name's records were last asked for.
    named: HashMap<String, f64>,
    /// Whether the node keeps an address index, or else an event index, to list them from.
    pub indexes_addresses: Option<bool>,
    pub indexes_events: Option<bool>,
    /// What each proof asked for is the proof of.
    targets: HashMap<String, Target>,
    /// Why a proof did not check, by what it was for.
    pub refused: HashMap<String, String>,
    /// The lowest height a receipt landed at below every header the client holds: what it takes
    /// ancestors down to, a stretch at a time.
    wanted_below: Option<u64>,
}

impl Chain {
    /// Starts the client waiting to start at a checkpoint from genesis instead.
    fn start_from_genesis(&mut self) {
        if let Some(starting) = self.starting.take() {
            match LightClient::new(starting.params, starting.header, starting.genesis) {
                Ok(client) => self.client = Some(client),
                Err(err) => self.trouble = Some(err.to_string()),
            }
        }
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

    /// What the light client needs next, if anything is due.
    pub fn next_ask(&mut self, time: f64) -> Option<Ask> {
        if self.trouble.is_some() || time < self.retry_at {
            return None;
        }

        let Some(client) = &mut self.client else {
            return Some(match &self.starting {
                Some(starting) => Ask::Checkpoint(starting.checkpoint.map(|block| block.to_string())),
                None => Ask::Light,
            });
        };

        if client.next().is_none() && time - self.followed >= FOLLOW_EVERY {
            self.followed = time;
            client.refresh();
        }

        if let Some(request) = client.next() {
            return Some(match request {
                LightRequest::Headers { from, count } => Ask::Headers { from, count },
                LightRequest::Finality { from } => Ask::Finality(from),
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
        if !self.caught_up() || time - self.proved < PROVE_EVERY {
            return Vec::new();
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

        let mut wanted = vec![Ask::Tokens];
        let mut targets = Vec::new();

        if self.has_names != Some(false) {
            wanted.push(Ask::Names);
        }

        for holder in holders {
            targets.push((Column::Accounts, address_key(holder), Target::Account(holder.to_string()), blocks.clone()));

            if self.has_names == Some(true) {
                wanted.push(Ask::OwnedNames(holder.to_string()));
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

        for (column, key, target, blocks) in targets {
            let key = to_hex(&key);
            self.targets.insert(key.clone(), target);

            for block in blocks {
                wanted.push(Ask::Proof {
                    column: column.name(),
                    key: key.clone(),
                    block: block.to_string(),
                });
            }
        }

        wanted
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
            if self.named.get(&name).is_some_and(|asked| time - asked < PROVE_EVERY) {
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

    /// Takes an answer meant for the chain. Returns false for one that is not.
    pub fn take(&mut self, ask: &Ask, result: &Result<Value, Failure>, trust: Trust, now: u64, time: f64) -> bool {
        let mine = matches!(
            ask,
            Ask::Light
                | Ask::Headers { .. }
                | Ask::Ancestors { .. }
                | Ask::Finality(_)
                | Ask::Checkpoint(_)
                | Ask::Proof { .. }
                | Ask::Receipt(_)
                | Ask::Tokens
                | Ask::History(_)
                | Ask::Events(_)
                | Ask::Names
                | Ask::OwnedNames(_)
        );

        if !mine {
            return false;
        }

        let value = match (ask, result) {
            (_, Ok(value)) => value,
            // Not on the chain yet, or a node without the index asked of: nothing to show yet.
            (Ask::Receipt(_) | Ask::Proof { .. }, Err(failure)) if failure.status == 404 => return true,
            (Ask::History(_), Err(failure)) if failure.status == 404 => {
                self.indexes_addresses = Some(false);

                return true;
            }
            (Ask::Events(_), Err(failure)) if failure.status == 404 => {
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
            (Ask::Checkpoint(Some(block)), Err(failure)) if failure.status != 0 => {
                self.trouble = Some(format!("the node cannot start the light client at checkpoint {block}: {}", failure.message));

                return true;
            }
            (_, Err(_)) => {
                self.retry_at = time + RETRY_AFTER;

                return true;
            }
        };

        if let Err(why) = self.take_value(ask, value, trust, now) {
            match ask {
                Ask::Proof { key, .. } => {
                    self.refused.insert(key.clone(), why);
                }
                Ask::Receipt(tx) => {
                    self.refused.insert(tx.clone(), why);
                }
                Ask::Tokens | Ask::History(_) | Ask::Events(_) | Ask::Names | Ask::OwnedNames(_) => {}
                _ => self.trouble = Some(why),
            }
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

                // Saved, it carries on. Else from a checkpoint pasted; else, met for the first time
                // with no genesis given, from its own finalized checkpoint, trusting it as much as
                // its genesis; else from genesis.
                match (restored, pasted, first_use) {
                    (Some(client), _, _) => self.client = Some(client),
                    (None, Some(checkpoint), _) => self.starting = Some(Starting { params, header, genesis, checkpoint: Some(checkpoint) }),
                    (None, None, true) => self.starting = Some(Starting { params, header, genesis, checkpoint: None }),
                    (None, None, false) => self.client = Some(LightClient::new(params, header, genesis).map_err(|err| err.to_string())?),
                }

                self.genesis = Some(genesis);
                self.dirty = true;

                Ok(())
            }
            Ask::Checkpoint(_) => {
                let starting = self.starting.take().ok_or("a checkpoint nobody asked for")?;
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

                match LightClient::from_checkpoint(starting.params.clone(), starting.header.clone(), starting.genesis, block, start) {
                    Ok(client) => self.client = Some(client),
                    Err(err) if starting.checkpoint.is_some() => return Err(format!("the checkpoint trusted does not check: {err}")),
                    // The node's own does not hold: following from genesis checks everything.
                    Err(_) => {
                        self.starting = Some(starting);
                        self.start_from_genesis();
                    }
                }

                self.dirty = true;

                Ok(())
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
            Ask::Finality(from) => {
                let steps: Vec<FinalityStep> = list(&value["steps"])?;
                self.dirty |= !steps.is_empty();

                self.with_client(|client| client.deliver_finality(*from, steps).map(|_| ()).map_err(|err| err.to_string()))
            }
            Ask::Tokens => {
                self.listed = value["tokens"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|token| token["address"].as_str().map(str::to_string))
                    .collect();

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
            Ask::OwnedNames(address) => {
                let owned = value["names"].as_array().into_iter().flatten().filter_map(|name| name["name"].as_str().map(str::to_string)).collect();
                self.owned.insert(address.clone(), owned);

                Ok(())
            }
            Ask::History(address) => {
                self.indexes_addresses = Some(true);
                self.mentions.insert(address.clone(), mentions(&value["txs"], "id"));

                Ok(())
            }
            Ask::Events(address) => {
                self.indexes_events = Some(true);
                self.mentions.insert(address.clone(), mentions(&value["events"], "tx"));

                Ok(())
            }
            Ask::Proof { column, key, .. } => {
                let target = self.targets.get(key).cloned().ok_or("a proof nobody asked for")?;
                let client = self.client.as_ref().ok_or("the light client has not started")?;
                let block = id(client, value["block"].as_str().unwrap_or_default()).ok_or("a proof at a block id that does not read")?;
                let column = [Column::Accounts, Column::Tokens, Column::Namespaces, Column::Names]
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
                }

                Ok(())
            }
            Ask::Receipt(tx) => {
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

    /// What the client checked, to save, once it changed.
    pub fn saved(&mut self) -> Option<(String, String)> {
        let client = self.client.as_ref()?;

        if !std::mem::take(&mut self.dirty) {
            return None;
        }

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
                let trust = Trust { node, pinned, checkpoints, saved: &saved };
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
                let trust = Trust { node, pinned, checkpoints: &checkpoints, saved: &saved };
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
