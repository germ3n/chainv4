//! Annotations: comment lines starting `--@` that say something about the script, in the style
//! of Luau's `--!strict`: about the function defined on the next line, about its storage layout, or,
//! with `--@const_params` at its top, that no function may assign its parameters. The source stays plain Luau, and every node reads the annotations the same way when
//! the contract is deployed.

use {
    crate::vm::{Entry, Layout, VmError},
    r#macro::document,
};

const PREFIX: &str = "--@";
/// The annotations that make up a storage layout, wherever they sit.
const LAYOUT_ENTRIES: &[&str] = &["namespace", "key", "retired"];

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
    // Whether a statement has come yet: --@const_params counts only before one, in the header,
    // which is where the Luau fork reads it.
    let mut header = Header::default();

    while let Some((index, line)) = lines.next() {
        let in_header = header.read(line);

        let Some(annotation) = line.trim().strip_prefix(PREFIX) else {
            continue;
        };

        let annotation = annotation.trim();

        if annotation != "reentrant" {
            match annotation.split_whitespace().next() {
                Some(word) if LAYOUT_ENTRIES.contains(&word) => continue,
                Some(CONST_PARAMS) if annotation == CONST_PARAMS && in_header => continue,
                Some(CONST_PARAMS) if annotation == CONST_PARAMS => {
                    return Err(error(index, format!("{PREFIX}{CONST_PARAMS} must come before the script's first statement, with the annotations at its top")));
                }
                Some("include") => {
                    return Err(error(
                        index,
                        format!("{PREFIX}include is for deploying: chainv4 send deploy and the explorer put the files together, so the source on chain never holds one"),
                    ));
                }
                _ => return Err(error(index, format!("unknown annotation {PREFIX}{annotation}"))),
            }
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

#[document(
    summary = "The storage layout a script declares with --@namespace, --@key and --@retired lines, anywhere in it: the namespaces it keeps its state in, maps of one kind or records of declared keys, and those it no longer uses.",
    category = "Lua VM",
    params = { source = "The Luau source." },
    returns = "The layout, empty for a script that declares none, or a Compile error for an entry Entry::parse refuses, or entries that do not fit together.",
    example = "--@namespace balances decimal\n--@namespace config\n--@key config owner address-hex\n--@retired fees\nlocal balances = storage.namespace(\"balances\")",
    see_also = "Layout",
)]
pub fn layout(source: &str) -> Result<Layout, VmError> {
    let mut entries = Vec::new();

    for (index, line) in source.lines().enumerate() {
        let Some(annotation) = line.trim().strip_prefix(PREFIX) else {
            continue;
        };

        if annotation.split_whitespace().next().is_some_and(|word| LAYOUT_ENTRIES.contains(&word)) {
            // A Lua comment may follow, as after any line: no word of an entry starts with --.
            let words: Vec<&str> = annotation.split_whitespace().take_while(|word| !word.starts_with("--")).collect();
            entries.push(Entry::parse(&words.join(" ")).map_err(|why| error(index, why))?);
        }
    }

    Layout::new(entries).map_err(VmError::Compile)
}

/// Every function parameter in the script const, as `const` before one's name makes it: a body that
/// assigns one fails to compile. The Luau fork reads it, in the header alone.
const CONST_PARAMS: &str = "const_params";

/// Where a script's header ends: at its first line that is neither blank nor a comment, a block
/// comment's lines included.
#[derive(Default)]
struct Header {
    over: bool,
    in_block: bool,
}

impl Header {
    /// Takes the next line; whether it is still in the header.
    fn read(&mut self, line: &str) -> bool {
        let line = line.trim();

        if self.in_block {
            self.in_block = !line.contains("]]") && !line.contains("]=");
        } else if line.starts_with("--[[") || line.starts_with("--[=") {
            self.in_block = !line[3..].contains("]]") && !line[3..].contains("]=");
        } else if !line.is_empty() && !line.starts_with("--") {
            self.over = true;
        }

        !self.over
    }
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
    fn layout_lines_declare_namespaces_wherever_they_sit() {
        let source = r#"
--@namespace balances u256
local M = {}

  --@ retired fees
--@reentrant
function M.on_payment(amount) end

--@key config owner address-hex   -- who may change it
--@namespace config
--@namespace points address-text -> decimal:points --keyed by who holds them
return M
"#;
        let layout = layout(source).unwrap();

        assert_eq!(
            layout.to_string(),
            "namespace balances u256\nnamespace config\nkey config owner address-hex\nretired fees\nnamespace points address-text -> decimal:points\n",
            "a comment after an entry is no part of it"
        );
        assert_eq!(reentrant(source, &exported()).unwrap(), ["on_payment"], "reentrant marks read past them");
        assert_eq!(super::layout("local M = {}").unwrap(), Layout::default());

        for (source, expected) in [
            ("--@namespace balances u2566", "line 1: there is no kind u2566"),
            ("--@retired", "line 1: a namespace is retired as"),
            ("--@namespace a u256\n--@retired a", "declared twice"),
            ("--@key a k u64", "not declared as a record"),
        ] {
            assert!(matches!(super::layout(source), Err(VmError::Compile(text)) if text.contains(expected)), "{source}");
        }
    }

    #[test]
    fn const_params_is_taken_in_the_header_alone() {
        let headed = "--@namespace balances u256\n--[[ a contract\n   whose parameters stay put ]]\n  --@ const_params\nlocal M = {}\nreturn M";
        assert_eq!(reentrant(headed, &exported()).unwrap(), Vec::<String>::new());
        assert!(layout(headed).is_ok(), "the layout reads past it");

        let late = "local M = {}\n--@const_params\nreturn M";
        let err = reentrant(late, &exported()).unwrap_err();
        assert!(matches!(&err, VmError::Compile(text) if text.contains("line 2") && text.contains("before the script's first statement")), "{err:?}");
        assert!(matches!(reentrant("--@const_params yes", &exported()), Err(VmError::Compile(text)) if text.contains("unknown annotation")), "it takes no words");
    }

    #[test]
    fn misplaced_unknown_or_unexported_marks_are_refused() {
        let cases = [
            ("--@reentrent\nfunction M.deposit() end", "unknown annotation"),
            ("--@reentrant\n\nfunction M.deposit() end", "right above"),
            ("--@reentrant\nlocal function deposit() end", "right above"),
            ("--@reentrant\nfunction M.hidden() end", "does not export"),
            ("--@reentrant", "right above"),
            ("--@include \"lib.lua\"", "is for deploying"),
        ];

        for (source, expected) in cases {
            let err = reentrant(source, &exported()).unwrap_err();

            assert!(matches!(&err, VmError::Compile(text) if text.contains(expected)), "{source}: {err:?}");
        }
    }
}
