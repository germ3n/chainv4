//! The explorer's light client. It trusts nothing but the genesis it saw the first time it met a
//! node, follows the chain from the node's headers and justifications, checking every one, and
//! proves the balances the account page shows, so a node that lies about them is caught. It starts
//! at a checkpoint pasted in Settings rather than genesis; and the first time it meets a node, at
//! the node's own finalized one, trusted as far as the genesis it pins then.

use {
    crate::api::{Ask, Failure},
    chainv4::db::{Column, StateProof, address_key},
    chainv4::light::{Assurance, CheckpointStart, FinalityStep, LightClient, LightParams, LightRequest},
    chainv4::structs::account::Account,
    chainv4::structs::address::Address,
    chainv4::structs::block::SealedHeader,
    chainv4::structs::hash::Hash,
    chainv4::structs::hex::{from_hex, to_hex},
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

/// What the explorer trusts at a node: the genesis pinned there, and a checkpoint pasted.
pub struct Trust<'a> {
    pub node: &'a str,
    pub pinned: &'a mut HashMap<String, String>,
    pub checkpoints: &'a HashMap<String, String>,
}

/// A light client about to start at a checkpoint, from what the node said of its chain.
struct Starting {
    params: LightParams,
    header: SealedHeader,
    /// The checkpoint pasted; None for the node's own, on first use.
    checkpoint: Option<Hash>,
}

#[derive(Default)]
pub struct Light {
    client: Option<LightClient>,
    /// Waiting for what starts the client at a checkpoint.
    starting: Option<Starting>,
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
    /// The lowest header it holds: 0 when it followed from genesis.
    #[cfg(test)]
    fn lowest(&self) -> Option<u64> {
        self.client.as_ref().map(LightClient::lowest)
    }

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
            return Some(match &self.starting {
                Some(starting) => Ask::Checkpoint(starting.checkpoint.map(|block| block.to_string())),
                None => Ask::Light,
            });
        };

        if client.next().is_none() && time - self.followed >= FOLLOW_EVERY {
            self.followed = time;
            client.refresh();
        }

        client.next().map(|request| match request {
            LightRequest::Headers { from, count } => Ask::Headers { from, count },
            LightRequest::Finality { from } => Ask::Finality(from),
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
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>, trust: Trust, now: u64, time: f64) -> bool {
        let mine = matches!(ask, Ask::Light | Ask::Headers { .. } | Ask::Finality(_) | Ask::Checkpoint(_) | Ask::Proof { .. });

        if !mine {
            return false;
        }

        let value = match (ask, result) {
            (_, Ok(value)) => value,
            // The node's own checkpoint is a shortcut: without one, it follows from genesis.
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

        if let Err(why) = self.take(ask, value, trust, now) {
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

    /// Starts the client waiting to start at a checkpoint from genesis instead.
    fn start_from_genesis(&mut self) {
        if let Some(starting) = self.starting.take() {
            let genesis = starting.header.id();

            match LightClient::new(starting.params, starting.header, genesis) {
                Ok(client) => self.client = Some(client),
                Err(err) => self.trouble = Some(err.to_string()),
            }
        }
    }

    fn take(&mut self, ask: &Ask, value: &Value, trust: Trust, now: u64) -> Result<(), String> {
        match ask {
            Ask::Light => {
                let params: LightParams = decode(&value["params"])?;
                let header: SealedHeader = decode(&value["header"])?;
                let genesis = header.id().to_string();

                // Trust on first use: the genesis seen the first time this node was met.
                let first_use = !trust.pinned.contains_key(trust.node);
                let trusted = trust.pinned.entry(trust.node.to_string()).or_insert_with(|| genesis.clone()).clone();

                if trusted != genesis {
                    return Err(format!("the node is now on chain {genesis}, not {trusted}, which the explorer first saw here"));
                }

                let pasted = trust.checkpoints.get(trust.node).and_then(|text| from_hex(text)).and_then(|bytes| Hash::from_bytes(params.hash, &bytes));

                match (pasted, first_use) {
                    (Some(checkpoint), _) => self.starting = Some(Starting { params, header, checkpoint: Some(checkpoint) }),
                    (None, true) => self.starting = Some(Starting { params, header, checkpoint: None }),
                    (None, false) => {
                        let trusted = header.id();
                        self.client = Some(LightClient::new(params, header, trusted).map_err(|err| err.to_string())?);
                    }
                }

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

                if starting.checkpoint.is_some_and(|checkpoint| checkpoint != block) {
                    return Err(format!("the node sent checkpoint {block}, not the one trusted"));
                }

                let genesis = starting.header.id();

                match LightClient::from_checkpoint(starting.params.clone(), starting.header.clone(), genesis, block, start) {
                    Ok(client) => self.client = Some(client),
                    Err(err) if starting.checkpoint.is_some() => return Err(format!("the checkpoint trusted does not check: {err}")),
                    Err(_) => {
                        self.starting = Some(starting);
                        self.start_from_genesis();
                    }
                }

                Ok(())
            }
            Ask::Headers { from, .. } => {
                let headers: Vec<SealedHeader> = list(&value["headers"])?;

                self.with_client(|client| client.deliver_headers(*from, headers, now).map(|_| ()).map_err(|err| err.to_string()))
            }
            Ask::Finality(from) => {
                let steps: Vec<FinalityStep> = list(&value["steps"])?;

                self.with_client(|client| client.deliver_finality(*from, steps).map(|_| ()).map_err(|err| err.to_string()))
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

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::api::{Api, Reply},
        std::time::{Duration, Instant},
    };

    /// Drives the light client against the node until it is caught up or stops on trouble, and
    /// says what it asked.
    fn follow(light: &mut Light, node: &str, pinned: &mut HashMap<String, String>) -> Vec<Ask> {
        let mut api = Api::new(node, Ok(chainv4_ui::net::Client::direct()), egui::Context::default());
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut asked = Vec::new();

        while !light.caught_up() && light.trouble.is_none() {
            assert!(Instant::now() < deadline, "never caught up; asked {asked:?}");
            let Some(ask) = light.ask(0.0) else {
                break;
            };
            asked.push(ask.clone());
            api.ask(ask);

            let replies = loop {
                let replies = api.replies();

                if !replies.is_empty() {
                    break replies;
                }

                std::thread::sleep(Duration::from_millis(5));
            };

            for reply in replies {
                if let Reply::Answer { ask, result } = reply {
                    let trust = Trust { node, pinned, checkpoints: &HashMap::new() };
                    light.answered(&ask, &result, trust, u64::MAX / 2, 0.0);
                }
            }
        }

        asked
    }

    #[test]
    #[ignore = "needs a node with voting finality: CHAINV4_NODE=http://127.0.0.1:8080 cargo test -- --ignored"]
    fn a_node_met_first_starts_the_light_client_at_its_checkpoint_and_one_met_before_at_genesis() {
        let node = std::env::var("CHAINV4_NODE").expect("CHAINV4_NODE names a node");
        let mut pinned = HashMap::new();
        let mut first = Light::default();
        let asked = follow(&mut first, &node, &mut pinned);

        assert_eq!(asked[..2], [Ask::Light, Ask::Checkpoint(None)]);
        assert!(first.lowest().is_some_and(|lowest| lowest > 0), "{:?}", first.trouble);

        // The next launch: the genesis pinned, so from it, checking every block.
        let mut again = Light::default();
        let asked = follow(&mut again, &node, &mut pinned);

        assert!(!asked.iter().any(|ask| matches!(ask, Ask::Checkpoint(_))));
        assert_eq!(again.lowest(), Some(0));
        assert_eq!(again.following().map(|(tip, _)| tip >= first.following().unwrap().0), Some(true));
    }
}
