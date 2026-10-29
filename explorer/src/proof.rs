//! Checking a proof the node sent against the light client, step by step: a chain-state entry's,
//! from GET /proofs/{column}/{key}, or a receipt's and its transaction's, from GET
//! /proofs/receipts/{tx}. The steps are the header the light client checked itself, how final it
//! is, the root the proof is against, and the path from the entry up to that root. Without a light
//! client following the chain, nothing is proven: a proof read is the node's word alone, and says
//! so.

use {
    crate::format,
    chainv4::db::{Column, STATE_COLUMNS, StateProof},
    chainv4::light::{LightClient, LightError},
    chainv4::structs::block::BlockHeader,
    chainv4::structs::hash::Hash,
    chainv4::structs::hex::from_hex,
    serde_json::Value,
};

/// A proof as the node sends it.
#[derive(Clone, Debug, PartialEq)]
pub enum Proof {
    State(StateEntry),
    Receipt(ReceiptEntry),
}

/// A chain-state entry's proof: what the entry holds at a block, and the path to its state root.
#[derive(Clone, Debug, PartialEq)]
pub struct StateEntry {
    pub block: String,
    pub height: u64,
    /// The block's state root, as the node says it.
    pub state_root: String,
    pub column: Column,
    pub key: Vec<u8>,
    /// What the node says the entry holds; None for nothing, which the proof proves too.
    pub value: Option<Vec<u8>>,
    pub proof: StateProof,
}

/// A receipt's proof: the receipt and its transaction, each encoded, with their Merkle paths to
/// the block's receipts root and tx_root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptEntry {
    pub tx: String,
    pub block: String,
    pub height: u64,
    pub receipts_root: String,
    pub tx_root: String,
    pub index: usize,
    pub count: usize,
    pub receipt: Vec<u8>,
    pub signed: Vec<u8>,
    pub path: Vec<String>,
    pub tx_path: Vec<String>,
}

impl Proof {
    /// The block it proves at, and its height.
    pub fn place(&self) -> (&str, u64) {
        match self {
            Proof::State(entry) => (&entry.block, entry.height),
            Proof::Receipt(entry) => (&entry.block, entry.height),
        }
    }
}

/// Reads a proof from the node's JSON: a state entry's, with its proof, or a receipt's.
pub fn read(value: &Value) -> Result<Proof, String> {
    let text = |name: &str| value[name].as_str().map(str::to_string).ok_or_else(|| format!("a proof without its {name}"));
    let bytes = |name: &str| from_hex(&text(name)?).ok_or_else(|| format!("a {name} that is not hex"));
    let number = |name: &str| value[name].as_u64().ok_or_else(|| format!("a proof without its {name}"));
    let hashes = |name: &str| -> Result<Vec<String>, String> {
        value[name]
            .as_array()
            .ok_or_else(|| format!("a proof without its {name}"))?
            .iter()
            .map(|hash| hash.as_str().map(str::to_string).ok_or_else(|| format!("a {name} of something but hashes")))
            .collect()
    };

    if value.get("receipt").is_some() {
        return Ok(Proof::Receipt(ReceiptEntry {
            tx: text("tx")?,
            block: text("block")?,
            height: number("height")?,
            receipts_root: text("receipts_root")?,
            tx_root: text("tx_root")?,
            index: number("index")? as usize,
            count: number("count")? as usize,
            receipt: bytes("receipt")?,
            signed: bytes("signed")?,
            path: hashes("path")?,
            tx_path: hashes("tx_path")?,
        }));
    }

    if value.get("proof").is_none() {
        return Err("not a proof as the node writes one: it has neither a proof nor a receipt".to_string());
    }

    let named = text("column")?;
    let column = column(&named)?;
    let held = match &value["value"] {
        Value::Null => None,
        Value::String(hex) => Some(from_hex(hex).ok_or("a value that is not hex")?),
        _ => return Err("a value that is neither hex nor null".to_string()),
    };
    let proof = wincode::deserialize(&bytes("proof")?).map_err(|_| "a proof that does not decode".to_string())?;

    Ok(Proof::State(StateEntry {
        block: text("block")?,
        height: number("height")?,
        state_root: text("state_root")?,
        column,
        key: bytes("key")?,
        value: held,
        proof,
    }))
}

/// A chain-state column by its name, such as accounts or validators.
pub fn column(name: &str) -> Result<Column, String> {
    STATE_COLUMNS.into_iter().find(|column| column.name() == name).ok_or_else(|| {
        let names: Vec<&str> = STATE_COLUMNS.iter().map(|column| column.name()).collect();

        format!("{name} is not a chain-state column; they are {}", names.join(", "))
    })
}

/// How a step went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// It checks.
    Good,
    /// It does not.
    Bad,
    /// Nothing to check it against, or not checked: the node's word.
    Unknown,
}

/// One step of checking a proof: what it is of, how it went, and what it found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub name: &'static str,
    pub mark: Mark,
    pub text: String,
}

impl Step {
    fn new(name: &'static str, mark: Mark, text: impl Into<String>) -> Step {
        Step { name, mark, text: text.into() }
    }
}

/// How a proof stands against the light client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It checks, against a header the light client checked itself at this height: final there,
    /// or not yet.
    Proven { final_: bool, height: u64 },
    /// It does not check: why.
    Wrong(String),
    /// At a block above the light client's tip, at this height: it can be checked once the light
    /// client gets there.
    Waiting(u64),
    /// It cannot be checked: why. What it says is the node's word alone.
    Unchecked(String),
}

/// A proof checked: each step, and the verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub steps: Vec<Step>,
    pub verdict: Verdict,
}

/// Checks a proof against the light client: None while it follows nothing, with why it stopped,
/// if it did.
pub fn check(proof: &Proof, client: Option<&LightClient>, trouble: Option<&str>) -> Checked {
    let Some(client) = client else {
        let why = match trouble {
            Some(trouble) => format!("the light client stopped ({trouble})"),
            None => "the light client is not following this node yet".to_string(),
        };

        return Checked {
            steps: unchecked(proof),
            verdict: Verdict::Unchecked(format!("{why}, so the proof can't be checked: what it says is the node's word alone")),
        };
    };
    let (block, height) = proof.place();
    let mut steps = Vec::new();
    let done = |steps: Vec<Step>, verdict: Verdict| Checked { steps, verdict };
    let Some(id) = from_hex(block).and_then(|bytes| Hash::from_bytes(client.params().hash, &bytes)) else {
        steps.push(Step::new("header", Mark::Bad, format!("{block} is not a block id of this chain's hash")));

        return done(steps, Verdict::Wrong("the proof names a block id that does not read".to_string()));
    };
    let header = match client.header_at(height) {
        Some(header) if header.id() == id => header,
        Some(header) => {
            steps.push(Step::new(
                "header",
                Mark::Bad,
                format!("the light client followed block {} at height {}, not {}", format::short(&header.id().to_string()), format::thousands(&height.to_string()), format::short(block)),
            ));

            return done(steps, Verdict::Wrong("the node proves at a block off the chain the light client follows".to_string()));
        }
        None if height > client.tip().height => {
            steps.push(Step::new(
                "header",
                Mark::Unknown,
                format!("block {}: the light client has reached block {} so far", format::thousands(&height.to_string()), format::thousands(&client.tip().height.to_string())),
            ));

            return done(steps, Verdict::Waiting(height));
        }
        None => {
            steps.push(Step::new("header", Mark::Unknown, format!("block {}: below the lowest header the light client holds, {}", format::thousands(&height.to_string()), format::thousands(&client.lowest().to_string()))));

            return done(steps, Verdict::Unchecked(format!("block {} is older than the headers the light client holds, so the proof can't be checked", format::thousands(&height.to_string()))));
        }
    };

    steps.push(Step::new("header", Mark::Good, format!("block {}, {}: a header the light client checked itself", format::thousands(&height.to_string()), format::short(block))));
    steps.push(finality(client, height));

    let (mut found, verdict) = match proof {
        Proof::State(entry) => state(client, &id, header, entry),
        Proof::Receipt(entry) => receipt(client, &id, header, entry),
    };
    steps.append(&mut found);

    done(steps, verdict)
}

/// How final a block the light client holds is.
fn finality(client: &LightClient, height: u64) -> Step {
    let final_height = format::thousands(&client.final_height().to_string());

    match (client.params().finality.is_some(), height <= client.final_height()) {
        (false, _) => Step::new("finality", Mark::Unknown, "the chain has no finality: the block can still be reorganized away"),
        (true, true) => Step::new("finality", Mark::Good, format!("final: finality reaches block {final_height}")),
        (true, false) => Step::new("finality", Mark::Unknown, format!("not final yet: finality reaches block {final_height}")),
    }
}

/// A root as the node says it, against the header's.
fn root(name: &'static str, claimed: &str, held: &Hash) -> Step {
    let held = held.to_string();

    match claimed.eq_ignore_ascii_case(&held) {
        true => Step::new(name, Mark::Good, format!("{}, the header's", format::short(&held))),
        false => Step::new(name, Mark::Bad, format!("the node says {}, the header {}", format::short(claimed), format::short(&held))),
    }
}

/// A state entry's root and path.
fn state(client: &LightClient, id: &Hash, header: &BlockHeader, entry: &StateEntry) -> (Vec<Step>, Verdict) {
    let root = root("state root", &entry.state_root, &header.state_root);
    let wrong_root = root.mark == Mark::Bad;
    let siblings = entry.proof.siblings.len();
    let held = match &entry.value {
        Some(value) => format!("that it holds {} bytes", value.len()),
        None => "that it holds nothing".to_string(),
    };

    match client.check_state(id, entry.column, &entry.key, entry.value.as_deref(), &entry.proof) {
        Ok(_) if wrong_root => (vec![root, Step::new("path", Mark::Good, "leads to the header's root")], Verdict::Wrong("the node names another state root than the header's".to_string())),
        Ok(assurance) => (
            vec![root, Step::new("path", Mark::Good, format!("{siblings} hashes beside the way lead from the entry to the root, proving {held}"))],
            Verdict::Proven { final_: assurance.final_, height: assurance.height },
        ),
        Err(LightError::BadProof) => (
            vec![root, Step::new("path", Mark::Bad, format!("{siblings} hashes beside the way do not lead from the entry to the root: it does not prove {held}"))],
            Verdict::Wrong("the path does not lead to the state root of the header the light client checked".to_string()),
        ),
        Err(err) => (vec![root, Step::new("path", Mark::Bad, err.to_string())], Verdict::Wrong(err.to_string())),
    }
}

/// A receipt's roots and paths, and that its transaction is the one asked about.
fn receipt(client: &LightClient, id: &Hash, header: &BlockHeader, entry: &ReceiptEntry) -> (Vec<Step>, Verdict) {
    let hash = client.params().hash;
    let roots = vec![root("receipts root", &entry.receipts_root, &header.receipts_root), root("tx root", &entry.tx_root, &header.tx_root)];
    let wrong_root = roots.iter().any(|step| step.mark == Mark::Bad);
    let hashes = |path: &[String]| path.iter().map(|hash_text| from_hex(hash_text).and_then(|bytes| Hash::from_bytes(hash, &bytes))).collect::<Option<Vec<Hash>>>();
    let (Some(path), Some(tx_path)) = (hashes(&entry.path), hashes(&entry.tx_path)) else {
        return (roots, Verdict::Wrong("a path of something but this chain's hashes".to_string()));
    };
    let mut steps = roots;
    let place = format!("leaf {} of {}", entry.index, entry.count);

    let receipt = match client.check_receipt(id, &entry.receipt, entry.index, entry.count, &path) {
        Ok((receipt, _)) => receipt,
        Err(err) => {
            steps.push(Step::new("path", Mark::Bad, format!("the receipt, as {place}, does not lead to the receipts root: {err}")));

            return (steps, Verdict::Wrong("the receipt's path does not lead to the receipts root of the header the light client checked".to_string()));
        }
    };
    let (signed, assurance) = match client.check_tx(id, &entry.signed, entry.index, entry.count, &tx_path) {
        Ok(checked) => checked,
        Err(err) => {
            steps.push(Step::new("path", Mark::Bad, format!("the transaction, as {place}, does not lead to the tx root: {err}")));

            return (steps, Verdict::Wrong("the transaction's path does not lead to the tx root of the header the light client checked".to_string()));
        }
    };

    if receipt.tx.to_string() != entry.tx.to_ascii_lowercase() || signed.id(hash) != receipt.tx {
        steps.push(Step::new("path", Mark::Bad, "the receipt and transaction proven are not the transaction asked about"));

        return (steps, Verdict::Wrong("the node proved another transaction".to_string()));
    }

    steps.push(Step::new(
        "path",
        Mark::Good,
        format!("the receipt, {place}, with {} hashes up to the receipts root, and its transaction the same with {} up to the tx root", path.len(), tx_path.len()),
    ));

    match wrong_root {
        true => (steps, Verdict::Wrong("the node names other roots than the header's".to_string())),
        false => (steps, Verdict::Proven { final_: assurance.final_, height: assurance.height }),
    }
}

/// What a proof says, step by step, with nothing to check it against.
fn unchecked(proof: &Proof) -> Vec<Step> {
    let (block, height) = proof.place();
    let header = Step::new("header", Mark::Unknown, format!("block {}, {}, as the node says", format::thousands(&height.to_string()), format::short(block)));

    match proof {
        Proof::State(entry) => vec![
            header,
            Step::new("state root", Mark::Unknown, format!("{}, as the node says", format::short(&entry.state_root))),
            Step::new("path", Mark::Unknown, format!("{} hashes beside the way, not checked", entry.proof.siblings.len())),
        ],
        Proof::Receipt(entry) => vec![
            header,
            Step::new("receipts root", Mark::Unknown, format!("{}, as the node says", format::short(&entry.receipts_root))),
            Step::new("tx root", Mark::Unknown, format!("{}, as the node says", format::short(&entry.tx_root))),
            Step::new("path", Mark::Unknown, format!("leaf {} of {}, {} and {} hashes, not checked", entry.index, entry.count, entry.path.len(), entry.tx_path.len())),
        ],
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use {
        super::*,
        chainv4::consensus::ConsensusConfig,
        chainv4::crypto::hashable::Hashable,
        chainv4::crypto::keypair::Keypair,
        chainv4::db::{MemoryDb, State, StateTree},
        chainv4::light::LightParams,
        chainv4::structs::block::{Seal, SealedHeader},
        chainv4::structs::hash::HashType,
        chainv4::structs::hex::to_hex,
        chainv4::structs::u256::u256,
        serde_json::json,
        std::sync::Arc,
    };

    pub const HASH: HashType = HashType::Blake3_256;

    /// A chain whose genesis state holds these entries, the light client following it, and the
    /// state, to prove entries from as a node does.
    pub fn chain(entries: &[(Column, &[u8], &[u8])]) -> (LightClient, State) {
        let state = State::new(Arc::new(MemoryDb::default()));

        for (column, key, value) in entries {
            state.put(*column, key.to_vec(), value.to_vec());
        }

        let root = StateTree::new(&state, HASH).follow().unwrap();
        let params = LightParams {
            hash: HASH,
            consensus: ConsensusConfig::Authority { key: Keypair::development().public(), slot_ms: 1_000 },
            finality: None,
            pow_window: None,
            governance: None,
        };
        let header = BlockHeader {
            parent: Hash::zero(HASH),
            height: 0,
            timestamp: 0,
            producer: Keypair::development().public().address(),
            difficulty: 0,
            tx_root: Hash::zero(HASH),
            proof_root: params.hash(HASH),
            pool_root: Hash::zero(HASH),
            receipts_root: Hash::zero(HASH),
            state_root: root,
            gas_used: 0,
            base_price: u256::ZERO,
        };
        let sealed = SealedHeader { header, seal: Seal::None };
        let genesis = sealed.id();

        (LightClient::new(params, sealed, genesis).unwrap(), state)
    }

    /// A state entry's proof at genesis, as GET /proofs/{column}/{key} writes it.
    pub fn proof_json(client: &LightClient, state: &State, column: Column, key: &[u8]) -> Value {
        let header = client.header_at(0).unwrap();
        let proof = StateTree::new(state, HASH).prove(column, key).unwrap();

        json!({
            "block": header.id().to_string(),
            "height": 0,
            "state_root": header.state_root.to_string(),
            "proof": to_hex(&wincode::serialize(&proof).unwrap()),
            "column": column.name(),
            "key": to_hex(key),
            "value": state.get(column, key).unwrap().map(|value| to_hex(&value)),
            "siblings": proof.siblings.iter().map(ToString::to_string).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn a_state_proof_checks_against_a_header_the_light_client_holds() {
        let (client, state) = chain(&[(Column::Validators, b"coin:supply", &[7; 64]), (Column::Accounts, b"someone", &[1, 2, 3])]);
        let proof = read(&proof_json(&client, &state, Column::Validators, b"coin:supply")).unwrap();
        let checked = check(&proof, Some(&client), None);

        assert_eq!(checked.verdict, Verdict::Proven { final_: false, height: 0 });
        assert_eq!(checked.steps.iter().map(|step| (step.name, step.mark)).collect::<Vec<_>>(), [
            ("header", Mark::Good),
            ("finality", Mark::Unknown),
            ("state root", Mark::Good),
            ("path", Mark::Good)
        ]);

        // Nothing at a key proves as nothing.
        let absent = read(&proof_json(&client, &state, Column::Accounts, b"nobody")).unwrap();
        assert!(matches!(&absent, Proof::State(entry) if entry.value.is_none()));
        assert_eq!(check(&absent, Some(&client), None).verdict, Verdict::Proven { final_: false, height: 0 });
    }

    #[test]
    fn a_proof_whose_path_does_not_lead_to_the_root_is_wrong() {
        let (client, state) = chain(&[(Column::Validators, b"coin:supply", &[7; 64]), (Column::Accounts, b"someone", &[1, 2, 3])]);
        let mut lying = proof_json(&client, &state, Column::Validators, b"coin:supply");
        lying["value"] = to_hex(&[8; 64]).into();
        let checked = check(&read(&lying).unwrap(), Some(&client), None);

        assert_eq!(checked.verdict, Verdict::Wrong("the path does not lead to the state root of the header the light client checked".to_string()));
        assert_eq!(checked.steps.last().map(|step| (step.name, step.mark)), Some(("path", Mark::Bad)));

        // A root that is not the header's.
        let mut rooted = proof_json(&client, &state, Column::Validators, b"coin:supply");
        rooted["state_root"] = Hash::zero(HASH).to_string().into();
        assert_eq!(check(&read(&rooted).unwrap(), Some(&client), None).verdict, Verdict::Wrong("the node names another state root than the header's".to_string()));

        // A block the light client did not follow, at a height it holds.
        let mut elsewhere = proof_json(&client, &state, Column::Validators, b"coin:supply");
        elsewhere["block"] = Hash::from_bytes(HASH, &[9; 32]).unwrap().to_string().into();
        assert_eq!(check(&read(&elsewhere).unwrap(), Some(&client), None).verdict, Verdict::Wrong("the node proves at a block off the chain the light client follows".to_string()));

        // Above its tip: checked once it gets there.
        let mut ahead = elsewhere.clone();
        ahead["height"] = 5.into();
        assert_eq!(check(&read(&ahead).unwrap(), Some(&client), None).verdict, Verdict::Waiting(5));
    }

    #[test]
    fn without_a_light_client_a_proof_is_the_node_s_word_alone() {
        let (client, state) = chain(&[(Column::Validators, b"coin:supply", &[7; 64])]);
        let proof = read(&proof_json(&client, &state, Column::Validators, b"coin:supply")).unwrap();
        let checked = check(&proof, None, None);

        assert!(matches!(&checked.verdict, Verdict::Unchecked(why) if why.contains("can't be checked")), "{:?}", checked.verdict);
        assert!(checked.steps.iter().all(|step| step.mark == Mark::Unknown));
        assert!(matches!(check(&proof, None, Some("another chain")).verdict, Verdict::Unchecked(why) if why.starts_with("the light client stopped (another chain)")));
    }

    #[test]
    fn proofs_read_as_the_node_writes_them_and_anything_else_says_why_not() {
        let (client, state) = chain(&[(Column::Validators, b"coin:supply", &[7; 64])]);
        let mut json = proof_json(&client, &state, Column::Validators, b"coin:supply");

        assert!(matches!(read(&json), Ok(Proof::State(entry)) if entry.column == Column::Validators && entry.key == b"coin:supply"));

        json["column"] = "blocks".into();
        assert!(read(&json).unwrap_err().starts_with("blocks is not a chain-state column"));
        assert_eq!(read(&json!({ "hello": 1 })).unwrap_err(), "not a proof as the node writes one: it has neither a proof nor a receipt");

        let receipt = read(&json!({
            "tx": "0x01", "block": "0x02", "height": 3, "receipts_root": "0x04", "index": 0, "count": 1,
            "receipt": "0x05", "path": [], "tx_root": "0x06", "signed": "0x07", "tx_path": [],
        }))
        .unwrap();

        assert!(matches!(&receipt, Proof::Receipt(entry) if entry.height == 3 && entry.receipt == [5]));
        assert_eq!(check(&receipt, None, None).steps.len(), 4);
    }
}
