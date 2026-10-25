//! chainv4 light: a light client on the command line. It trusts a genesis and nothing else,
//! follows a node's chain from its headers and justifications, and answers only with what it
//! proved.

use {
    super::CliResult,
    super::genesis::DevChain,
    super::send::{agent, fetch, ok},
    crate::db::{Column, STATE_COLUMNS, StateProof, address_key},
    crate::light::{Assurance, LightClient, LightParams, LightRequest, LightUpdate},
    crate::structs::account::Account,
    crate::structs::address::Address,
    crate::structs::block::SealedHeader,
    crate::structs::hash::Hash,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::receipt::CommittedOutcome,
    crate::structs::vote::Justification,
    serde_json::Value,
    std::path::PathBuf,
    std::time::{SystemTime, UNIX_EPOCH},
    ureq::Agent,
};

#[derive(clap::Args)]
pub struct LightArgs {
    /// The node to follow. It is not trusted: everything it sends is checked.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    node: String,
    /// Trust the chain a genesis file describes.
    #[arg(long, conflicts_with = "genesis_id")]
    genesis: Option<PathBuf>,
    /// Trust the chain with this genesis id, as another node or someone you trust shows it.
    #[arg(long)]
    genesis_id: Option<String>,
    /// Without --genesis or --genesis-id, trust this development chain.
    #[arg(long, value_enum, default_value_t = DevChain::Pow)]
    consensus: DevChain,
    #[command(subcommand)]
    query: LightQuery,
}

#[derive(clap::Subcommand)]
enum LightQuery {
    /// Follows the chain and says where it stands.
    Status,
    /// An account's balance and nonce, proven.
    Account { address: Address },
    /// One chain-state entry, proven: its column and its key in hex.
    Entry { column: String, key: String },
    /// A transaction's receipt, proven.
    Receipt { tx: String },
}

/// Follows the chain from the trusted genesis, then answers the query with what it proved.
pub fn light(args: LightArgs) -> CliResult<()> {
    let node = args.node.trim_end_matches('/').to_string();
    let agent = agent();
    let started = ok(agent.get(format!("{node}/light")).call())?;
    let params: LightParams = decode(&started["params"])?;
    let genesis: SealedHeader = decode(&started["header"])?;
    let trusted = trusted(&args, &params)?;
    let mut client = LightClient::new(params, genesis, trusted)?;
    follow(&agent, &node, &mut client)?;

    let tip = client.tip();
    println!("followed {} headers to {} {}, each checked by {}", tip.height, tip.height, tip.id(), client.params().consensus.name());

    if client.params().finality.is_some_and(|finality| finality.checkpoint_interval().is_some()) {
        let (justified, finalized) = (client.justified(), client.finalized());
        println!("justified epoch {} {}, finalized epoch {} {}", justified.epoch, justified.block, finalized.epoch, finalized.block);
    }

    match args.query {
        LightQuery::Status => Ok(()),
        LightQuery::Account { address } => {
            let key = address_key(&address);

            for (value, assurance) in entry(&agent, &node, &client, Column::Accounts, &key)? {
                let account: Account = match value {
                    Some(bytes) => wincode::deserialize(&bytes).map_err(|_| "the node sent an account that does not decode")?,
                    None => Account::default(),
                };
                println!("balance {}, nonce {}: {}", account.balance, account.nonce, described(&assurance));
            }

            Ok(())
        }
        LightQuery::Entry { column, key } => {
            let column = STATE_COLUMNS
                .into_iter()
                .find(|known| known.name() == column)
                .ok_or_else(|| format!("{column} is not a chain-state column"))?;
            let key = from_hex(&key).ok_or("the key is not hex")?;

            for (value, assurance) in entry(&agent, &node, &client, column, &key)? {
                let shown = value.map_or_else(|| "nothing".to_string(), |value| to_hex(&value));
                println!("{shown}: {}", described(&assurance));
            }

            Ok(())
        }
        LightQuery::Receipt { tx } => {
            let proof = ok(agent.get(format!("{node}/proofs/receipts/{tx}")).call())?;
            let block = hash(&proof["block"], &client)?;
            let receipt = from_hex(proof["receipt"].as_str().unwrap_or_default()).ok_or("the node sent a receipt that is not hex")?;
            let path = proof["path"].as_array().into_iter().flatten().map(|sibling| hash(sibling, &client)).collect::<CliResult<Vec<_>>>()?;
            let index = proof["index"].as_u64().ok_or("the node sent no index")? as usize;
            let count = proof["count"].as_u64().ok_or("the node sent no count")? as usize;
            let (receipt, assurance) = client.check_receipt(&block, &receipt, index, count, &path)?;
            let outcome = match receipt.outcome {
                CommittedOutcome::Done => "done".to_string(),
                CommittedOutcome::Deployed(address) => format!("deployed {address}"),
                CommittedOutcome::Returned(values) => format!("returned {} values", values.len()),
                CommittedOutcome::Failed => "failed".to_string(),
                CommittedOutcome::Upgraded(address) => format!("upgraded {address}"),
            };
            println!("{} {outcome}, {} events, gas {}, fee {}: {}", receipt.tx, receipt.events.len(), receipt.gas_used, receipt.fee, described(&assurance));

            Ok(())
        }
    }
}

/// The genesis id to trust: the one a genesis file or development chain makes, or one given, read
/// with the hash the chain says it has. A genesis header that hashed to those bytes under another
/// hash would take a preimage, so the chain's word on which hash is enough.
fn trusted(args: &LightArgs, params: &LightParams) -> CliResult<Hash> {
    if let Some(text) = &args.genesis_id {
        let bytes = from_hex(text).ok_or("--genesis-id is not hex")?;

        return Hash::from_bytes(params.hash, &bytes).ok_or_else(|| "--genesis-id is not as long as the chain's ids".into());
    }

    let genesis = match &args.genesis {
        Some(path) => super::genesis::read(path)?,
        None => super::genesis::dev(args.consensus),
    };

    Ok(genesis.block().id())
}

/// Asks the node for everything the client wants until it is caught up.
fn follow(agent: &Agent, node: &str, client: &mut LightClient) -> CliResult<()> {
    while let Some(request) = client.next() {
        match request {
            LightRequest::Headers { from, count } => {
                let answer = ok(agent.get(format!("{node}/headers?from={from}&count={count}")).call())?;
                let headers = list(&answer["headers"])?;
                client.deliver_headers(from, headers, now())?;
            }
            LightRequest::Update { block } => {
                let update: Option<LightUpdate> = match fetch(agent.get(format!("{node}/light/updates/{block}")).call())? {
                    (404, _) => None,
                    (200, answer) => Some(decode(&answer["update"])?),
                    (status, answer) => return Err(format!("the node answered {status}: {answer}").into()),
                };
                client.take_update(&block, update)?;
            }
            LightRequest::Justifications { from } => {
                let answer = ok(agent.get(format!("{node}/justifications?from={from}")).call())?;
                let justifications: Vec<Justification> = list(&answer["justifications"])?;
                client.deliver_justifications(&justifications)?;
            }
        }
    }

    Ok(())
}

/// An entry proven at the finalized block, when the node can still prove there, and at the tip.
fn entry(agent: &Agent, node: &str, client: &LightClient, column: Column, key: &[u8]) -> CliResult<Vec<(Option<Vec<u8>>, Assurance)>> {
    let tip = client.tip().id();
    let finalized = client.header_at(client.final_height()).map(|header| header.id()).filter(|block| *block != tip && client.final_height() > 0);
    let mut proven = Vec::new();

    for block in finalized.into_iter().chain([tip]) {
        let url = format!("{node}/proofs/{}/{}?block={block}", column.name(), to_hex(key));
        let answer = match fetch(agent.get(url).call())? {
            (200, answer) => answer,
            // Too old for the node to prove at: the tip will do.
            (404, _) if block != tip => continue,
            (status, answer) => return Err(format!("the node answered {status}: {answer}").into()),
        };
        let value = answer["value"].as_str().map(|text| from_hex(text).ok_or("the node sent a value that is not hex")).transpose()?;
        let proof: StateProof = decode(&answer["proof"])?;
        proven.push((value.clone(), client.check_state(&block, column, key, value.as_deref(), &proof)?));
    }

    Ok(proven)
}

fn described(assurance: &Assurance) -> String {
    match assurance.final_ {
        true => format!("proven at block {} {}, which is final", assurance.height, assurance.block),
        false => format!("proven at block {} {}, not final yet", assurance.height, assurance.block),
    }
}

fn decode<T>(value: &Value) -> CliResult<T>
where
    T: for<'de> wincode::SchemaRead<'de, wincode::config::DefaultConfig, Dst = T>,
{
    let bytes = from_hex(value.as_str().ok_or("the node sent nothing to decode")?).ok_or("the node sent something that is not hex")?;

    wincode::deserialize(&bytes).map_err(|_| "the node sent something that does not decode".into())
}

fn list<T>(value: &Value) -> CliResult<Vec<T>>
where
    T: for<'de> wincode::SchemaRead<'de, wincode::config::DefaultConfig, Dst = T>,
{
    value.as_array().ok_or("the node sent no list")?.iter().map(decode).collect()
}

fn hash(value: &Value, client: &LightClient) -> CliResult<Hash> {
    let bytes = from_hex(value.as_str().ok_or("the node sent no hash")?).ok_or("the node sent a hash that is not hex")?;

    Hash::from_bytes(client.params().hash, &bytes).ok_or_else(|| "the node sent a hash of the wrong length".into())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
}
