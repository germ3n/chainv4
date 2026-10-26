//! chainv4 light: a light client on the command line. It trusts a genesis and nothing else,
//! follows a node's chain from its headers and justifications, and answers only with what it
//! proved.

use {
    super::CliResult,
    super::genesis::DevChain,
    super::send::{agent, fetch, ok},
    crate::db::{Column, STATE_COLUMNS, StateProof, address_key},
    crate::light::{Assurance, CheckpointStart, FinalityStep, LightClient, LightParams, LightRequest},
    crate::structs::account::Account,
    crate::structs::address::Address,
    crate::structs::block::SealedHeader,
    crate::structs::hash::Hash,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::receipt::{CommittedOutcome, CommittedReceipt},
    crate::structs::tx::{SignedTx, Tx},
    serde_json::Value,
    std::path::PathBuf,
    std::time::{SystemTime, UNIX_EPOCH},
    ureq::Agent,
};

#[derive(clap::Args)]
pub struct LightArgs {
    /// The node to follow, http:// or https://. It is not trusted: everything it sends is checked.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    node: String,
    #[command(flatten)]
    tls: super::TlsArgs,
    /// Trust the chain a genesis file describes.
    #[arg(long, conflicts_with = "genesis_id")]
    genesis: Option<PathBuf>,
    /// Trust the chain with this genesis id, as another node or someone you trust shows it.
    #[arg(long)]
    genesis_id: Option<String>,
    /// Without --genesis or --genesis-id, trust this development chain.
    #[arg(long, value_enum, default_value_t = DevChain::Pow)]
    consensus: DevChain,
    /// Start at this finalized checkpoint, a block id someone you trust shows, rather than follow
    /// from genesis: the client trusts it, and checks everything from there on.
    #[arg(long)]
    checkpoint: Option<String>,
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
    /// A transaction and its receipt, proven: who sent what, and what it did.
    Receipt { tx: String },
    /// The transactions touching an address, newest first, each with its receipt, proven. The
    /// node must keep the address index (--index-addresses); it only says where to look.
    History {
        address: Address,
        /// The most to list.
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=200))]
        limit: u32,
    },
}

/// Follows the chain from the trusted genesis, or a checkpoint given, then answers the query with
/// what it proved.
pub fn light(args: LightArgs) -> CliResult<()> {
    let node = args.node.trim_end_matches('/').to_string();
    let agent = agent(&args.tls)?;
    let started = ok(agent.get(format!("{node}/light")).call())?;
    let params: LightParams = decode(&started["params"])?;
    let genesis: SealedHeader = decode(&started["header"])?;
    let trusted = trusted(&args, &params)?;
    let mut client = match &args.checkpoint {
        Some(text) => {
            let bytes = from_hex(text).ok_or("--checkpoint is not hex")?;
            let checkpoint = Hash::from_bytes(params.hash, &bytes).ok_or("--checkpoint is not as long as the chain's ids")?;
            let answer = ok(agent.get(format!("{node}/light/checkpoint/{checkpoint}")).call())?;
            let start: CheckpointStart = decode(&answer["start"])?;

            LightClient::from_checkpoint(params, genesis, trusted, checkpoint, start)?
        }
        None => LightClient::new(params, genesis, trusted)?,
    };
    // Genesis, or the checkpoint: where following starts.
    let start = client.tip().height;
    follow(&agent, &node, &mut client)?;

    let tip = client.tip();
    let followed = tip.height - start;
    match start {
        0 => println!("followed {followed} headers to {} {}, each checked by {}", tip.height, tip.id(), client.params().consensus.name()),
        _ => println!(
            "followed {followed} headers from the checkpoint at {start} to {} {}, each checked by {}",
            tip.height,
            tip.id(),
            client.params().consensus.name()
        ),
    }

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
            let (signed, receipt, assurance) = proven_tx(&agent, &node, &client, &tx)?;
            println!("{} from {}: {}", receipt.tx, signed.sender(), summary(&signed.tx));
            println!(
                "{}, {} events, gas {}, fee {}: {}",
                outcome(&receipt.outcome),
                receipt.events.len(),
                receipt.gas_used,
                receipt.fee,
                described(&assurance)
            );

            Ok(())
        }
        LightQuery::History { address, limit } => {
            let listing = match fetch(agent.get(format!("{node}/accounts/{address}/txs?limit={limit}")).call())? {
                (200, listing) => listing,
                (404, _) => return Err("the node keeps no address index: it lists histories when started with --index-addresses".into()),
                (status, answer) => return Err(format!("the node answered {status}: {answer}").into()),
            };
            let ids: Vec<&str> = listing["txs"].as_array().into_iter().flatten().filter_map(|tx| tx["id"].as_str()).collect();

            if ids.is_empty() {
                println!("nothing touched {address}, as far back as height {}", listing["indexed_from"]);
            }

            for id in ids {
                let (signed, receipt, assurance) = proven_tx(&agent, &node, &client, id)?;
                let mark = if assurance.final_ { "final" } else { "not final" };
                println!("{:>8} {id} {}: {} [{mark}]", assurance.height, summary(&signed.tx), outcome(&receipt.outcome));
            }

            if listing["next"].is_string() {
                println!("older ones follow: ask for more with --limit");
            }

            Ok(())
        }
    }
}

/// A transaction and its receipt, each proven against the block it landed in.
fn proven_tx(agent: &Agent, node: &str, client: &LightClient, tx: &str) -> CliResult<(SignedTx, CommittedReceipt, Assurance)> {
    let proof = ok(agent.get(format!("{node}/proofs/receipts/{tx}")).call())?;
    let block = hash(&proof["block"], client)?;
    let bytes = |name: &str| from_hex(proof[name].as_str().unwrap_or_default()).ok_or_else(|| format!("the node sent a {name} that is not hex"));
    let path = |name: &str| proof[name].as_array().into_iter().flatten().map(|sibling| hash(sibling, client)).collect::<CliResult<Vec<_>>>();
    let index = proof["index"].as_u64().ok_or("the node sent no index")? as usize;
    let count = proof["count"].as_u64().ok_or("the node sent no count")? as usize;
    let (receipt, assurance) = client.check_receipt(&block, &bytes("receipt")?, index, count, &path("path")?)?;
    let (signed, _) = client.check_tx(&block, &bytes("signed")?, index, count, &path("tx_path")?)?;

    if receipt.tx.to_string() != tx || signed.id(client.params().hash) != receipt.tx {
        return Err(format!("the node proved another transaction than {tx}").into());
    }

    Ok((signed, receipt, assurance))
}

/// What a transaction asks for, in a line.
fn summary(tx: &Tx) -> String {
    match tx {
        Tx::Transfer { from, to, amount } => format!("transfer {} from {from} to {to}", amount.to_u256()),
        Tx::Mint { to, amount } => format!("mint {} to {to}", amount.to_u256()),
        Tx::Burn { from, amount } => format!("burn {} from {from}", amount.to_u256()),
        Tx::ContractCall { to, amount, data, .. } => format!("call {to} with {} arguments, sending {}", data.len(), amount.to_u256()),
        Tx::EvmCall { to, value, input } => format!("EVM call to {to} with {} bytes of input, sending {}", input.len(), value.to_u256()),
        Tx::Deploy { .. } => "deploy a contract".to_string(),
        Tx::Upgrade { contract, .. } => format!("upgrade {contract}"),
        Tx::Bond { voter, amount } => format!("bond {} for voter {voter}", amount.to_u256()),
        Tx::Unbond { voter } => format!("unbond voter {voter}"),
        Tx::Withdraw { voter } => format!("withdraw voter {voter}'s bond"),
        Tx::Slash { .. } => "slash a voter that voted twice".to_string(),
        Tx::ApproveSigners { version, .. } => format!("approve signers version {version}"),
        Tx::WithdrawApproval { version } => format!("withdraw approval of signers version {version}"),
        Tx::ApproveChange { version, change } => format!("approve a {} change from version {version}", change.name()),
        Tx::WithdrawChange { version } => format!("withdraw approval of a change from version {version}"),
        Tx::Name { name, action } => format!("{} the name {name}", action.kind()),
        Tx::PrimaryName { name: Some(name) } => format!("make {name} the sender's primary name"),
        Tx::PrimaryName { name: None } => "clear the sender's primary name".to_string(),
        Tx::Ethereum { raw } => format!("an Ethereum-signed transaction of {} bytes", raw.len()),
    }
}

fn outcome(outcome: &CommittedOutcome) -> String {
    match outcome {
        CommittedOutcome::Done => "done".to_string(),
        CommittedOutcome::Deployed(address) => format!("deployed {address}"),
        CommittedOutcome::Returned(values) => format!("returned {} values", values.len()),
        CommittedOutcome::Failed => "failed".to_string(),
        CommittedOutcome::Upgraded(address) => format!("upgraded {address}"),
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
            LightRequest::Finality { from } => {
                let answer = ok(agent.get(format!("{node}/light/finality?from={from}")).call())?;
                let steps: Vec<FinalityStep> = list(&answer["steps"])?;
                client.deliver_finality(from, steps)?;
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
