//! The explorer's light client. It trusts nothing but the genesis it saw the first time it met a
//! node, follows the chain from the node's headers and justifications, checking every one, and
//! proves the balances the account page shows, so a node that lies about them is caught.

use {
    crate::api::{Ask, Failure},
    chainv4::db::{Column, StateProof, address_key},
    chainv4::light::{Assurance, LightClient, LightParams, LightRequest, LightUpdate},
    chainv4::structs::account::Account,
    chainv4::structs::address::Address,
    chainv4::structs::block::SealedHeader,
    chainv4::structs::hash::Hash,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::vote::Justification,
    serde_json::Value,
    std::collections::HashMap,
};

/// Seconds between following the chain again once it caught up.
const FOLLOW_EVERY: f64 = 4.0;
/// Seconds before asking again what failed to come.
const RETRY_AFTER: f64 = 3.0;

/// A balance the light client proved.
#[derive(Clone, Debug)]
pub struct Proven {
    pub balance: String,
    pub nonce: u64,
    pub assurance: Assurance,
}

#[derive(Default)]
pub struct Light {
    client: Option<LightClient>,
    /// What stopped it: a node on another chain than the one trusted, or one that sent something
    /// that does not check.
    pub trouble: Option<String>,
    followed: f64,
    retry_at: f64,
    /// Balances proven, by address, newest block last; or why a proof did not check.
    pub proven: HashMap<String, Result<Vec<Proven>, String>>,
    /// The address each proven key is for.
    keys: HashMap<String, String>,
}

impl Light {
    /// How far it follows: the tip's height and the height up to which blocks are final.
    pub fn following(&self) -> Option<(u64, u64)> {
        self.client.as_ref().map(|client| (client.tip().height, client.final_height()))
    }

    /// Whether it has every header and justification the node had when last asked.
    pub fn caught_up(&self) -> bool {
        self.client.as_ref().is_some_and(|client| client.next().is_none())
    }

    /// What to ask next, if anything is due.
    pub fn ask(&mut self, time: f64) -> Option<Ask> {
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

    /// The proofs to ask for an account's balance: at the tip, and at the final block when the
    /// node can still prove there. Nothing before it caught up.
    pub fn prove(&mut self, address: &str) -> Vec<Ask> {
        let (Some(client), Ok(parsed)) = (&self.client, address.parse::<Address>()) else {
            return Vec::new();
        };

        if client.next().is_some() {
            return Vec::new();
        }

        let key = to_hex(&address_key(&parsed));
        self.keys.insert(key.clone(), address.to_string());
        let tip = client.tip().id();
        let mut blocks = vec![tip];

        if let Some(final_block) = client.header_at(client.final_height()).map(|header| header.id())
            && client.final_height() > 0
            && final_block != tip
        {
            blocks.insert(0, final_block);
        }

        blocks
            .into_iter()
            .map(|block| Ask::Proof {
                column: Column::Accounts.name().to_string(),
                key: key.clone(),
                block: Some(block.to_string()),
            })
            .collect()
    }

    /// Takes an answer to one of its asks. Returns false for an ask that is not its.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>, pinned: &mut HashMap<String, String>, node: &str, now: u64, time: f64) -> bool {
        let mine = matches!(ask, Ask::Light | Ask::Headers { .. } | Ask::LightUpdate(_) | Ask::Justifications(_) | Ask::Proof { .. });

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
            (_, Err(_)) => {
                self.retry_at = time + RETRY_AFTER;

                return true;
            }
        };

        if let Err(why) = self.take(ask, value, pinned, node, now) {
            match ask {
                Ask::Proof { key, .. } => {
                    let address = self.keys.get(key).cloned().unwrap_or_default();
                    self.proven.insert(address, Err(why));
                }
                _ => self.trouble = Some(why),
            }
        }

        true
    }

    fn take(&mut self, ask: &Ask, value: &Value, pinned: &mut HashMap<String, String>, node: &str, now: u64) -> Result<(), String> {
        match ask {
            Ask::Light => {
                let params: LightParams = decode(&value["params"])?;
                let header: SealedHeader = decode(&value["header"])?;
                let genesis = header.id().to_string();

                // Trust on first use: the genesis seen the first time this node was met.
                let trusted = pinned.entry(node.to_string()).or_insert_with(|| genesis.clone()).clone();

                if trusted != genesis {
                    return Err(format!("the node is now on chain {genesis}, not {trusted}, which the explorer first saw here"));
                }

                let trusted = header.id();
                self.client = Some(LightClient::new(params, header, trusted).map_err(|err| err.to_string())?);

                Ok(())
            }
            Ask::Headers { from, .. } => {
                let headers: Vec<SealedHeader> = list(&value["headers"])?;

                self.with_client(|client| client.deliver_headers(*from, headers, now).map(|_| ()).map_err(|err| err.to_string()))
            }
            Ask::LightUpdate(block) => {
                let update: LightUpdate = decode(&value["update"])?;

                self.with_client(|client| {
                    let block = id(client, block).ok_or("an update for a block id that does not read")?;

                    client.take_update(&block, Some(update)).map_err(|err| err.to_string())
                })
            }
            Ask::Justifications(_) => {
                let justifications: Vec<Justification> = list(&value["justifications"])?;

                self.with_client(|client| client.deliver_justifications(&justifications).map(|_| ()).map_err(|err| err.to_string()))
            }
            Ask::Proof { key, .. } => {
                let address = self.keys.get(key).cloned().ok_or("a proof nobody asked for")?;
                let client = self.client.as_ref().ok_or("the light client has not started")?;
                let block = id(client, value["block"].as_str().unwrap_or_default()).ok_or("a proof at a block id that does not read")?;
                let raw_key = from_hex(key).ok_or("a key that is not hex")?;
                let held = value["value"].as_str().map(|text| from_hex(text).ok_or("a value that is not hex")).transpose()?;
                let proof: StateProof = decode(&value["proof"])?;
                let assurance = client
                    .check_state(&block, Column::Accounts, &raw_key, held.as_deref(), &proof)
                    .map_err(|err| format!("the node's proof of this balance does not check: {err}"))?;
                let account: Account = match held {
                    Some(bytes) => wincode::deserialize(&bytes).map_err(|_| "an account that does not decode")?,
                    None => Account::default(),
                };
                let proven = Proven {
                    balance: account.balance.to_string(),
                    nonce: account.nonce,
                    assurance,
                };
                let entry = self.proven.entry(address).or_insert_with(|| Ok(Vec::new()));

                if entry.is_err() {
                    *entry = Ok(Vec::new());
                }

                if let Ok(list) = entry {
                    list.retain(|known| known.assurance.block != block && (known.assurance.final_ || known.assurance.height > proven.assurance.height));
                    list.push(proven);
                    list.sort_by_key(|known| known.assurance.height);
                }

                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn with_client(&mut self, take: impl FnOnce(&mut LightClient) -> Result<(), String>) -> Result<(), String> {
        match &mut self.client {
            Some(client) => take(client),
            None => Err("the light client has not started".to_string()),
        }
    }
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
