//! chainv4 bridge … btc bitvm: BitVM's side of the Bitcoin bridge by hand (src/bitvm). A depositor's
//! request address; a peg-in's graph for an operator, written as a file every signer presigns; each
//! of the graph's transactions completed from the presignatures; the federation's verdict on a
//! challenge; an operator's fronted withdrawal and a graph's take, shown to the gateway; and a
//! watcher's check of an operator's claim, which says whether to challenge it.

use {
    super::{Bridge, Proof},
    crate::bitvm::{Graph, GraphTerms, request_script},
    crate::cli::CliResult,
    crate::crypto::bitcoin::tx::{Transaction, TxIn, TxOut, p2wsh},
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
    /// Posts a depositor's request to the gateway, once its payment to the request address is
    /// made: what the signers' pass works from.
    Request {
        txid: String,
        vout: u32,
        sats: u64,
        recipient: Address,
        /// The depositor's compressed key, as its request address names it.
        #[arg(long)]
        depositor: String,
        #[arg(long, default_value_t = 1_008)]
        refund_after: u16,
    },
    /// An operator, with --evm-key as its Bitcoin key and --key as the account it fronts from,
    /// posts its graph on a request: built on the request's confirm transaction, by the federation
    /// in force.
    PostGraph {
        request: u64,
        /// Its funding output for the kickoff, a P2WPKH of its key, txid:vout.
        #[arg(long)]
        funding: String,
        #[arg(long)]
        funding_value: u64,
        #[arg(long, default_value_t = 100_000)]
        stake: u64,
        /// Each transaction's fee, the confirm's too.
        #[arg(long, default_value_t = 2_000)]
        fee: u64,
        #[arg(long, default_value_t = 144)]
        challenge_window: u16,
        #[arg(long, default_value_t = 144)]
        dispute_window: u16,
    },
    /// A signer's pass, with --evm-key as its federation key: presigns every graph on every request
    /// that checks out, confirms a request once one of its graphs is presigned by every signer,
    /// broadcasts a confirm every signer signed, and shows a mined one to mint. Each step stands
    /// alone, as the release pass's do.
    Pass,
    /// An operator, with --key, claims the withdrawals it fronted that a graph's kickoff is
    /// reimbursed for: before its kickoff, or watchers challenge it.
    Claim {
        graph: u64,
        #[arg(num_args = 1..)]
        withdrawals: Vec<u64>,
    },
    /// A watcher, with --evm-key paying for challenges from its own P2WPKH coins: checks every
    /// graph's kickoff on Bitcoin against its operator's claim on the gateway, and challenges one
    /// whose claim falls short, again after a pause, until stopped; --once for one pass.
    Watch {
        #[arg(long, default_value_t = 60)]
        every: u64,
        #[arg(long)]
        once: bool,
    },
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

/// A request as the gateway keeps it, with what it confirms into.
struct Request {
    outpoint: ([u8; 32], u32),
    sats: u64,
    recipient: Address,
    script: Vec<u8>,
    peg_in_script: Vec<u8>,
}

/// A graph as the gateway keeps it.
struct Posted {
    request: u64,
    graph: Graph,
    account: String,
}

impl Bridge {
    fn chain_id(&self) -> CliResult<u64> {
        Ok(self.status()?["chain_id"].as_u64().ok_or("the node names no chain id")?)
    }

    fn bitvm_count(&self, function: &str) -> CliResult<u64> {
        Ok(self.read(&self.btc_gateway()?, function, json!([]))?.first().and_then(|value| value["value"].as_str()?.parse().ok()).unwrap_or(0))
    }

    /// A request from the gateway: its output, satoshis, recipient, its request script under the
    /// federation in force, and the peg-in script it confirms into.
    fn bitvm_request(&self, id: u64) -> CliResult<Request> {
        let gateway = self.btc_gateway()?;
        let found = self.read(&gateway, "bitvm_request_of", json!([{ "type": "u64", "value": id.to_string() }]))?;
        let text = |at: usize| found.get(at).and_then(|value| value["value"].as_str()).map(str::to_string);
        let outpoint = outpoint(&text(0).ok_or("no such request")?)?;
        let sats = text(1).and_then(|sats| sats.parse().ok()).ok_or("a request of no satoshis")?;
        let recipient: Address = text(2).ok_or("a request for no address")?.parse()?;
        let depositor = found.get(3).map(super::raw).and_then(|bytes| <[u8; 33]>::try_from(bytes).ok()).ok_or("a request of no depositor's key")?;
        let refund_after = text(4).and_then(|blocks| blocks.parse().ok()).ok_or("a request of no refund")?;
        let (signers, _) = self.bitvm_federation()?;

        Ok(Request {
            outpoint,
            sats,
            recipient,
            script: request_script(&signers, &depositor, refund_after),
            peg_in_script: crate::bitvm::peg_in_script(&recipient, self.chain_id()?, &gateway, &signers),
        })
    }

    /// A graph from the gateway, rebuilt from its terms.
    fn bitvm_posted(&self, id: u64) -> CliResult<Posted> {
        let found = self.read(&self.btc_gateway()?, "bitvm_graph_of", json!([{ "type": "u64", "value": id.to_string() }]))?;
        let request = found.first().and_then(|value| value["value"].as_str()?.parse().ok()).ok_or("no such graph")?;
        let terms = found.get(2).map(super::raw).ok_or("a graph of no terms")?;
        let account = found.get(3).and_then(|value| value["value"].as_str()).ok_or("a graph of no account")?.to_string();
        let path = std::env::temp_dir().join(format!("chainv4-bitvm-{}-{id}.json", std::process::id()));
        std::fs::write(&path, &terms).map_err(|err| err.to_string())?;
        let graph = graph_of(&path);
        let _ = std::fs::remove_file(&path);

        Ok(Posted { request, graph: graph?, account })
    }

    /// Whether a graph is one this signer may presign: on its request's confirm, by the federation
    /// in force, every signer presigning.
    fn bitvm_sound(&self, posted: &Posted, request: &Request) -> CliResult<()> {
        let (signers, threshold) = self.bitvm_federation()?;
        let terms = &posted.graph.terms;
        let confirm = crate::bitvm::confirm_tx(request.outpoint, request.sats, &request.peg_in_script, terms.fee);

        if terms.peg_in != (confirm.txid(), 0) || terms.peg_in_value != confirm.outputs[0].value || terms.peg_in_script != request.peg_in_script {
            return Err("a graph on another peg-in than its request's".into());
        }

        if terms.signers != signers || terms.federation != signers || terms.threshold != threshold {
            return Err("a graph under another federation than the one in force".into());
        }

        if terms.challenge_window < 6 || terms.dispute_window < 6 {
            return Err("windows too short to watch".into());
        }

        Ok(())
    }

    /// Every signer's presignatures of a graph that the gateway holds and that check out, by the
    /// signers' order: None while any is missing.
    fn bitvm_all_presigned(&self, graph_id: u64, graph: &Graph) -> CliResult<Option<Vec<Vec<Vec<u8>>>>> {
        let gateway = self.btc_gateway()?;
        let mut all = Vec::new();

        for signer in &graph.terms.signers {
            let found = self.read(&gateway, "bitvm_presigned", json!([{ "type": "u64", "value": graph_id.to_string() }, { "type": "bytes", "value": to_hex(signer) }]))?;
            let Some(posted) = found.first().map(super::raw).filter(|bytes| !bytes.is_empty()) else {
                return Ok(None);
            };
            let theirs = hexes(&serde_json::from_slice::<Value>(&posted).unwrap_or_default()["presigned"]);

            if !graph.check_presigned(signer, &theirs) {
                return Ok(None);
            }

            all.push(theirs);
        }

        Ok(Some(all))
    }

    fn bitvm_pass(&self) -> CliResult<()> {
        let gateway = self.btc_gateway()?;
        let secret = self.evm_secret()?;
        let mine = crate::crypto::secp256k1::public_key(&secret).ok_or("--evm-key is no secp256k1 key")?;
        let chain_id = self.chain_id()?;
        let (signers, _) = self.bitvm_federation()?;

        if !signers.contains(&mine) {
            return Err("--evm-key is no signer of the federation in force".into());
        }

        let graphs: Vec<(u64, CliResult<Posted>)> = (1..=self.bitvm_count("bitvm_graph_count")?).map(|id| (id, self.bitvm_posted(id))).collect();

        for request_id in 1..=self.bitvm_count("bitvm_request_count")? {
            let request = match self.bitvm_request(request_id) {
                Ok(request) => request,
                Err(why) => {
                    eprintln!("request {request_id}: {why}");
                    continue;
                }
            };
            let mut complete = None;

            // Presign each sound graph on it, once.
            for (graph_id, posted) in graphs.iter().filter_map(|(id, posted)| posted.as_ref().ok().filter(|posted| posted.request == request_id).map(|posted| (*id, posted))) {
                if let Err(why) = self.bitvm_sound(posted, &request) {
                    eprintln!("graph {graph_id}: not presigned: {why}");
                    continue;
                }

                let had = self.read(&gateway, "bitvm_presigned", json!([{ "type": "u64", "value": graph_id.to_string() }, { "type": "bytes", "value": to_hex(&mine) }]))?;

                if had.first().map(super::raw).is_none_or(|bytes| bytes.is_empty()) {
                    let presigned = serde_json::to_vec(&json!({ "signer": to_hex(&mine), "presigned": posted.graph.presign(&secret).iter().map(|signature| to_hex(signature)).collect::<Vec<_>>() }))?;
                    let signature = crate::bridge::sign_digest(&secret, &crate::bitvm::ceremony_digest("chainv4-bitvm-presign", chain_id, &gateway, graph_id, &presigned));
                    self.call(gateway, "bitvm_presign", u256::ZERO, vec![TxCallArgumentType::U64(graph_id), TxCallArgumentType::Bytes(presigned), TxCallArgumentType::Bytes(signature.to_vec())])?;
                    println!("presigned graph {graph_id} on request {request_id}");
                }

                if complete.is_none() && self.bitvm_all_presigned(graph_id, &posted.graph)?.is_some() {
                    complete = Some(posted.graph.terms.fee);
                }
            }

            // Once one of its graphs is presigned by every signer: confirm it, broadcast it once every
            // signer confirmed, and show it mined.
            let Some(fee) = complete else {
                continue;
            };
            let confirm = crate::bitvm::confirm_tx(request.outpoint, request.sats, &request.peg_in_script, fee);
            let confirmed = |key: &[u8; 33]| -> CliResult<Option<Vec<u8>>> {
                let found = self.read(&gateway, "bitvm_confirmed", json!([{ "type": "u64", "value": request_id.to_string() }, { "type": "bytes", "value": to_hex(key) }]))?;
                Ok(found.first().map(super::raw).filter(|bytes| crate::bitvm::check_confirm(&confirm, &request.script, request.sats, key, bytes)))
            };

            if confirmed(&mine)?.is_none() {
                let signature = crate::bitvm::confirm_signature(&confirm, &request.script, request.sats, &secret);
                let posted = crate::bridge::sign_digest(&secret, &crate::bitvm::ceremony_digest("chainv4-bitvm-confirm", chain_id, &gateway, request_id, &signature));
                self.call(gateway, "bitvm_confirm", u256::ZERO, vec![TxCallArgumentType::U64(request_id), TxCallArgumentType::Bytes(signature), TxCallArgumentType::Bytes(posted.to_vec())])?;
                println!("confirmed request {request_id}");
            }

            let all: Option<Vec<Vec<u8>>> = signers.iter().map(|key| confirmed(key).ok().flatten()).collect();
            let Some(all) = all else {
                continue;
            };
            let txid = {
                let mut shown = confirm.txid();
                shown.reverse();
                to_hex(&shown).trim_start_matches("0x").to_string()
            };
            let bitcoin = self.btc_api()?;

            match bitcoin.tx_status(&txid) {
                Ok(status) if status.confirmed => {
                    let state = self.read(&gateway, "utxo_of", json!([{ "type": "string", "value": format!("{txid}:0") }]))?;

                    if state.get(3).is_none_or(|value| value["value"].is_null()) {
                        let mut args = vec![];
                        let Proof { tx, block, index, path, coinbase, coinbase_path } = self.proof(&txid)?;
                        args.extend([TxCallArgumentType::Bytes(block.to_vec()), TxCallArgumentType::Bytes(tx), TxCallArgumentType::U64(index), TxCallArgumentType::Bytes(path), TxCallArgumentType::Bytes(coinbase), TxCallArgumentType::Bytes(coinbase_path), TxCallArgumentType::U64(0), TxCallArgumentType::Address(request.recipient)]);
                        match self.call(gateway, "bitvm_deposit", u256::ZERO, args) {
                            Ok(()) => println!("request {request_id}'s peg-in minted"),
                            Err(why) => eprintln!("request {request_id}'s peg-in: {why}"),
                        }
                    }
                }
                _ => {
                    let signed = crate::bitvm::signed_confirm(&confirm, &request.script, all);
                    match bitcoin.broadcast(to_hex(&signed.encode()).trim_start_matches("0x")) {
                        Ok(_) => println!("broadcast request {request_id}'s confirm {txid}"),
                        Err(why) => eprintln!("request {request_id}'s confirm: {why}"),
                    }
                }
            }
        }

        Ok(())
    }

    /// One watcher pass: each graph's kickoff, once mined with its challenge connector unspent,
    /// against its operator's claim; one that falls short challenged from this watcher's coins.
    fn bitvm_watch(&self) -> CliResult<()> {
        let gateway = self.btc_gateway()?;
        let (network, _) = self.followed(&gateway, None)?;
        let bitcoin = self.btc_api()?;
        let secret = self.evm_secret()?;
        let watcher = crate::crypto::secp256k1::public_key(&secret).ok_or("--evm-key is no secp256k1 key")?;
        let watcher_script = crate::crypto::bitcoin::tx::p2wpkh(&crate::crypto::bitcoin::hash160(&watcher));

        for graph_id in 1..=self.bitvm_count("bitvm_graph_count")? {
            let Ok(posted) = self.bitvm_posted(graph_id) else {
                continue;
            };
            let graph = &posted.graph;
            let kickoff = {
                let mut shown = graph.kickoff.txid();
                shown.reverse();
                to_hex(&shown).trim_start_matches("0x").to_string()
            };

            if !bitcoin.tx_status(&kickoff).is_ok_and(|status| status.confirmed) {
                continue;
            }

            // Challenged already, or taken: the connector is gone.
            let connector = network.address(&graph.kickoff.outputs[1].script);
            if !bitcoin.coins(&connector)?.iter().any(|coin| coin.txid == kickoff && coin.vout == 1) {
                continue;
            }

            let why = match self.bitvm_claim_short(graph_id, &posted) {
                Ok(None) => continue,
                Ok(Some(why)) => why,
                Err(why) => format!("its claim could not be read: {why}"),
            };
            println!("graph {graph_id}'s kickoff {kickoff}: challenging, as {why}");
            let Some(signatures) = self.bitvm_all_presigned(graph_id, graph)? else {
                eprintln!("graph {graph_id}: its presignatures are not all on the gateway");
                continue;
            };
            let coin = bitcoin.coins(&network.address(&watcher_script))?.into_iter().max_by_key(|coin| coin.value).ok_or("no coins of the watcher's to pay a challenge with")?;
            let fee = graph.terms.fee;
            let spent = outpoint(&format!("{}:{}", coin.txid, coin.vout))?;
            let mut challenge = graph.challenge_with(
                &signatures,
                vec![TxIn { txid: spent.0, vout: spent.1, script_sig: Vec::new(), sequence: 0xffff_fffd, witness: Vec::new() }],
                vec![TxOut { value: coin.value.saturating_sub(fee), script: watcher_script.clone() }],
            );
            let preimage = challenge.preimage(1, &crate::crypto::bitcoin::hash160(&watcher), coin.value);
            let signature = crate::crypto::secp256k1::sign_hash(&secret, &crate::crypto::bitcoin::sha256d(&preimage));
            challenge.inputs[1].witness = vec![[crate::crypto::bitcoin::tx::der(&signature), vec![1]].concat(), watcher.to_vec()];

            match bitcoin.broadcast(to_hex(&challenge.encode()).trim_start_matches("0x")) {
                Ok(txid) => println!("challenged graph {graph_id}: {txid}"),
                Err(why) => eprintln!("graph {graph_id}'s challenge: {why}"),
            }
        }

        Ok(())
    }

    /// Why a graph's claim falls short, or None for one that holds: every withdrawal it names
    /// fronted by the graph's operator, together at least what the peg-in reimburses it.
    fn bitvm_claim_short(&self, graph_id: u64, posted: &Posted) -> CliResult<Option<String>> {
        let gateway = self.btc_gateway()?;
        let claim = self.read(&gateway, "bitvm_claim_of", json!([{ "type": "u64", "value": graph_id.to_string() }]))?;
        let Some(ids) = claim.first().and_then(|value| value["value"].as_str()).map(str::to_string) else {
            return Ok(Some("its operator claimed nothing".to_string()));
        };
        let mut covered = 0u64;

        for id in ids.split(',').filter_map(|id| id.parse::<u64>().ok()) {
            let fronted = self.read(&gateway, "fronted", json!([{ "type": "u64", "value": id.to_string() }]))?;

            if fronted.first().and_then(|value| value["value"].as_str()) != Some(posted.account.as_str()) {
                return Ok(Some(format!("withdrawal {id} was not fronted by its operator")));
            }

            let record = self.read(&gateway, "withdrawal", json!([{ "type": "u64", "value": id.to_string() }]))?;
            covered = covered.saturating_add(record.get(1).and_then(|value| value["value"].as_str()?.parse().ok()).unwrap_or(0));
        }

        Ok((covered < posted.graph.terms.peg_in_value).then(|| format!("its withdrawals cover {covered} of the {} satoshis it takes", posted.graph.terms.peg_in_value)))
    }

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
            BitvmAction::Request { txid, vout, sats, recipient, depositor, refund_after } => {
                key(depositor, "--depositor")?;
                let args = vec![TxCallArgumentType::String(txid.clone()), number(u64::from(*vout)), number(*sats), TxCallArgumentType::Address(*recipient), bytes(from_hex(depositor).unwrap_or_default()), number(u64::from(*refund_after))];

                self.call(self.btc_gateway()?, "bitvm_request", u256::ZERO, args)
            }
            BitvmAction::PostGraph { request, funding, funding_value, stake, fee, challenge_window, dispute_window } => {
                let gateway = self.btc_gateway()?;
                let found = self.bitvm_request(*request)?;
                let (signers, threshold) = self.bitvm_federation()?;
                let secret = self.evm_secret()?;
                let operator = crate::crypto::secp256k1::public_key(&secret).ok_or("--evm-key is no secp256k1 key")?;
                let confirm = crate::bitvm::confirm_tx(found.outpoint, found.sats, &found.peg_in_script, *fee);
                let terms = GraphTerms {
                    federation: signers.clone(),
                    signers,
                    threshold,
                    operator,
                    funding: outpoint(funding)?,
                    funding_value: *funding_value,
                    peg_in: (confirm.txid(), 0),
                    peg_in_value: confirm.outputs[0].value,
                    peg_in_script: found.peg_in_script.clone(),
                    stake: *stake,
                    fee: *fee,
                    challenge_window: *challenge_window,
                    dispute_window: *dispute_window,
                };
                Graph::build(terms.clone())?;
                let mut file = terms_json(&terms);
                file["request"] = json!(request);
                let posted = serde_json::to_vec(&file)?;
                let id = self.bitvm_count("bitvm_graph_count")? + 1;
                let signature = crate::bridge::sign_digest(&secret, &crate::bitvm::ceremony_digest("chainv4-bitvm-graph", self.chain_id()?, &gateway, id, &posted));

                self.call(gateway, "bitvm_graph", u256::ZERO, vec![number(*request), bytes(operator.to_vec()), bytes(posted), bytes(signature.to_vec())])
            }
            BitvmAction::Pass => self.bitvm_pass(),
            BitvmAction::Claim { graph, withdrawals } => {
                let mut args = vec![number(*graph)];
                args.extend(withdrawals.iter().map(|id| number(*id)));

                self.call(self.btc_gateway()?, "bitvm_claim", u256::ZERO, args)
            }
            BitvmAction::Watch { every, once } => loop {
                if let Err(why) = self.bitvm_watch() {
                    eprintln!("watch: {why}");
                }

                if *once {
                    return Ok(());
                }

                std::thread::sleep(std::time::Duration::from_secs(*every));
            },
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
