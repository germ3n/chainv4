//! Validity proofs through the chain, without a zkVM: a range run off the node over the witness a
//! node builds for it comes to the chain's own end, a witness or block changed in any way does not,
//! a range that scans carries the whole state, what left through the gateway is in its root; and
//! from the proofs fork Tx::Prove moves the proven tip and pays, by the test proof kind.

use {
    super::voting::{INTERVAL, finalize_through, nonce, voters},
    super::*,
    crate::governance::{Change, ChangeVote, GovernanceConfig},
    crate::params::forks::{ForkSchedule, PROOFS, ScheduledFork},
    crate::proofs::{GatewayMode, ProofKind, ProofSettings, ProvenChain, ProvenRange, RangeEnd, RangeInput, Witness, replay, withdrawals_root},
    crate::registry::changes::Changes,
    crate::structs::code::Code,
    crate::structs::tx::Tx,
};

/// FFG of the three voters, governed by the dev key alone, the proofs fork from a height.
fn genesis(fork: u64) -> Genesis {
    Genesis {
        finality: Some(super::voting::ffg()),
        governance: Some(GovernanceConfig::Council { members: vec![dev().public().address()], threshold: 1, approval_epochs: 8, quorum_permille: 0 }),
        directory_admin: None,
        forks: ForkSchedule { forks: vec![ScheduledFork { name: PROOFS.name.to_string(), height: fork, epoch: None, least: None }], lead: None },
        ..genesis_with(None)
    }
}

fn json(genesis: &Genesis) -> Vec<u8> {
    serde_json::to_vec(&crate::chain::json::to_json(genesis)).unwrap()
}

/// Sends transactions from the dev key in the next block, with whatever justifications are unpaid.
fn send_all(chain: &mut Chain, txs: Vec<Tx>) -> Vec<Receipt> {
    let first = nonce(chain, &dev());
    let signed = txs.into_iter().enumerate().map(|(at, tx)| SignedTx::sign(tx, first + at as u64, FeeOffer::NONE, &dev(), &chain.genesis())).collect();
    let proofs = chain.unpaid_proofs().unwrap();

    advance(chain, signed, proofs).unwrap()
}

/// Votes Yes on a change as the council of one, which decides it at once.
fn decide(chain: &mut Chain, change: Change) {
    let version = Changes::new(&chain.state()).version_of(change.kind()).unwrap();
    let receipts = send_all(chain, vec![Tx::VoteChange { version, change, vote: ChangeVote::Yes }]);

    assert!(matches!(receipts[0].result, Ok(Effect::Decided { .. })), "{:?}", receipts[0].result);
}

fn end_at(chain: &Chain, height: u64) -> RangeEnd {
    let header = chain.block_at(height).unwrap().unwrap().header;

    RangeEnd { height, block: header.id(), state_root: header.state_root }
}

/// A chain some epochs on, its checkpoints justified and paid inside its blocks, with transfers
/// and a contract on the way.
fn grown() -> (Genesis, Chain) {
    let genesis = genesis(2 * INTERVAL);
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();

    for epoch in 1..=4 {
        finalize_through(&mut chain, epoch);
        send_all(&mut chain, vec![Tx::Transfer { from: dev().public().address(), to: key(40 + epoch as u8).public().address(), amount: crate::structs::amount::Amount::U128((5 + epoch).into()) }]);
    }

    (genesis, chain)
}

#[test]
fn a_range_run_over_its_witness_comes_to_the_chains_own_end() {
    let (genesis, chain) = grown();
    let (from, to) = (INTERVAL, 4 * INTERVAL);
    let input = RangeInput::build(&chain, chain.db().snapshot(), json(&genesis), from, to).unwrap();

    assert!(matches!(input.witness, Witness::Partial { .. }), "nothing scanned");
    assert!(input.blocks.iter().any(|block| !block.proofs.is_empty()), "justifications inside the range");
    assert!(input.headers.len() > 1, "the checkpoints they name, below the start");

    // Through its encoding, as a zkVM reads it.
    let input: RangeInput = wincode::deserialize(&wincode::serialize(&input).unwrap()).unwrap();
    let proven = replay(&input).unwrap();

    assert_eq!(proven.start, end_at(&chain, from));
    assert_eq!(proven.end, end_at(&chain, to));
    assert_eq!((proven.genesis, proven.chain_id), (chain.genesis(), crate::crypto::ethereum::chain_id(&chain.genesis())));
    assert_eq!((proven.withdrawals, proven.program, proven.gateway), ([0; 32], [0; 32], GatewayMode::ProofOrFederation));
}

#[test]
fn a_witness_or_block_changed_in_any_way_runs_to_nothing() {
    let (genesis, chain) = grown();
    let input = RangeInput::build(&chain, chain.db().snapshot(), json(&genesis), INTERVAL, 3 * INTERVAL).unwrap();
    let fails = |input: &RangeInput| replay(input).expect_err("refused").to_string();
    let Witness::Partial { nodes, entries } = &input.witness else { panic!("a partial witness") };
    let at = entries.iter().position(|entry| entry.value.is_some()).unwrap();

    // An entry's value changed, or one left out.
    let mut changed = entries.clone();
    changed[at].value.as_mut().unwrap().push(1);
    let why = fails(&RangeInput { witness: Witness::Partial { nodes: nodes.clone(), entries: changed }, ..input.clone() });
    assert!(why.contains("witness does not hold"), "{why}");

    let mut fewer = entries.clone();
    fewer.remove(at);
    let why = fails(&RangeInput { witness: Witness::Partial { nodes: nodes.clone(), entries: fewer }, ..input.clone() });
    assert!(why.contains("holds no"), "{why}");

    // A node changed.
    let mut moved = nodes.clone();
    let node = moved.iter_mut().find(|node| node.node.is_some()).unwrap();
    node.node.as_mut().unwrap()[1] ^= 1;
    assert!(replay(&RangeInput { witness: Witness::Partial { nodes: moved, entries: entries.clone() }, ..input.clone() }).is_err());

    // A transaction changed, a block left out, a header not its child's parent.
    let mut blocks = input.blocks.clone();
    let carrying = blocks.iter().position(|block| !block.txs.is_empty()).unwrap();
    blocks[carrying].txs.pop();
    let why = fails(&RangeInput { blocks, ..input.clone() });
    assert!(why.contains("roots are not its own"), "{why}");

    let mut blocks = input.blocks.clone();
    blocks.remove(1);
    assert!(fails(&RangeInput { blocks, ..input.clone() }).contains("does not build on"));

    let mut headers = input.headers.clone();
    headers[1].header.timestamp += 1;
    assert!(fails(&RangeInput { headers, ..input.clone() }).contains("not its child's parent"));

    // Another genesis is another chain, whose state is not this one's.
    let other = Genesis { reward: crate::params::Reward { initial: u256::new(1), ..genesis.reward }, ..genesis.clone() };
    assert!(replay(&RangeInput { genesis: json(&other), ..input.clone() }).is_err());

    assert!(replay(&input).is_ok(), "and as built, it runs");
}

/// A Lua contract that counts its entries by scanning them, and one that reports what left as the
/// gateway does: Sent(nonce, target, digest, sender).
const SCANNER: &str = r#"local M = {}
function M.add(key) storage.set(key, "1") end
function M.count()
    local n = 0
    for _ in storage.iter("", "") do n = n + 1 end
    storage.set("~count", tostring(n))
end
return M
"#;
const GATEWAY: &str = r#"local M = {}
function M.send(text)
    events.emit("Sent", 1, 11155111, hash.keccak256(text):bytes(), ctx.sender)
end
return M
"#;

fn deploy(chain: &mut Chain, source: &str) -> Address {
    let receipts = send_all(chain, vec![Tx::Deploy { code: Code::Lua(source.to_string()) }]);

    match &receipts[0].result {
        Ok(Effect::Deployed(address)) => *address,
        other => panic!("not deployed: {other:?}"),
    }
}

fn call(chain: &mut Chain, contract: Address, function: &str, args: Vec<crate::structs::tx::TxCallArgumentType>) -> Receipt {
    let call = Tx::ContractCall { to: contract, func: crate::structs::hash::Hash::Xxh3_64(crate::vm::selector(function)), amount: crate::structs::amount::Amount::U128(0), data: args };
    let mut receipts = send_all(chain, vec![call]);
    let receipt = receipts.remove(0);
    assert!(receipt.result.is_ok(), "{function}: {:?}", receipt.result);

    receipt
}

#[test]
fn a_range_that_scans_carries_the_whole_state_and_what_left_is_in_its_root() {
    let genesis = genesis(2 * INTERVAL);
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
    finalize_through(&mut chain, 1);
    let scanner = deploy(&mut chain, SCANNER);
    let gateway = deploy(&mut chain, GATEWAY);
    call(&mut chain, scanner, "add", vec![crate::structs::tx::TxCallArgumentType::String("a".to_string())]);
    let mut directory = Changes::new(&chain.state()).directory().unwrap().unwrap_or_default();
    let target = chain.tip().height / INTERVAL + 1;
    directory.gateway = Some(gateway);
    decide(&mut chain, Change::Directory(directory));
    let from = chain.tip().height;

    call(&mut chain, scanner, "count", Vec::new());
    let sent = call(&mut chain, gateway, "send", vec![crate::structs::tx::TxCallArgumentType::String("away".to_string())]);
    let other = deploy(&mut chain, GATEWAY);
    call(&mut chain, other, "send", vec![crate::structs::tx::TxCallArgumentType::String("not the gateway".to_string())]);
    let _ = target;
    let to = chain.tip().height;

    let input = RangeInput::build(&chain, chain.db().snapshot(), json(&genesis), from, to).unwrap();
    assert!(matches!(input.witness, Witness::Whole { .. }), "it scanned");
    let proven = replay(&input).unwrap();
    assert_eq!(proven.end, end_at(&chain, to));

    let crate::structs::tx::TxCallArgumentType::Bytes(digest) = &sent.events[0].data[2] else { panic!("a digest") };
    assert_eq!(proven.withdrawals, withdrawals_root(&[digest.as_slice().try_into().unwrap()]), "the gateway's alone");

    // A whole witness missing an entry is not the start's state.
    let Witness::Whole { mut entries } = input.witness.clone() else { unreachable!() };
    entries.pop();
    let why = replay(&RangeInput { witness: Witness::Whole { entries }, ..input }).expect_err("refused").to_string();
    assert!(why.contains("not the start's state"), "{why}");
}

fn settings(reward: u64) -> ProofSettings {
    ProofSettings { kind: ProofKind::Test, program: [5; 32], reward_per_block: u256::new(reward.into()), gateway: GatewayMode::Both }
}

/// A test proof of the range from the proven tip to a height, as the replay would prove it.
fn prove_to(chain: &Chain, height: u64, program: [u8; 32]) -> Tx {
    let start = ProvenChain::new(&chain.state()).tip().unwrap().unwrap();
    let range = ProvenRange {
        chain_id: crate::crypto::ethereum::chain_id(&chain.genesis()),
        genesis: chain.genesis(),
        start,
        end: end_at(chain, height),
        withdrawals: [0; 32],
        program,
        gateway: GatewayMode::Both,
    };
    let proof = crate::proofs::TestProofs::proof(&program, &range.public_values());

    Tx::Prove { kind: ProofKind::Test, range, proof }
}

#[test]
fn from_the_fork_a_proof_of_the_range_from_the_proven_tip_moves_it_and_pays_for_each_block() {
    let fork = 2 * INTERVAL;
    let genesis = genesis(fork);
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();

    // Before the fork: no proven tip, and neither settings nor a proof is a transaction.
    finalize_through(&mut chain, 1);
    assert_eq!(ProvenChain::new(&chain.state()).tip().unwrap(), None);
    let version = Changes::new(&chain.state()).version_of(crate::governance::ChangeKind::Proofs).unwrap();
    assert!(send_all(&mut chain, vec![Tx::VoteChange { version, change: Change::Proofs(settings(3)), vote: ChangeVote::Yes }]).is_empty(), "left out of the block");

    // From it, the tip starts at the block before it, and each block records its parent.
    grow_to(&mut chain, fork);
    assert_eq!(ProvenChain::new(&chain.state()).tip().unwrap(), Some(end_at(&chain, fork - 1)));
    assert_eq!(ProvenChain::new(&chain.state()).block_at(fork - 1).unwrap(), Some(chain.block_at(fork - 1).unwrap().unwrap().id()));

    // No settings, no proofs.
    finalize_through(&mut chain, 3);
    let refused = { let tx = prove_to(&chain, 3 * INTERVAL, [5; 32]); send_all(&mut chain, vec![tx]) };
    assert!(refused[0].result.as_ref().is_err_and(|why| why.to_string().contains("no proof system")), "{:?}", refused[0].result);

    decide(&mut chain, Change::Proofs(settings(3)));
    let balance = |chain: &Chain| super::voting::balance(chain, &dev().public().address());

    // Not to a checkpoint, by another program, or from somewhere else: refused, and nothing paid.
    for (tx, why) in [
        (prove_to(&chain, 3 * INTERVAL - 1, [5; 32]), "later checkpoint"),
        (prove_to(&chain, 3 * INTERVAL, [6; 32]), "does not hold"),
    ] {
        let refused = send_all(&mut chain, vec![tx]);
        assert!(refused[0].result.as_ref().is_err_and(|failure| failure.to_string().contains(why)), "{why}: {:?}", refused[0].result);
    }

    let before = balance(&chain);
    let proved = { let tx = prove_to(&chain, 3 * INTERVAL, [5; 32]); send_all(&mut chain, vec![tx]) };
    assert!(proved[0].result.is_ok(), "{:?}", proved[0].result);
    assert_eq!(ProvenChain::new(&chain.state()).tip().unwrap(), Some(end_at(&chain, 3 * INTERVAL)));
    let covered = 3 * INTERVAL - (fork - 1);
    assert_eq!(balance(&chain), before + u256::new((3 * covered).into()), "3 a block, the vote's fee being none");

    // The same range again starts behind the tip.
    let Tx::Prove { kind, mut range, proof } = prove_to(&chain, 3 * INTERVAL, [5; 32]) else { unreachable!() };
    range.start = end_at(&chain, fork - 1);
    let again = send_all(&mut chain, vec![Tx::Prove { kind, range, proof }]);
    assert!(again[0].result.as_ref().is_err_and(|why| why.to_string().contains("proven tip")), "{:?}", again[0].result);
}

#[test]
fn a_range_holding_a_proof_and_the_settings_proves_the_program_governance_names() {
    let fork = 2 * INTERVAL;
    let genesis = genesis(fork);
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
    finalize_through(&mut chain, 3);
    let from = chain.tip().height;
    decide(&mut chain, Change::Proofs(settings(0)));
    { let tx = prove_to(&chain, 3 * INTERVAL, [5; 32]); send_all(&mut chain, vec![tx]) };
    finalize_through(&mut chain, 5);
    let to = 5 * INTERVAL;

    let proven = replay(&RangeInput::build(&chain, chain.db().snapshot(), json(&genesis), from, to).unwrap()).unwrap();
    assert_eq!(proven.end, end_at(&chain, to));
    assert_eq!((proven.program, proven.gateway), ([5; 32], GatewayMode::Both), "governance's, at the end");
    let _ = voters();
}

/// The program SP1 proves runs a range as the node did: Lua calls, a failing one, a scan, a
/// transfer and the justifications blocks carry, executed in SP1's RISC-V with no proof made, its
/// public values the replay's own. Only with the sp1-prover feature, and only once cargo prove build
/// wrote the program (proofs/sp1/elf); it says so and passes when it has not.
#[cfg(feature = "sp1-prover")]
#[test]
fn the_sp1_program_runs_a_range_to_what_the_replay_proves() {
    use crate::proofs::Prover as _;

    let elf = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("proofs/sp1/elf/chainv4-proof-program");
    let Ok(elf) = std::fs::read(&elf) else {
        eprintln!("no program at {}: build it with cargo prove build in proofs/sp1/program", elf.display());

        return;
    };
    let genesis = genesis(2 * INTERVAL);
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
    finalize_through(&mut chain, 1);
    let from = chain.tip().height;
    let scanner = deploy(&mut chain, SCANNER);
    call(&mut chain, scanner, "add", vec![crate::structs::tx::TxCallArgumentType::String("a".to_string())]);
    call(&mut chain, scanner, "count", Vec::new());
    let failing = Tx::ContractCall { to: scanner, func: crate::structs::hash::Hash::Xxh3_64(crate::vm::selector("add")), amount: crate::structs::amount::Amount::U128(0), data: Vec::new() };
    assert!(send_all(&mut chain, vec![failing])[0].result.is_err(), "a Lua error, inside a block");
    send_all(&mut chain, vec![Tx::Transfer { from: dev().public().address(), to: key(77).public().address(), amount: crate::structs::amount::Amount::U128(9) }]);
    let to = chain.tip().height;

    let input = RangeInput::build(&chain, chain.db().snapshot(), json(&genesis), from, to).unwrap();
    let replayed = replay(&input).unwrap();
    let prover = crate::proofs::Sp1Prover::new(elf).unwrap();
    let (values, cycles) = prover.execute(&input.to_bytes()).unwrap();

    assert_eq!(ProvenRange::from_public_values(chain.hash(), &values), Some(replayed), "in {cycles} cycles");
    eprintln!("{} blocks in {cycles} SP1 cycles", to - from);
}
