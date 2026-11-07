//! A Lua contract's own unit tests, kept out of what is deployed. A contract may carry test
//! functions beside its code, each named `test_` and something, defined at the top of the file as
//! `function M.test_x(...)`, `function test_x(...)` or `local function test_x(...)`, which
//! `chainv4 test` runs. Whatever deploys a Lua contract, the CLI, the wallet and the test runner
//! itself, takes them out first (strip_tests), so the code on chain never holds them and pays
//! nothing for them. Each is replaced by as many empty lines as it took, so an error's line in
//! what is deployed is its line in the file as written.

use r#macro::document;

/// One token of Lua as the stripper reads it: enough to find each test function's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token<'a> {
    Word(&'a str),
    /// A `.` or `:` between names, as in `M.test_x`.
    Dot,
    Other,
}

/// The tokens of a source, with where each starts and ends, skipping strings and comments.
fn tokens(source: &str) -> Vec<(Token<'_>, usize, usize)> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;

    // A long bracket's level at a place, [[ or [==[, if one opens there.
    let long = |at: usize| -> Option<usize> {
        if bytes.get(at) != Some(&b'[') {
            return None;
        }

        let level = bytes[at + 1..].iter().take_while(|byte| **byte == b'=').count();

        (bytes.get(at + 1 + level) == Some(&b'[')).then_some(level)
    };
    // Where a long bracket of a level closes, past its ]==].
    let close = |from: usize, level: usize| -> usize {
        let end = format!("]{}]", "=".repeat(level));

        source[from..].find(&end).map_or(source.len(), |found| from + found + end.len())
    };

    while at < bytes.len() {
        let byte = bytes[at];

        if byte.is_ascii_whitespace() {
            at += 1;
        } else if source[at..].starts_with("--") {
            at = match long(at + 2) {
                Some(level) => close(at + 2, level),
                None => source[at..].find('\n').map_or(source.len(), |end| at + end),
            };
        } else if byte == b'"' || byte == b'\'' {
            let start = at;
            at += 1;

            while at < bytes.len() && bytes[at] != byte && bytes[at] != b'\n' {
                at += if bytes[at] == b'\\' { 2 } else { 1 };
            }

            at = (at + 1).min(bytes.len());
            found.push((Token::Other, start, at));
        } else if let Some(level) = long(at) {
            let start = at;
            at = close(at, level);
            found.push((Token::Other, start, at));
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = at;

            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }

            found.push((Token::Word(&source[start..at]), start, at));
        } else if byte.is_ascii_digit() {
            let start = at;

            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_' || bytes[at] == b'.') {
                at += 1;
            }

            found.push((Token::Other, start, at));
        } else {
            let token = match byte {
                b'.' if bytes.get(at + 1) != Some(&b'.') => Token::Dot,
                b':' if bytes.get(at + 1) != Some(&b':') => Token::Dot,
                _ => Token::Other,
            };
            found.push((token, at, at + 1));
            at += 1;
        }
    }

    found
}

/// Whether a word opens a block that an `end` closes, or `repeat`, which `until` closes.
fn opens(word: &str) -> bool {
    matches!(word, "function" | "if" | "do" | "repeat")
}

fn closes(word: &str) -> bool {
    matches!(word, "end" | "until")
}

#[document(
    summary = "The test functions a Lua contract defines at its top level, by name, in order: `function M.test_x`, `function test_x` and `local function test_x`.",
    category = "VM",
    see_also = "strip_tests",
)]
pub fn test_names(source: &str) -> Vec<String> {
    tests_in(source).into_iter().map(|(name, ..)| name.to_string()).collect()
}

/// Each top-level test function: its name, and the bytes it takes, from `local` or `function` to
/// its `end`.
fn tests_in(source: &str) -> Vec<(&str, usize, usize)> {
    let tokens = tokens(source);
    let mut found = Vec::new();
    let mut depth = 0usize;
    let mut at = 0;

    while at < tokens.len() {
        let (token, start, _) = tokens[at];
        let Token::Word(word) = token else {
            at += 1;
            continue;
        };

        // At the top: [local] function [Table.|Table:]test_name
        if depth == 0 && (word == "function" || (word == "local" && matches!(tokens.get(at + 1), Some((Token::Word("function"), ..))))) {
            let name_at = if word == "local" { at + 2 } else { at + 1 };
            let name = match (tokens.get(name_at), tokens.get(name_at + 1), tokens.get(name_at + 2)) {
                (Some((Token::Word(_), ..)), Some((Token::Dot, ..)), Some((Token::Word(name), ..))) => Some(*name),
                (Some((Token::Word(name), ..)), ..) => Some(*name),
                _ => None,
            };

            if let Some(name) = name.filter(|name| name.starts_with("test_")) {
                // To the end that closes this function.
                let mut inner = 0usize;
                let mut end = source.len();

                for (token, _, to) in &tokens[at..] {
                    if let Token::Word(word) = token {
                        if opens(word) {
                            inner += 1;
                        } else if closes(word) {
                            inner = inner.saturating_sub(1);

                            if inner == 0 {
                                end = *to;
                                break;
                            }
                        }
                    }
                }

                found.push((name, start, end));
                at = tokens.iter().position(|(_, from, _)| *from >= end).unwrap_or(tokens.len());
                continue;
            }
        }

        if opens(word) {
            depth += 1;
        } else if closes(word) {
            depth = depth.saturating_sub(1);
        }

        at += 1;
    }

    found
}

#[document(
    summary = "A Lua contract without its own test functions, as it is deployed: each top-level `test_` function taken out, replaced by as many empty lines as it took, so every other line stays where it was.",
    category = "VM",
    returns = "The source to deploy: the same text when it holds no tests.",
    see_also = "test_names",
)]
pub fn strip_tests(source: &str) -> String {
    let tests = tests_in(source);

    if tests.is_empty() {
        return source.to_string();
    }

    let mut stripped = String::with_capacity(source.len());
    let mut from = 0;

    for (_, start, end) in tests {
        stripped.push_str(&source[from..start]);
        stripped.extend(source[start..end].chars().filter(|letter| *letter == '\n'));
        from = end;
    }

    stripped.push_str(&source[from..]);

    stripped
}

#[cfg(test)]
mod tests;
