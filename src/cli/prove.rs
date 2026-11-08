//! chainv4 prove: validity proofs made by hand on this machine, of ranges of blocks (--of range), of
//! a link of finality for Ethereum (--of finality), or of the header chain for the apps (--of
//! headers). A range's
//! input comes from a node (GET /validity/input); check runs it off the node with no zkVM, as a
//! check that a proof of it would hold; and under the sp1-prover feature, execute runs it in SP1
//! and run proves it, writing a proof file that chainv4 send prove submits to the chain and
//! chainv4 bridge finality-prove to Ethereum.

use {
    super::CliResult,
    super::send::{TlsArgs, ok, waiting},
    crate::proofs::{ProvenRange, RangeInput, replay},
    crate::structs::hash::HashType,
    crate::structs::hex::to_hex,
    serde_json::{Value, json},
    std::path::PathBuf,
};

#[derive(clap::Args)]
pub struct ProveArgs {
    /// The node's HTTP API, an archive or one whose --state-history reaches the range's start.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    #[command(flatten)]
    pub tls: TlsArgs,
    #[command(subcommand)]
    pub action: ProveAction,
}

/// What a proof proves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Of {
    /// A range of blocks: the state they come to.
    Range,
    /// A link of finality: two thirds of an epoch's voters signing it, for ChainV4Finality.sol.
    Finality,
    /// The header chain: finalized checkpoints from genesis, for the apps and Tx::ProveHeaders.
    Headers,
}

impl Of {
    #[cfg(feature = "sp1-prover")]
    fn name(self) -> &'static str {
        match self {
            Of::Range => "range",
            Of::Finality => "finality",
            Of::Headers => "headers",
        }
    }

    /// The program's ELF, as proofs/sp1/build.sh writes it.
    #[cfg(feature = "sp1-prover")]
    fn elf(self) -> &'static str {
        match self {
            Of::Range => "proofs/sp1/elf/chainv4-proof-program",
            Of::Finality => "proofs/sp1/elf/chainv4-finality-program",
            Of::Headers => "proofs/sp1/elf/chainv4-headers-program",
        }
    }
}

/// Which range: from the chain's proven tip to its newest final checkpoint, unless told, or an
/// input written before. For a finality proof, the link to --epoch, the newest justified unless
/// given; for the header chain, on from the chain's last proof to --epoch final, the newest
/// finalized unless given.
#[derive(clap::Args)]
pub struct RangeArgs {
    /// What to prove.
    #[arg(long, value_enum, default_value = "range")]
    pub of: Of,
    /// For --of finality the link's target epoch; for --of headers the epoch to finalize.
    #[arg(long)]
    pub epoch: Option<u64>,
    /// For --of headers: go on from this proof file, one chainv4 prove run wrote that the chain
    /// has not taken, rather than from the chain's last; a stretch older than the chain keeps is
    /// proven so, a proof at a time, and the last one sent.
    #[arg(long, conflicts_with = "input")]
    pub previous: Option<PathBuf>,
    /// The height the range starts at: the chain's proven tip unless given.
    #[arg(long)]
    pub from: Option<u64>,
    /// The height it ends at, a checkpoint's: the newest finalized unless given.
    #[arg(long)]
    pub to: Option<u64>,
    /// An input chainv4 prove input wrote, in place of asking the node.
    #[arg(long, conflicts_with_all = ["from", "to"])]
    pub input: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
pub enum ProveAction {
    /// What the chain has of proofs: governance's settings, the proven tip, and the newest final
    /// checkpoint a proof could end at.
    Status,
    /// Fetches a range's input from the node and writes it, to prove later or elsewhere.
    Input {
        #[command(flatten)]
        range: RangeArgs,
        /// Where to write it.
        #[arg(long)]
        out: PathBuf,
    },
    /// Runs a range off the node, with no zkVM, and says what a proof of it would prove: the
    /// check that it would, in a moment rather than the hours a proof takes.
    Check {
        #[command(flatten)]
        range: RangeArgs,
    },
    /// The program's verifying key hash, what governance names in Change::Proofs (program,
    /// finality_program or headers_program), from the ELF proofs/sp1/build.sh wrote.
    #[cfg(feature = "sp1-prover")]
    Program {
        /// Which program.
        #[arg(long, value_enum, default_value = "range")]
        of: Of,
        /// The program's ELF, the one for --of unless given.
        #[arg(long)]
        elf: Option<PathBuf>,
    },
    /// Runs it in SP1 without proving it: what it would prove, and the cycles it takes.
    #[cfg(feature = "sp1-prover")]
    Execute {
        #[command(flatten)]
        range: RangeArgs,
        /// The program's ELF, the one for --of unless given.
        #[arg(long)]
        elf: Option<PathBuf>,
    },
    /// Proves a range with SP1 on this machine and writes the proof file: slow, and heavy on
    /// memory. Groth16 and PLONK download SP1's circuit the first time, gigabytes of it.
    #[cfg(feature = "sp1-prover")]
    Run {
        #[command(flatten)]
        range: RangeArgs,
        /// The program's ELF, the one for --of unless given.
        #[arg(long)]
        elf: Option<PathBuf>,
        /// The kind of proof: sp1-groth16 or sp1-plonk, which Ethereum and the chain take, or
        /// sp1-stark, which only the chain does. One governance names, or the chain refuses it.
        #[arg(long, default_value = "sp1-groth16", value_parser = proof_kind)]
        kind: crate::proofs::ProofKind,
        /// Where to write the proof.
        #[arg(long)]
        out: PathBuf,
    },
}

pub fn prove(args: ProveArgs) -> CliResult<()> {
    let node = args.node.trim_end_matches('/').to_string();
    let agent = waiting(&args.tls, 3, 1_800)?;
    // The node's status, asked for once and only by what needs the node.
    let asked = std::cell::OnceCell::new();
    let status = || -> CliResult<&Value> {
        if asked.get().is_none() {
            let _ = asked.set(ok(agent.get(format!("{node}/status")).call())?);
        }

        Ok(asked.get().expect("set just now"))
    };
    let hash = || -> CliResult<HashType> { Ok(status()?["hash"].as_str().ok_or("the node did not say its chain's hash")?.parse()?) };

    // The input: from the file given, or built by the node.
    let input = |range: &RangeArgs| -> CliResult<Vec<u8>> {
        if let Some(path) = &range.input {
            return Ok(std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?);
        }

        let status = status()?;
        let url = match range.of {
            Of::Range => {
                let from = match range.from {
                    Some(from) => from,
                    None => status["validity"]["tip"]["height"].as_u64().ok_or("the chain has no proven tip yet: give --from")?,
                };
                let to = match range.to {
                    Some(to) => to,
                    None => status["finality"]["finalized"]["height"].as_u64().ok_or("the chain's finality names no final checkpoint: give --to")?,
                };
                println!("the range from {from} to {to}: asking the node for its input");

                format!("{node}/validity/input?from={from}&to={to}")
            }
            Of::Finality => {
                let epoch = match range.epoch {
                    Some(epoch) => epoch,
                    None => status["finality"]["justified"]["epoch"].as_u64().ok_or("the chain's finality names no justified checkpoint: give --epoch")?,
                };
                println!("the link to epoch {epoch}: asking the node for its input");

                format!("{node}/validity/finality-input?epoch={epoch}")
            }
            Of::Headers => {
                let epoch = match range.epoch {
                    Some(epoch) => epoch,
                    None => status["finality"]["finalized"]["epoch"].as_u64().ok_or("the chain's finality names no final checkpoint: give --epoch")?,
                };
                println!("the header chain to epoch {epoch}: asking the node for its input");

                match previous_justified(range)? {
                    Some(from) => format!("{node}/validity/headers-input?to={epoch}&from={from}"),
                    None => format!("{node}/validity/headers-input?to={epoch}"),
                }
            }
        };
        let mut response = agent.get(url).call().map_err(|err| format!("cannot reach the node: {err}"))?;

        let bytes = match response.status().as_u16() {
            200 => response.body_mut().with_config().limit(1 << 30).read_to_vec()?,
            code => return Err(format!("{code} {}", response.body_mut().read_to_string().unwrap_or_default()).into()),
        };

        with_previous(range, bytes)
    };

    match args.action {
        ProveAction::Status => {
            let status = status()?;
            let validity = &status["validity"];
            println!("{}", serde_json::to_string_pretty(&json!({
                "settings": validity["settings"],
                "tip": validity["tip"],
                "headers": validity["headers"].is_object(),
                "justified": status["finality"]["justified"],
                "finalized": status["finality"]["finalized"],
            }))?);
        }
        ProveAction::Input { range, out } => {
            let bytes = input(&range)?;
            let what = match range.of {
                Of::Range => {
                    let decoded = RangeInput::from_bytes(&bytes)?;
                    format!("{} blocks, {} headers, {}", decoded.blocks.len(), decoded.headers.len(), witness_kind(&decoded))
                }
                of => described(of, &bytes)?,
            };
            std::fs::write(&out, &bytes).map_err(|err| format!("{}: {err}", out.display()))?;
            println!("wrote {} bytes to {}: {what}", bytes.len(), out.display());
        }
        ProveAction::Check { range } => {
            let bytes = input(&range)?;
            let started = std::time::Instant::now();
            let proven = match range.of {
                Of::Range => {
                    let decoded = RangeInput::from_bytes(&bytes)?;
                    println!("{}", witness_kind(&decoded));
                    range_json(&replay(&decoded)?)
                }
                of => checked(of, &bytes)?,
            };
            println!("ran off the node in {} ms", started.elapsed().as_millis());
            println!("{}", serde_json::to_string_pretty(&proven)?);
        }
        #[cfg(feature = "sp1-prover")]
        ProveAction::Program { of, elf } => {
            use crate::proofs::Prover as _;

            println!("{}", to_hex(&sp1(&elf.unwrap_or_else(|| of.elf().into()), crate::proofs::ProofKind::Sp1Groth16)?.program()));
        }
        #[cfg(feature = "sp1-prover")]
        ProveAction::Execute { range, elf } => {
            use crate::proofs::Prover as _;

            let prover = sp1(&elf.unwrap_or_else(|| range.of.elf().into()), crate::proofs::ProofKind::Sp1Groth16)?;
            let bytes = input(&range)?;
            let started = std::time::Instant::now();
            let (values, cycles) = prover.execute(&bytes)?;
            println!("ran in SP1 in {} ms: {cycles} cycles", started.elapsed().as_millis());
            println!("{}", serde_json::to_string_pretty(&values_json(range.of, hash()?, &values)?)?);
        }
        #[cfg(feature = "sp1-prover")]
        ProveAction::Run { range, elf, kind, out } => {
            use crate::proofs::Prover as _;

            if let Ok(status) = status()
                && let Some(kinds) = status["validity"]["settings"]["kinds"].as_array()
                && !kinds.iter().any(|named| named.as_str() == Some(kind.name()))
            {
                println!("note: the chain takes {}, not {}: a proof it refuses, unless governance names it first", kinds.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" or "), kind.name());
            }

            let prover = sp1(&elf.unwrap_or_else(|| range.of.elf().into()), kind)?;
            let bytes = input(&range)?;
            let started = std::time::Instant::now();
            println!("proving with SP1 on this machine, as {}: this takes a while", kind.name());
            let (values, proof) = prover.prove(&bytes)?;
            let file = json!({
                "of": range.of.name(),
                "kind": prover.kind().name(),
                "program": to_hex(&prover.program()),
                "public_values": to_hex(&values),
                "proof": to_hex(&proof),
                "proves": values_json(range.of, hash()?, &values)?,
            });
            std::fs::write(&out, serde_json::to_string_pretty(&file)?).map_err(|err| format!("{}: {err}", out.display()))?;
            println!("proved in {} s, {} bytes of proof: {}", started.elapsed().as_secs(), proof.len(), out.display());
        }
    }

    let _ = hash;

    Ok(())
}

#[cfg(feature = "sp1-prover")]
fn sp1(elf: &std::path::Path, kind: crate::proofs::ProofKind) -> CliResult<crate::proofs::Sp1Prover> {
    let bytes = std::fs::read(elf).map_err(|err| format!("{}: {err}: build it with proofs/sp1/build.sh", elf.display()))?;

    Ok(crate::proofs::Sp1Prover::new(bytes, kind)?)
}

#[cfg(feature = "sp1-prover")]
fn proof_kind(name: &str) -> Result<crate::proofs::ProofKind, String> {
    crate::proofs::ProofKind::parse(name).ok_or_else(|| format!("{name} is not {}", crate::proofs::ProofKind::ALL.map(crate::proofs::ProofKind::name).join(", ")))
}

fn witness_kind(input: &RangeInput) -> String {
    match &input.witness {
        crate::proofs::Witness::Partial { nodes, entries } => format!("a witness of {} entries and {} tree nodes", entries.len(), nodes.len()),
        crate::proofs::Witness::Whole { entries } => format!("the whole state, {} entries, as the range scans", entries.len()),
    }
}

/// A proof file's kind, public values and proof, as the chain would take them.
fn proof_file(path: &std::path::Path) -> CliResult<(crate::proofs::ProofKind, Vec<u8>, Vec<u8>)> {
    let file: Value = serde_json::from_slice(&std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?).map_err(|err| format!("{}: {err}", path.display()))?;
    let field = |name: &str| file[name].as_str().and_then(crate::structs::hex::from_hex).ok_or_else(|| format!("{}: no {name}", path.display()));
    let kind = file["kind"].as_str().and_then(crate::proofs::ProofKind::parse).ok_or_else(|| format!("{}: no kind of proof this build knows", path.display()))?;

    Ok((kind, field("public_values")?, field("proof")?))
}

/// The epoch of the justified checkpoint a --previous proof ends at.
#[cfg(feature = "header-proofs")]
fn previous_justified(range: &RangeArgs) -> CliResult<Option<u64>> {
    let Some(path) = &range.previous else {
        return Ok(None);
    };
    let (_, values, _) = proof_file(path)?;
    let words = values.len() / 32;
    // ProvenHeaders's eighth word, the justified epoch, whatever the chain's hash.
    let epoch = values.get(7 * 32..8 * 32).filter(|_| words == crate::proofs::ProvenHeaders::WORDS).map(|word| u64::from_be_bytes(word[24..].try_into().expect("8 bytes"))).ok_or_else(|| format!("{}: no header-chain proof", path.display()))?;

    Ok(Some(epoch))
}

#[cfg(not(feature = "header-proofs"))]
fn previous_justified(_range: &RangeArgs) -> CliResult<Option<u64>> {
    Ok(None)
}

/// A header-chain input with the --previous proof put in, as the node built it without.
#[cfg(feature = "header-proofs")]
fn with_previous(range: &RangeArgs, bytes: Vec<u8>) -> CliResult<Vec<u8>> {
    let Some(path) = range.previous.as_ref().filter(|_| range.of == Of::Headers) else {
        return Ok(bytes);
    };
    let (kind, values, proof) = proof_file(path)?;
    let input = crate::proofs::HeadersInput::from_bytes(&bytes)?;

    Ok(crate::proofs::HeadersInput { previous: Some(crate::proofs::TakenHeaders { kind, values, proof }), ..input }.to_bytes())
}

#[cfg(not(feature = "header-proofs"))]
fn with_previous(_range: &RangeArgs, bytes: Vec<u8>) -> CliResult<Vec<u8>> {
    Ok(bytes)
}

/// What a finality or header-chain input holds, in a line.
#[cfg(feature = "header-proofs")]
fn described(of: Of, bytes: &[u8]) -> CliResult<String> {
    Ok(match of {
        Of::Finality => {
            let decoded = crate::proofs::FinalityInput::from_bytes(bytes)?;
            format!("the link from epoch {}, {} votes", decoded.link.source.epoch, decoded.link.votes.len())
        }
        _ => {
            let decoded = crate::proofs::HeadersInput::from_bytes(bytes)?;
            format!("{} links, {}", decoded.links.len(), if decoded.previous.is_some() { "on from the chain's last proof" } else { "from genesis" })
        }
    })
}

/// A finality or header-chain input run off the node, with no zkVM: what a proof of it proves.
#[cfg(feature = "header-proofs")]
fn checked(of: Of, bytes: &[u8]) -> CliResult<Value> {
    Ok(match of {
        Of::Finality => link_json(&crate::proofs::prove_finality(&crate::proofs::FinalityInput::from_bytes(bytes)?)?),
        _ => headers_json(&crate::proofs::prove_headers(&crate::proofs::HeadersInput::from_bytes(bytes)?)?),
    })
}

#[cfg(not(feature = "header-proofs"))]
fn described(_of: Of, _bytes: &[u8]) -> CliResult<String> {
    Err("this build makes no finality or header-chain proofs".into())
}

#[cfg(not(feature = "header-proofs"))]
fn checked(_of: Of, _bytes: &[u8]) -> CliResult<Value> {
    Err("this build makes no finality or header-chain proofs".into())
}

/// What a program committed to, as JSON, read by what it proves.
#[cfg(feature = "sp1-prover")]
fn values_json(of: Of, hash: HashType, values: &[u8]) -> CliResult<Value> {
    Ok(match of {
        Of::Range => range_json(&ProvenRange::from_public_values(hash, values).ok_or("the program committed to no range")?),
        Of::Finality => link_json(&crate::proofs::ProvenLink::from_public_values(hash, values).ok_or("the program committed to no link")?),
        Of::Headers => headers_json(&crate::proofs::ProvenHeaders::from_public_values(hash, values).ok_or("the program committed to no header chain")?),
    })
}

fn checkpoint_json(checkpoint: &crate::structs::vote::Checkpoint) -> Value {
    json!({ "epoch": checkpoint.epoch, "block": checkpoint.block.to_string() })
}

/// A proven link as JSON.
pub(super) fn link_json(link: &crate::proofs::ProvenLink) -> Value {
    json!({
        "chain_id": link.chain_id,
        "genesis": link.genesis.to_string(),
        "source": checkpoint_json(&link.source),
        "source_root": link.source_root.to_string(),
        "target": checkpoint_json(&link.target),
        "height": link.height,
        "state_root": link.state_root.to_string(),
        "next_attesting_set": to_hex(&link.next_attesting_set),
        "finality_program": to_hex(&link.finality_program),
    })
}

/// A proven header chain as JSON.
pub(super) fn headers_json(headers: &crate::proofs::ProvenHeaders) -> Value {
    let point = |point: &crate::proofs::ChainPoint| json!({ "epoch": point.checkpoint.epoch, "block": point.checkpoint.block.to_string(), "height": point.height, "state_root": point.state_root.to_string() });

    json!({
        "chain_id": headers.chain_id,
        "genesis": headers.genesis.to_string(),
        "program": to_hex(&headers.program),
        "finalized": point(&headers.finalized),
        "justified": point(&headers.justified),
    })
}

/// A proven range as JSON, as proof files and the CLI show it.
pub(super) fn range_json(range: &ProvenRange) -> Value {
    let end = |end: &crate::proofs::RangeEnd| json!({ "height": end.height, "block": end.block.to_string(), "state_root": end.state_root.to_string() });

    json!({
        "chain_id": range.chain_id,
        "genesis": range.genesis.to_string(),
        "start": end(&range.start),
        "end": end(&range.end),
        "withdrawals": to_hex(&range.withdrawals),
        "program": to_hex(&range.program),
        "gateway": range.gateway.name(),
    })
}
