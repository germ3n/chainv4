//! The fuel and memory fixture: calls whose outcome rests on how the Luau fork counts fuel and
//! memory, run on one chain, each in a block of its own, and written out with the fuel each burned,
//! how each ended, and the roots the chain ended on. Every node must come out the same, on every OS
//! and processor, idle or loaded: a transaction that runs out of fuel or memory on one node and not
//! another splits the chain. The printout is compared byte for byte with `fixture.txt` beside this
//! file, so a machine that differs fails here. After a change meant to move it, a consensus one,
//! write it again with `CHAINV4_WRITE_FIXTURE=1 cargo test --lib chain::tests::fixture`.

use super::*;

const CALLS: &str = r#"
local M = {}

-- Powers by repeated squaring, whole exponents only.
function M.power(a, b)
    return tostring(tonumber(a) ^ tonumber(b))
end

-- The matcher's steps, each paid for.
function M.backtrack(length)
    local s = string.rep("a", length)
    return string.find(s, ".-.-.-b") ~= nil
end

-- Sorting, by its comparisons.
function M.sorted(size)
    local t = {}
    for i = 1, size do t[i] = (i * 7919) % size end
    table.sort(t)
    return t[1] .. "," .. t[size]
end

-- Strings built past the memory limit: where it fails is where the bytes counted cross it.
function M.grown(rounds)
    local s = "ab"
    for i = 1, rounds do s = s .. s end
    return #s
end

-- Tables kept until the memory limit, each entry's bytes counted.
function M.held(n)
    local kept = {}
    for i = 1, n do kept[i] = { i, tostring(i), string.rep("x", 32) } end
    return #kept
end

-- Garbage made and dropped: the collector, paced by bytes, never by the clock.
function M.churn(n)
    local total = 0
    for i = 1, n do
        local t = { i, i + 1, tostring(i) }
        total = total + #t
    end
    return total
end

-- The integer types, through every operator.
function M.integers()
    local a, b = u256("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u256(7)
    local c = i256("-1_000_000_000_000_000_000")
    return tostring(a // b) .. " " .. tostring(a % b) .. " " .. tostring(c * i256(3)) .. " " .. tostring(u64(2):checked_pow(70))
end

-- utf8 and the string library over what they go through.
function M.texts(n)
    local s = string.rep("héllo wörld ", n)
    return utf8.len(s) .. " " .. #string.upper(s) .. " " .. #table.concat({ s, s }, "|") .. " " .. select(2, string.gsub(s, "o", "0"))
end

-- What a run out of fuel leaves.
function M.forever()
    local n = 0
    while true do n = n + 1 end
end

-- A script's own error, caught, its message as every node writes it.
function M.caught()
    local ok, why = pcall(function() error({ code = 7 }) end)
    local ok2, why2 = pcall(function() local t = nil; return t.x end)
    return tostring(ok) .. " " .. type(why) .. " " .. tostring(ok2) .. " " .. tostring(why2)
end

-- Storage, written and read back.
function M.stored(n)
    for i = 1, n do storage.set("k" .. i, string.rep("v", i)) end
    local total = 0
    for i = 1, n do total = total + #storage.get("k" .. i) end
    return total
end

return M
"#;

fn sign(chain: &Chain, tx: Tx) -> SignedTx {
    SignedTx::sign(tx, nonce(chain, &authority()), FeeOffer::NONE, &authority(), &chain.genesis())
}

fn number(value: f64) -> TxCallArgumentType {
    TxCallArgumentType::Number(value)
}

/// Every call, its fuel and how it ended, then the roots.
fn printout() -> String {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let deploy = sign(&chain, Tx::Deploy { code: Code::Lua(CALLS.to_string()) });
    let deployed = advance(&mut chain, vec![deploy]).remove(0);
    let Ok(Effect::Deployed(contract)) = deployed.result else { panic!("{:?}", deployed.result) };
    let mut lines = vec![format!("deploy: fuel {}", deployed.fuel_used)];
    let calls: &[(&str, Vec<TxCallArgumentType>)] = &[
        ("power", vec![number(3.0), number(33.0)]),
        ("power", vec![number(2.0), number(53.0)]),
        ("power", vec![number(2.0), number(54.0)]),
        ("backtrack", vec![number(60.0)]),
        ("backtrack", vec![number(400.0)]),
        ("sorted", vec![number(5_000.0)]),
        ("grown", vec![number(12.0)]),
        ("grown", vec![number(40.0)]),
        ("held", vec![number(1_000.0)]),
        ("held", vec![number(10_000_000.0)]),
        ("churn", vec![number(50_000.0)]),
        ("integers", vec![]),
        ("texts", vec![number(500.0)]),
        ("forever", vec![]),
        ("caught", vec![]),
        ("stored", vec![number(64.0)]),
    ];

    for (function, args) in calls {
        let tx = sign(&chain, Tx::ContractCall { to: contract, func: Hash::Xxh3_64(crate::vm::selector(function)), amount: Amount::U128(0), data: args.clone() });
        let receipt = advance(&mut chain, vec![tx]).remove(0);
        let ended = match &receipt.result {
            Ok(effect) => format!("ok {effect:?}"),
            Err(err) => format!("failed {err}"),
        };
        let args: Vec<String> = args.iter().map(|arg| format!("{arg:?}")).collect();

        lines.push(format!("{function}({}): fuel {}, {ended}", args.join(", "), receipt.fuel_used));
    }

    let tip = chain.tip();
    lines.push(format!("receipts root {}", tip.receipts_root));
    lines.push(format!("state root {}", tip.state_root));

    lines.join("\n") + "\n"
}

#[test]
fn the_fixture_comes_out_byte_for_byte_as_written() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/chain/tests/fixture.txt");
    let printed = printout();

    if std::env::var_os("CHAINV4_WRITE_FIXTURE").is_some() {
        std::fs::write(&path, &printed).unwrap();
        return;
    }

    let written = std::fs::read_to_string(&path).unwrap_or_default();

    if printed != written {
        let differ: Vec<String> = printed
            .lines()
            .zip(written.lines().chain(std::iter::repeat("")))
            .filter(|(here, there)| here != there)
            .map(|(here, there)| format!("  here:    {here}\n  written: {there}"))
            .collect();

        panic!(
            "this machine's fuel, memory or roots differ from fixture.txt ({} on {}), which would split the chain:\n{}\n\
             After a consensus change meant to move it: CHAINV4_WRITE_FIXTURE=1 cargo test --lib chain::tests::fixture",
            std::env::consts::OS,
            std::env::consts::ARCH,
            differ.join("\n")
        );
    }
}
