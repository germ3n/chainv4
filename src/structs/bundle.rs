//! Lua contracts split across files. A contract goes on chain as one source, which every node
//! compiles, so no node ever reads a file of its own: whoever deploys puts the files together
//! first. A line `--@include "path"` is replaced by that file's text, as C's `#include` is, the
//! path relative to the file that includes it. The text shares the includer's scope, so its locals
//! are the includer's from there on.
//!
//! Each file goes in once: a second include of it is left out, and a file that includes itself,
//! through any number of others, is refused.

use {r#macro::document, std::fmt};

/// The annotation that includes a file.
const DIRECTIVE: &str = "--@include";

#[document(
    summary = "A Lua contract put together from its files: the one source a deploy carries, and where each of its lines came from.",
    category = "VM",
    see_also = "bundle",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    #[document(summary = "The source, every include replaced by the file's text between two comment lines that name it. A contract of one file is its source as it was.")]
    pub source: String,
    #[document(summary = "The files it was put together from, in the order they went in: the entry first, then each by its path relative to the entry's folder.")]
    pub files: Vec<String>,
    /// For each line of source, its file's place among files and its line there, from 1; None
    /// for the comment lines the bundle adds.
    lines: Vec<Option<(usize, usize)>>,
}

#[document(
    summary = "Why files did not go together: an include that does not read, names a file that cannot be loaded, or leads back to a file already being included.",
    category = "VM",
    see_also = "bundle",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleError {
    #[document(summary = "The file with the include, by its path relative to the entry's folder.")]
    pub file: String,
    #[document(summary = "The include's line in it, from 1.")]
    pub line: usize,
    #[document(summary = "What is wrong.")]
    pub message: String,
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.message)
    }
}

impl std::error::Error for BundleError {}

#[document(
    summary = "Puts a Lua contract split across files together: every --@include \"path\" line is replaced by that file's text, each file at most once.",
    category = "VM",
    params = {
        entry = "The entry file's name, such as main.lua. Paths it includes are relative to its folder.",
        source = "The entry file's text.",
        load = "Reads a file by its path relative to the entry's folder, with / between folders, such as lib/math.lua or ../shared/owner.lua.",
    },
    returns = "The bundle, or why the files do not go together.",
    example = "let bundle = bundle(\"main.lua\", &source, |path| std::fs::read_to_string(folder.join(path)).map_err(|err| err.to_string()))?;\nlet code = Code::Lua(bundle.source);",
    see_also = "Bundle, Code",
)]
pub fn bundle(entry: &str, source: &str, load: impl FnMut(&str) -> Result<String, String>) -> Result<Bundle, BundleError> {
    let entry = entry.rsplit(['/', '\\']).next().unwrap_or(entry).to_string();
    let mut builder = Builder {
        load,
        files: vec![entry],
        stack: Vec::new(),
        source: String::new(),
        lines: Vec::new(),
    };
    builder.add(0, source)?;

    // Nothing to put together: the source stays exactly as written.
    if builder.files.len() == 1 {
        let lines = (1..=source.lines().count()).map(|line| Some((0, line))).collect();

        return Ok(Bundle {
            source: source.to_string(),
            files: builder.files,
            lines,
        });
    }

    Ok(Bundle {
        source: builder.source,
        files: builder.files,
        lines: builder.lines,
    })
}

impl Bundle {
    #[document(
        parent = "Bundle",
        summary = "Where a line of the bundle came from: its file and its line there.",
        params = { line = "The bundle's line, from 1, as an error message names it." },
        returns = "The file and its line, from 1, or None for a line the bundle added, or past the end.",
    )]
    pub fn locate(&self, line: usize) -> Option<(&str, usize)> {
        let (file, at) = (*self.lines.get(line.checked_sub(1)?)?)?;

        Some((&self.files[file], at))
    }

    #[document(
        parent = "Bundle",
        summary = "An error message about the bundle, its line put as the file and line it came from: syntax error: 4: … becomes syntax error: lib/math.lua:2: …, and line 4: … becomes lib/math.lua:2: ….",
        params = { message = "A compile error, an annotation's, or a runtime error naming contract:LINE." },
        returns = "The message, with its first line number placed, or as it was when it names none.",
    )]
    pub fn explain(&self, message: &str) -> String {
        for (marker, keep) in [("syntax error: ", true), ("contract:", false), ("line ", false)] {
            let Some(at) = message.find(marker) else {
                continue;
            };
            let start = at + marker.len();
            let digits = message[start..].chars().take_while(char::is_ascii_digit).count();

            if digits == 0 || !message[start + digits..].starts_with(':') {
                continue;
            }

            let Some((file, line)) = message[start..start + digits].parse().ok().and_then(|line| self.locate(line)) else {
                continue;
            };
            let head = if keep { &message[..start] } else { &message[..at] };

            return format!("{head}{file}:{line}{}", &message[start + digits..]);
        }

        message.to_string()
    }
}

struct Builder<L> {
    load: L,
    files: Vec<String>,
    /// The files being included, outermost first: one of them again is a cycle.
    stack: Vec<usize>,
    source: String,
    lines: Vec<Option<(usize, usize)>>,
}

impl<L: FnMut(&str) -> Result<String, String>> Builder<L> {
    /// Adds the text of the file at this place among files, and what it includes.
    fn add(&mut self, file: usize, text: &str) -> Result<(), BundleError> {
        let name = self.files[file].clone();
        let folder = folder(&name);
        self.stack.push(file);

        for (index, line) in text.lines().enumerate() {
            let fail = |message: String| BundleError {
                file: name.clone(),
                line: index + 1,
                message,
            };
            let Some(path) = directive(line).map_err(fail)? else {
                self.push(line, Some((file, index + 1)));

                continue;
            };
            let path = resolve(folder, path).map_err(fail)?;
            let indent = &line[..line.len() - line.trim_start().len()];

            match self.files.iter().position(|known| *known == path) {
                Some(known) if self.stack.contains(&known) => return Err(fail(format!("{path} is already being included: it includes itself"))),
                Some(_) => self.push(&format!("{indent}-- {path} is included above"), None),
                None => {
                    let included = (self.load)(&path).map_err(|why| fail(format!("cannot include {path}: {why}")))?;
                    self.files.push(path.clone());
                    self.push(&format!("{indent}-- included {path}"), None);
                    self.add(self.files.len() - 1, &included)?;
                    self.push(&format!("{indent}-- end of {path}"), None);
                }
            }
        }

        self.stack.pop();

        Ok(())
    }

    fn push(&mut self, line: &str, from: Option<(usize, usize)>) {
        self.source.push_str(line);
        self.source.push('\n');
        self.lines.push(from);
    }
}

/// The path an include line names, or None for any other line.
fn directive(line: &str) -> Result<Option<&str>, String> {
    let Some(rest) = line.trim_start().strip_prefix(DIRECTIVE) else {
        return Ok(None);
    };

    // --@includes, --@include_x: some other annotation, for the node to judge.
    if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
        return Ok(None);
    }

    let rest = rest.trim();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\'')).ok_or(format!("{DIRECTIVE} takes a path in quotes, such as {DIRECTIVE} \"lib.lua\""))?;
    let (path, after) = rest[1..].split_once(quote).ok_or(format!("{DIRECTIVE}'s path has no closing quote"))?;

    if !after.trim().is_empty() {
        return Err(format!("nothing may follow {DIRECTIVE}'s path on its line"));
    }

    Ok(Some(path))
}

/// A path relative to a folder, as one relative to the entry's: `.` and `..` worked out, and `/`
/// between folders whichever way it was written.
fn resolve(folder: &str, path: &str) -> Result<String, String> {
    let path = path.replace('\\', "/");

    if path.is_empty() || path.starts_with('/') || path.contains(':') {
        return Err(format!("{DIRECTIVE} takes a path relative to the file, such as \"lib/math.lua\", not {path:?}"));
    }

    let mut parts: Vec<&str> = folder.split('/').filter(|part| !part.is_empty()).collect();

    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|last| *last != "..") => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }

    Ok(parts.join("/"))
}

/// The folder a path is in, empty for the entry's own.
fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

#[cfg(test)]
mod tests {
    use {super::*, std::collections::HashMap};

    /// Bundles main.lua from these files.
    fn put_together(files: &[(&str, &str)]) -> Result<Bundle, BundleError> {
        let files: HashMap<_, _> = files.iter().copied().collect();

        bundle("main.lua", files["main.lua"], |path| files.get(path).map(|text| text.to_string()).ok_or_else(|| "no such file".to_string()))
    }

    #[test]
    fn an_include_is_replaced_by_the_file_and_shares_its_scope() {
        let bundle = put_together(&[
            ("main.lua", "local M = {}\n--@include \"balances.lua\"\nfunction M.f() return move() end\nreturn M\n"),
            ("balances.lua", "local function move()\n    return 1\nend"),
        ])
        .unwrap();

        assert_eq!(
            bundle.source,
            "local M = {}\n-- included balances.lua\nlocal function move()\n    return 1\nend\n-- end of balances.lua\nfunction M.f() return move() end\nreturn M\n"
        );
        assert_eq!(bundle.files, ["main.lua", "balances.lua"]);
        assert_eq!(bundle.locate(1), Some(("main.lua", 1)));
        assert_eq!(bundle.locate(2), None, "the bundle's own comment");
        assert_eq!(bundle.locate(4), Some(("balances.lua", 2)));
        assert_eq!(bundle.locate(7), Some(("main.lua", 3)));
        assert_eq!(bundle.locate(99), None);
    }

    #[test]
    fn paths_are_relative_to_the_includer_and_each_file_goes_in_once() {
        let bundle = put_together(&[
            ("main.lua", "--@include 'lib/a.lua'\n  --@include \"./lib/b.lua\"  \n--@include \"lib/../lib/a.lua\""),
            ("lib/a.lua", "-- a\n--@include \"b.lua\""),
            ("lib/b.lua", "-- b"),
        ])
        .unwrap();

        assert_eq!(bundle.files, ["main.lua", "lib/a.lua", "lib/b.lua"]);
        assert_eq!(
            bundle.source,
            "-- included lib/a.lua\n-- a\n-- included lib/b.lua\n-- b\n-- end of lib/b.lua\n-- end of lib/a.lua\n  -- lib/b.lua is included above\n-- lib/a.lua is included above\n"
        );
    }

    #[test]
    fn a_contract_of_one_file_stays_as_written() {
        let source = "local M = {}\r\nreturn M";
        let bundle = put_together(&[("main.lua", source)]).unwrap();

        assert_eq!(bundle.source, source);
        assert_eq!(bundle.locate(2), Some(("main.lua", 2)));
    }

    #[test]
    fn cycles_missing_files_and_bad_lines_say_where() {
        let cycle = put_together(&[("main.lua", "--@include \"a.lua\""), ("a.lua", "\n--@include \"main.lua\"")]).unwrap_err();
        let missing = put_together(&[("main.lua", "local x\n--@include \"gone.lua\"")]).unwrap_err();

        assert_eq!(cycle.to_string(), "a.lua:2: main.lua is already being included: it includes itself");
        assert_eq!(missing.to_string(), "main.lua:2: cannot include gone.lua: no such file");

        for (line, why) in [
            ("--@include lib.lua", "in quotes"),
            ("--@include \"lib.lua", "no closing quote"),
            ("--@include \"lib.lua\" -- more", "nothing may follow"),
            ("--@include \"/etc/lib.lua\"", "relative to the file"),
            ("--@include \"C:/lib.lua\"", "relative to the file"),
        ] {
            let error = put_together(&[("main.lua", line)]).unwrap_err();

            assert!(error.message.contains(why), "{line}: {error}");
        }

        assert_eq!(put_together(&[("main.lua", "--@includes \"x\"")]).unwrap().source, "--@includes \"x\"", "another annotation, for the node to judge");
    }

    #[test]
    fn errors_about_the_bundle_name_the_file_and_line() {
        let bundle = put_together(&[("main.lua", "local M = {}\n--@include \"lib.lua\"\nreturn M"), ("lib.lua", "local a = 1\nlocal b =")]).unwrap();

        assert_eq!(bundle.explain("syntax error: 4: Expected identifier"), "syntax error: lib.lua:2: Expected identifier");
        assert_eq!(bundle.explain("line 6: unknown annotation --@x"), "main.lua:3: unknown annotation --@x");
        assert_eq!(bundle.explain("runtime error: contract:3: attempt to index nil"), "runtime error: lib.lua:1: attempt to index nil");
        assert_eq!(bundle.explain("out of fuel"), "out of fuel");
        assert_eq!(bundle.explain("syntax error: 2: in a comment line"), "syntax error: 2: in a comment line", "a line the bundle added");
    }

    #[test]
    fn an_entry_given_with_its_folder_is_named_by_its_file() {
        let bundle = bundle("contracts/main.lua", "--@include \"lib.lua\"", |path| Ok(format!("-- {path}"))).unwrap();

        assert_eq!(bundle.files, ["main.lua", "lib.lua"]);
    }
}
