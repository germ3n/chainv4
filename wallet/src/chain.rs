//! What the wallet knows about the chain, all of it checked. The light client follows the node
//! from the genesis the wallet trusts, and every balance, token and receipt the wallet shows is
//! one it proved against a header the client checked. The node only ever says where to look:
//! which tokens exist, which transactions touch an address. What is there is proven: each
//! transaction against its block's tx_root, and what it did against its receipts root.

use {
    crate::node::{Ask, Failure},
    chainv4::db::{Column, StateProof, address_key},
    chainv4::light::{Assurance, LightClient, LightError, LightParams, LightRequest, LightState, LightUpdate, token_balance, token_balance_key},
    chainv4::registry::tokens::{Token, token_key},
    chainv4::structs::account::Account,
    chainv4::structs::address::Address,
    chainv4::structs::block::SealedHeader,
    chainv4::structs::hash::Hash,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::receipt::CommittedReceipt,
    chainv4::structs::tx::SignedTx,
    chainv4::structs::u256::u256,
    chainv4::structs::vote::Justification,
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

/// What the wallet trusts at a node: the genesis pinned there, and what it saved of the chain it
/// followed from each.
pub struct Trust<'a> {
    pub node: &'a str,
    /// Genesis ids by node.
    pub pinned: &'a mut HashMap<String, String>,
    /// Saved light client states by genesis id.
    pub saved: &'a HashMap<String, String>,
}

#[derive(Default)]
pub struct Chain {
    pub client: Option<LightClient>,
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
    /// Whether the node keeps an address index, or else an event index, to list them from.
    pub indexes_addresses: Option<bool>,
    pub indexes_events: Option<bool>,
    /// What each proof asked for is the proof of.
    targets: HashMap<String, Target>,
    /// Why a proof did not check, by what it was for.
    pub refused: HashMap<String, String>,
}

impl Chain {
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
            return Some(Ask::Light);
        };

        if client.next().is_none() && time - self.followed >= FOLLOW_EVERY {
            self.followed = time;
            client.refresh();
        }

        client.next().map(|request| match request {
            LightRequest::Headers { from, count } => Ask::Headers { from, count },
            LightRequest::Update { block } => Ask::LightUpdate(block.to_string()),
            LightRequest::Justifications { from } => Ask::Justifications(from),
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

        for holder in holders {
            targets.push((Column::Accounts, address_key(holder), Target::Account(holder.to_string()), blocks.clone()));

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

    /// Takes an answer meant for the chain. Returns false for one that is not.
    pub fn take(&mut self, ask: &Ask, result: &Result<Value, Failure>, trust: Trust, now: u64, time: f64) -> bool {
        let mine = matches!(
            ask,
            Ask::Light
                | Ask::Headers { .. }
                | Ask::LightUpdate(_)
                | Ask::Justifications(_)
                | Ask::Proof { .. }
                | Ask::Receipt(_)
                | Ask::Tokens
                | Ask::History(_)
                | Ask::Events(_)
        );

        if !mine {
            return false;
        }

        let value = match (ask, result) {
            (_, Ok(value)) => value,
            // A block that keeps no update, as one before a snapshot the node synced from.
            (Ask::LightUpdate(block), Err(failure)) if failure.status == 404 => {
                let taken = self.with_client(|client| {
                    let block = id(client, block).ok_or("an update for a block id that does not read")?;

                    client.take_update(&block, None).map_err(|err| err.to_string())
                });

                if let Err(why) = taken {
                    self.trouble = Some(why);
                }

                return true;
            }
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
                Ask::Tokens | Ask::History(_) | Ask::Events(_) => {}
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
                self.client = Some(match restored {
                    Some(client) => client,
                    None => LightClient::new(params, header, genesis).map_err(|err| err.to_string())?,
                });
                self.genesis = Some(genesis);
                self.dirty = true;

                Ok(())
            }
            Ask::Headers { from, .. } => {
                let headers: Vec<SealedHeader> = list(&value["headers"])?;
                self.dirty |= !headers.is_empty();

                self.with_client(|client| client.deliver_headers(*from, headers, now).map(|_| ()).map_err(|err| err.to_string()))
            }
            Ask::LightUpdate(block) => {
                let update: LightUpdate = decode(&value["update"])?;
                self.dirty = true;

                self.with_client(|client| {
                    let block = id(client, block).ok_or("an update for a block id that does not read")?;

                    client.take_update(&block, Some(update)).map_err(|err| err.to_string())
                })
            }
            Ask::Justifications(_) => {
                let justifications: Vec<Justification> = list(&value["justifications"])?;
                self.dirty |= !justifications.is_empty();

                self.with_client(|client| client.deliver_justifications(&justifications).map(|_| ()).map_err(|err| err.to_string()))
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
                let column = [Column::Accounts, Column::Tokens, Column::Namespaces]
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
                    // In a block the client has not followed yet: proven at the next round.
                    Err(LightError::NotOnChain) => return Ok(()),
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
