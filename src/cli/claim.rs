//! chainv4 claim: claims a miner's earnings from a mining pool. It asks the pool's node for the
//! miner's claim, all it earned and the Merkle path to a root a block carries, and sends it as a
//! Tx::Claim: the coins go to the miner, whoever's key pays the fee.

use {
    super::multisig::send_own,
    super::send::{TlsArgs, agent, ok},
    super::{CliResult, signer},
    crate::structs::address::Address,
    crate::structs::hash::{Hash, HashType},
    crate::structs::hex::from_hex,
    crate::structs::tx::Tx,
    crate::structs::u256::u256,
    std::path::PathBuf,
    std::time::Duration,
};

#[derive(clap::Args)]
pub struct ClaimArgs {
    /// The node to send the claim to: http://, or https:// behind TLS.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    /// The pool's node, which keeps its claims, when it is not --node.
    #[arg(long)]
    pub pool: Option<String>,
    #[command(flatten)]
    pub tls: TlsArgs,
    /// The key file that sends the claim and pays its fee, from keygen. Without one, the
    /// well-known development key.
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// The miner to claim for, its payout address: the key's own address without one.
    #[arg(long)]
    pub to: Option<Address>,
    /// How long to wait for the claim to land, in seconds.
    #[arg(long, default_value_t = 60)]
    pub timeout: u64,
}

pub fn claim(args: ClaimArgs) -> CliResult<()> {
    let agent = agent(&args.tls)?;
    let node = args.node.trim_end_matches('/');
    let pool = args.pool.as_deref().unwrap_or(node).trim_end_matches('/');
    let keypair = signer(args.key.as_deref())?;
    let to = args.to.unwrap_or(keypair.public().address());
    let claim = ok(agent.get(format!("{pool}/pool/claim/{to}")).call())?;
    let number = |field: &str| -> CliResult<u256> { Ok(u256::from_str_radix(claim[field].as_str().ok_or(format!("the pool sent no {field}"))?, 10)?) };
    let (earned, claimable) = (number("earned")?, number("claimable")?);

    if claimable == u256::ZERO {
        println!("{to} has claimed all {earned} it earned from the pool");

        return Ok(());
    }

    let hash: HashType = ok(agent.get(format!("{node}/status")).call())?["hash"].as_str().and_then(|name| name.parse().ok()).ok_or("the node sent no hash")?;
    let path = claim["path"]
        .as_array()
        .ok_or("the pool sent no path")?
        .iter()
        .map(|step| step.as_str().and_then(from_hex).and_then(|bytes| Hash::from_bytes(hash, &bytes)).ok_or("a hash in the path that does not read"))
        .collect::<Result<Vec<_>, _>>()?;
    let tx = Tx::Claim {
        height: claim["height"].as_u64().ok_or("the pool sent no height")?,
        to,
        earned,
        index: claim["index"].as_u64().ok_or("the pool sent no index")?,
        path,
    };
    println!("claiming {claimable} for {to}, of {earned} earned from the pool {}", claim["pool"].as_str().unwrap_or("?"));

    send_own(&agent, node, &keypair, tx, Duration::from_secs(args.timeout))
}
