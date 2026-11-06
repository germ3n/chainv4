//! chainv4 multisig evm: a native multisig's MultiSig.sol on EVM chains, at one address on every
//! one. Its members' EVM keys sign its calls as bundles, written one a line to a file; anyone
//! deploys it, through the deterministic deployment proxy, and anyone sends a call once threshold
//! of its owners signed. When the members change here, sync proposes the matching change of
//! owners there, which the owners before sign.

use {
    super::super::CliResult,
    super::super::send::ok,
    super::super::swap::rpc::Rpc,
    super::plural,
    crate::abi::multisig::{EvmBundle, change_owners_input, multisig_contract, multisig_deployment, multisig_read, multisig_reads},
    crate::crypto::keypair::{Keypair, PrivateKey},
    crate::registry::multisig::EvmStart,
    crate::structs::address::Address,
    crate::structs::hex::from_hex,
    crate::structs::u256::u256,
    crate::swap::evm::{Call, DEPLOYMENT_PROXY, chain_name, parse_address, show_address},
    serde_json::Value,
    std::path::{Path, PathBuf},
    ureq::Agent,
};

#[derive(clap::Subcommand)]
pub enum EvmAction {
    /// Where its contract sits on every EVM chain, deployed or not; the owners and threshold it
    /// starts with, and the ones its members name now.
    Address,
    /// Its contract on one chain: whether it is deployed, its coin, owners, threshold and nonce,
    /// and whether its owners are the ones its members name now.
    Status {
        /// The chain's JSON-RPC URL.
        #[arg(long)]
        eth_rpc: String,
    },
    /// Deploys it on a chain through the deterministic deployment proxy, --key paying the gas.
    /// Anyone may: its owners are fixed by its address. submit deploys it too.
    Deploy {
        #[arg(long)]
        eth_rpc: String,
    },
    /// Proposes a call by it: --key signs as one of its owners, and the bundle is written for the
    /// others.
    Propose {
        #[arg(long)]
        eth_rpc: String,
        /// Whom it calls, or pays: 0x and 40 hex digits.
        #[arg(long)]
        to: String,
        /// The coin it sends, in wei.
        #[arg(long, default_value = "0")]
        value: String,
        /// The call's data, 0x and hex; none for a plain payment.
        #[arg(long)]
        data: Option<String>,
        /// The contract's nonce it runs at: by default its next. Give later ones to line up calls.
        #[arg(long)]
        nonce: Option<u64>,
        /// Where to write the bundle.
        #[arg(long)]
        out: PathBuf,
    },
    /// Proposes changing its owners and threshold to the ones its members name now, when they
    /// differ: the owners before sign it.
    Sync {
        #[arg(long)]
        eth_rpc: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Signs every call in a bundle's file as --key, one of its owners. Needs no node or chain.
    Sign {
        bundle: PathBuf,
        /// Sign without asking, for scripts: it still shows what it signs.
        #[arg(long)]
        yes: bool,
    },
    /// Sends a bundle's calls in order, each once threshold of the owners signed it, deploying the
    /// contract first if it is not yet; --key pays the gas, and need not be an owner.
    Submit {
        bundle: PathBuf,
        #[arg(long)]
        eth_rpc: String,
    },
}

/// A multisig's side on EVM chains: the owners and threshold its contract starts with, the ones its
/// members name now, and the contract's address.
pub(crate) struct EvmSide {
    pub start: EvmStart,
    pub now: Option<EvmStart>,
    pub contract: [u8; 20],
}

fn owners(value: &Value) -> CliResult<EvmStart> {
    let owners = value["owners"].as_array().ok_or("the node sent no owners")?.iter().map(|owner| parse_address(owner.as_str().unwrap_or_default())).collect::<Result<Vec<_>, _>>()?;

    Ok(EvmStart { owners, threshold: u32::try_from(value["threshold"].as_u64().ok_or("the node sent no threshold")?)? })
}

/// A multisig's side on EVM chains, as the node says its members' EVM addresses are; its contract's
/// address worked out here.
pub(crate) fn evm_side(agent: &Agent, node: &str, account: &Address) -> CliResult<EvmSide> {
    let multisig = ok(agent.get(format!("{node}/multisig/{account}")).call())?;
    let evm = &multisig["evm"];

    if evm.is_null() {
        return Err("its members name no EVM addresses: it has no side on EVM chains".into());
    }

    let start = owners(&evm["start"])?;
    let now = match evm["now"].is_null() {
        true => None,
        false => Some(owners(&evm["now"])?),
    };

    Ok(EvmSide { contract: multisig_contract(account, &start), start, now })
}

/// The contract as it stands on a chain: its owners and threshold, and its nonce; None before it
/// is deployed.
fn on_chain(rpc: &Rpc, contract: &[u8; 20]) -> CliResult<Option<(EvmStart, u256)>> {
    if !rpc.has_code(contract)? {
        return Ok(None);
    }

    let [owners, threshold, nonce] = multisig_reads().map(|input| rpc.call(contract, &input));

    Ok(Some(multisig_read(&owners?, &threshold?, &nonce?).ok_or("the code there is not a MultiSig.sol")?))
}

/// The owners, threshold and nonce a call is checked against: the contract's, or before it is
/// deployed, the ones it starts with at nonce 0.
fn standing(rpc: &Rpc, side: &EvmSide) -> CliResult<(EvmStart, u256)> {
    Ok(on_chain(rpc, &side.contract)?.unwrap_or((side.start.clone(), u256::ZERO)))
}

/// Deploys the contract through the proxy, unless it is there already.
fn deploy(rpc: &Rpc, account: &Address, side: &EvmSide, secret: &[u8; 32]) -> CliResult<()> {
    if rpc.has_code(&side.contract)? {
        return Ok(());
    }

    if !rpc.has_code(&DEPLOYMENT_PROXY)? {
        return Err("this chain has no deterministic deployment proxy, so the contract cannot sit at its address here".into());
    }

    let call = Call { to: DEPLOYMENT_PROXY, value: u256::ZERO, input: multisig_deployment(account, &side.start), what: "Deploy the multisig's contract" };
    rpc.send(&call, false, secret)?;

    match rpc.has_code(&side.contract)? {
        true => Ok(()),
        false => Err(format!("the proxy ran, but there is no code at {}", show_address(&side.contract)).into()),
    }
}

pub(crate) fn evm_secret(keypair: &Keypair) -> CliResult<zeroize::Zeroizing<[u8; 32]>> {
    match keypair.private() {
        PrivateKey::Secp256k1(secret) => Ok(zeroize::Zeroizing::new(*secret)),
        _ => Err("an EVM chain takes a secp256k1 key: give --key a member's EVM key file".into()),
    }
}

fn read(path: &Path) -> CliResult<Vec<EvmBundle>> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let bundles = text.lines().filter(|line| !line.trim().is_empty()).map(EvmBundle::from_text).collect::<Option<Vec<_>>>();

    match bundles {
        Some(bundles) if !bundles.is_empty() => Ok(bundles),
        _ => Err(format!("{} holds no EVM multisig bundles, one a line", path.display()).into()),
    }
}

fn write(path: &Path, bundles: &[EvmBundle]) -> CliResult<()> {
    let text: String = bundles.iter().map(|bundle| bundle.to_text() + "\n").collect();
    std::fs::write(path, text).map_err(|err| format!("{}: {err}", path.display()))?;

    Ok(())
}

/// Shows what signing bundles has an owner agree to, each call in full, and asks for yes unless
/// told.
pub(crate) fn review(bundles: &[EvmBundle], yes: bool, input: &mut dyn std::io::BufRead) -> CliResult<()> {
    use crate::structs::hex::to_hex as hex;

    for bundle in bundles {
        println!("on chain {}, {} calls {} with {} wei, at nonce {}: {}", bundle.chain_id, hex(&bundle.contract), hex(&bundle.to), bundle.value, bundle.nonce, hex(&bundle.data));
    }

    crate::cli::send::typed_yes("Sign this?", yes, input, "nothing is signed")
}

/// How many of the owners signed a bundle.
fn signed_by(bundle: &EvmBundle, owners: &EvmStart) -> usize {
    bundle.signers().iter().filter(|(signer, _)| owners.owners.contains(signer)).count()
}

/// Proposes calls by a multisig's contract, one after another from its next nonce: --key signs
/// each as an owner, and they are sent at once when that is enough, or else written to out, or
/// printed, for the others.
pub(crate) fn propose_calls(rpc: &Rpc, account: &Address, side: &EvmSide, calls: &[Call], secret: &[u8; 32], first: Option<u64>, out: Option<&Path>) -> CliResult<()> {
    let chain_id = rpc.chain_id()?;
    let (owners, next) = standing(rpc, side)?;
    let first = first.map_or(next, u256::from);
    let bundles: Vec<EvmBundle> = calls
        .iter()
        .zip(0u64..)
        .map(|(call, offset)| {
            let mut bundle = EvmBundle { chain_id, contract: side.contract, to: call.to, value: call.value, data: call.input.clone(), nonce: first + u256::from(offset), signatures: Vec::new() };
            bundle.sign(secret);
            bundle
        })
        .collect();
    let signed = signed_by(&bundles[0], &owners);

    if signed == 0 {
        eprintln!("the key is none of the contract's owners: its signature counts for nothing");
    }

    if signed >= owners.threshold as usize && first == next {
        return submit(rpc, account, side, &bundles, secret);
    }

    match out {
        Some(path) => {
            write(path, &bundles)?;
            println!("{} call{} on {}, signed by 1 of the {} it needs, in {}", bundles.len(), plural(bundles.len()), chain_name(chain_id), owners.threshold, path.display());
            println!("next: chainv4 multisig --key <member's EVM key> evm {account} sign {}, then submit", path.display());
        }
        None => bundles.iter().for_each(|bundle| println!("{}", bundle.to_text())),
    }

    Ok(())
}

/// Sends bundles' calls in order, deploying the contract first; each must be at the contract's
/// nonce, and signed by threshold of its owners then.
fn submit(rpc: &Rpc, account: &Address, side: &EvmSide, bundles: &[EvmBundle], secret: &[u8; 32]) -> CliResult<()> {
    let chain_id = rpc.chain_id()?;

    for bundle in bundles {
        if bundle.chain_id != chain_id || bundle.contract != side.contract {
            return Err(format!("a bundle for {} on {}, not this multisig's contract on {}", show_address(&bundle.contract), chain_name(bundle.chain_id), chain_name(chain_id)).into());
        }

        let (owners, nonce) = standing(rpc, side)?;

        if bundle.nonce < nonce {
            println!("the call at nonce {} ran already", bundle.nonce);
            continue;
        }

        if bundle.nonce > nonce {
            return Err(format!("a call at nonce {}, and the contract is at {nonce}: send the ones before it first", bundle.nonce).into());
        }

        let signed = signed_by(bundle, &owners);

        if signed < owners.threshold as usize {
            return Err(format!("{signed} of the contract's owners signed the call at nonce {nonce}, and it needs {}", owners.threshold).into());
        }

        deploy(rpc, account, side, secret)?;
        rpc.send(&Call { to: side.contract, value: u256::ZERO, input: bundle.execute_input(&owners.owners), what: "Execute" }, false, secret)?;
    }

    Ok(())
}

fn show_owners(owners: &EvmStart) -> String {
    format!("{} of {}", owners.threshold, owners.owners.iter().map(show_address).collect::<Vec<_>>().join(", "))
}

pub(super) fn evm(agent: &Agent, node: &str, account: &Address, keypair: &Keypair, action: EvmAction) -> CliResult<()> {
    let rpc = |url: &str| Rpc::new(agent.clone(), url);

    match action {
        EvmAction::Address => {
            let side = evm_side(agent, node, account)?;
            println!("{} on every EVM chain", show_address(&side.contract));
            println!("starts with {}", show_owners(&side.start));

            if let Some(now) = side.now.filter(|now| *now != side.start) {
                println!("its members name {} now: sync each chain it is deployed on", show_owners(&now));
            }
        }
        EvmAction::Status { eth_rpc } => {
            let (side, rpc) = (evm_side(agent, node, account)?, rpc(&eth_rpc));
            let chain = chain_name(rpc.chain_id()?);
            let balance = rpc.balance(&side.contract)?;

            match on_chain(&rpc, &side.contract)? {
                None => println!("{} on {chain}: not deployed yet, {balance} wei waiting; it starts with {}", show_address(&side.contract), show_owners(&side.start)),
                Some((owners, nonce)) => {
                    println!("{} on {chain}: {balance} wei, owners {}, next nonce {nonce}", show_address(&side.contract), show_owners(&owners));

                    if side.now.as_ref().is_some_and(|now| *now != owners) {
                        println!("its members name other owners now: chainv4 multisig evm {account} sync --eth-rpc … proposes the change");
                    }
                }
            }
        }
        EvmAction::Deploy { eth_rpc } => {
            let side = evm_side(agent, node, account)?;
            deploy(&rpc(&eth_rpc), account, &side, &*evm_secret(keypair)?)?;
            println!("deployed at {}", show_address(&side.contract));
        }
        EvmAction::Propose { eth_rpc, to, value, data, nonce, out } => {
            let side = evm_side(agent, node, account)?;
            let value = u256::from_str_radix(value.trim(), 10).map_err(|_| format!("{value} is not a number of wei"))?;
            let data = match data {
                Some(hex) => from_hex(&hex).ok_or_else(|| format!("{hex} is not 0x and hex"))?,
                None => Vec::new(),
            };
            let call = Call { to: parse_address(&to)?, value, input: data, what: "Execute" };
            propose_calls(&rpc(&eth_rpc), account, &side, &[call], &*evm_secret(keypair)?, nonce, Some(&out))?;
        }
        EvmAction::Sync { eth_rpc, out } => {
            let (side, rpc) = (evm_side(agent, node, account)?, rpc(&eth_rpc));
            let now = side.now.clone().ok_or("its members name no EVM addresses now")?;
            let (owners, _) = standing(&rpc, &side)?;

            if owners == now {
                println!("its owners are its members' already: {}", show_owners(&owners));
                return Ok(());
            }

            println!("from {} to {}", show_owners(&owners), show_owners(&now));
            let call = Call { to: side.contract, value: u256::ZERO, input: change_owners_input(&now), what: "Execute" };
            propose_calls(&rpc, account, &side, &[call], &*evm_secret(keypair)?, None, Some(&out))?;
        }
        EvmAction::Sign { bundle: path, yes } => {
            let secret = evm_secret(keypair)?;
            let mut bundles = read(&path)?;
            review(&bundles, yes, &mut std::io::stdin().lock())?;
            bundles.iter_mut().for_each(|bundle| bundle.sign(&secret));
            write(&path, &bundles)?;
            let counts: Vec<String> = bundles.iter().map(|bundle| bundle.signers().len().to_string()).collect();
            println!("signed {} call{}; signatures each: {}", bundles.len(), plural(bundles.len()), counts.join(", "));
        }
        EvmAction::Submit { bundle: path, eth_rpc } => {
            let side = evm_side(agent, node, account)?;
            submit(&rpc(&eth_rpc), account, &side, &read(&path)?, &*evm_secret(keypair)?)?;
        }
    }

    Ok(())
}
