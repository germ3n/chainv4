//! chainv4 bridge … btc bitvm: BitVM's side of the Bitcoin bridge by hand (src/bitvm). A depositor's
//! request address; a peg-in's graph for an operator, written as a file every signer presigns; each
//! of the graph's transactions completed from the presignatures; the federation's verdict on a
//! challenge; an operator's fronted withdrawal and a graph's take, shown to the gateway; and a
//! watcher's check of an operator's claim, which says whether to challenge it.

use {
    super::{Bridge, Proof},
    crate::bitvm::{Graph, GraphTerms, request_script},
    crate::cli::CliResult,
    crate::crypto::bitcoin::tx::{Transaction, p2wsh},
    crate::structs::address::Address,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    serde_json::{Value, json},
    std::path::{Path, PathBuf},
};

#[derive(clap::Subcommand)]
pub enum BitvmAction {
    /// A depositor's peg-in request address: the federation's signers, every one, who confirm it
    /// into the peg-in once they presigned its graph, or the depositor alone after --refund-after
    /// blocks, taking it back.
    RequestAddress {
        /// The depositor's compressed secp256k1 key, 0x and 66 hex digits.
        #[arg(long)]
        depositor: String,
        /// Blocks before the depositor may take it back.
        #[arg(long, default_value_t = 1_008)]
        refund_after: u16,
        /// main, test or regtest: the address's network.
        #[arg(long, default_value = "test")]
        network: String,
    },
    /// Shows a confirmed peg-in to the gateway, minting its wBTC to the address it names.
    Deposit { txid: String, vout: u32, recipient: Address },
    /// Writes a peg-in's graph for an operator: what every signer presigns before confirming it.
    Graph {
        /// The confirmed peg-in, txid:vout, as explorers write the txid.
        #[arg(long)]
        peg_in: String,
        /// What it holds, in satoshis.
        #[arg(long)]
        peg_in_value: u64,
        /// The chainv4 address it mints to.
        #[arg(long)]
        recipient: Address,
        /// The operator's compressed secp256k1 key.
        #[arg(long)]
        operator: String,
        /// The chainv4 account the operator fronts withdrawals from, which watchers hold it to.
        #[arg(long)]
        operator_account: Address,
        /// The operator's funding output for the kickoff, a P2WPKH of its key, txid:vout.
        #[arg(long)]
        funding: String,
        #[arg(long)]
        funding_value: u64,
        #[arg(long, default_value_t = 100_000)]
        stake: u64,
        #[arg(long, default_value_t = 2_000)]
        fee: u64,
        #[arg(long, default_value_t = 144)]
        challenge_window: u16,
        #[arg(long, default_value_t = 144)]
        dispute_window: u16,
        /// Where to write it.
        #[arg(long)]
        out: PathBuf,
    },
    /// A signer's presignatures of a graph, with --evm-key, its federation key, checked to be
    /// its own: what it hands the others and the operator.
    Presign {
        #[arg(long)]
        graph: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// A federation member's verdict for the operator on a challenge, with --evm-key: its
    /// signature on take after the challenge, once it checked the operator's withdrawal proof.
    Verdict {
        #[arg(long)]
        graph: PathBuf,
    },
    /// One of the graph's transactions, whole, from every signer's presignatures, the operator's
    /// key as --evm-key where it signs, and the federation's verdicts after a challenge; printed,
    /// or broadcast with --broadcast.
    Finish {
        #[arg(long)]
        graph: PathBuf,
        /// kickoff, take, vindicated or default.
        #[arg(long)]
        step: String,
        /// Every signer's presignature files, in the signers' order.
        #[arg(long, num_args = 1..)]
        presigned: Vec<PathBuf>,
        /// For vindicated: the federation's verdicts, by its keys' order, hex.
        #[arg(long, num_args = 0..)]
        verdict: Vec<String>,
        #[arg(long)]
        broadcast: bool,
    },
    /// An operator, with --key, shows the gateway its payment of a queued withdrawal on Bitcoin.
    Front { withdrawal: u64, txid: String, vout: u32 },
    /// Shows the gateway a graph's transaction that spent peg-ins.
    Taken { txid: String },
    /// A watcher's check of an operator's kickoff: whether the withdrawal it fronted is proven, by
    /// the withdrawal proof given, under the program governance names, and the gateway's record of
    /// it. Says challenge or not; with --challenge-with, prints nothing else to do.
    Check {
        #[arg(long)]
        graph: PathBuf,
        /// The withdrawal the operator fronted.
        #[arg(long)]
        withdrawal: u64,
        /// The withdrawal proof, chainv4 prove run --of withdrawal's file.
        #[arg(long)]
        proof: PathBuf,
    },
}

/// An outpoint as explorers write it, txid:vout, as transactions hold it: the txid reversed.
fn outpoint(text: &str) -> CliResult<([u8; 32], u32)> {
    let (txid, vout) = text.split_once(':').ok_or("an outpoint is txid:vout")?;
    let mut bytes: [u8; 32] = from_hex(&format!("0x{}", txid.trim_start_matches("0x"))).and_then(|bytes| bytes.try_into().ok()).ok_or("a txid is 64 hex digits")?;
    bytes.reverse();

    Ok((bytes, vout.parse().map_err(|_| "a vout is a number")?))
}

fn key(text: &str, what: &str) -> CliResult<[u8; 33]> {
    from_hex(text).and_then(|bytes| bytes.try_into().ok()).filter(crate::crypto::secp256k1::valid_key).ok_or_else(|| format!("{what}: a compressed secp256k1 key, 0x and 66 hex digits").into())
}

fn read_json(path: &Path) -> CliResult<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?).map_err(|err| format!("{}: {err}", path.display()))?)
}

fn hexes(value: &Value) -> Vec<Vec<u8>> {
    value.as_array().into_iter().flatten().filter_map(|item| item.as_str().and_then(from_hex)).collect()
}

/// A graph's terms as its file holds them.
fn terms_json(terms: &GraphTerms) -> Value {
    let point = |(txid, vout): ([u8; 32], u32)| {
        let mut shown = txid;
        shown.reverse();
        format!("{}:{vout}", to_hex(&shown).trim_start_matches("0x"))
    };

    json!({
        "signers": terms.signers.iter().map(|key| to_hex(key)).collect::<Vec<_>>(),
        "federation": terms.federation.iter().map(|key| to_hex(key)).collect::<Vec<_>>(),
        "threshold": terms.threshold,
        "operator": to_hex(&terms.operator),
        "funding": point(terms.funding),
        "funding_value": terms.funding_value,
        "peg_in": point(terms.peg_in),
        "peg_in_value": terms.peg_in_value,
        "peg_in_script": to_hex(&terms.peg_in_script),
        "stake": terms.stake,
        "fee": terms.fee,
        "challenge_window": terms.challenge_window,
        "dispute_window": terms.dispute_window,
    })
}

/// The graph a file holds, built again from its terms.
fn graph_of(path: &Path) -> CliResult<Graph> {
    let file = read_json(path)?;
    let keys = |name: &str| -> CliResult<Vec<[u8; 33]>> { file[name].as_array().into_iter().flatten().map(|item| key(item.as_str().unwrap_or_default(), name)).collect() };
    let whole = |name: &str| -> CliResult<u64> { file[name].as_u64().ok_or_else(|| format!("{}: no {name}", path.display()).into()) };
    let small = |name: &str| -> CliResult<u16> { u16::try_from(whole(name)?).map_err(|_| format!("{}: {name} too large", path.display()).into()) };
    let terms = GraphTerms {
        signers: keys("signers")?,
        federation: keys("federation")?,
        threshold: whole("threshold")? as usize,
        operator: key(file["operator"].as_str().unwrap_or_default(), "operator")?,
        funding: outpoint(file["funding"].as_str().unwrap_or_default())?,
        funding_value: whole("funding_value")?,
        peg_in: outpoint(file["peg_in"].as_str().unwrap_or_default())?,
        peg_in_value: whole("peg_in_value")?,
        peg_in_script: file["peg_in_script"].as_str().and_then(from_hex).ok_or("no peg_in_script")?,
        stake: whole("stake")?,
        fee: whole("fee")?,
        challenge_window: small("challenge_window")?,
        dispute_window: small("dispute_window")?,
    };

    Ok(Graph::build(terms)?)
}

impl Bridge {
    /// The federation in force, as the node says it: its keys, all of which presign, and threshold.
    fn bitvm_federation(&self) -> CliResult<(Vec<[u8; 33]>, usize)> {
        let status = self.status()?;
        let federation = &status["federation"];
        let keys = federation["signers"].as_array().into_iter().flatten().map(|item| key(item.as_str().unwrap_or_default(), "a signer")).collect::<CliResult<Vec<_>>>()?;
        let threshold = federation["threshold"].as_u64().ok_or("the node names no federation")? as usize;

        Ok((keys, threshold))
    }

    pub(super) fn bitvm(&self, action: &BitvmAction) -> CliResult<()> {
        let bytes = TxCallArgumentType::Bytes;
        let number = TxCallArgumentType::U64;
        let proof_args = |proof: Proof| vec![bytes(proof.block.to_vec()), bytes(proof.tx), number(proof.index), bytes(proof.path), bytes(proof.coinbase), bytes(proof.coinbase_path)];

        match action {
            BitvmAction::RequestAddress { depositor, refund_after, network } => {
                let (signers, _) = self.bitvm_federation()?;
                let script = request_script(&signers, &key(depositor, "--depositor")?, *refund_after);
                let hrp = match network.as_str() {
                    "main" => "bc",
                    "regtest" => "bcrt",
                    _ => "tb",
                };
                println!("{}", crate::structs::bech32::encode_segwit(hrp, &p2wsh(&script)[2..]));
                println!("script {}", to_hex(&script));
                println!("every one of the {} signers confirms it once they presigned its graph; you take it back after {refund_after} blocks otherwise", signers.len());

                Ok(())
            }
            BitvmAction::Deposit { txid, vout, recipient } => {
                let mut args = proof_args(self.proof(txid)?);
                args.extend([number(u64::from(*vout)), TxCallArgumentType::Address(*recipient)]);

                self.call(self.btc_gateway()?, "bitvm_deposit", u256::ZERO, args)
            }
            BitvmAction::Graph { peg_in, peg_in_value, recipient, operator, operator_account, funding, funding_value, stake, fee, challenge_window, dispute_window, out } => {
                let (signers, threshold) = self.bitvm_federation()?;
                let status = self.status()?;
                let chain_id = status["chain_id"].as_u64().ok_or("the node names no chain id")?;
                let script = crate::bitvm::peg_in_script(recipient, chain_id, &self.btc_gateway()?, &signers);
                let terms = GraphTerms {
                    federation: signers.clone(),
                    signers,
                    threshold,
                    operator: key(operator, "--operator")?,
                    funding: outpoint(funding)?,
                    funding_value: *funding_value,
                    peg_in: outpoint(peg_in)?,
                    peg_in_value: *peg_in_value,
                    peg_in_script: script,
                    stake: *stake,
                    fee: *fee,
                    challenge_window: *challenge_window,
                    dispute_window: *dispute_window,
                };
                let graph = Graph::build(terms)?;
                let mut file = terms_json(&graph.terms);
                file["operator_account"] = Value::String(operator_account.to_string());
                std::fs::write(out, serde_json::to_string_pretty(&file)?).map_err(|err| format!("{}: {err}", out.display()))?;
                println!("the graph, {} presignatures a signer: {}", Graph::presigns().len(), out.display());

                Ok(())
            }
            BitvmAction::Presign { graph, out } => {
                let graph = graph_of(graph)?;
                let secret = self.evm_secret()?;
                let mine = crate::crypto::secp256k1::public_key(&secret).ok_or("not a secp256k1 key")?;

                if !graph.terms.signers.contains(&mine) {
                    return Err("--evm-key is none of the graph's signers".into());
                }

                let signatures = graph.presign(&secret);
                assert!(graph.check_presigned(&mine, &signatures));
                std::fs::write(out, serde_json::to_string_pretty(&json!({ "signer": to_hex(&mine), "presigned": signatures.iter().map(|signature| to_hex(signature)).collect::<Vec<_>>() }))?).map_err(|err| format!("{}: {err}", out.display()))?;
                println!("presigned as {}: {}", to_hex(&mine), out.display());

                Ok(())
            }
            BitvmAction::Verdict { graph } => {
                let graph = graph_of(graph)?;
                println!("{}", to_hex(&graph.verdict_signature(&*self.evm_secret()?)));

                Ok(())
            }
            BitvmAction::Finish { graph, step, presigned, verdict, broadcast } => {
                let graph = graph_of(graph)?;
                let mut signatures = Vec::new();

                for (place, path) in presigned.iter().enumerate() {
                    let file = read_json(path)?;
                    let theirs = hexes(&file["presigned"]);
                    let signer = graph.terms.signers.get(place).ok_or("more presignature files than signers")?;

                    if !graph.check_presigned(signer, &theirs) {
                        return Err(format!("{}: not signer {}'s presignatures, in the signers' order", path.display(), place + 1).into());
                    }

                    signatures.push(theirs);
                }

                let full = |needed: bool| -> CliResult<()> {
                    match needed && signatures.len() != graph.terms.signers.len() {
                        true => Err(format!("give every signer's presignatures, {} files", graph.terms.signers.len()).into()),
                        false => Ok(()),
                    }
                };
                let tx: Transaction = match step.as_str() {
                    "kickoff" => graph.signed_kickoff(&*self.evm_secret()?),
                    "take" => {
                        full(true)?;
                        graph.signed_take(&signatures, &*self.evm_secret()?)
                    }
                    "vindicated" => {
                        full(true)?;
                        graph.signed_vindicated(&signatures, verdict.iter().filter_map(|text| from_hex(text)).collect())
                    }
                    "default" => {
                        full(true)?;
                        graph.signed_default(&signatures)
                    }
                    other => return Err(format!("{other} is not kickoff, take, vindicated or default; a challenger finishes the challenge with its own inputs").into()),
                };
                let raw = to_hex(&tx.encode());
                let shown = raw.trim_start_matches("0x");

                match broadcast {
                    true => println!("broadcast {}", self.btc_api()?.broadcast(shown)?),
                    false => println!("{shown}"),
                }

                Ok(())
            }
            BitvmAction::Front { withdrawal, txid, vout } => {
                let mut args = vec![number(*withdrawal)];
                args.extend(proof_args(self.proof(txid)?));
                args.push(number(u64::from(*vout)));

                self.call(self.btc_gateway()?, "bitvm_front", u256::ZERO, args)
            }
            BitvmAction::Taken { txid } => self.call(self.btc_gateway()?, "bitvm_taken", u256::ZERO, proof_args(self.proof(txid)?)),
            BitvmAction::Check { graph, withdrawal, proof } => {
                let account = read_json(graph)?["operator_account"].as_str().ok_or("the graph file names no operator_account")?.to_string();

                self.bitvm_check(&account, *withdrawal, proof)
            }
        }
    }

    /// A watcher's check: the withdrawal proof holds under governance's withdrawal program, of this
    /// chain, for the withdrawal the gateway records the graph's operator account fronting.
    fn bitvm_check(&self, account: &str, withdrawal: u64, proof: &Path) -> CliResult<()> {
        let status = self.status()?;
        let hash: crate::structs::hash::HashType = status["hash"].as_str().ok_or("the node did not say its chain's hash")?.parse()?;
        let file = read_json(proof)?;
        let field = |name: &str| file[name].as_str().and_then(from_hex).ok_or_else(|| format!("{}: no {name}", proof.display()));
        let kind = file["kind"].as_str().and_then(crate::proofs::ProofKind::parse).ok_or("a proof of no kind this build knows")?;
        let program: [u8; 32] = status["validity"]["settings"]["withdrawal_program"].as_str().and_then(from_hex).and_then(|bytes| bytes.try_into().ok()).filter(|program| *program != [0; 32]).ok_or("governance names no withdrawal program")?;
        let values = field("public_values")?;
        let gateway = self.btc_gateway()?;
        let verdict = |why: String| -> CliResult<()> {
            println!("challenge: {why}");
            Ok(())
        };

        if let Err(why) = crate::proofs::verifier(kind).ok_or("this build checks no such proofs")?.verify(&program, &values, &field("proof")?) {
            return verdict(format!("the withdrawal proof does not hold: {why}"));
        }

        let proven = crate::proofs::ProvenWithdrawal::from_public_values(hash, &values).ok_or("public values that are no withdrawal")?;
        let record = self.read(&gateway, "withdrawal", json!([{ "type": "u64", "value": withdrawal.to_string() }]))?;
        let script = record.first().and_then(|value| value["value"].as_str()).and_then(from_hex).ok_or("the gateway knows no such withdrawal")?;
        let sats = record.get(1).and_then(|value| value["value"].as_str()?.parse::<u64>().ok()).ok_or("the gateway gave no amount")?;
        let fronted = self.read(&gateway, "fronted", json!([{ "type": "u64", "value": withdrawal.to_string() }]))?;
        let operator = fronted.first().and_then(|value| value["value"].as_str()).map(str::to_string);

        if proven.digest != crate::proofs::btc_withdrawal_digest(&gateway, withdrawal, &script, sats) {
            return verdict("the proof is of another withdrawal".to_string());
        }

        match operator.as_deref() {
            Some(named) if named == account => {
                println!("holds: withdrawal {withdrawal} of {sats} satoshis is proven, finalized by epoch {}, and fronted by the graph's operator {account}", proven.epoch);
                Ok(())
            }
            Some(named) => verdict(format!("withdrawal {withdrawal} was fronted by {named}, not by the graph's operator {account}")),
            None => verdict(format!("no operator fronted withdrawal {withdrawal}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A graph's file reads back to the very graph, so every signer presigns the same transactions,
    /// and its outpoints are as explorers write them.
    #[test]
    fn a_graph_file_reads_back_to_the_same_graph() {
        let key = |byte: u8| crate::crypto::secp256k1::public_key(&[byte; 32]).unwrap();
        let signers = vec![key(11), key(12), key(13)];
        let terms = GraphTerms {
            federation: signers.clone(),
            peg_in_script: crate::bitvm::peg_in_script(&Address::Ed25519([7; 32]), 1_234, &Address::Contract([9; 32]), &signers),
            signers,
            threshold: 2,
            operator: key(21),
            funding: ([2; 32], 1),
            funding_value: 1_000_000,
            peg_in: ([1; 32], 0),
            peg_in_value: 10_000_000,
            stake: 200_000,
            fee: 2_000,
            challenge_window: 6,
            dispute_window: 4,
        };
        let graph = Graph::build(terms.clone()).unwrap();
        let dir = std::env::temp_dir().join(format!("chainv4-bitvm-graph-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("graph.json");
        std::fs::write(&path, serde_json::to_string(&terms_json(&terms)).unwrap()).unwrap();

        assert_eq!(graph_of(&path).unwrap(), graph);
        assert_eq!(terms_json(&terms)["peg_in"], format!("{}:0", "01".repeat(32)));
        assert_eq!(outpoint(&format!("{}:7", "ab".repeat(31) + "cd")).unwrap(), ({ let mut id = [0xab; 32]; id[31] = 0xcd; id.reverse(); id }, 7));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
