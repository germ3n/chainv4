//! chainv4 multisig evm: a native multisig's MultiSig.sol on EVM chains, at one address on every
//! one. Its members sign its calls as bundles, written one a line to a file, each as its kind of
//! owner: an EVM key, an Ed25519 key, or a contract, another multisig's MultiSig.sol whose own
//! members approve the call for it, or anything else answering ERC-1271 with the bytes it gives.
//! Anyone deploys it, through the deterministic deployment proxy, and anyone sends a call once
//! threshold of its owners signed. When the members change here, sync proposes the matching change
//! of owners there, which the owners before sign.

use {
    super::super::CliResult,
    super::super::send::ok,
    super::super::swap::rpc::Rpc,
    super::plural,
    crate::abi::multisig::{ERC1271_VALID, EvmBundle, change_owners_input, ed25519_holds, execute_signer, is_valid_signature_input, multisig_contract, multisig_deployment, multisig_read, multisig_reads},
    crate::crypto::keypair::{Keypair, PrivateKey},
    crate::registry::multisig::{EvmKind, EvmStart},
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
    /// Signs every call in a bundle's file as --key, one of its owners: an EVM key or an Ed25519
    /// key. Needs no node or chain.
    Sign {
        bundle: PathBuf,
        /// Sign without asking, for scripts: it still shows what it signs.
        #[arg(long)]
        yes: bool,
    },
    /// Approves every call in another multisig's bundle for this one, whose contract is an owner
    /// of that one's: --key signs as one of this one's owners, over the call wrapped in this
    /// contract's domain, and once enough of them have, the bundle carries their approvals as this
    /// contract's signature. Needs no node or chain beyond this multisig's owners.
    Approve {
        bundle: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Adds a contract owner's signature to every call in a bundle's file as given: the bytes its
    /// isValidSignature reads, as a Safe or a smart-account wallet makes them. submit asks the
    /// contract before it counts them.
    AddSignature {
        bundle: PathBuf,
        /// The owner contract, 0x and 40 hex digits.
        #[arg(long)]
        owner: String,
        /// Its signature, 0x and hex.
        #[arg(long)]
        signature: String,
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
    /// chainv4's chain id, which the contract's init code names.
    pub home: u64,
}

fn owners(value: &Value) -> CliResult<EvmStart> {
    Ok(crate::structs::json::evm_start(value).ok_or("the node sent no owners, or ones that do not read")?)
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

    let home = evm["home"].as_u64().ok_or("the node sent no chain id for the contract")?;

    Ok(EvmSide { contract: multisig_contract(account, &start, home), start, now, home })
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

    let call = Call { to: DEPLOYMENT_PROXY, value: u256::ZERO, input: multisig_deployment(account, &side.start, side.home), what: "Deploy the multisig's contract" };
    rpc.send(&call, false, secret)?;

    match rpc.has_code(&side.contract)? {
        true => Ok(()),
        false => Err(format!("the proxy ran, but there is no code at {}", show_address(&side.contract)).into()),
    }
}

pub(crate) fn evm_secret(keypair: &Keypair) -> CliResult<zeroize::Zeroizing<[u8; 32]>> {
    match keypair.private() {
        PrivateKey::Secp256k1(secret) => Ok(zeroize::Zeroizing::new(*secret)),
        _ => Err("an EVM chain takes a secp256k1 key to pay its gas: give --key an EVM key file".into()),
    }
}

/// Signs a bundle as one of its owners.
fn sign_as(bundle: &mut EvmBundle, keypair: &Keypair) -> CliResult<()> {
    bundle.sign_with(keypair)?;

    Ok(())
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

/// How many of the owners signed a bundle: an EVM key or Ed25519 key's signatures checked here, a
/// contract owner's asked of the contract when a chain is at hand, and counted as given otherwise.
fn signed_by(rpc: Option<&Rpc>, bundle: &EvmBundle, owners: &EvmStart) -> CliResult<usize> {
    let digest = bundle.digest();
    let mut count = 0;

    for (signer, signature) in bundle.signers() {
        let Some(owner) = owners.owners.iter().find(|owner| owner.address == signer) else {
            continue;
        };
        let holds = match owner.kind {
            EvmKind::Key => <[u8; 65]>::try_from(signature.as_slice()).ok().and_then(|signature| execute_signer(&digest, &signature)) == Some(signer),
            EvmKind::Ed25519(key) => ed25519_holds(&key, &digest, &signature),
            EvmKind::Contract => match rpc {
                // A refusal is an answer too: an eth_call that reverts holds nothing.
                Some(rpc) => rpc.call(&signer, &is_valid_signature_input(&digest, &signature)).is_ok_and(|answer| answer.get(..4) == Some(&ERC1271_VALID[..])),
                None => true,
            },
        };

        count += usize::from(holds);
    }

    Ok(count)
}

/// Proposes calls by a multisig's contract, one after another from its next nonce: --key signs
/// each as an owner, and they are sent at once when that is enough, or else written to out, or
/// printed, for the others.
pub(crate) fn propose_calls(rpc: &Rpc, account: &Address, side: &EvmSide, calls: &[Call], keypair: &Keypair, first: Option<u64>, out: Option<&Path>) -> CliResult<()> {
    let chain_id = rpc.chain_id()?;
    let (owners, next) = standing(rpc, side)?;
    let first = first.map_or(next, u256::from);
    let mut bundles = Vec::new();

    for (call, offset) in calls.iter().zip(0u64..) {
        let mut bundle = EvmBundle::new(chain_id, side.contract, call.to, call.value, call.input.clone(), first + u256::from(offset));
        sign_as(&mut bundle, keypair)?;
        bundles.push(bundle);
    }

    let signed = signed_by(Some(rpc), &bundles[0], &owners)?;

    if signed == 0 {
        eprintln!("the key is none of the contract's owners: its signature counts for nothing");
    }

    // Sent at once when that is enough and the key can pay the gas: an Ed25519 key cannot.
    if let (true, Ok(secret)) = (signed >= owners.threshold as usize && first == next, evm_secret(keypair)) {
        return submit(rpc, account, side, &bundles, &secret);
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

        let signed = signed_by(Some(rpc), bundle, &owners)?;

        if signed < owners.threshold as usize {
            return Err(format!("{signed} of the contract's owners signed the call at nonce {nonce}, and it needs {}", owners.threshold).into());
        }

        deploy(rpc, account, side, secret)?;
        rpc.send(&Call { to: side.contract, value: u256::ZERO, input: bundle.execute_input(&owners.owners), what: "Execute" }, false, secret)?;
    }

    Ok(())
}

fn show_owners(owners: &EvmStart) -> String {
    let shown: Vec<String> = owners
        .owners
        .iter()
        .map(|owner| match owner.kind {
            EvmKind::Key => show_address(&owner.address),
            EvmKind::Contract => format!("{} (a contract)", show_address(&owner.address)),
            EvmKind::Ed25519(key) => format!("{} (Ed25519 {})", show_address(&owner.address), crate::structs::hex::to_hex(&key)),
        })
        .collect();

    format!("{} of {}", owners.threshold, shown.join(", "))
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
            propose_calls(&rpc(&eth_rpc), account, &side, &[call], keypair, nonce, Some(&out))?;
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
            propose_calls(&rpc, account, &side, &[call], keypair, None, Some(&out))?;
        }
        EvmAction::Sign { bundle: path, yes } => {
            let mut bundles = read(&path)?;
            review(&bundles, yes, &mut std::io::stdin().lock())?;

            for bundle in &mut bundles {
                sign_as(bundle, keypair)?;
            }

            write(&path, &bundles)?;
            let counts: Vec<String> = bundles.iter().map(|bundle| bundle.signers().len().to_string()).collect();
            println!("signed {} call{}; signatures each: {}", bundles.len(), plural(bundles.len()), counts.join(", "));
        }
        EvmAction::Approve { bundle: path, yes } => {
            // This multisig's contract is the owner; its own owners now, or as it starts.
            let side = evm_side(agent, node, account)?;
            let ours = side.now.clone().unwrap_or_else(|| side.start.clone());
            let mut bundles = read(&path)?;
            println!("approving for {}, an owner of the contract each call is to:", show_address(&side.contract));
            review(&bundles, yes, &mut std::io::stdin().lock())?;

            for bundle in &mut bundles {
                let signer = bundle.approve_with(side.contract, keypair)?;

                if !ours.owners.iter().any(|owner| owner.address == signer) {
                    eprintln!("the key is none of {}'s owners: its approval counts for nothing", show_address(&side.contract));
                }
            }

            write(&path, &bundles)?;
            let counts: Vec<String> = bundles.iter().map(|bundle| bundle.nested.iter().find(|nested| nested.owner == side.contract).map_or(0, |nested| nested.signatures.len()).to_string()).collect();
            println!("approved {} call{} for {}; of the {} it needs, each has: {}", bundles.len(), plural(bundles.len()), show_address(&side.contract), ours.threshold, counts.join(", "));
        }
        EvmAction::AddSignature { bundle: path, owner, signature } => {
            let owner = parse_address(&owner)?;
            let signature = from_hex(&signature).ok_or_else(|| format!("{signature} is not 0x and hex"))?;
            let mut bundles = read(&path)?;
            bundles.iter_mut().for_each(|bundle| bundle.add_contract(owner, signature.clone()));
            write(&path, &bundles)?;
            println!("added {}'s signature to {} call{}: submit asks it before counting it", show_address(&owner), bundles.len(), plural(bundles.len()));
        }
        EvmAction::Submit { bundle: path, eth_rpc } => {
            let side = evm_side(agent, node, account)?;
            submit(&rpc(&eth_rpc), account, &side, &read(&path)?, &*evm_secret(keypair)?)?;
        }
    }

    Ok(())
}
