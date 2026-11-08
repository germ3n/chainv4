//! chainv4 bridge finality-deploy and finality-relay: ChainV4Finality.sol on an EVM chain, which
//! follows chainv4's finality by its validators' attestations. Deploying starts it from the
//! finalized checkpoint the node names; relaying shows it each link justified since, as GET
//! /attestations gives them, until it is at the tip. Anyone may relay: the contract checks the
//! attestations itself, so a relayer can only be late, never wrong.

use {
    super::Bridge,
    crate::abi::{AbiType, AbiValue, encode_values},
    crate::cli::CliResult,
    crate::crypto::secp256k1::keccak,
    crate::structs::hex::from_hex,
    crate::structs::u256::u256,
    crate::swap::evm::Call,
    serde_json::Value,
};

/// The most links one pass looks through for what to relay.
const MAX_PLANNED: usize = 10_000;

/// ChainV4Finality.sol's init code, as solc writes it.
const FINALITY_CODE: &str = include_str!("../../../contracts/evm/ChainV4Finality.bin");

fn word(value: &Value, what: &str) -> CliResult<[u8; 32]> {
    value.as_str().and_then(from_hex).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| format!("GET /attestations sent no {what}").into())
}

fn number(value: &Value, what: &str) -> CliResult<u256> {
    match value {
        Value::Number(number) => number.as_u64().map(u256::from).ok_or_else(|| format!("GET /attestations sent no {what}").into()),
        Value::String(text) => u256::from_str_radix(text, 10).map_err(|_| format!("GET /attestations sent no {what}").into()),
        _ => Err(format!("GET /attestations sent no {what}").into()),
    }
}

fn selector(signature: &str) -> Vec<u8> {
    keccak(signature.as_bytes())[..4].to_vec()
}

/// submit's calldata for a link as GET /attestations writes it.
pub(super) fn submit_input(link: &Value) -> CliResult<Vec<u8>> {
    let set = &link["set"];
    let attesters: Vec<[u8; 20]> = set["attesters"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|address| address.as_str().and_then(from_hex).and_then(|bytes| bytes.try_into().ok()).ok_or("an attester that is no address"))
        .collect::<Result<_, _>>()?;
    let weights: Vec<u256> = set["weights"].as_array().into_iter().flatten().map(|weight| number(weight, "weight")).collect::<CliResult<_>>()?;
    let mut places = Vec::new();
    let mut signatures = Vec::new();

    for attestation in link["attestations"].as_array().into_iter().flatten() {
        let attester: [u8; 20] = attestation["attester"].as_str().and_then(from_hex).and_then(|bytes| bytes.try_into().ok()).ok_or("an attestation by no address")?;
        let place = attesters.iter().position(|known| *known == attester).ok_or("an attestation by no attester of the set")?;
        places.push(AbiValue::Uint(u256::from(place as u64)));
        signatures.extend(attestation["signature"].as_str().and_then(from_hex).filter(|bytes| bytes.len() == 65).ok_or("an attestation that is not 65 bytes")?);
    }

    let words = [
        number(&link["source"]["epoch"], "source epoch")?.to_be_bytes(),
        word(&link["source"]["block"], "source block")?,
        number(&link["target"]["epoch"], "target epoch")?.to_be_bytes(),
        word(&link["target"]["block"], "target block")?,
        number(&link["height"], "height")?.to_be_bytes(),
        word(&link["state_root"], "state root")?,
    ];
    let types = [
        AbiType::FixedArray(Box::new(AbiType::FixedBytes(32)), 6),
        AbiType::Uint(256),
        AbiType::Uint(256),
        AbiType::Array(Box::new(AbiType::Address)),
        AbiType::Array(Box::new(AbiType::Uint(256))),
        AbiType::FixedBytes(32),
        AbiType::Array(Box::new(AbiType::Uint(256))),
        AbiType::Bytes,
    ];
    let values = [
        AbiValue::Array(words.iter().map(|word| AbiValue::FixedBytes(word.to_vec())).collect()),
        AbiValue::Uint(number(&set["total"], "total")?),
        // The weight a link needs, which the set's hash names from the proofs fork on.
        AbiValue::Uint(number(&set["needed"], "the weight a link needs: a link from before the proofs fork, which this contract takes none of")?),
        AbiValue::Array(attesters.into_iter().map(AbiValue::Address).collect()),
        AbiValue::Array(weights.into_iter().map(AbiValue::Uint).collect()),
        AbiValue::FixedBytes(word(&link["next_set"]["hash"], "next set")?.to_vec()),
        AbiValue::Array(places),
        AbiValue::Bytes(signatures),
    ];

    Ok([selector("submit(bytes32[6],uint256,uint256,address[],uint256[],bytes32,uint256[],bytes)"), encode_values(&types, &values)?].concat())
}

impl Bridge {
    /// The first link justified after an epoch, as GET /attestations gives it, or None when none
    /// is yet.
    fn attested_after(&self, epoch: u64) -> CliResult<Option<Value>> {
        match crate::cli::send::fetch(self.agent.get(format!("{}/attestations?after={epoch}", self.node)).call())? {
            (200..=299, link) => Ok(Some(link)),
            (404, answer) if answer["error"].as_str().is_some_and(|why| why.contains("no checkpoint justified")) => Ok(None),
            (status, answer) => Err(format!("GET /attestations: {status} {}", answer["error"].as_str().unwrap_or_default()).into()),
        }
    }

    /// Deploys ChainV4Finality.sol on the EVM chain of --eth-rpc, from the finalized checkpoint
    /// the node names, with the attesting set that signs the link from it.
    pub(super) fn finality_deploy(&self, sp1_verifiers: &[String], program: Option<&str>) -> CliResult<()> {
        let (rpc, secret) = (self.rpc()?, self.evm_secret()?);
        let status = self.status()?;
        let finalized = status["finality"]["finalized"]["epoch"].as_u64().ok_or("the node's finality does not vote")?;
        let bytes20 = |text: &str, what: &str| -> CliResult<[u8; 20]> { from_hex(text).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| format!("{what}: 0x and 40 hex digits").into()) };
        let verifiers = sp1_verifiers.iter().map(|text| bytes20(text, "--sp1-verifier")).collect::<CliResult<Vec<_>>>()?;
        let validity = &status["validity"];
        let program = match program {
            Some(text) => from_hex(text).and_then(|bytes| <[u8; 32]>::try_from(bytes).ok()).ok_or("--program: 0x and 64 hex digits")?,
            None => match validity["settings"]["program"].as_str() {
                Some(text) => word(&Value::String(text.to_string()), "the program")?,
                None => [0; 32],
            },
        };
        let link = self.attested_after(finalized)?.ok_or("no link from the finalized checkpoint is justified yet: try again in an epoch")?;
        let source = &link["source"];

        if source["epoch"].as_u64() != Some(finalized) {
            return Err(format!("the next justified link starts at epoch {}, not the finalized {finalized}: try again in an epoch", source["epoch"]).into());
        }

        // Finality proofs by the finality program governance names, from any checkpoint recorded.
        let finality_program = match validity["settings"]["finality_program"].as_str() {
            Some(text) => word(&Value::String(text.to_string()), "the finality program")?,
            None => [0; 32],
        };

        // Proofs start where chainv4's do, or from this trusted checkpoint while it takes none.
        let tip = &validity["tip"];
        let proven = match tip.is_object() {
            true => (number(&tip["height"], "the proven tip's height")?, word(&tip["block"], "the proven tip")?, word(&tip["state_root"], "the proven tip's state root")?),
            false => (number(&source["height"], "source height")?, word(&source["block"], "source block")?, word(&source["state_root"], "source state root")?),
        };
        let genesis = word(&status["genesis"], "the genesis")?;
        let constructor = encode_values(
            &[AbiType::Uint(256), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::Array(Box::new(AbiType::Address)), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::FixedBytes(32)],
            &[
                AbiValue::Uint(number(&link["chain_id"], "chain id")?),
                AbiValue::Uint(u256::from(finalized)),
                AbiValue::FixedBytes(word(&source["block"], "source block")?.to_vec()),
                AbiValue::Uint(number(&source["height"], "source height")?),
                AbiValue::FixedBytes(word(&source["state_root"], "source state root")?.to_vec()),
                AbiValue::FixedBytes(word(&link["set"]["hash"], "set")?.to_vec()),
                AbiValue::Array(verifiers.iter().map(|verifier| AbiValue::Address(*verifier)).collect()),
                AbiValue::FixedBytes(genesis.to_vec()),
                AbiValue::FixedBytes(program.to_vec()),
                AbiValue::Uint(proven.0),
                AbiValue::FixedBytes(proven.1.to_vec()),
                AbiValue::FixedBytes(proven.2.to_vec()),
                AbiValue::FixedBytes(finality_program.to_vec()),
            ],
        )?;
        let init = [from_hex(FINALITY_CODE.trim()).ok_or("built-in code is hex")?, constructor].concat();
        let receipt = rpc.send(&Call { to: [0; 20], value: u256::ZERO, input: init, what: "Deploy ChainV4Finality.sol" }, true, &secret)?;
        println!("finality {} from epoch {finalized}", receipt["contractAddress"].as_str().ok_or("the deploy's receipt names no contract")?);

        if verifiers.is_empty() || program == [0; 32] {
            println!("it takes no validity proofs: give --sp1-verifier, and --program while governance names none");
        }

        Ok(())
    }

    /// What to show ChainV4Finality.sol next, past the epoch it took last and by the set it holds:
    /// the link where the set changes, which it must take before any signed by the next set; or
    /// else the newest two links in a row, the first recording a checkpoint and the second
    /// finalizing it; or else the newest link. Never every link: each costs gas, and any later one
    /// its set signed will do.
    fn finality_plan(&self, taken: u64, set: [u8; 32]) -> CliResult<Vec<Value>> {
        let mut links: Vec<Value> = Vec::new();
        let mut epoch = taken;

        while let Some(link) = self.attested_after(epoch)? {
            let signed_by = word(&link["set"]["hash"], "set")?;
            let next = word(&link["next_set"]["hash"], "next set")?;
            epoch = link["target"]["epoch"].as_u64().ok_or("GET /attestations sent no target epoch")?;

            // Signed by a set the contract does not hold: the change came before, out of reach.
            if signed_by != set {
                break;
            }

            // The set moves after this one: it goes alone, and the next pass goes on from it.
            if next != set {
                return Ok(vec![link]);
            }

            links.push(link);

            if links.len() >= MAX_PLANNED {
                break;
            }
        }

        let in_a_row = |first: &Value, second: &Value| {
            second["source"]["epoch"] == first["target"]["epoch"] && second["target"]["epoch"].as_u64() == first["target"]["epoch"].as_u64().map(|epoch| epoch + 1)
        };

        for at in (1..links.len()).rev() {
            if in_a_row(&links[at - 1], &links[at]) {
                return Ok(vec![links[at - 1].clone(), links[at].clone()]);
            }
        }

        Ok(links.pop().into_iter().collect())
    }

    /// Brings ChainV4Finality.sol up to chainv4's finality in a few links, not one an epoch: with
    /// every, again that often, and only once at least min_epochs are new.
    pub(super) fn finality_relay(&self, contract: &[u8; 20], every: Option<u64>, min_epochs: u64) -> CliResult<()> {
        let (rpc, secret) = (self.rpc()?, self.evm_secret()?);
        let read = |getter: &str| -> CliResult<[u8; 32]> {
            let output = rpc.call(contract, &selector(getter))?;

            output.get(..32).and_then(|word| word.try_into().ok()).ok_or_else(|| format!("the contract gave nothing for {getter}").into())
        };

        loop {
            let taken = u256::from_be_bytes(read("justifiedEpoch()")?);
            let taken = u64::try_from(taken).map_err(|_| "the contract's justified epoch is past any chainv4 epoch")?;
            let plan = self.finality_plan(taken, read("setHash()")?)?;
            let newest = plan.last().and_then(|link| link["target"]["epoch"].as_u64()).unwrap_or(taken);
            let set_moves = plan.len() == 1 && plan[0]["next_set"]["hash"] != plan[0]["set"]["hash"];

            if plan.is_empty() || (newest.saturating_sub(taken) < min_epochs && !set_moves) {
                println!("nothing to relay yet: the contract is at epoch {taken}");
            }

            for link in plan.iter().filter(|_| set_moves || newest.saturating_sub(taken) >= min_epochs) {
                let target = link["target"]["epoch"].as_u64().unwrap_or_default();

                if link["enough"].as_bool() != Some(true) {
                    println!("epoch {target}: its attestations fall short of two thirds of the set; waiting for more validators to attest");
                    break;
                }

                rpc.send(&Call { to: *contract, value: u256::ZERO, input: submit_input(link)?, what: "Submit a justified link" }, false, &secret)?;
                println!("epoch {target} justified there, state root {}", link["state_root"].as_str().unwrap_or("?"));
            }

            match every {
                Some(seconds) => std::thread::sleep(std::time::Duration::from_secs(seconds.max(1))),
                None => return Ok(()),
            }
        }
    }

    /// Takes a proof file to ChainV4Finality.sol: its public values and proof, at the epoch whose
    /// checkpoint the range ends at, which the contract must have finalized.
    pub(super) fn finality_prove(&self, contract: &[u8; 20], proof: &std::path::Path) -> CliResult<()> {
        let (rpc, secret) = (self.rpc()?, self.evm_secret()?);
        let file: Value = serde_json::from_slice(&std::fs::read(proof).map_err(|err| format!("{}: {err}", proof.display()))?).map_err(|err| format!("{}: {err}", proof.display()))?;
        let field = |name: &str| file[name].as_str().and_then(from_hex).ok_or_else(|| format!("{}: no {name}", proof.display()));

        if let Some(kind) = file["kind"].as_str().and_then(crate::proofs::ProofKind::parse)
            && !kind.on_evm()
        {
            return Err(format!("{}: a {} proof, which no EVM chain can check: prove the range with --kind sp1-groth16 or sp1-plonk for Ethereum", proof.display(), kind.name()).into());
        }

        // A link of finality: proveLink, which needs no checkpoint final there first.
        if file["of"].as_str() == Some("finality") {
            let input = [selector("proveLink(bytes,bytes)"), encode_values(&[AbiType::Bytes, AbiType::Bytes], &[AbiValue::Bytes(field("public_values")?), AbiValue::Bytes(field("proof")?)])?].concat();
            rpc.send(&Call { to: *contract, value: u256::ZERO, input, what: "Prove a link of finality" }, false, &secret)?;
            println!("the link to epoch {} proven there", file["proves"]["target"]["epoch"]);

            return Ok(());
        }

        if file["of"].as_str() == Some("headers") {
            return Err(format!("{}: a header-chain proof goes to the chain: chainv4 send prove", proof.display()).into());
        }

        let height = file["proves"]["end"]["height"].as_u64().or(file["range"]["end"]["height"].as_u64()).ok_or_else(|| format!("{}: no range end", proof.display()))?;
        let interval = self.status()?["finality"]["checkpoint_interval"].as_u64().filter(|interval| *interval > 0).ok_or("the node's finality has no checkpoints")?;

        if !height.is_multiple_of(interval) {
            return Err(format!("the range ends at {height}, no checkpoint").into());
        }

        let epoch = height / interval;
        let finalized = u256::from_be_bytes(rpc.call(contract, &[selector("finalAt(uint256)"), u256::from(epoch).to_be_bytes().to_vec()].concat())?.get(..32).and_then(|word| word.try_into().ok()).unwrap_or([0; 32]));

        if finalized == u256::ZERO {
            return Err(format!("epoch {epoch} is not final on the contract yet: finality-relay brings it there").into());
        }

        let input = [selector("prove(bytes,bytes,uint256)"), encode_values(&[AbiType::Bytes, AbiType::Bytes, AbiType::Uint(256)], &[AbiValue::Bytes(field("public_values")?), AbiValue::Bytes(field("proof")?), AbiValue::Uint(u256::from(epoch))])?].concat();
        rpc.send(&Call { to: *contract, value: u256::ZERO, input, what: "Prove a range" }, false, &secret)?;
        println!("proven there through block {height}, epoch {epoch}");

        Ok(())
    }

    /// Points Gateway.sol at a ChainV4Finality.sol, as its owner, while it follows none.
    pub(super) fn gateway_finality(&self, gateway: &[u8; 20], finality: &[u8; 20]) -> CliResult<()> {
        let (rpc, secret) = (self.rpc()?, self.evm_secret()?);
        let mut word = [0; 32];
        word[12..].copy_from_slice(finality);
        rpc.send(&Call { to: *gateway, value: u256::ZERO, input: [selector("setFinality(address)"), word.to_vec()].concat(), what: "Follow a ChainV4Finality.sol" }, false, &secret)?;
        println!("Gateway.sol {} takes proven withdrawals by {}", crate::structs::hex::to_hex(gateway), crate::structs::hex::to_hex(finality));

        Ok(())
    }
}
