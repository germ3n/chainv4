//! chainv4 multisig, and send --multisig: a native multisig's transactions, from proposing one to
//! the quorum that runs it. A member proposes with send --multisig, which signs and writes a
//! bundle; the others sign it where their keys are, offline too, or approve it on chain; anyone
//! submits it once enough back it, the multisig paying its fee.

use {
    super::send::{SendArgs, TlsArgs, Wanted, agent, offer, ok, report, transaction, wait},
    crate::chain::fees::OfferChoice,
    super::{CliResult, signer},
    crate::crypto::keypair::Keypair,
    crate::registry::multisig::{MultisigBody, MultisigBundle},
    crate::structs::address::Address,
    crate::structs::hash::{Hash, HashType},
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::{FeeOffer, SignedTx, Tx},
    serde_json::{Value, json},
    std::path::{Path, PathBuf},
    std::time::Duration,
    ureq::Agent,
};

#[cfg(all(feature = "swap", feature = "abi"))]
pub(crate) mod evm;

#[derive(clap::Args)]
pub struct MultisigArgs {
    /// The node's HTTP API: http://, or https:// behind TLS.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    #[command(flatten)]
    pub tls: TlsArgs,
    /// The key file to sign, approve or carry with, from keygen. Without one, the well-known
    /// development key.
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// How long to wait for a transaction to land, in seconds.
    #[arg(long, default_value_t = 60)]
    pub timeout: u64,
    #[command(subcommand)]
    pub action: MultisigAction,
}

#[derive(clap::Subcommand)]
pub enum MultisigAction {
    /// A multisig's members, threshold, next nonce and balance, and the proposals open for that
    /// nonce with who approves each.
    Show {
        /// The multisig, multisig:0x….
        account: Address,
    },
    /// Adds --key's signature to a bundle, as a member, and writes it back. Needs no node: the
    /// bundle names its chain.
    Sign {
        /// The bundle's file, from send --multisig.
        bundle: PathBuf,
        /// Submit it too, once it holds enough signatures with the approvals on chain.
        #[arg(long)]
        submit: bool,
    },
    /// Approves a bundle's transaction on chain, from --key's own account at its own cost, and
    /// submits it once the approvals and the bundle's signatures are enough.
    Approve {
        /// The bundle's file, from send --multisig.
        bundle: PathBuf,
    },
    /// Takes back --key's approval of an open proposal.
    Withdraw {
        /// The multisig.
        account: Address,
        /// The proposal's id, as show lists it.
        #[arg(long)]
        proposal: String,
    },
    /// Submits a bundle that its signatures and the approvals on chain back enough, carried by
    /// --key, which pays nothing: the multisig pays its fee.
    Submit {
        /// The bundle's file.
        bundle: PathBuf,
    },
    /// The multisig's coins on Bitcoin, for a multisig whose members name xpubs: its addresses and
    /// descriptors, its balance, and spends as PSBTs its members sign here, in Sparrow, Nunchuk or
    /// on the offline signer, then combined and broadcast.
    #[cfg(feature = "bitcoin")]
    Bitcoin {
        /// The multisig, multisig:0x….
        account: Address,
        #[command(subcommand)]
        action: BitcoinAction,
    },
    /// The multisig's contract on EVM chains, for a multisig whose members name EVM addresses: a
    /// MultiSig.sol at one address on every chain, its owners their EVM keys. Its address, its
    /// deploy, and calls as bundles its owners sign, then sent.
    #[cfg(all(feature = "swap", feature = "abi"))]
    Evm {
        /// The multisig, multisig:0x….
        account: Address,
        #[command(subcommand)]
        action: evm::EvmAction,
    },
}

#[cfg(feature = "bitcoin")]
#[derive(clap::Subcommand)]
pub enum BitcoinAction {
    /// A receiving address, or with --change a change address: number --index, from 0.
    Address {
        #[arg(long)]
        change: bool,
        #[arg(long, default_value_t = 0)]
        index: u32,
    },
    /// Its descriptors, receiving and change, with their checksums: what Sparrow, Nunchuk or
    /// Bitcoin Core imports to watch it.
    Descriptor,
    /// What its addresses hold, as an Esplora API finds them: every address until 20 in a row
    /// were never used, receiving and change.
    Balance {
        /// An Esplora API, such as https://mempool.space/api or https://mempool.space/testnet4/api.
        #[arg(long)]
        esplora: String,
    },
    /// Builds a spend as a PSBT, from its coins, largest first, its change to its next unused
    /// change address, and writes it for the members to sign.
    Send {
        #[arg(long)]
        esplora: String,
        /// Whom to pay: a native segwit address, bc1q… or tb1q….
        #[arg(long)]
        to: String,
        /// How much, in satoshis.
        #[arg(long)]
        amount: u64,
        /// The fee rate, in satoshis a virtual byte.
        #[arg(long, default_value_t = 2)]
        fee_rate: u64,
        /// Where to write the PSBT, in base64.
        #[arg(long)]
        out: PathBuf,
    },
    /// Signs a PSBT as a member, with its seed phrase read from input: the inputs it names this
    /// seed's keys for, at m/48'/coin'/0'/2'.
    Sign {
        psbt: PathBuf,
        /// Ask for the seed's passphrase too.
        #[arg(long)]
        passphrase: bool,
    },
    /// Adds the signatures in other members' PSBTs of the same spend to the first.
    Combine {
        psbt: PathBuf,
        #[arg(required = true)]
        others: Vec<PathBuf>,
    },
    /// Finishes a PSBT that holds enough signatures into the transaction, and broadcasts it
    /// through --esplora, or prints it.
    Finish {
        psbt: PathBuf,
        #[arg(long)]
        esplora: Option<String>,
    },
}

pub fn multisig(args: MultisigArgs) -> CliResult<()> {
    let keypair = signer(args.key.as_deref())?;
    let node = args.node.trim_end_matches('/').to_string();
    let agent = agent(&args.tls)?;
    let timeout = Duration::from_secs(args.timeout);

    match args.action {
        MultisigAction::Show { account } => show(&agent, &node, &account),
        MultisigAction::Sign { bundle: path, submit } => {
            let mut bundle = read(&path)?;
            bundle.sign(&keypair);
            write(&path, &bundle)?;
            println!("signed as {}: {} signature{} now", keypair.public(), bundle.signatures.len(), plural(bundle.signatures.len()));

            match submit {
                true => submit_if_backed(&agent, &node, &bundle, &keypair, timeout),
                false => Ok(()),
            }
        }
        MultisigAction::Approve { bundle: path } => {
            let bundle = read(&path)?;
            let body = &bundle.body;
            let approval = Tx::ApproveMultisig { account: body.account, nonce: body.nonce, fee: body.fee, tx: Box::new(body.tx.clone()) };
            println!("approving {} of {} on chain", body.id(&bundle.genesis), body.account);
            send_own(&agent, &node, &keypair, approval, timeout)?;

            submit_if_backed(&agent, &node, &bundle, &keypair, timeout)
        }
        MultisigAction::Withdraw { account, proposal } => {
            let proposal = parse_id(&agent, &node, &proposal)?;

            send_own(&agent, &node, &keypair, Tx::WithdrawMultisig { account, proposal }, timeout)
        }
        #[cfg(feature = "bitcoin")]
        MultisigAction::Bitcoin { account, action } => bitcoin(&agent, &node, &account, action),
        #[cfg(all(feature = "swap", feature = "abi"))]
        MultisigAction::Evm { account, action } => evm::evm(&agent, &node, &account, &keypair, action),
        MultisigAction::Submit { bundle: path } => {
            let bundle = read(&path)?;
            let (backed, needed) = backing(&agent, &node, &bundle)?;

            if backed < needed {
                return Err(format!("{backed} of the multisig's members back it, and it needs {needed}").into());
            }

            submit(&agent, &node, &bundle, &keypair, timeout)
        }
    }
}

/// send --multisig: builds the transaction as the multisig, sizes its fee by a dry run, signs it
/// as --key, and submits it if that is enough, or writes the bundle for the others.
pub(super) fn propose(args: SendArgs, account: Address) -> CliResult<()> {
    let keypair = signer(args.key.as_deref())?;
    let node = args.node.trim_end_matches('/');
    let agent = agent(&args.tls)?;
    let status = ok(agent.get(format!("{node}/status")).call())?;
    let genesis = genesis(&status)?;
    let multisig = ok(agent.get(format!("{node}/multisig/{account}")).call())?;
    let nonce = multisig["nonce"].as_u64().ok_or("the node sent no nonce")?;
    let threshold = multisig["threshold"].as_u64().ok_or("the node sent no threshold")? as usize;
    let tx = transaction(args.tx, account, &agent, node)?;

    // The probe carries as many signatures as the quorum will, so it costs what the real one will.
    let mut sized = MultisigBundle { genesis, body: MultisigBody { account, tx: tx.clone(), nonce, fee: FeeOffer::NONE }, signatures: Vec::new() };
    sized.sign(&keypair);
    sized.signatures = std::iter::repeat_n(sized.signatures[0].clone(), threshold.max(1)).collect();
    let chosen = OfferChoice { gas_limit: args.gas_limit, max_price: args.max_price, tip: args.tip };
    let (fee, failure, _) = offer(&agent, node, &sized.envelope(&keypair), chosen)?;

    if let Some(error) = failure {
        if !args.anyway {
            return Err(format!("a dry run says this would fail: {error}. Propose it anyway with --anyway").into());
        }

        eprintln!("a dry run says this would fail: {error}; proposing it anyway");
    }

    let mut bundle = MultisigBundle { genesis, body: MultisigBody { account, tx, nonce, fee }, signatures: Vec::new() };
    bundle.sign(&keypair);
    let (backed, needed) = backing(&agent, node, &bundle)?;

    if backed >= needed {
        return submit(&agent, node, &bundle, &keypair, Duration::from_secs(args.timeout));
    }

    match &args.out {
        Some(path) => {
            write(path, &bundle)?;
            println!("signed 1 of the {needed} it needs, as {}; bundle {} in {}", keypair.public(), bundle.body.id(&genesis), path.display());
            println!("next: chainv4 multisig sign {} --key <member.key>, or approve it on chain", path.display());
        }
        None => println!("{}", bundle.to_text()),
    }

    Ok(())
}

/// The members backing a bundle now, its signatures and the approvals on chain together, and
/// how many it needs.
fn backing(agent: &Agent, node: &str, bundle: &MultisigBundle) -> CliResult<(usize, usize)> {
    let multisig = ok(agent.get(format!("{node}/multisig/{}", bundle.body.account)).call())?;
    let needed = multisig["threshold"].as_u64().ok_or("the node sent no threshold")? as usize;
    let id = bundle.body.id(&bundle.genesis).to_string();
    let mut backers: Vec<String> = bundle.signatures.iter().map(|signed| signed.key.to_string()).collect();

    if multisig["nonce"].as_u64() != Some(bundle.body.nonce) {
        return Err(format!("the multisig's next nonce is {}, and the bundle is for {}", multisig["nonce"], bundle.body.nonce).into());
    }

    let open = multisig["proposals"].as_array().into_iter().flatten().find(|proposal| proposal["id"].as_str() == Some(id.as_str()));

    for approver in open.and_then(|proposal| proposal["approvals"].as_array()).into_iter().flatten() {
        if let Some(key) = approver.as_str().filter(|key| !backers.iter().any(|known| known == key)) {
            backers.push(key.to_string());
        }
    }

    Ok((backers.len(), needed))
}

fn submit_if_backed(agent: &Agent, node: &str, bundle: &MultisigBundle, carrier: &Keypair, timeout: Duration) -> CliResult<()> {
    let (backed, needed) = backing(agent, node, bundle)?;

    match backed >= needed {
        true => submit(agent, node, bundle, carrier, timeout),
        false => {
            println!("{backed} of the {needed} it needs back it so far");

            Ok(())
        }
    }
}

fn submit(agent: &Agent, node: &str, bundle: &MultisigBundle, carrier: &Keypair, timeout: Duration) -> CliResult<()> {
    let signed = bundle.envelope(carrier);
    println!("submitting {}'s transaction {}", bundle.body.account, bundle.body.nonce);

    post_and_wait(agent, node, &signed, timeout)
}

/// Sends one of --key's own transactions, with the fee a dry run of it says.
pub(super) fn send_own(agent: &Agent, node: &str, keypair: &Keypair, tx: Tx, timeout: Duration) -> CliResult<()> {
    let sender = keypair.public().address();
    let nonce = ok(agent.get(format!("{node}/accounts/{sender}")).call())?["nonce"].as_u64().ok_or("the node sent no nonce")?;
    let (fee, failure, _) = offer(agent, node, &SignedTx::sign(tx.clone(), nonce, FeeOffer::NONE, keypair), Default::default())?;

    if let Some(error) = failure {
        return Err(format!("a dry run says this would fail: {error}").into());
    }

    post_and_wait(agent, node, &SignedTx::sign(tx, nonce, fee, keypair), timeout)
}

fn post_and_wait(agent: &Agent, node: &str, signed: &SignedTx, timeout: Duration) -> CliResult<()> {
    let body = json!({ "tx": to_hex(&wincode::serialize(signed)?) }).to_string();
    let submitted = ok(agent.post(format!("{node}/txs")).header("content-type", "application/json").send(body))?;
    let id = submitted["id"].as_str().ok_or("the node sent no transaction id")?;
    println!("submitted {id}");

    report(&wait(agent, node, id, Wanted { confirmations: 1, finalized: false }, timeout)?)
}

fn show(agent: &Agent, node: &str, account: &Address) -> CliResult<()> {
    let multisig = ok(agent.get(format!("{node}/multisig/{account}")).call())?;
    let members = multisig["members"].as_array().cloned().unwrap_or_default();

    println!("{account}: {} of {} members, next nonce {}, balance {}", multisig["threshold"], members.len(), multisig["nonce"], multisig["balance"].as_str().unwrap_or("?"));

    for member in &members {
        let others = match (member["bitcoin"].as_str(), member["evm"].as_str()) {
            (Some(bitcoin), Some(evm)) => format!(", Bitcoin {bitcoin}, EVM {evm}"),
            _ => String::new(),
        };
        println!("  {}{others}", member["key"].as_str().unwrap_or("?"));
    }

    for proposal in multisig["proposals"].as_array().into_iter().flatten() {
        let approvals = proposal["approvals"].as_array().map_or(0, Vec::len);
        println!("proposal {}: {}, approved by {approvals}", proposal["id"].as_str().unwrap_or("?"), proposal["tx"]["type"].as_str().unwrap_or("?"));
    }

    Ok(())
}

/// The genesis id GET /status names, which members sign over.
fn genesis(status: &Value) -> CliResult<Hash> {
    let hash: HashType = status["hash"].as_str().and_then(|name| name.parse().ok()).ok_or("the node sent no hash")?;

    Ok(status["genesis"].as_str().and_then(from_hex).and_then(|bytes| Hash::from_bytes(hash, &bytes)).ok_or("the node sent no genesis id that reads")?)
}

/// A proposal's id as show prints it, read with the chain's hash.
fn parse_id(agent: &Agent, node: &str, text: &str) -> CliResult<Hash> {
    let hash: HashType = ok(agent.get(format!("{node}/status")).call())?["hash"].as_str().and_then(|name| name.parse().ok()).ok_or("the node sent no hash")?;

    Ok(from_hex(text).and_then(|bytes| Hash::from_bytes(hash, &bytes)).ok_or_else(|| format!("{text} is not a proposal id"))?)
}

fn read(path: &Path) -> CliResult<MultisigBundle> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;

    Ok(MultisigBundle::from_text(&text).ok_or_else(|| format!("{} holds no multisig bundle", path.display()))?)
}

fn write(path: &Path, bundle: &MultisigBundle) -> CliResult<()> {
    std::fs::write(path, bundle.to_text() + "\n").map_err(|err| format!("{}: {err}", path.display()))?;

    Ok(())
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// A multisig's side on Bitcoin, as the node says its members' xpubs are.
#[cfg(feature = "bitcoin")]
pub(super) fn bitcoin_wallet(agent: &Agent, node: &str, account: &Address) -> CliResult<crate::crypto::bitcoin::psbt::MultisigWallet> {
    use crate::crypto::bitcoin::{multisig::parse_xpub, tx::Network};

    let multisig = ok(agent.get(format!("{node}/multisig/{account}")).call())?;
    let network = match multisig["testnet"].as_bool() {
        Some(true) => Network::Test,
        _ => Network::Main,
    };
    let xpubs = multisig["members"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|member| member["bitcoin"].as_str().ok_or("its members name no Bitcoin xpubs: it has no side on Bitcoin").map(parse_xpub))
        .collect::<Result<Result<Vec<_>, _>, _>>()??;

    Ok(crate::crypto::bitcoin::psbt::MultisigWallet {
        threshold: u32::try_from(multisig["threshold"].as_u64().ok_or("the node sent no threshold")?)?,
        xpubs: xpubs.into_iter().map(|(xpub, _)| xpub).collect(),
        network,
    })
}

#[cfg(feature = "bitcoin")]
fn bitcoin(agent: &Agent, node: &str, account: &Address, action: BitcoinAction) -> CliResult<()> {
    use crate::crypto::bitcoin::multisig::{bitcoin_address, descriptor};
    use crate::crypto::bitcoin::tx::script_of;

    match action {
        BitcoinAction::Address { change, index } => {
            let wallet = bitcoin_wallet(agent, node, account)?;
            println!("{}", bitcoin_address(wallet.threshold, &wallet.xpubs, wallet.network, u32::from(change), index).ok_or("a member's key that does not derive")?);
        }
        BitcoinAction::Descriptor => {
            let wallet = bitcoin_wallet(agent, node, account)?;
            println!("receiving {}", descriptor(wallet.threshold, &wallet.xpubs, wallet.network, 0));
            println!("change    {}", descriptor(wallet.threshold, &wallet.xpubs, wallet.network, 1));
        }
        BitcoinAction::Balance { esplora } => {
            let wallet = bitcoin_wallet(agent, node, account)?;
            let (utxos, next) = scan(agent, &esplora, &wallet)?;
            let held: u64 = utxos.iter().map(|utxo| utxo.coin.value).sum();
            println!("{held} satoshis in {} coin{}; next receiving address {}", utxos.len(), plural(utxos.len()), next.0);
        }
        BitcoinAction::Send { esplora, to, amount, fee_rate, out } => {
            let wallet = bitcoin_wallet(agent, node, account)?;
            let (utxos, (_, change)) = scan(agent, &esplora, &wallet)?;
            let psbt = wallet.spend(&utxos, script_of(&to, wallet.network)?, amount, fee_rate, change)?;
            std::fs::write(&out, psbt.to_base64() + "\n")?;
            println!("a spend of {amount} satoshis to {to}, from {} coin{}, in {}", psbt.tx.inputs.len(), plural(psbt.tx.inputs.len()), out.display());
            println!("next: each member signs it, chainv4 multisig bitcoin {account} sign {}, or in their own wallet", out.display());
        }
        BitcoinAction::Sign { psbt: path, passphrase } => sign_file(&path, &read_seed(passphrase)?)?,
        BitcoinAction::Combine { psbt: path, others } => {
            let mut psbt = read_psbt(&path)?;

            for other in &others {
                psbt.combine(&read_psbt(other)?)?;
            }

            std::fs::write(&path, psbt.to_base64() + "\n")?;
            println!("{}", progress(&psbt));
        }
        BitcoinAction::Finish { psbt: path, esplora } => {
            let tx = read_psbt(&path)?.finish_multisig()?;
            let hex = to_hex(&tx.encode());
            let hex = hex.trim_start_matches("0x");

            match esplora {
                Some(esplora) => {
                    let mut answer = agent.post(format!("{}/tx", esplora.trim_end_matches('/'))).send(hex.to_string()).map_err(|err| format!("Esplora would not take it: {err}"))?;
                    println!("broadcast {}", answer.body_mut().read_to_string()?.trim());
                }
                None => println!("{hex}"),
            }
        }
    }

    Ok(())
}

/// Signs a PSBT's file with a member's seed, and writes it back.
#[cfg(feature = "bitcoin")]
pub(super) fn sign_file(path: &Path, seed: &crate::crypto::mnemonic::Seed) -> CliResult<()> {
    let mut psbt = read_psbt(path)?;
    let fingerprint = seed.multisig_account(0).fingerprint;
    let signed = psbt.sign_multisig(fingerprint, |path| match *path {
        [_, coin, _, _, chain, index] => match seed.multisig_key(coin & !(1 << 31), chain, index).private() {
            crate::crypto::keypair::PrivateKey::Bitcoin(secret) => Some(*secret),
            _ => None,
        },
        _ => None,
    })?;
    std::fs::write(path, psbt.to_base64() + "\n")?;
    println!("signed {signed} input{}; {}", plural(signed), progress(&psbt));

    Ok(())
}

/// How far a PSBT's signing has come, input by input.
#[cfg(feature = "bitcoin")]
fn progress(psbt: &crate::crypto::bitcoin::psbt::Psbt) -> String {
    let inputs: Vec<String> = psbt.multisig_progress().iter().map(|(held, needed)| format!("{held} of {needed}")).collect();

    format!("signatures: {}", inputs.join(", "))
}

#[cfg(feature = "bitcoin")]
fn read_psbt(path: &Path) -> CliResult<crate::crypto::bitcoin::psbt::Psbt> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;

    Ok(crate::crypto::bitcoin::psbt::Psbt::from_base64(&text).map_err(|why| format!("{}: {why}", path.display()))?)
}

/// A member's seed, its phrase, and passphrase if asked, read from input.
#[cfg(feature = "bitcoin")]
fn read_seed(passphrase: bool) -> CliResult<crate::crypto::mnemonic::Seed> {
    use std::io::{BufRead, Write};

    let mut lines = std::io::stdin().lock().lines();
    let mut ask = |what: &str| -> CliResult<String> {
        eprint!("{what}: ");
        std::io::stderr().flush()?;

        Ok(lines.next().transpose()?.unwrap_or_default())
    };
    let phrase = crate::crypto::mnemonic::Phrase::parse(&ask("the member's seed phrase")?)?;
    let passphrase = match passphrase {
        true => ask("the passphrase")?,
        false => String::new(),
    };

    Ok(phrase.seed(&passphrase))
}

/// The multisig's coins, as Esplora finds them at its addresses, receiving and change, each
/// chain until 20 in a row were never used; and the first unused address on each.
#[cfg(feature = "bitcoin")]
fn scan(agent: &Agent, esplora: &str, wallet: &crate::crypto::bitcoin::psbt::MultisigWallet) -> CliResult<(Vec<crate::crypto::bitcoin::psbt::Utxo>, (u32, u32))> {
    use crate::crypto::bitcoin::multisig::bitcoin_address;
    use crate::crypto::bitcoin::psbt::{MultisigCoin, Utxo};

    let base = esplora.trim_end_matches('/');
    let get = |path: String| -> CliResult<Value> {
        let mut answer = agent.get(format!("{base}{path}")).call().map_err(|err| format!("Esplora {path}: {err}"))?;

        Ok(serde_json::from_str(&answer.body_mut().read_to_string()?).map_err(|_| format!("Esplora {path}: not JSON"))?)
    };
    let (mut utxos, mut next) = (Vec::new(), [0u32; 2]);

    for chain in 0..2 {
        let (mut index, mut unused) = (0u32, 0);

        while unused < 20 {
            let address = bitcoin_address(wallet.threshold, &wallet.xpubs, wallet.network, chain, index).ok_or("a member's key that does not derive")?;
            let stats = get(format!("/address/{address}"))?;
            let used = stats["chain_stats"]["tx_count"].as_u64().unwrap_or(0) + stats["mempool_stats"]["tx_count"].as_u64().unwrap_or(0) > 0;

            match used {
                true => {
                    unused = 0;
                    next[chain as usize] = index + 1;

                    for utxo in get(format!("/address/{address}/utxo"))?.as_array().into_iter().flatten() {
                        let mut txid: [u8; 32] = from_hex(&format!("0x{}", utxo["txid"].as_str().unwrap_or_default())).and_then(|bytes| bytes.try_into().ok()).ok_or("Esplora sent a txid that is not one")?;
                        // Explorers write a txid backwards.
                        txid.reverse();
                        utxos.push(Utxo {
                            txid,
                            vout: u32::try_from(utxo["vout"].as_u64().unwrap_or(0))?,
                            coin: MultisigCoin { value: utxo["value"].as_u64().unwrap_or(0), chain, index },
                        });
                    }
                }
                false => unused += 1,
            }

            index += 1;
        }
    }

    Ok((utxos, (next[0], next[1])))
}
