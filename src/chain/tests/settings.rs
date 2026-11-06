//! Settings governance makes votable, each decided through the chain by a council of one and
//! taking effect from the block after the one that decides it.

use {
    super::*,
    crate::governance::{Change, ChangeVote, GovernanceConfig},
    crate::registry::changes::Changes,
};

/// The dev chain with a council of the authority alone, deciding any change with one Yes.
pub(super) fn governed(genesis: Genesis) -> Chain {
    let genesis = Genesis {
        governance: Some(GovernanceConfig::Council { members: vec![authority().public().address()], threshold: 1, approval_epochs: 8, quorum_permille: 0 }),
        ..genesis
    };

    Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap()
}

/// Votes Yes on a change at its kind's version, which the council of one decides at once.
pub(super) fn decide(chain: &mut Chain, change: Change) {
    let version = Changes::new(&chain.state()).version_of(change.kind()).unwrap();
    let vote = Tx::VoteChange { version, change, vote: ChangeVote::Yes };
    // On a chain with fees the vote pays for itself, well above the base price.
    let fee = match chain.fees() {
        Some(_) => FeeOffer { gas_limit: 1_000_000, max_price: chain.next_base_price() * u256::from(2u64), tip: u256::ZERO },
        None => FeeOffer::NONE,
    };
    let receipts = advance(chain, vec![SignedTx::sign(vote, nonce(chain, &authority()), fee, &authority(), &chain.genesis())]);

    assert!(matches!(receipts[0].result, Ok(Effect::Decided { .. })), "{:?}", receipts[0].result);
}

/// What the block after the tip runs with.
fn next(chain: &Chain) -> crate::chain::BlockParams {
    chain.params_in(&chain.state(), chain.tip().height + 1).unwrap()
}

#[cfg(feature = "finality-ffg")]
#[test]
fn a_change_of_fees_moves_vote_rewards_to_the_pool_from_the_next_block() {
    let fees = crate::params::fees::FeeConfig::dev();
    let mut chain = governed(Genesis { fees: Some(fees.clone()), ..genesis() });

    assert!(!next(&chain).rules.pooled_fees, "genesis mints them");

    decide(&mut chain, Change::Fees(crate::params::fees::FeeConfig { vote_rewards: Some(VoteRewardSource::Fees), ..fees }));

    assert!(next(&chain).rules.pooled_fees);
}

/// Votes Yes on a change, and says why it was refused, if it was.
fn refused_change(chain: &mut Chain, change: Change) -> String {
    let version = Changes::new(&chain.state()).version_of(change.kind()).unwrap();
    let vote = Tx::VoteChange { version, change, vote: ChangeVote::Yes };
    let receipts = advance(chain, vec![SignedTx::sign(vote, nonce(chain, &authority()), FeeOffer::NONE, &authority(), &chain.genesis())]);

    receipts[0].result.as_ref().expect_err("refused").to_string()
}

/// Calls a contract as the authority on a chain without fees, and says what it burned.
fn fuel_of(chain: &mut Chain, to: Address, function: &str) -> u64 {
    let tx = Tx::ContractCall { to, func: Hash::Xxh3_64(crate::vm::selector(function)), amount: Amount::U128(0), data: Vec::new() };
    let receipt = advance(chain, vec![SignedTx::sign(tx, nonce(chain, &authority()), FeeOffer::NONE, &authority(), &chain.genesis())]).remove(0);
    assert!(receipt.result.is_ok(), "{:?}", receipt.result);

    receipt.fuel_used
}

#[test]
fn host_costs_a_change_of_fees_names_charge_every_call_from_the_next_block() {
    let mut chain = governed(genesis());
    let deploy = Tx::Deploy { code: Code::Lua("local M = {}\nfunction M.write() storage.set(\"k\", \"v\") end\nreturn M".to_string()) };
    let signed = SignedTx::sign(deploy, nonce(&chain, &authority()), FeeOffer::NONE, &authority(), &chain.genesis());
    let receipt = advance(&mut chain, vec![signed]).remove(0);
    let Ok(Effect::Deployed(contract)) = receipt.result else { panic!("{:?}", receipt.result) };
    let before = fuel_of(&mut chain, contract, "write");

    // Fees turned on with storage writes ten times dearer, from the next block's rules on.
    let costs = crate::params::vm::HostCosts { storage_write: 5_000, ..crate::params::vm::HostCosts::DEFAULT };
    let fees = crate::params::fees::FeeConfig { costs: Some(costs), ..crate::params::fees::FeeConfig::dev() };
    assert_eq!(next(&chain).rules.vm.costs, crate::params::vm::HostCosts::DEFAULT);
    decide(&mut chain, Change::Fees(fees));
    assert_eq!(next(&chain).rules.vm.costs.storage_write, 5_000);

    let bad = crate::params::fees::FeeConfig { costs: Some(crate::params::vm::HostCosts { storage_bytes_per_fuel: 0, ..costs }), ..crate::params::fees::FeeConfig::dev() };
    assert!(bad.costs.unwrap().problem().is_some(), "a divisor of 0 is no cost table");
    assert!(before > 0);
}

#[test]
fn lua_costs_read_and_write_by_name_reach_the_next_block_and_refuse_a_rate_of_nothing() {
    use crate::params::{
        json::{fees_from_text, fees_json},
        vm::LuaCosts,
    };

    let mut costs = LuaCosts::DEFAULT;
    costs.slots[LuaCosts::slot("ADD").unwrap()] = 7;
    costs.slots[LuaCosts::slot("bytes_per_fuel").unwrap()] = 32;
    let fees = crate::params::fees::FeeConfig { lua_costs: Some(Box::new(costs)), ..crate::params::fees::FeeConfig::dev() };

    // Written by name, only what differs from the fork's own, and read back the same.
    let json = fees_json(&fees);
    assert_eq!(json["lua_costs"], serde_json::json!({ "ADD": 7, "bytes_per_fuel": 32 }));
    assert_eq!(fees_from_text(&json.to_string()).unwrap(), fees);

    let mut unknown = json.clone();
    unknown["lua_costs"]["ADDD"] = 1.into();
    assert!(fees_from_text(&unknown.to_string()).unwrap_err().contains("ADDD"));
    let mut nothing = json.clone();
    nothing["lua_costs"] = serde_json::json!({ "elements_per_fuel": 0 });
    assert!(fees_from_text(&nothing.to_string()).unwrap_err().contains("at least 1"));

    // A change of fees naming them sets the next block's.
    let mut chain = governed(genesis());
    assert_eq!(next(&chain).rules.vm.lua_costs, LuaCosts::DEFAULT);
    decide(&mut chain, Change::Fees(fees));
    assert_eq!(next(&chain).rules.vm.lua_costs, costs);
}

#[cfg(feature = "wasm")]
#[test]
fn an_engine_turned_off_freezes_its_contracts_and_turned_on_again_brings_them_back() {
    use crate::structs::code::Engine;

    let mut chain = governed(genesis());
    let deploy = |chain: &mut Chain| {
        let source = "local M = {}\nfunction M.write() storage.set(\"k\", \"v\") return storage.get(\"k\") end\nreturn M".to_string();
        let signed = SignedTx::sign(Tx::Deploy { code: Code::Lua(source) }, nonce(chain, &authority()), FeeOffer::NONE, &authority(), &chain.genesis());

        advance(chain, vec![signed]).remove(0).result
    };
    let Ok(Effect::Deployed(contract)) = deploy(&mut chain) else { panic!("a deploy") };
    fuel_of(&mut chain, contract, "write");
    let call = |chain: &mut Chain| {
        let tx = Tx::ContractCall { to: contract, func: Hash::Xxh3_64(crate::vm::selector("write")), amount: Amount::U128(0), data: Vec::new() };

        advance(chain, vec![SignedTx::sign(tx, nonce(chain, &authority()), FeeOffer::NONE, &authority(), &chain.genesis())]).remove(0).result
    };

    // Lua turned off: its contract is refused, nothing deploys on it, and what it keeps stays.
    assert!(refused_change(&mut chain, Change::Engines(vec![Engine::Wasm, Engine::Wasm])).contains("each once"));
    decide(&mut chain, Change::Engines(vec![Engine::Wasm]));
    assert!(!next(&chain).rules.vm.engines.contains(Engine::Lua));
    assert!(call(&mut chain).unwrap_err().to_string().contains("turned the lua engine off"));
    assert!(deploy(&mut chain).is_err(), "nothing deploys on it");
    assert!(chain.state().get(crate::db::Column::Storage, &crate::db::storage_key(&contract, b"k")).unwrap().is_some(), "its storage is kept");

    // On again, it is as it was.
    decide(&mut chain, Change::Engines(vec![Engine::Lua, Engine::Wasm]));
    assert!(call(&mut chain).is_ok());
}

#[test]
fn contract_limits_rise_never_fall_and_bound_the_transaction_and_multisig() {
    let mut chain = governed(genesis());
    let standing = genesis().block_limits();
    let raised = crate::params::vm::ContractLimits { key_len: 512, call_depth: 12, ..crate::params::vm::ContractLimits::DEFAULT };

    decide(&mut chain, Change::Limits(crate::params::BlockLimits { contract: Some(raised), max_tx_bytes: Some(256 * 1024), multisig_members: Some(2), ..standing }));
    let vm = next(&chain).rules.vm;
    assert_eq!((vm.limits, vm.max_tx_bytes, vm.multisig_members), (raised, Some(256 * 1024), 2));
    assert_eq!((vm.tx_bytes_limit(false), vm.tx_bytes_limit(true)), (Some(256 * 1024), Some(256 * 1024)), "Ethereum-signed ones too");

    // Lowering what a deployed contract may rely on is refused; the module size may go down.
    let lowered = crate::params::vm::ContractLimits { value_len: 1_024, ..raised };
    assert!(refused_change(&mut chain, Change::Limits(crate::params::BlockLimits { contract: Some(lowered), ..standing })).contains("never lowers"));
    decide(&mut chain, Change::Limits(crate::params::BlockLimits { contract: Some(crate::params::vm::ContractLimits { module_len: 64 * 1024, ..raised }), ..standing }));

    // A multisig of three is past the two the chain allows now.
    let members = [Keypair::from_seed([41; 32]), Keypair::from_seed([42; 32]), Keypair::from_seed([43; 32])];
    let create = Tx::CreateMultisig { members: members.iter().map(|key| crate::registry::multisig::MultisigMember { key: key.public(), bitcoin: None, evm: None }).collect(), threshold: 2, testnet: false };
    decide(&mut chain, Change::Limits(crate::params::BlockLimits { multisig_members: Some(2), ..standing }));
    let signed = SignedTx::sign(create, nonce(&chain, &authority()), FeeOffer::NONE, &authority(), &chain.genesis());
    let receipt = advance(&mut chain, vec![signed]).remove(0);
    assert!(matches!(&receipt.result, Err(failure) if failure.to_string().contains("multisig_members")), "{:?}", receipt.result);
    assert_eq!(crate::params::vm::VmRules::default().tx_bytes_limit(true), Some(crate::params::vm::ETHEREUM_TX_BYTES), "unset, Ethereum's own bound");
}

#[cfg(feature = "evm")]
#[test]
fn an_evm_spec_runs_from_the_height_its_change_names_and_never_goes_back() {
    let mut chain = governed(genesis());
    use crate::params::vm::EvmSpec;

    assert!(refused_change(&mut chain, Change::EvmSpec { spec: EvmSpec::Osaka, from: 1 }).contains("ahead"));
    assert!(refused_change(&mut chain, Change::EvmSpec { spec: EvmSpec::Cancun, from: 100 }).contains("no older"));

    let from = chain.tip().height + 5;
    decide(&mut chain, Change::EvmSpec { spec: EvmSpec::Osaka, from });
    let at = |chain: &Chain, height: u64| chain.params_in(&chain.state(), height).unwrap().rules.vm.evm_spec;

    assert_eq!((at(&chain, from - 1), at(&chain, from), at(&chain, from + 10)), (EvmSpec::Prague, EvmSpec::Osaka, EvmSpec::Osaka));
    assert_eq!(crate::vm::evm::spec_id(EvmSpec::Osaka), revm::primitives::hardfork::SpecId::OSAKA);
}

#[test]
fn token_functions_names_and_the_dex_fee_each_change_from_the_next_block() {
    let mut chain = governed(Genesis { names: Some(crate::registry::names::NamesConfig::dev()), ..genesis() });

    decide(&mut chain, Change::TokenFunctions(vec!["balance_of".to_string(), "transfer".to_string()]));
    assert_eq!(next(&chain).rules.standards_in(&chain.state()).unwrap().tokens.to_vec(), ["balance_of", "transfer"]);
    assert!(refused_change(&mut chain, Change::TokenFunctions(vec!["transfer".to_string(), "transfer".to_string()])).contains("once"));

    let settings = crate::registry::names::NameSettings { grace_ms: 1_000, max_years: 3, ..crate::registry::names::NamesConfig::dev().settings() };
    decide(&mut chain, Change::Names(settings.clone()));
    let names = next(&chain).rules.names.unwrap();
    assert_eq!((names.grace_ms, names.max_years, names.suffix.as_str()), (1_000, 3, "dev"), "the suffix stays genesis's");

    use crate::params::vm::DexSettings;

    assert_eq!(next(&chain).rules.vm.dex, DexSettings::DEFAULT, "0.05%, 0.3% and 1%, nothing burned, until governance says otherwise");
    let decided = DexSettings { tiers: [1, 30, 0, 0], protocol_permille: 250 };
    decide(&mut chain, Change::Dex(decided));
    assert_eq!((next(&chain).rules.vm.dex.offered(), next(&chain).rules.vm.dex.protocol_permille), (vec![1, 30], 250));
    for (bad, why) in [
        (DexSettings { tiers: [1_001, 0, 0, 0], ..decided }, "10%"),
        (DexSettings { tiers: [30, 5, 0, 0], ..decided }, "go up"),
        (DexSettings { tiers: [5, 0, 30, 0], ..decided }, "none left out"),
        (DexSettings { tiers: [0; 4], ..decided }, "at least one"),
        (DexSettings { protocol_permille: 1_001, ..decided }, "protocol_permille"),
    ] {
        assert!(refused_change(&mut chain, Change::Dex(bad)).contains(why), "{bad:?}");
    }
}
