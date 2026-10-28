//! chainv4 claim: claims a miner's earnings from a mining pool. It asks the pool's node for the
//! miner's claim, all it earned and the Merkle path to a root a block carries, and sends it as a
//! Tx::Claim: the coins go to the miner, whoever's key pays the fee. With --saved, it takes the
//! claims a worker kept instead and asks no pool: any node checks them against the roots on its
//! chain, so they pay even once the pool's node is gone.

use {
    super::multisig::send_own,
    super::send::{TlsArgs, agent, ok},
    super::{CliResult, signer},
    crate::mining::claims::read_claims,
    crate::registry::pools::{PoolClaim, PoolRoot},
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
    #[arg(long, conflicts_with = "saved")]
    pub pool: Option<String>,
    /// Claim from the claims a worker kept, its --claims file, rather than asking the pool's node:
    /// the newest that holds on --node's chain, for each pool it holds claims on.
    #[arg(long)]
    pub saved: Option<PathBuf>,
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

    if let Some(file) = &args.saved {
        return claim_saved(&agent, node, &keypair, file, &to, Duration::from_secs(args.timeout));
    }

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

/// Claims from a claims file: for each pool, the newest claim on it that holds under the root on
/// the node's chain and pays something, sent as it is.
fn claim_saved(agent: &ureq::Agent, node: &str, keypair: &crate::crypto::keypair::Keypair, file: &std::path::Path, to: &Address, timeout: Duration) -> CliResult<()> {
    let saved = read_claims(file)?;
    let mut pools: Vec<Address> = saved.of(None, to).map(|claim| claim.pool).collect();
    pools.dedup();

    if pools.is_empty() {
        return Err(format!("{} keeps no claims for {to}", file.display()).into());
    }

    for pool in pools {
        match holding(agent, node, saved.of(Some(&pool), to))? {
            Some((claim, claimed)) if claim.earned > claimed => {
                println!("claiming {} for {to}, of {} earned from the pool {pool}, by the claim kept at block {}", claim.earned - claimed, claim.earned, claim.height);
                send_own(agent, node, keypair, claim.tx(), timeout)?;
            }
            Some((claim, _)) => println!("{to} has claimed all {} the claims kept on the pool {pool} hold", claim.earned),
            None => println!("none of the claims kept on the pool {pool} holds on this chain: their blocks were undone"),
        }
    }

    Ok(())
}

/// The newest of a pool's claims that holds under the root the node's chain has at its height,
/// with what the miner has claimed from the pool already.
fn holding<'a>(agent: &ureq::Agent, node: &str, claims: impl Iterator<Item = &'a PoolClaim>) -> CliResult<Option<(&'a PoolClaim, u256)>> {
    for claim in claims {
        let answer = ok(agent.get(format!("{node}/claims/{}/{}?height={}", claim.pool, claim.to, claim.height)).call())?;
        let claimed = u256::from_str_radix(answer["claimed"].as_str().ok_or("the node sent no claimed")?, 10)?;
        let hash = claim.block.algorithm();
        let root = answer["root"]["root"].as_str().and_then(from_hex).and_then(|bytes| Hash::from_bytes(hash, &bytes));

        if let (Some(root), Some(count)) = (root, answer["root"]["count"].as_u64())
            && claim.holds(&PoolRoot { pool: claim.pool, root, count }, hash)
        {
            return Ok(Some((claim, claimed)));
        }
    }

    Ok(None)
}
