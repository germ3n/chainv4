//! Annotations: comment lines starting `--@` that say something about the function defined on
//! the next line, in the style of Luau's `--!strict`. The source stays plain Luau, and every node
//! reads the annotations the same way when the contract is deployed.

use {
    crate::vm::VmError,
    r#macro::document,
};

const PREFIX: &str = "--@";

#[document(
    summary = "The functions a script marks with a --@reentrant line right above their definition. Only those may be entered while their contract is already running further up the chain of calls.",
    category = "Lua VM",
    params = {
        source = "The Luau source.",
        exported = "The functions the script exports. Only those can be marked.",
    },
    returns = "The marked names, sorted, or a Compile error for an unknown annotation, one that is not right above `function M.name(`, or one on a function the script does not export.",
    example = "--@reentrant\nfunction M.on_payment(amount)\n    -- may run while this contract is mid-call, for example in a callback\nend",
    see_also = "Host::call",
)]
pub fn reentrant(source: &str, exported: &[String]) -> Result<Vec<String>, VmError> {
    let mut marked = Vec::new();
    let mut lines = source.lines().enumerate();

    while let Some((index, line)) = lines.next() {
        let Some(annotation) = line.trim().strip_prefix(PREFIX) else {
            continue;
        };

        if annotation.trim() != "reentrant" {
            return Err(error(index, format!("unknown annotation {PREFIX}{}", annotation.trim())));
        }

        let next = lines.next().map_or("", |(_, line)| line.trim());
        let Some(name) = defined_function(next) else {
            return Err(error(index, format!("{PREFIX}reentrant must sit right above `function M.name(`")));
        };

        if !exported.contains(&name) {
            return Err(error(index, format!("{PREFIX}reentrant on {name}, which the script does not export")));
        }

        marked.push(name);
    }

    marked.sort();
    marked.dedup();

    Ok(marked)
}

/// The name in `function M.name(` or `function M:name(`.
fn defined_function(line: &str) -> Option<String> {
    let head = line.strip_prefix("function ")?.split('(').next()?.trim();
    let (_, name) = head.split_once(['.', ':'])?;
    let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');

    valid.then(|| name.to_string())
}

fn error(index: usize, message: String) -> VmError {
    VmError::Compile(format!("line {}: {message}", index + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exported() -> Vec<String> {
        ["deposit", "on_payment", "withdraw"].map(String::from).to_vec()
    }

    #[test]
    fn marks_the_function_below() {
        let source = r#"
local M = {}

--@reentrant
function M.on_payment(amount) end

  --@ reentrant
function M:deposit() end

---@param amount number
function M.withdraw(amount) end

return M
"#;

        assert_eq!(reentrant(source, &exported()).unwrap(), ["deposit", "on_payment"]);
    }

    #[test]
    fn misplaced_unknown_or_unexported_marks_are_refused() {
        let cases = [
            ("--@reentrent\nfunction M.deposit() end", "unknown annotation"),
            ("--@reentrant\n\nfunction M.deposit() end", "right above"),
            ("--@reentrant\nlocal function deposit() end", "right above"),
            ("--@reentrant\nfunction M.hidden() end", "does not export"),
            ("--@reentrant", "right above"),
        ];

        for (source, expected) in cases {
            let err = reentrant(source, &exported()).unwrap_err();

            assert!(matches!(&err, VmError::Compile(text) if text.contains(expected)), "{source}: {err:?}");
        }
    }
}
