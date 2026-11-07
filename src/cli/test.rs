//! chainv4 test: unit tests for Lua contracts, run on an in-memory chain with no node or network.
//!
//! A test is a Lua function named `test_` and something. They live in a contract itself, defined
//! at its top level beside its code, and deploys leave them out (structs::lua_tests); in a
//! `name_test.lua` file; or in any file under a `tests` folder. Each runs on a fresh chain of its
//! own: the development chain, free of fees, every engine on, ten accounts funded. The contract a
//! file's tests are in is deployed there first, without its tests, as `t.self`.
//!
//! A test runs off chain, in a Lua state with the chain's libraries, so it can name addresses and
//! numbers as a contract does, and `t`, which drives the chain: each `t.call` or `t.deploy` is a
//! real transaction in a block of its own, run by the chain's own engines and fuel, and its
//! failure is a Lua error the test sees.

use {
    super::CliResult,
    crate::chain::{Allocation, Chain, Genesis, Imported},
    crate::consensus::{AuthorityProducer, DevAuthority, Producer},
    crate::crypto::keypair::Keypair,
    crate::db::MemoryDb,
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::structs::code::Code,
    crate::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType},
    crate::structs::u256::u256,
    mlua::{Lua, MultiValue, Table, Value},
    std::path::{Path, PathBuf},
    std::sync::{Arc, Mutex},
};

/// How many accounts a test chain funds, t.account(1) to t.account(10).
const ACCOUNTS: u8 = 10;

/// How long a test chain's slot is, in milliseconds.
const SLOT: u64 = 1_000;

#[derive(clap::Args)]
pub struct TestArgs {
    /// Files or folders to test: contracts with their own tests, name_test.lua files and files
    /// under tests folders. The current folder when none is given.
    pub paths: Vec<PathBuf>,
    /// Run only the tests whose names hold this.
    #[arg(long)]
    pub filter: Option<String>,
    /// The seed t.fuzz draws from, so a failing run comes out the same again.
    #[arg(long, default_value_t = 1)]
    pub seed: u64,
}

/// What `t` does in Lua beside what the chain answers: assertions, failures expected, a sender
/// for a while, and runs of random inputs.
const PRELUDE: &str = r#"
local t = ...

function t.eq(got, want, message)
    if got ~= want and tostring(got) ~= tostring(want) then
        error((message and (message .. ": ") or "") .. "expected " .. tostring(want) .. ", got " .. tostring(got), 2)
    end
end

function t.ok(value, message)
    if not value then
        error(message or "expected it to hold", 2)
    end
end

function t.expect_error(fn, words, ...)
    local ok, err = pcall(fn, ...)

    if ok then
        error("expected it to fail" .. (words and (" with " .. words) or ""), 2)
    end

    if words and not string.find(tostring(err), words, 1, true) then
        error("expected it to fail with " .. words .. ", got " .. tostring(err), 2)
    end

    return err
end

function t.as(who, fn, ...)
    local before = t.sender()
    t.from(who)
    local results = table.pack(pcall(fn, ...))
    t.from(before)

    if not results[1] then
        error(results[2], 0)
    end

    return table.unpack(results, 2, results.n)
end

function t.fuzz(runs, fn)
    for run = 1, runs do
        local ok, err = pcall(fn, t.gen)

        if not ok then
            error("fuzz run " .. run .. " of " .. runs .. ", seed " .. t.seed() .. ": " .. tostring(err), 0)
        end
    end
end
"#;

/// A test chain and what the last transaction on it did.
struct TestChain {
    chain: Chain,
    producer: AuthorityProducer,
    keys: Vec<Keypair>,
    /// The account sending, from 1.
    sender: usize,
    /// The coin the next call sends.
    value: u256,
    /// When the next block is made.
    now: u64,
    /// The last transaction's fuel and events.
    fuel: u64,
    events: Vec<crate::structs::event::Event>,
    /// Where t.fuzz draws from.
    seed: u64,
    rng: u64,
    /// Where paths a test names are read from.
    folder: PathBuf,
}

impl TestChain {
    fn new(folder: &Path, seed: u64) -> CliResult<TestChain> {
        let authority = DevAuthority::dev_keypair();
        let keys: Vec<Keypair> = (1..=ACCOUNTS).map(|n| Keypair::from_seed([0x70 + n; 32])).collect();
        let mut genesis = Genesis {
            fees: None,
            finality: None,
            consensus: crate::consensus::ConsensusConfig::Authority { key: authority.public(), slot_ms: SLOT },
            ..Genesis::dev_authority(authority.public())
        };
        genesis.allocations.extend(keys.iter().map(|key| Allocation { address: key.public().address(), amount: u256::from(10u64).pow(24) }));
        let chain = Chain::open(Arc::new(MemoryDb::default()), &genesis)?;
        let now = chain.tip().timestamp;

        Ok(TestChain {
            chain,
            producer: AuthorityProducer::new(authority, SLOT),
            keys,
            sender: 1,
            value: u256::ZERO,
            now,
            fuel: 0,
            events: Vec::new(),
            seed,
            rng: seed.max(1),
            folder: folder.to_path_buf(),
        })
    }

    /// Builds the next block with these transactions and takes it.
    fn block(&mut self, txs: Vec<SignedTx>) -> Result<Vec<crate::chain::Receipt>, String> {
        self.now = self.now.max(self.chain.tip().timestamp) + SLOT;
        let mut block = self.chain.build(txs, Vec::new(), self.producer.address(&[]), self.now).map_err(|err| err.to_string())?;
        block.seal = self.producer.seal(&block.header).ok_or("the test chain's producer could not seal")?;

        match self.chain.import(&block, self.now).map_err(|err| err.to_string())? {
            Imported::Extended(receipts) => Ok(receipts),
            other => Err(format!("the test chain did not take its own block: {other:?}")),
        }
    }

    /// Sends one transaction from the sender, in a block of its own, and keeps what it did.
    fn send(&mut self, tx: Tx) -> Result<crate::chain::Receipt, String> {
        let key = self.keys[self.sender - 1].clone();
        let nonce = crate::registry::Accounts::new(&self.chain.state()).get(&key.public().address()).map_err(|err| err.to_string())?.nonce;
        let signed = SignedTx::sign(tx, nonce, FeeOffer::NONE, &key, &self.chain.genesis());
        let receipt = self.block(vec![signed])?.into_iter().next().ok_or("the test chain left the transaction out of its block")?;
        self.value = u256::ZERO;
        self.fuel = receipt.fuel_used;
        self.events = receipt.events.clone();

        Ok(receipt)
    }

    /// What a deploy sent made, or why it failed.
    fn deploy(&mut self, code: Code) -> Result<Address, String> {
        match self.send(Tx::Deploy { code })?.result {
            Ok(crate::chain::Effect::Deployed(address)) => Ok(address),
            Ok(other) => Err(format!("the deploy did {other:?}")),
            Err(failure) => Err(failure.to_string()),
        }
    }

    fn path(&self, named: &str) -> PathBuf {
        self.folder.join(named)
    }

    /// The next of t.fuzz's numbers: xorshift64, from the seed.
    fn next(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;

        x
    }
}

/// A Lua contract's source as deployed: its includes put together, its own tests left out.
fn contract_source(path: &Path) -> CliResult<String> {
    Ok(crate::structs::lua_tests::strip_tests(&super::lua_bundle(path)?.source))
}

fn lua_error(message: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(message.to_string())
}

/// A Lua value as a call's argument.
fn argument(value: Value) -> mlua::Result<TxCallArgumentType> {
    crate::vm::lvm::argument_from_lua(value).map_err(lua_error)
}

fn to_value(lua: &Lua, argument: &TxCallArgumentType) -> mlua::Result<Value> {
    crate::vm::lvm::argument_to_lua(lua, argument).map_err(lua_error)
}

fn address_of(value: Value) -> mlua::Result<Address> {
    match argument(value)? {
        TxCallArgumentType::Address(address) => Ok(address),
        other => Err(lua_error(format!("{other:?} is not an address"))),
    }
}

/// An amount from Lua: a number, a u256 or decimal text.
fn amount_of(value: Value) -> mlua::Result<u256> {
    match argument(value)? {
        TxCallArgumentType::U256(amount) => Ok(amount),
        TxCallArgumentType::Amount(amount) => Ok(amount.to_u256()),
        TxCallArgumentType::U64(number) => Ok(u256::from(number)),
        TxCallArgumentType::I64(number) if number >= 0 => Ok(u256::from(number as u64)),
        TxCallArgumentType::String(text) => u256::from_str_radix(&text, 10).map_err(|_| lua_error(format!("{text} is not an amount"))),
        other => Err(lua_error(format!("{other:?} is not an amount"))),
    }
}

fn lock(chain: &Arc<Mutex<TestChain>>) -> std::sync::MutexGuard<'_, TestChain> {
    chain.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `t`, bound to a test chain.
fn bind(lua: &Lua, env: &Table, chain: Arc<Mutex<TestChain>>, contract: Option<Address>) -> mlua::Result<()> {
    let t = lua.create_table()?;
    let held = |chain: &Arc<Mutex<TestChain>>| chain.clone();

    if let Some(contract) = contract {
        t.set("self", to_value(lua, &TxCallArgumentType::Address(contract))?)?;
    }

    let c = held(&chain);
    t.set(
        "call",
        lua.create_function(move |lua, (contract, function, args): (Value, String, MultiValue)| {
            let to = address_of(contract)?;
            let data = args.into_iter().map(argument).collect::<mlua::Result<Vec<_>>>()?;
            let mut chain = c.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let amount = Amount::U256(chain.value);
            let receipt = chain.send(Tx::ContractCall { to, func: crate::crypto::hasher::Hasher::digest(crate::structs::hash::HashType::Xxh3_64, function.as_bytes()), amount, data }).map_err(lua_error)?;

            match receipt.result {
                Ok(crate::chain::Effect::Returned(values)) => values.iter().map(|value| to_value(lua, value)).collect::<mlua::Result<MultiValue>>(),
                Ok(_) => Ok(MultiValue::new()),
                Err(failure) => Err(lua_error(failure)),
            }
        })?,
    )?;

    let c = held(&chain);
    t.set(
        "deploy",
        lua.create_function(move |lua, named: String| {
            let mut chain = c.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let source = contract_source(&chain.path(&named)).map_err(lua_error)?;
            let address = chain.deploy(Code::Lua(source)).map_err(lua_error)?;

            to_value(lua, &TxCallArgumentType::Address(address))
        })?,
    )?;

    let c = held(&chain);
    t.set(
        "deploy_wasm",
        lua.create_function(move |lua, named: String| {
            let mut chain = c.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let module = std::fs::read(chain.path(&named)).map_err(|err| lua_error(format!("cannot read {named}: {err}")))?;
            let address = chain.deploy(Code::Wasm(module)).map_err(lua_error)?;

            to_value(lua, &TxCallArgumentType::Address(address))
        })?,
    )?;

    #[cfg(all(feature = "evm", feature = "client"))]
    {
        let c = held(&chain);
        t.set(
            "deploy_evm",
            lua.create_function(move |lua, (named, types, args): (String, Option<String>, MultiValue)| {
                let mut chain = c.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let text = std::fs::read_to_string(chain.path(&named)).map_err(|err| lua_error(format!("cannot read {named}: {err}")))?;
                let mut init = crate::structs::hex::from_hex(text.trim()).ok_or_else(|| lua_error(format!("{named} is not init code in hex")))?;

                if let Some(types) = types {
                    let args: Vec<String> = args.into_iter().map(|value| lua.coerce_string(value).map(|text| text.map(|text| text.to_string_lossy()).unwrap_or_default())).collect::<mlua::Result<_>>()?;
                    init.extend(super::evm::encode(&types, &args).map_err(lua_error)?);
                }

                let address = chain.deploy(Code::Evm(init)).map_err(lua_error)?;

                to_value(lua, &TxCallArgumentType::Address(address))
            })?,
        )?;

        let c = held(&chain);
        t.set(
            "evm_call",
            lua.create_function(move |lua, (contract, signature, args): (Value, String, MultiValue)| {
                let to = address_of(contract)?;
                let open = signature.find('(').ok_or_else(|| lua_error(format!("{signature} is no function signature")))?;
                let args: Vec<String> = args.into_iter().map(|value| lua.coerce_string(value).map(|text| text.map(|text| text.to_string_lossy()).unwrap_or_default())).collect::<mlua::Result<_>>()?;
                let input = [crate::crypto::secp256k1::keccak(signature.as_bytes())[..4].to_vec(), super::evm::encode(&signature[open..], &args).map_err(lua_error)?].concat();
                let mut chain = c.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let value = Amount::U256(chain.value);
                let receipt = chain.send(Tx::EvmCall { to, value, input }).map_err(lua_error)?;

                match receipt.result {
                    Ok(crate::chain::Effect::Returned(values)) => match values.as_slice() {
                        [TxCallArgumentType::Bytes(bytes)] => Ok(crate::structs::hex::to_hex(bytes)),
                        _ => Ok("0x".to_string()),
                    },
                    Ok(_) => Ok("0x".to_string()),
                    Err(failure) => Err(lua_error(failure)),
                }
            })?,
        )?;
    }

    let c = held(&chain);
    t.set(
        "transfer",
        lua.create_function(move |_, (to, amount): (Value, Value)| {
            let to = address_of(to)?;
            let amount = amount_of(amount)?;
            let mut chain = c.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let from = chain.keys[chain.sender - 1].public().address();

            match chain.send(Tx::Transfer { from, to, amount: Amount::U256(amount) }).map_err(lua_error)?.result {
                Ok(_) => Ok(()),
                Err(failure) => Err(lua_error(failure)),
            }
        })?,
    )?;

    let c = held(&chain);
    t.set(
        "account",
        lua.create_function(move |lua, number: usize| {
            let chain = c.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let key = chain.keys.get(number.wrapping_sub(1)).ok_or_else(|| lua_error(format!("accounts are 1 to {ACCOUNTS}")))?;

            to_value(lua, &TxCallArgumentType::Address(key.public().address()))
        })?,
    )?;

    let c = held(&chain);
    t.set(
        "from",
        lua.create_function(move |_, who: Value| {
            let mut chain = c.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let number = match who {
                Value::Integer(number) => number as usize,
                Value::Number(number) => number as usize,
                other => {
                    let address = address_of(other)?;
                    chain.keys.iter().position(|key| key.public().address() == address).map(|place| place + 1).ok_or_else(|| lua_error("only the test accounts send"))?
                }
            };

            if !(1..=ACCOUNTS as usize).contains(&number) {
                return Err(lua_error(format!("accounts are 1 to {ACCOUNTS}")));
            }

            chain.sender = number;

            Ok(())
        })?,
    )?;

    let c = held(&chain);
    t.set("sender", lua.create_function(move |_, ()| Ok(lock(&c).sender))?)?;

    let c = held(&chain);
    t.set(
        "pay",
        lua.create_function(move |_, amount: Value| {
            lock(&c).value = amount_of(amount)?;

            Ok(())
        })?,
    )?;

    let c = held(&chain);
    t.set(
        "balance",
        lua.create_function(move |lua, who: Value| {
            let address = address_of(who)?;
            let chain = lock(&c);
            let balance = crate::registry::Accounts::new(&chain.chain.state()).get(&address).map_err(lua_error)?.balance;

            to_value(lua, &TxCallArgumentType::U256(balance))
        })?,
    )?;

    let c = held(&chain);
    t.set(
        "storage",
        lua.create_function(move |lua, (contract, key, namespace): (Value, mlua::LuaString, Option<mlua::LuaString>)| {
            let contract = address_of(contract)?;
            let chain = lock(&c);
            let state = chain.chain.state();
            let stored = match namespace {
                Some(namespace) => state.get(crate::db::Column::Namespaces, &crate::db::namespace_key(&contract, &namespace.as_bytes(), &key.as_bytes())),
                None => state.get(crate::db::Column::Storage, &crate::db::storage_key(&contract, &key.as_bytes())),
            }
            .map_err(lua_error)?;

            stored.map(|bytes| lua.create_string(&bytes).map(Value::String)).transpose().map(|value| value.unwrap_or(Value::Nil))
        })?,
    )?;

    let c = held(&chain);
    t.set(
        "events",
        lua.create_function(move |lua, ()| {
            let events = lock(&c).events.clone();
            let list = lua.create_table()?;

            for event in events {
                let entry = lua.create_table()?;
                entry.set("contract", to_value(lua, &TxCallArgumentType::Address(event.contract))?)?;
                entry.set("name", event.name)?;
                let data = lua.create_table()?;

                for value in &event.data {
                    data.push(to_value(lua, value)?)?;
                }

                entry.set("data", data)?;
                list.push(entry)?;
            }

            Ok(list)
        })?,
    )?;

    let c = held(&chain);
    t.set("fuel", lua.create_function(move |_, ()| Ok(lock(&c).fuel))?)?;

    let c = held(&chain);
    t.set("time", lua.create_function(move |_, ()| Ok(lock(&c).chain.tip().timestamp))?)?;

    let c = held(&chain);
    t.set("height", lua.create_function(move |_, ()| Ok(lock(&c).chain.tip().height))?)?;

    let c = held(&chain);
    t.set(
        "warp",
        lua.create_function(move |_, milliseconds: u64| {
            lock(&c).now += milliseconds;

            Ok(())
        })?,
    )?;

    let c = held(&chain);
    t.set(
        "roll",
        lua.create_function(move |_, blocks: u64| {
            let mut chain = lock(&c);

            for _ in 0..blocks {
                chain.block(Vec::new()).map_err(lua_error)?;
            }

            Ok(())
        })?,
    )?;

    let c = held(&chain);
    t.set("seed", lua.create_function(move |_, ()| Ok(lock(&c).seed))?)?;

    // What t.fuzz hands each run: numbers, amounts, bytes and choices, all from the seed.
    let draws = lua.create_table()?;
    let c = held(&chain);
    draws.set(
        "int",
        lua.create_function(move |_, (low, high): (i64, i64)| {
            if high < low {
                return Err(lua_error("gen.int's high is below its low"));
            }

            let span = (high - low) as u64 + 1;

            Ok(low + (lock(&c).next() % span.max(1)) as i64)
        })?,
    )?;
    let c = held(&chain);
    draws.set(
        "u256",
        lua.create_function(move |lua, ()| {
            let mut chain = lock(&c);
            let mut bytes = [0u8; 32];

            for chunk in bytes.chunks_mut(8) {
                chunk.copy_from_slice(&chain.next().to_be_bytes());
            }

            to_value(lua, &TxCallArgumentType::U256(u256::from_be_bytes(bytes)))
        })?,
    )?;
    let c = held(&chain);
    draws.set(
        "bytes",
        lua.create_function(move |lua, length: usize| {
            let mut chain = lock(&c);
            let bytes: Vec<u8> = (0..length).map(|_| chain.next() as u8).collect();

            lua.create_string(&bytes)
        })?,
    )?;
    let c = held(&chain);
    draws.set("bool", lua.create_function(move |_, ()| Ok(lock(&c).next() & 1 == 1))?)?;
    let c = held(&chain);
    draws.set(
        "pick",
        lua.create_function(move |_, list: Table| {
            let length = list.raw_len();

            if length == 0 {
                return Err(lua_error("gen.pick of an empty list"));
            }

            list.raw_get::<Value>((lock(&c).next() % length as u64) as usize + 1)
        })?,
    )?;
    let c = held(&chain);
    draws.set(
        "address",
        lua.create_function(move |lua, ()| {
            let mut chain = lock(&c);
            let mut bytes = [0u8; 32];

            for chunk in bytes.chunks_mut(8) {
                chunk.copy_from_slice(&chain.next().to_be_bytes());
            }

            to_value(lua, &TxCallArgumentType::Address(Address::Ed25519(bytes)))
        })?,
    )?;
    t.set("gen", draws)?;

    lua.load(PRELUDE).set_name("=t").call::<()>(t.clone())?;
    env.raw_set("t", t)?;

    Ok(())
}

/// How a file's tests run: a contract's own, deployed first as t.self, or a test file's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Contract,
    Tests,
}

/// What a file holds to test: its kind and its tests' names.
fn tests_of(path: &Path) -> CliResult<Option<(Kind, Vec<String>)>> {
    let source = std::fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let names = crate::structs::lua_tests::test_names(&source);
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let in_tests = path.components().any(|part| part.as_os_str() == "tests");
    let kind = if name.ends_with("_test.lua") || in_tests { Kind::Tests } else { Kind::Contract };

    Ok((!names.is_empty()).then_some((kind, names)))
}

/// Every Lua file under the paths given that holds tests, in order.
fn files(paths: &[PathBuf]) -> CliResult<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut walk = paths.to_vec();

    if walk.is_empty() {
        walk.push(PathBuf::from("."));
    }

    while let Some(path) = walk.pop() {
        if path.is_dir() {
            let skip = path.file_name().is_some_and(|name| matches!(name.to_str(), Some("target" | "node_modules" | ".git")));

            if !skip {
                let mut entries: Vec<PathBuf> = std::fs::read_dir(&path)?.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
                entries.sort();
                walk.extend(entries.into_iter().rev());
            }
        } else if path.extension().is_some_and(|extension| extension == "lua") {
            found.push(path);
        }
    }

    Ok(found)
}

/// Runs one test on a chain of its own: the error that failed it, with its line, if any.
fn run_one(path: &Path, kind: Kind, name: &str, seed: u64) -> Result<(), String> {
    let folder = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut chain = TestChain::new(&folder, seed ^ crate::crypto::secp256k1::keccak(name.as_bytes())[0] as u64).map_err(|err| err.to_string())?;
    let contract = match kind {
        Kind::Contract => Some(chain.deploy(Code::Lua(contract_source(path).map_err(|err| err.to_string())?)).map_err(|why| format!("deploying it: {why}"))?),
        Kind::Tests => None,
    };
    let chain = Arc::new(Mutex::new(chain));
    let (lua, env) = crate::vm::lvm::test_state().map_err(|err| err.to_string())?;
    bind(&lua, &env, chain, contract).map_err(|err| err.to_string())?;
    let source = super::lua_bundle(path).map_err(|err| err.to_string())?.source;
    let chunk = format!("@{}", path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default());
    let returned: Value = lua.load(source.as_str()).set_name(chunk).set_environment(env.clone()).eval().map_err(|err| format!("loading it: {err}"))?;
    // M.test_x in what it returns, or test_x as a global.
    let function = match &returned {
        Value::Table(module) => module.get::<Value>(name).ok().filter(|value| matches!(value, Value::Function(_))),
        _ => None,
    }
    .or_else(|| env.get::<Value>(name).ok().filter(|value| matches!(value, Value::Function(_))));
    let Some(Value::Function(function)) = function else {
        return Err("a local function: name it M.test_x or test_x, which the runner can reach".to_string());
    };

    function.call::<()>(()).map_err(|err| said(&err.to_string()))
}

/// A failure as a person reads it: its message, without Lua's tracebacks or the runtime error
/// prefixes each layer adds.
fn said(error: &str) -> String {
    let first = error.split("\nstack traceback:").next().unwrap_or(error);
    let mut text = first.trim();

    while let Some(rest) = text.strip_prefix("runtime error: ") {
        text = rest;
    }

    text.replace("got runtime error: runtime error: ", "got ").replace("got runtime error: ", "got ")
}

pub fn test(args: TestArgs) -> CliResult<()> {
    let mut passed = 0;
    let mut failed = Vec::new();

    for path in files(&args.paths)? {
        let Some((kind, names)) = tests_of(&path)? else {
            continue;
        };
        println!("{}", path.display());

        for name in names.iter().filter(|name| args.filter.as_ref().is_none_or(|filter| name.contains(filter.as_str()))) {
            match run_one(&path, kind, name, args.seed) {
                Ok(()) => {
                    passed += 1;
                    println!("  ok    {name}");
                }
                Err(why) => {
                    println!("  FAIL  {name}: {why}");
                    failed.push(format!("{}: {name}", path.display()));
                }
            }
        }
    }

    println!("{passed} passed, {} failed", failed.len());

    match failed.is_empty() {
        true => Ok(()),
        false => Err(format!("{} failed: {}", failed.len(), failed.join(", ")).into()),
    }
}

#[cfg(test)]
mod tests;
