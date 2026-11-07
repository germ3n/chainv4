use super::*;

/// A counter with its own tests beside its code.
const COUNTER: &str = r#"local M = {}

function M.init()
    storage.set("count", "0")
end

function M.add(by)
    local count = tonumber(storage.get("count")) + by
    storage.set("count", tostring(count))
    events.emit("Added", ctx.sender, by)

    return count
end

function M.owner_only()
    if storage.get("owner") == nil then
        error("only the owner")
    end
end

function M.paid()
    return ctx.value
end

function M.now()
    return ctx.time
end

function M.test_adds_and_emits()
    t.eq(t.call(t.self, "add", 2), 2)
    t.eq(t.call(t.self, "add", 3), 5, "counts on")
    local events = t.events()
    t.eq(#events, 1)
    t.eq(events[1].name, "Added")
    t.eq(events[1].data[1], t.account(1))
    t.ok(t.fuel() > 0, "a call burns fuel")
    t.eq(t.storage(t.self, "count"), "5")
end

function M.test_a_failure_is_an_error_the_test_sees()
    t.expect_error(function() t.call(t.self, "owner_only") end, "only the owner")
end

function M.test_accounts_value_time_and_height()
    local before = t.balance(t.account(2))
    t.as(2, function()
        t.pay(7)
        t.eq(t.call(t.self, "paid"), u256(7))
    end)
    t.ok(t.balance(t.account(2)) < before, "account 2 paid")
    t.eq(t.sender(), 1, "back to account 1")
    local height = t.height()
    t.roll(3)
    t.eq(t.height(), height + 3)
    t.warp(60000)
    t.ok(t.call(t.self, "now") >= t.time() - 1000)
end

function M.test_fuzz_adds_whatever_it_is_given()
    t.fuzz(20, function(gen)
        local by = gen.int(1, 100)
        local before = t.call(t.self, "add", 0)
        t.eq(t.call(t.self, "add", by), before + by)
    end)
end

function M.test_fails()
    t.eq(t.call(t.self, "add", 1), 2, "on purpose")
end

return M
"#;

/// A test file of its own, deploying the counter from beside it.
const COUNTER_TEST: &str = r#"
function test_deploys_what_it_tests()
    local counter = t.deploy("counter.lua")
    t.eq(t.call(counter, "add", 4), 4)
    t.transfer(t.account(3), 10)
end
"#;

fn folder() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("chainv4-test-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::write(dir.join("counter.lua"), COUNTER).unwrap();
    std::fs::write(dir.join("counter_test.lua"), COUNTER_TEST).unwrap();
    std::fs::write(dir.join("tests/again.lua"), COUNTER_TEST.replace("\"counter.lua\"", "\"../counter.lua\"")).unwrap();

    dir
}

#[test]
fn a_contracts_own_tests_run_on_a_chain_of_their_own_and_a_failure_says_why() {
    let dir = folder();
    let counter = dir.join("counter.lua");
    let (kind, names) = tests_of(&counter).unwrap().unwrap();

    assert_eq!(kind, Kind::Contract);
    assert_eq!(names.len(), 5);

    for name in &names[..4] {
        assert_eq!(run_one(&counter, kind, name, 1), Ok(()), "{name}");
    }

    let failed = run_one(&counter, kind, "test_fails", 1).unwrap_err();
    assert!(failed.contains("on purpose: expected 2, got 1") && failed.contains("counter.lua:"), "{failed}");

    // What deploys goes without them, and the file found three ways.
    let deployed = contract_source(&counter).unwrap();
    assert!(!deployed.contains("test_") && deployed.lines().count() == COUNTER.lines().count());
    assert_eq!(files(std::slice::from_ref(&dir)).unwrap().len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_test_file_deploys_what_it_tests_beside_it_or_from_a_tests_folder() {
    let dir = folder();

    for path in [dir.join("counter_test.lua"), dir.join("tests/again.lua")] {
        let (kind, names) = tests_of(&path).unwrap().unwrap();

        assert_eq!((kind, names.as_slice()), (Kind::Tests, ["test_deploys_what_it_tests".to_string()].as_slice()));
        assert_eq!(run_one(&path, kind, &names[0], 1), Ok(()), "{}", path.display());
    }

    // The whole run: every test but the one failing on purpose passes, and it says which failed.
    let result = test(TestArgs { paths: vec![dir.clone()], filter: None, seed: 1 });
    assert!(result.unwrap_err().to_string().contains("1 failed"));
    assert!(test(TestArgs { paths: vec![dir.clone()], filter: Some("adds".to_string()), seed: 1 }).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(all(feature = "evm", feature = "client"))]
#[test]
fn a_lua_test_deploys_and_calls_an_evm_contract_too() {
    let dir = folder();
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/evm/Counter.bin"), dir.join("Counter.bin")).unwrap();
    std::fs::write(
        dir.join("evm_test.lua"),
        r#"
function test_counts_on_the_evm()
    local counter = t.deploy_evm("Counter.bin", "(uint256)", 5)
    t.evm_call(counter, "bump(uint256)", 2)
    t.eq(t.evm_call(counter, "count()"), "0x0000000000000000000000000000000000000000000000000000000000000007")
    t.ok(t.fuel() >= 0)
end
"#,
    )
    .unwrap();

    let path = dir.join("evm_test.lua");
    assert_eq!(run_one(&path, Kind::Tests, "test_counts_on_the_evm", 1), Ok(()));
    let _ = std::fs::remove_dir_all(&dir);
}
