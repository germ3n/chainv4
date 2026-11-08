//! chainv4 prove: validity proofs of ranges of blocks, made by hand on this machine. A range's
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

/// Which range: from the chain's proven tip to its newest final checkpoint, unless told, or an
/// input written before.
#[derive(clap::Args)]
pub struct RangeArgs {
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
    /// The program's verifying key hash, what governance names in Change::Proofs, from the ELF
    /// cargo prove build wrote.
    #[cfg(feature = "sp1-prover")]
    Program {
        /// The program's ELF.
        #[arg(long, default_value = "proofs/sp1/elf/chainv4-proof-program")]
        elf: PathBuf,
    },
    /// Runs a range in SP1 without proving it: what it would prove, and the cycles it takes.
    #[cfg(feature = "sp1-prover")]
    Execute {
        #[command(flatten)]
        range: RangeArgs,
        #[arg(long, default_value = "proofs/sp1/elf/chainv4-proof-program")]
        elf: PathBuf,
    },
    /// Proves a range with SP1 on this machine, in Groth16, and writes the proof file: slow, and
    /// heavy on memory.
    #[cfg(feature = "sp1-prover")]
    Run {
        #[command(flatten)]
        range: RangeArgs,
        #[arg(long, default_value = "proofs/sp1/elf/chainv4-proof-program")]
        elf: PathBuf,
        /// Where to write the proof.
        #[arg(long)]
        out: PathBuf,
    },
}

pub fn prove(args: ProveArgs) -> CliResult<()> {
    let node = args.node.trim_end_matches('/').to_string();
    let agent = waiting(&args.tls, 3, 1_800)?;
    let status = ok(agent.get(format!("{node}/status")).call())?;
    let hash: HashType = status["hash"].as_str().ok_or("the node did not say its chain's hash")?.parse()?;

    // A range's input: from the file given, or built by the node.
    let input = |range: &RangeArgs| -> CliResult<Vec<u8>> {
        if let Some(path) = &range.input {
            return Ok(std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?);
        }

        let from = match range.from {
            Some(from) => from,
            None => status["validity"]["tip"]["height"].as_u64().ok_or("the chain has no proven tip yet: give --from")?,
        };
        let to = match range.to {
            Some(to) => to,
            None => status["finality"]["finalized"]["height"].as_u64().ok_or("the chain's finality names no final checkpoint: give --to")?,
        };
        println!("the range from {from} to {to}: asking the node for its input");
        let mut response = agent.get(format!("{node}/validity/input?from={from}&to={to}")).call().map_err(|err| format!("cannot reach the node: {err}"))?;

        match response.status().as_u16() {
            200 => Ok(response.body_mut().with_config().limit(1 << 30).read_to_vec()?),
            code => Err(format!("GET /validity/input: {code} {}", response.body_mut().read_to_string().unwrap_or_default()).into()),
        }
    };

    match args.action {
        ProveAction::Status => {
            let validity = &status["validity"];
            println!("{}", serde_json::to_string_pretty(&json!({
                "settings": validity["settings"],
                "tip": validity["tip"],
                "finalized": status["finality"]["finalized"],
            }))?);
        }
        ProveAction::Input { range, out } => {
            let bytes = input(&range)?;
            let decoded = RangeInput::from_bytes(&bytes)?;
            std::fs::write(&out, &bytes).map_err(|err| format!("{}: {err}", out.display()))?;
            println!("wrote {} bytes to {}: {} blocks, {} headers, {}", bytes.len(), out.display(), decoded.blocks.len(), decoded.headers.len(), witness_kind(&decoded));
        }
        ProveAction::Check { range } => {
            let decoded = RangeInput::from_bytes(&input(&range)?)?;
            println!("{}", witness_kind(&decoded));
            let started = std::time::Instant::now();
            let proven = replay(&decoded)?;
            println!("ran off the node in {} ms", started.elapsed().as_millis());
            println!("{}", serde_json::to_string_pretty(&range_json(&proven))?);
        }
        #[cfg(feature = "sp1-prover")]
        ProveAction::Program { elf } => {
            use crate::proofs::Prover as _;

            println!("{}", to_hex(&sp1(&elf)?.program()));
        }
        #[cfg(feature = "sp1-prover")]
        ProveAction::Execute { range, elf } => {
            use crate::proofs::Prover as _;

            let prover = sp1(&elf)?;
            let bytes = input(&range)?;
            let started = std::time::Instant::now();
            let (values, cycles) = prover.execute(&bytes)?;
            let proven = ProvenRange::from_public_values(hash, &values).ok_or("the program committed to no range")?;
            println!("ran in SP1 in {} ms: {cycles} cycles", started.elapsed().as_millis());
            println!("{}", serde_json::to_string_pretty(&range_json(&proven))?);
        }
        #[cfg(feature = "sp1-prover")]
        ProveAction::Run { range, elf, out } => {
            use crate::proofs::Prover as _;

            let prover = sp1(&elf)?;
            let bytes = input(&range)?;
            let started = std::time::Instant::now();
            println!("proving with SP1 on this machine: this takes a while");
            let (values, proof) = prover.prove(&bytes)?;
            let proven = ProvenRange::from_public_values(hash, &values).ok_or("the program committed to no range")?;
            let file = json!({
                "kind": prover.kind().name(),
                "program": to_hex(&prover.program()),
                "public_values": to_hex(&values),
                "proof": to_hex(&proof),
                "range": range_json(&proven),
            });
            std::fs::write(&out, serde_json::to_string_pretty(&file)?).map_err(|err| format!("{}: {err}", out.display()))?;
            println!("proved in {} s, {} bytes of proof: {}", started.elapsed().as_secs(), proof.len(), out.display());
        }
    }

    let _ = hash;

    Ok(())
}

#[cfg(feature = "sp1-prover")]
fn sp1(elf: &std::path::Path) -> CliResult<crate::proofs::Sp1Prover> {
    let bytes = std::fs::read(elf).map_err(|err| format!("{}: {err}: build it with cargo prove build in proofs/sp1/program", elf.display()))?;

    Ok(crate::proofs::Sp1Prover::new(bytes)?)
}

fn witness_kind(input: &RangeInput) -> String {
    match &input.witness {
        crate::proofs::Witness::Partial { nodes, entries } => format!("a witness of {} entries and {} tree nodes", entries.len(), nodes.len()),
        crate::proofs::Witness::Whole { entries } => format!("the whole state, {} entries, as the range scans", entries.len()),
    }
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
