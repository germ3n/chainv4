use super::*;

const CONTRACT: &str = r#"local M = {}

-- A test_ in a comment, and in a string, is no test: "function M.test_fake() end"
local text = [[ function test_long() end ]]

function M.count()
    if storage.get("n") then
        return 1
    end

    return 0
end

function M.test_count()
    for i = 1, 2 do
        t.call(t.self, "count")
    end

    repeat local x = 1 until true
    t.eq(t.call(t.self, "count"), 0)
end

local function test_helper()
    while false do end
end

function test_global() return "end" end

return M
"#;

#[test]
fn a_contracts_tests_are_found_and_taken_out_line_for_line() {
    assert_eq!(test_names(CONTRACT), ["test_count", "test_helper", "test_global"]);

    let stripped = strip_tests(CONTRACT);

    assert_eq!(stripped.lines().count(), CONTRACT.lines().count(), "every line where it was");
    assert!(!stripped.contains("t.call") && !stripped.contains("test_helper") && !stripped.contains("test_global"));
    assert!(stripped.contains("function M.count()") && stripped.contains("return M"));
    assert!(stripped.contains("test_fake") && stripped.contains("test_long"), "comments and strings stay as they are");
    assert_eq!(stripped.lines().position(|line| line == "return M"), CONTRACT.lines().position(|line| line == "return M"));
    assert_eq!(strip_tests("local M = {}\nreturn M\n"), "local M = {}\nreturn M\n", "nothing to take out");
    assert!(test_names("local function inner() function M.test_nested() end end").is_empty(), "only the top level's");
}
