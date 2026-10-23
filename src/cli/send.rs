use {
    super::{CliResult, amount, deployed_at, parse_amount, parse_argument, signer},
    crate::structs::address::Address,
    crate::structs::code::Code,
    crate::structs::hash::Hash,
    crate::structs::hex::to_hex,
    crate::structs::tx::{SignedTx, Tx, TxCallArgumentType},
    crate::structs::u256::u256,
    crate::vm::selector,
    serde_json::{Value, json},
    std::path::PathBuf,
    std::time::{Duration, Instant},
    ureq::Agent,
};

/// How often send asks whether its transaction landed.
const POLL: Duration = Duration::from_millis(500);

#[derive(clap::Args)]
pub struct SendArgs {
    /// The node's HTTP API.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    /// The key file to sign with, from keygen. Without one, the well-known development key signs.
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// Return once the node takes the transaction, without waiting for a block.
    #[arg(long)]
    pub no_wait: bool,
    /// How long to wait for the transaction to land, in seconds.
    #[arg(long, default_value_t = 60)]
    pub timeout: u64,
    /// How many blocks must hold the transaction, its own included, before send returns. More
    /// than one guards against a proof-of-work fork taking it back off.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..))]
    pub confirmations: u64,
    #[command(subcommand)]
    pub tx: TxArgs,
}

#[derive(clap::Subcommand)]
pub enum TxArgs {
    /// Sends funds to an address.
    Transfer {
        /// Who receives them, such as ed25519:0x….
        #[arg(long)]
        to: Address,
        /// A whole amount, in the smallest unit.
        #[arg(long, value_parser = parse_amount)]
        amount: u256,
    },
    /// Creates funds. Only the chain's minter may.
    Mint {
        /// Who receives them.
        #[arg(long)]
        to: Address,
        /// A whole amount, in the smallest unit.
        #[arg(long, value_parser = parse_amount)]
        amount: u256,
    },
    /// Destroys funds the signer holds.
    Burn {
        /// A whole amount, in the smallest unit.
        #[arg(long, value_parser = parse_amount)]
        amount: u256,
    },
    /// Deploys a contract from a Luau source file or a WebAssembly module.
    Deploy {
        /// A Luau source file.
        #[arg(long, conflicts_with = "wasm", required_unless_present = "wasm")]
        lua: Option<PathBuf>,
        /// A WebAssembly module, such as one built with the SDK.
        #[arg(long)]
        wasm: Option<PathBuf>,
    },
    /// Calls a function on a contract.
    Call {
        /// The contract, such as contract:0x….
        #[arg(long)]
        to: Address,
        /// The exported function's name.
        #[arg(long)]
        function: String,
        /// What to send to the contract with the call.
        #[arg(long, default_value = "0", value_parser = parse_amount)]
        amount: u256,
        /// Arguments as type:value, such as i64:5, string:hi or address:ed25519:0x….
        #[arg(value_parser = parse_argument)]
        args: Vec<TxCallArgumentType>,
    },
}

/// Signs a transaction with the signer's next nonce, submits it, and unless told not to, waits
/// until enough blocks hold it and prints what it did. A transaction that ran but failed is an
/// error, so scripts can tell.
pub fn send(args: SendArgs) -> CliResult<()> {
    let keypair = signer(args.key.as_deref())?;
    let sender = keypair.public().address();
    let node = args.node.trim_end_matches('/');
    let agent: Agent = Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let account = ok(agent.get(format!("{node}/accounts/{sender}")).call())?;
    let nonce = account["nonce"].as_u64().ok_or("the node sent no nonce")?;
    let tx = transaction(args.tx, sender)?;
    let deploys = matches!(tx, Tx::Deploy { .. });
    let signed = SignedTx::sign(tx, nonce, &keypair);
    let body = json!({ "tx": to_hex(&wincode::serialize(&signed)?) }).to_string();
    let submitted = ok(agent
        .post(format!("{node}/txs"))
        .header("content-type", "application/json")
        .send(body))?;
    let id = submitted["id"].as_str().ok_or("the node sent no transaction id")?;

    println!("submitted {id}");

    if deploys {
        println!("contract {}", deployed_at(&sender, nonce));
    }

    if args.no_wait {
        return Ok(());
    }

    let receipt = wait(&agent, node, id, args.confirmations, Duration::from_secs(args.timeout))?;
    report(&receipt)
}

/// Asks the node about the transaction until enough blocks hold it or the time runs out.
fn wait(agent: &Agent, node: &str, id: &str, confirmations: u64, timeout: Duration) -> CliResult<Value> {
    let deadline = Instant::now() + timeout;

    loop {
        let (status, receipt) = fetch(agent.get(format!("{node}/txs/{id}")).call())?;

        match status {
            200 if receipt["confirmations"].as_u64().unwrap_or(0) >= confirmations => return Ok(receipt),
            // 404 means not in a block yet; 200 means in one, but not deep enough.
            200 | 404 => {}
            _ => return Err(refusal(status, &receipt)),
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "{id} was not in a block with {confirmations} confirmations after {}s; it may still land",
                timeout.as_secs()
            )
            .into());
        }

        std::thread::sleep(POLL);
    }
}

/// Prints where the transaction landed, what it did and what it emitted.
fn report(receipt: &Value) -> CliResult<()> {
    let confirmations = receipt["confirmations"].as_u64().unwrap_or(0);

    println!(
        "included in block {} {} ({confirmations} confirmation{})",
        receipt["height"],
        receipt["block"].as_str().unwrap_or("?"),
        if confirmations == 1 { "" } else { "s" }
    );

    match receipt["status"].as_str() {
        Some("deployed") => println!("deployed {}", receipt["contract"].as_str().unwrap_or("?")),
        Some("returned") => println!("returned {}", receipt["values"]),
        Some("failed") => {}
        _ => println!("done"),
    }

    if let Some(fuel) = receipt["fuel_used"].as_u64().filter(|fuel| *fuel > 0) {
        println!("fuel used {fuel}");
    }

    for event in receipt["events"].as_array().into_iter().flatten() {
        println!(
            "event {} from {} {}",
            event["name"].as_str().unwrap_or("?"),
            event["contract"].as_str().unwrap_or("?"),
            event["data"]
        );
    }

    match receipt["status"].as_str() {
        Some("failed") => Err(format!("the transaction failed: {}", receipt["error"].as_str().unwrap_or("?")).into()),
        _ => Ok(()),
    }
}

fn transaction(tx: TxArgs, sender: Address) -> CliResult<Tx> {
    let tx = match tx {
        TxArgs::Transfer { to, amount: value } => Tx::Transfer {
            from: sender,
            to,
            amount: amount(value),
        },
        TxArgs::Mint { to, amount: value } => Tx::Mint { to, amount: amount(value) },
        TxArgs::Burn { amount: value } => Tx::Burn {
            from: sender,
            amount: amount(value),
        },
        TxArgs::Deploy { lua: Some(path), .. } => Tx::Deploy {
            code: Code::Lua(std::fs::read_to_string(&path).map_err(|err| format!("cannot read {}: {err}", path.display()))?),
        },
        TxArgs::Deploy { wasm: Some(path), .. } => Tx::Deploy {
            code: Code::Wasm(std::fs::read(&path).map_err(|err| format!("cannot read {}: {err}", path.display()))?),
        },
        TxArgs::Deploy { .. } => return Err("deploy needs --lua or --wasm".into()),
        TxArgs::Call {
            to,
            function,
            amount: value,
            args,
        } => Tx::ContractCall {
            to,
            func: Hash::Xxh3_64(selector(&function)),
            amount: amount(value),
            data: args,
        },
    };

    Ok(tx)
}

type Response = Result<ureq::http::Response<ureq::Body>, ureq::Error>;

/// The JSON a request came back with, or the node's error.
fn ok(response: Response) -> CliResult<Value> {
    match fetch(response)? {
        (200..=299, json) => Ok(json),
        (status, json) => Err(refusal(status, &json)),
    }
}

/// The status and JSON a request came back with, whatever the status.
fn fetch(response: Response) -> CliResult<(u16, Value)> {
    let mut response = response.map_err(|err| format!("cannot reach the node: {err}"))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string()?;
    let json = serde_json::from_str(&text).map_err(|_| format!("the node answered {status}: {text}"))?;

    Ok((status, json))
}

fn refusal(status: u16, json: &Value) -> Box<dyn std::error::Error> {
    let message = json["error"].as_str().map_or_else(|| json.to_string(), str::to_string);

    format!("the node refused ({status}): {message}").into()
}
