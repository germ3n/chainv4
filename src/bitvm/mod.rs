//! BitVM's side of the Bitcoin bridge, phase 1: the transaction graph a peg-in's coins can only
//! leave by, presigned by every one of the bridge's signers before the peg-in counts, so that one
//! honest signer who then forgets its key keeps the coins on the graph for good.
//!
//! **The graph, for one peg-in and one operator.** An operator pays a withdrawal on Bitcoin out of
//! its own coins, then asks to be reimbursed from the peg-in:
//!
//! - **Kickoff:** the operator's own transaction, from a funding output named in the graph, with
//!   three outputs: the claim connector, which only the operator spends and only `challenge_window`
//!   blocks after the kickoff; the challenge connector, under the signers; and the verdict
//!   connector, the operator's stake, which the federation's threshold spends at once or the
//!   signers' presigned default `challenge_window + dispute_window` blocks on.
//! - **Take:** the peg-in, the claim connector and the challenge connector together, to the
//!   operator. Presigned, so it needs nothing but the challenge window to pass with the challenge
//!   connector unspent.
//! - **Challenge:** anyone who finds the operator's claim false spends the challenge connector,
//!   presigned SIGHASH_SINGLE|ANYONECANPAY, paying it to the operator while the challenger adds
//!   inputs for the fee. Take can no longer happen. Nothing in the graph spends the challenge, so
//!   what the challenger adds changes nothing presigned.
//! - **The verdict:** in phase 1 the federation rules, its threshold signing the verdict connector,
//!   having checked the operator's withdrawal proof off Bitcoin (proofs::prove_withdrawal):
//!   - for the operator, *take after a challenge*, presigned on the peg-in, pays it and its stake;
//!   - against it, the federation spends the stake as it rules, and the peg-in stays on the graph
//!     for another operator;
//!   - with no verdict in time, *the default*, presigned, gives the operator its stake back and
//!     leaves the peg-in where it is; unchallenged, it is how the operator's stake comes back after
//!     Take.
//!
//! What phase 1 trusts: one of the signers to forget its key after presigning, so the peg-in leaves
//! by the graph alone; and the federation's threshold to rule on challenges honestly. A dishonest
//! federation can at worst reimburse a false claim; it can never move a peg-in elsewhere. Phase 2
//! takes the federation out: Bitcoin's own script checks the withdrawal proof.
//!
//! **The peg-in itself** is two steps, so a depositor is never stuck: its coins go first to a
//! request, which the depositor takes back after `refund_after` blocks, unless the signers, having
//! presigned the graph, confirm it into the peg-in output. Only the confirmed peg-in mints.
//!
//! Everything is P2WSH, CHECKMULTISIG and CHECKSEQUENCEVERIFY, as this crate's Bitcoin code signs
//! by BIP143; taproot and Schnorr come with phase 2.

#[cfg(test)]
mod tests;

use {
    crate::crypto::bitcoin::hash160,
    crate::crypto::bitcoin::tx::{SIGHASH_ALL, SIGHASH_ANYONECANPAY, SIGHASH_SINGLE, Transaction, TxIn, TxOut, der, p2wpkh, p2wsh},
    r#macro::document,
};

/// Opcodes the graph's scripts use.
const OP_0: u8 = 0x00;
const OP_IF: u8 = 0x63;
const OP_ELSE: u8 = 0x67;
const OP_ENDIF: u8 = 0x68;
const OP_DROP: u8 = 0x75;
const OP_CHECKSIG: u8 = 0xac;
const OP_CHECKMULTISIG: u8 = 0xae;
const OP_CHECKSEQUENCEVERIFY: u8 = 0xb2;

#[document(
    summary = "What every output of the graph holds at least, so Bitcoin's nodes relay it: 330 satoshis, a P2WSH output's dust limit.",
    category = "Bridges",
)]
pub const GRAPH_DUST: u64 = 330;

/// A number as script pushes it: its own opcode to 16, else its little-endian bytes, minimal, with a
/// sign byte where the top bit is set, as CScriptNum has it.
fn number(value: u64) -> Vec<u8> {
    match value {
        0 => vec![OP_0],
        1..=16 => vec![0x50 + value as u8],
        _ => {
            let mut bytes = value.to_le_bytes().to_vec();
            while bytes.last() == Some(&0) {
                bytes.pop();
            }
            if bytes.last().is_some_and(|last| last & 0x80 != 0) {
                bytes.push(0);
            }

            [vec![bytes.len() as u8], bytes].concat()
        }
    }
}

fn push(bytes: &[u8]) -> Vec<u8> {
    [vec![bytes.len() as u8], bytes.to_vec()].concat()
}

#[document(
    summary = "m of these keys, as CHECKMULTISIG takes them, in the order given: what signatures must follow.",
    category = "Bridges",
)]
pub fn multisig(threshold: usize, keys: &[[u8; 33]]) -> Vec<u8> {
    let mut script = number(threshold as u64);
    for key in keys {
        script.extend(push(key));
    }
    script.extend(number(keys.len() as u64));
    script.push(OP_CHECKMULTISIG);

    script
}

/// After so many blocks from the output's confirmation, as BIP68 counts them.
fn after(blocks: u16) -> Vec<u8> {
    [number(blocks.into()), vec![OP_CHECKSEQUENCEVERIFY, OP_DROP]].concat()
}

#[document(
    summary = "Who and what one peg-in's graph is for: the signers who presign it, every one of them; the federation that rules on a challenge, by its threshold; the operator and its funding output; the peg-in; the stake, the fee each transaction pays, and the windows, in blocks.",
    category = "Bridges",
    see_also = "Graph",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphTerms {
    #[document(summary = "The signers, every one of whom presigns the graph: the n of the n-of-n.")]
    pub signers: Vec<[u8; 33]>,
    #[document(summary = "The federation's keys, which rule on a challenge.")]
    pub federation: Vec<[u8; 33]>,
    #[document(summary = "How many of them a verdict takes.")]
    pub threshold: usize,
    #[document(summary = "The operator's key: it signs the kickoff and the claim, and is paid.")]
    pub operator: [u8; 33],
    #[document(summary = "The operator's funding output for the kickoff, a P2WPKH of its key: its transaction's id and index.")]
    pub funding: ([u8; 32], u32),
    #[document(summary = "What the funding output holds.")]
    pub funding_value: u64,
    #[document(summary = "The confirmed peg-in output: its transaction's id and index.")]
    pub peg_in: ([u8; 32], u32),
    #[document(summary = "What it holds.")]
    pub peg_in_value: u64,
    #[document(summary = "Its witness script, as peg_in_script made it.")]
    pub peg_in_script: Vec<u8>,
    #[document(summary = "The operator's stake, held in the challenge connector, which a lost challenge costs it.")]
    pub stake: u64,
    #[document(summary = "The fee each of the graph's transactions pays.")]
    pub fee: u64,
    #[document(summary = "Blocks after the kickoff before the operator may take: the time watchers have to challenge.")]
    pub challenge_window: u16,
    #[document(summary = "Blocks after a challenge before the default holds, if the federation rules on nothing.")]
    pub dispute_window: u16,
}

#[document(
    summary = "The peg-in request's script: the signers together, every one of them, which confirm it into the peg-in; or, refund_after blocks on, the depositor alone, taking its coins back.",
    category = "Bridges",
    see_also = "peg_in_script",
)]
pub fn request_script(signers: &[[u8; 33]], depositor: &[u8; 33], refund_after: u16) -> Vec<u8> {
    [vec![OP_IF], multisig(signers.len(), signers), vec![OP_ELSE], after(refund_after), push(depositor), vec![OP_CHECKSIG, OP_ENDIF]].concat()
}

#[document(
    summary = "The confirmed peg-in's script: the chainv4 recipient, chain and gateway named and dropped, as the gateway's deposit scripts name them, then every one of the signers. What the gateway mints by, and only the graph spends.",
    category = "Bridges",
    see_also = "request_script, btc_deposit_script",
)]
pub fn peg_in_script(recipient: &crate::structs::address::Address, chain_id: u64, gateway: &crate::structs::address::Address, signers: &[[u8; 33]]) -> Vec<u8> {
    crate::bridge::btc_deposit_script(recipient, chain_id, gateway, signers.len() as u32, signers)
}

#[document(
    summary = "A transaction of the graph by name: what the signers presign, and the operator, the challenger and the federation complete.",
    category = "Bridges",
    see_also = "Graph",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GraphStep {
    #[document(summary = "The operator's kickoff.")]
    Kickoff,
    #[document(summary = "Take, unchallenged.")]
    Take,
    #[document(summary = "The challenge.")]
    Challenge,
    #[document(summary = "Take after a challenge, on the federation's verdict.")]
    Vindicated,
    #[document(summary = "The default: the stake back, once no verdict came in time.")]
    Default,
}

#[document(
    summary = "One signature the signers presign: which transaction, which input, under which sighash type.",
    category = "Bridges",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presign {
    #[document(summary = "The transaction.")]
    pub step: GraphStep,
    #[document(summary = "Its input.")]
    pub input: usize,
    #[document(summary = "The sighash type.")]
    pub sighash: u32,
}

#[document(
    summary = "One peg-in's graph for one operator, every transaction built, none signed: what the signers presign before they confirm the peg-in.",
    category = "Bridges",
    see_also = "GraphTerms, GraphStep",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graph {
    #[document(summary = "What it is for.")]
    pub terms: GraphTerms,
    #[document(summary = "The kickoff.")]
    pub kickoff: Transaction,
    #[document(summary = "Take, unchallenged.")]
    pub take: Transaction,
    #[document(summary = "The challenge, its first output paying the operator; a challenger adds its own inputs and outputs after.")]
    pub challenge: Transaction,
    #[document(summary = "Take after a challenge.")]
    pub vindicated: Transaction,
    #[document(summary = "The default.")]
    pub default: Transaction,
}

fn input(txid: [u8; 32], vout: u32, sequence: u32) -> TxIn {
    TxIn { txid, vout, script_sig: Vec::new(), sequence, witness: Vec::new() }
}

fn transaction(inputs: Vec<TxIn>, outputs: Vec<TxOut>) -> Transaction {
    Transaction { version: 2, inputs, outputs, lock_time: 0 }
}

/// No relative lock, and opted into replace-by-fee.
const NO_LOCK: u32 = 0xffff_fffd;

impl Graph {
    #[document(parent = "Graph", summary = "The claim connector's script: the operator alone, challenge_window blocks after the kickoff.")]
    pub fn claim_script(&self) -> Vec<u8> {
        [after(self.terms.challenge_window), push(&self.terms.operator), vec![OP_CHECKSIG]].concat()
    }

    #[document(parent = "Graph", summary = "The challenge connector's script: every one of the signers, which presigned Take and the challenge on it.")]
    pub fn challenge_script(&self) -> Vec<u8> {
        multisig(self.terms.signers.len(), &self.terms.signers)
    }

    #[document(parent = "Graph", summary = "The verdict connector's script: the federation's threshold, ruling; or, challenge_window + dispute_window blocks after the kickoff, every one of the signers, who presigned the default.")]
    pub fn verdict_script(&self) -> Vec<u8> {
        let terms = &self.terms;

        [vec![OP_IF], multisig(terms.threshold, &terms.federation), vec![OP_ELSE], after(self.default_after()), multisig(terms.signers.len(), &terms.signers), vec![OP_ENDIF]].concat()
    }

    #[document(parent = "Graph", summary = "Blocks after the kickoff before the default holds: the challenge window and the dispute window together.")]
    pub fn default_after(&self) -> u16 {
        self.terms.challenge_window.saturating_add(self.terms.dispute_window)
    }

    #[document(
        parent = "Graph",
        summary = "Builds the graph from its terms: the kickoff from the funding output, the claim connector holding what one transaction's fee needs, the challenge connector the stake, the rest back to the operator; and every transaction that spends them.",
        returns = "The graph, or why not: a funding output too small for the stake, the connectors and the fee, or a peg-in under its fees.",
    )]
    pub fn build(terms: GraphTerms) -> Result<Graph, String> {
        let operator_out = p2wpkh(&hash160(&terms.operator));
        let connector = terms.fee.max(GRAPH_DUST);
        let needed = 2 * connector + terms.stake + terms.fee;

        if terms.funding_value < needed || terms.stake < GRAPH_DUST {
            return Err(format!("a funding output of {} satoshis, short of the stake, the connectors and the fee, {needed}", terms.funding_value));
        }

        if terms.peg_in_value <= 2 * terms.fee + GRAPH_DUST {
            return Err("a peg-in that its fees would eat".to_string());
        }

        if terms.signers.is_empty() || terms.threshold == 0 || terms.threshold > terms.federation.len() {
            return Err("signers, and a threshold of the federation it has".to_string());
        }

        let mut graph = Graph { terms: terms.clone(), kickoff: transaction(Vec::new(), Vec::new()), take: transaction(Vec::new(), Vec::new()), challenge: transaction(Vec::new(), Vec::new()), vindicated: transaction(Vec::new(), Vec::new()), default: transaction(Vec::new(), Vec::new()) };
        let change = terms.funding_value - needed;
        let mut kickoff_outputs = vec![
            TxOut { value: connector, script: p2wsh(&graph.claim_script()) },
            TxOut { value: connector, script: p2wsh(&graph.challenge_script()) },
            TxOut { value: terms.stake, script: p2wsh(&graph.verdict_script()) },
        ];
        if change >= GRAPH_DUST {
            kickoff_outputs.push(TxOut { value: change, script: operator_out.clone() });
        }
        graph.kickoff = transaction(vec![input(terms.funding.0, terms.funding.1, NO_LOCK)], kickoff_outputs);
        let kickoff = graph.kickoff.txid();

        // Take: the claim connector waits out the challenge window by its sequence, BIP68's blocks.
        graph.take = transaction(
            vec![input(terms.peg_in.0, terms.peg_in.1, NO_LOCK), input(kickoff, 0, terms.challenge_window.into()), input(kickoff, 1, NO_LOCK)],
            vec![TxOut { value: terms.peg_in_value + 2 * connector - terms.fee, script: operator_out.clone() }],
        );
        // The challenge: its first output, which SINGLE signs, pays the connector to the operator.
        graph.challenge = transaction(vec![input(kickoff, 1, NO_LOCK)], vec![TxOut { value: connector, script: operator_out.clone() }]);
        graph.vindicated = transaction(vec![input(terms.peg_in.0, terms.peg_in.1, NO_LOCK), input(kickoff, 2, NO_LOCK)], vec![TxOut { value: terms.peg_in_value + terms.stake - terms.fee, script: operator_out.clone() }]);
        graph.default = transaction(vec![input(kickoff, 2, graph.default_after().into())], vec![TxOut { value: terms.stake - terms.fee.min(terms.stake - GRAPH_DUST), script: operator_out }]);

        Ok(graph)
    }

    #[document(parent = "Graph", summary = "A transaction of the graph by its step.")]
    pub fn tx(&self, step: GraphStep) -> &Transaction {
        match step {
            GraphStep::Kickoff => &self.kickoff,
            GraphStep::Take => &self.take,
            GraphStep::Challenge => &self.challenge,
            GraphStep::Vindicated => &self.vindicated,
            GraphStep::Default => &self.default,
        }
    }

    #[document(parent = "Graph", summary = "Every signature the signers presign, in order: Take's peg-in and challenge connector, the challenge's connector as SINGLE|ANYONECANPAY, take after a challenge's peg-in, and the default's verdict connector.")]
    pub fn presigns() -> [Presign; 5] {
        [
            Presign { step: GraphStep::Take, input: 0, sighash: SIGHASH_ALL },
            Presign { step: GraphStep::Take, input: 2, sighash: SIGHASH_ALL },
            Presign { step: GraphStep::Challenge, input: 0, sighash: SIGHASH_SINGLE | SIGHASH_ANYONECANPAY },
            Presign { step: GraphStep::Vindicated, input: 0, sighash: SIGHASH_ALL },
            Presign { step: GraphStep::Default, input: 0, sighash: SIGHASH_ALL },
        ]
    }

    /// The script and value an input of a step spends.
    fn spent(&self, step: GraphStep, input: usize) -> (Vec<u8>, u64) {
        let connector = self.kickoff.outputs[0].value;
        let peg_in = (self.terms.peg_in_script.clone(), self.terms.peg_in_value);

        match (step, input) {
            (GraphStep::Take | GraphStep::Vindicated, 0) => peg_in,
            (GraphStep::Take, 1) => (self.claim_script(), connector),
            (GraphStep::Take, _) | (GraphStep::Challenge, _) => (self.challenge_script(), connector),
            (GraphStep::Vindicated, _) | (GraphStep::Default, _) => (self.verdict_script(), self.terms.stake),
            (GraphStep::Kickoff, _) => (Vec::new(), self.terms.funding_value),
        }
    }

    #[document(parent = "Graph", summary = "The hash an input of a step is signed over, under a sighash type: BIP143's, of the script and value it spends.")]
    pub fn sighash(&self, step: GraphStep, input: usize, sighash: u32) -> [u8; 32] {
        let (script, value) = self.spent(step, input);

        crate::crypto::bitcoin::sha256d(&self.tx(step).sighash_preimage(input, &script, value, sighash))
    }

    #[document(
        parent = "Graph",
        summary = "One signer's presignatures, in the order presigns lists them, each as a witness takes it: DER, then the sighash byte.",
        params = { secret = "The signer's secret key." },
    )]
    pub fn presign(&self, secret: &[u8; 32]) -> Vec<Vec<u8>> {
        Graph::presigns().iter().map(|presign| sign(secret, &self.sighash(presign.step, presign.input, presign.sighash), presign.sighash)).collect()
    }

    #[document(
        parent = "Graph",
        summary = "Whether one signer's presignatures are all its own and all there: what every signer checks of every other's before confirming the peg-in, and the gateway before it mints.",
        params = { key = "The signer's key.", signatures = "Its presignatures, as presign gives them." },
    )]
    pub fn check_presigned(&self, key: &[u8; 33], signatures: &[Vec<u8>]) -> bool {
        signatures.len() == Graph::presigns().len()
            && Graph::presigns().iter().zip(signatures).all(|(presign, signature)| {
                let Some((kind, body)) = signature.split_last() else {
                    return false;
                };

                u32::from(*kind) == presign.sighash && crate::crypto::bitcoin::tx::from_der(body).is_some_and(|signature| crate::crypto::secp256k1::verify_hash(key, &self.sighash(presign.step, presign.input, presign.sighash), &signature))
            })
    }

    /// Every signer's presignature for one presign, in the signers' order, as CHECKMULTISIG takes them.
    fn presigned(&self, signatures: &[Vec<Vec<u8>>], at: usize) -> Vec<Vec<u8>> {
        signatures.iter().map(|signer| signer[at].clone()).collect()
    }

    #[document(
        parent = "Graph",
        summary = "The kickoff, signed by the operator over its funding output.",
        params = { operator = "The operator's secret key." },
    )]
    pub fn signed_kickoff(&self, operator: &[u8; 32]) -> Transaction {
        let mut kickoff = self.kickoff.clone();
        let preimage = kickoff.preimage(0, &hash160(&self.terms.operator), self.terms.funding_value);
        kickoff.inputs[0].witness = vec![sign(operator, &crate::crypto::bitcoin::sha256d(&preimage), SIGHASH_ALL), self.terms.operator.to_vec()];

        kickoff
    }

    #[document(
        parent = "Graph",
        summary = "Take, whole: the signers' presignatures on the peg-in and the challenge connector, and the operator's own on the claim connector.",
        params = { signatures = "Every signer's presignatures, in the signers' order.", operator = "The operator's secret key." },
    )]
    pub fn signed_take(&self, signatures: &[Vec<Vec<u8>>], operator: &[u8; 32]) -> Transaction {
        let mut take = self.take.clone();
        take.inputs[0].witness = multisig_witness(self.presigned(signatures, 0), &self.terms.peg_in_script, None);
        take.inputs[1].witness = vec![sign(operator, &self.sighash(GraphStep::Take, 1, SIGHASH_ALL), SIGHASH_ALL), self.claim_script()];
        take.inputs[2].witness = multisig_witness(self.presigned(signatures, 1), &self.challenge_script(), None);

        take
    }

    #[document(
        parent = "Graph",
        summary = "The challenge, its presigned connector with the challenger's own inputs and outputs after it, which the challenger signs itself.",
        params = { signatures = "Every signer's presignatures.", inputs = "The challenger's inputs, unsigned.", outputs = "Its outputs, such as change." },
        returns = "The challenge, its connector signed; the challenger signs the inputs it added.",
    )]
    pub fn challenge_with(&self, signatures: &[Vec<Vec<u8>>], inputs: Vec<TxIn>, outputs: Vec<TxOut>) -> Transaction {
        let mut challenge = self.challenge.clone();
        challenge.inputs[0].witness = multisig_witness(self.presigned(signatures, 2), &self.challenge_script(), None);
        challenge.inputs.extend(inputs);
        challenge.outputs.extend(outputs);

        challenge
    }

    #[document(
        parent = "Graph",
        summary = "Take after a challenge, whole: the signers' presignatures on the peg-in, and the federation's verdict, its threshold's signatures, on the verdict connector.",
        params = { signatures = "Every signer's presignatures.", verdict = "The federation's signatures, by its keys' order, as verdict_signature gives each." },
    )]
    pub fn signed_vindicated(&self, signatures: &[Vec<Vec<u8>>], verdict: Vec<Vec<u8>>) -> Transaction {
        let mut vindicated = self.vindicated.clone();
        vindicated.inputs[0].witness = multisig_witness(self.presigned(signatures, 3), &self.terms.peg_in_script, None);
        vindicated.inputs[1].witness = multisig_witness(verdict, &self.verdict_script(), Some(true));

        vindicated
    }

    #[document(
        parent = "Graph",
        summary = "A federation member's verdict for the operator: its signature on take after a challenge's verdict connector.",
        params = { secret = "The member's secret key." },
    )]
    pub fn verdict_signature(&self, secret: &[u8; 32]) -> Vec<u8> {
        sign(secret, &self.sighash(GraphStep::Vindicated, 1, SIGHASH_ALL), SIGHASH_ALL)
    }

    #[document(
        parent = "Graph",
        summary = "The default, whole: the signers' presignatures on the verdict connector's later branch, valid default_after blocks after the kickoff.",
        params = { signatures = "Every signer's presignatures." },
    )]
    pub fn signed_default(&self, signatures: &[Vec<Vec<u8>>]) -> Transaction {
        let mut default = self.default.clone();
        default.inputs[0].witness = multisig_witness(self.presigned(signatures, 4), &self.verdict_script(), Some(false));

        default
    }
}

/// A CHECKMULTISIG witness: the empty item its off-by-one pops, the signatures, the branch an IF
/// takes when the script has one, and the script.
fn multisig_witness(signatures: Vec<Vec<u8>>, script: &[u8], branch: Option<bool>) -> Vec<Vec<u8>> {
    let mut witness = vec![Vec::new()];
    witness.extend(signatures);

    match branch {
        Some(true) => witness.push(vec![1]),
        Some(false) => witness.push(Vec::new()),
        None => {}
    }

    witness.push(script.to_vec());

    witness
}

/// A signature as a witness takes it: DER, then the sighash byte.
fn sign(secret: &[u8; 32], hash: &[u8; 32], sighash: u32) -> Vec<u8> {
    [der(&crate::crypto::secp256k1::sign_hash(secret, hash)), vec![sighash as u8]].concat()
}
