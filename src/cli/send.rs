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
    std::time::Duration,
    ureq::Agent,
};

#[derive(clap::Args)]
pub struct SendArgs {
    /// The node's HTTP API.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    /// The key file to sign with, from keygen. Without one, the well-known development key signs.
    #[arg(long)]
    pub key: Option<PathBuf>,
    #[command(subcommand)]
    pub tx: TxArgs,
}

#[derive(clap::Subcommand)]
pub enum TxArgs {
    /// Sends funds to an address.
    Transfer {
        #[arg(long)]
        to: Address,
        /// A whole amount, in the smallest unit.
        #[arg(long, value_parser = parse_amount)]
        amount: u256,
    },
    /// Creates funds. Only the chain's minter may.
    Mint {
        #[arg(long)]
        to: Address,
        #[arg(long, value_parser = parse_amount)]
        amount: u256,
    },
    /// Destroys funds the signer holds.
    Burn {
        #[arg(long, value_parser = parse_amount)]
        amount: u256,
    },
    /// Deploys a contract from a Luau source file or a WebAssembly module.
    Deploy {
        #[arg(long, conflicts_with = "wasm", required_unless_present = "wasm")]
        lua: Option<PathBuf>,
        #[arg(long)]
        wasm: Option<PathBuf>,
    },
    /// Calls a function on a contract.
    Call {
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

/// Signs a transaction with the signer's next nonce and submits it.
pub fn send(args: SendArgs) -> CliResult<()> {
    let keypair = signer(args.key.as_deref())?;
    let sender = keypair.public().address();
    let node = args.node.trim_end_matches('/');
    let agent: Agent = Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let account = answer(agent.get(format!("{node}/accounts/{sender}")).call())?;
    let nonce = account["nonce"].as_u64().ok_or("the node sent no nonce")?;
    let tx = transaction(args.tx, sender)?;
    let deploys = matches!(tx, Tx::Deploy { .. });
    let signed = SignedTx::sign(tx, nonce, &keypair);
    let body = json!({ "tx": to_hex(&wincode::serialize(&signed)?) }).to_string();
    let submitted = answer(
        agent
            .post(format!("{node}/txs"))
            .header("content-type", "application/json")
            .send(body),
    )?;

    println!("submitted {}", submitted["id"].as_str().unwrap_or("?"));

    if deploys {
        println!("contract {}", deployed_at(&sender, nonce));
    }

    Ok(())
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

/// The JSON a request came back with, or the node's error.
fn answer(response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> CliResult<Value> {
    let mut response = response.map_err(|err| format!("cannot reach the node: {err}"))?;
    let status = response.status();
    let text = response.body_mut().read_to_string()?;
    let json: Value = serde_json::from_str(&text).map_err(|_| format!("the node answered {status}: {text}"))?;

    if !status.is_success() {
        let message = json["error"].as_str().unwrap_or(&text);

        return Err(format!("the node refused ({status}): {message}").into());
    }

    Ok(json)
}
