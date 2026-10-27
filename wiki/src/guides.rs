//! The guides: docs/*.md and the README, rendered for the wiki beside the reference. Each is a
//! page of its own kind, with its headings for the sidebar and its text for search. Links between
//! them become the wiki's own addresses, a link into another file of the repository becomes its
//! path, and a link to a page or heading that is not there fails the build, as a broken
//! `#[document]` does.

use {
    pulldown_cmark::{CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd, html},
    serde::Serialize,
    std::collections::{HashMap, HashSet},
    std::fs,
    std::path::{Component, Path, PathBuf},
};

/// Each guide's section in the sidebar, in order. A guide not listed goes under More, after them.
const SECTIONS: &[(&str, &[&str])] = &[
    ("Start here", &["readme", "good-to-know", "architecture", "development"]),
    ("The chain", &["chain", "genesis", "consensus", "finality", "fees", "crypto", "storage"]),
    ("Contracts", &["contracts", "contract-storage", "token-standard"]),
    ("Running a node", &["node", "api", "light-client", "docker", "faucet"]),
    ("Apps", &["explorer", "wallet"]),
    ("Project", &["todo"]),
];

/// The id of docs/README.md, which is the guides' home rather than a page in the sidebar.
pub const HOME: &str = "index";

#[derive(Debug, Serialize)]
pub struct Guide {
    pub id: String,
    pub lang: &'static str,
    pub kind: &'static str,
    pub parent: &'static str,
    pub name: String,
    pub category: String,
    /// Where it sits in the sidebar, across every section.
    pub order: usize,
    pub summary: String,
    pub html: String,
    pub headings: Vec<Heading>,
    /// Its text alone, for search.
    pub text: String,
    // Empty, so the reference's code reads a guide as it reads any page.
    pub params: [(); 0],
    pub returns: [(); 0],
    pub see_also: [(); 0],
    pub access: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Heading {
    pub level: u8,
    pub id: String,
    pub text: String,
}

/// One Markdown file, where it is in the repository, and what id it gets.
struct Source {
    id: String,
    /// Its path from the repository root, such as docs/fees.md.
    path: PathBuf,
    text: String,
}

/// Reads every guide: README.md at the repository root and each docs/*.md. None without a docs
/// directory, as for a wiki of only the reference.
pub fn read(root: &Path) -> Result<Vec<Guide>, String> {
    let docs = root.join("docs");

    if !docs.is_dir() {
        return Ok(Vec::new());
    }

    let mut sources = Vec::new();
    let readme = root.join("README.md");

    if readme.is_file() {
        sources.push(Source { id: "readme".to_string(), path: PathBuf::from("README.md"), text: load(&readme)? });
    }

    let mut names: Vec<String> = fs::read_dir(&docs)
        .map_err(|err| format!("failed to list {}: {err}", docs.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .filter(|name| name.ends_with(".md"))
        .collect();
    names.sort();

    for name in names {
        let stem = name.trim_end_matches(".md");
        let id = if stem == "README" { HOME.to_string() } else { stem.to_string() };
        sources.push(Source { id, path: Path::new("docs").join(&name), text: load(&docs.join(&name))? });
    }

    // Every guide's headings first, so a link can be checked against the page it points at.
    let anchors: HashMap<String, HashSet<String>> = sources
        .iter()
        .map(|source| (source.id.clone(), headings(&source.text).into_iter().map(|heading| heading.id).collect()))
        .collect();
    let by_path: HashMap<PathBuf, String> = sources.iter().map(|source| (source.path.clone(), source.id.clone())).collect();
    let mut errors = Vec::new();
    let guides: Vec<Guide> = sources
        .iter()
        .map(|source| render(source, &anchors, &by_path, &mut errors))
        .collect();

    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }

    Ok(guides)
}

fn load(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|err| format!("failed to read {}: {err}", path.display()))
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// The section and place a guide gets in the sidebar.
fn placed(id: &str) -> (String, usize) {
    let mut order = 0;

    for (section, ids) in SECTIONS {
        for listed in *ids {
            if *listed == id {
                return (section.to_string(), order);
            }

            order += 1;
        }
    }

    ("More".to_string(), order)
}

/// A heading's anchor as GitHub writes it, which the docs link by: lowercase, spaces to hyphens,
/// and nothing else but letters, digits, hyphens and underscores.
pub fn slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | ' '))
        .map(|ch| if ch == ' ' { '-' } else { ch })
        .collect()
}

/// The headings of a file, each with its anchor; a second heading of the same text gets -1, as
/// on GitHub.
fn headings(text: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut current: Option<(u8, String)> = None;

    for event in Parser::new_ext(text, options()) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => current = Some((level_number(level), String::new())),
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, heading)) = &mut current {
                    heading.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, text)) = current.take() {
                    let base = slug(&text);
                    let count = seen.entry(base.clone()).or_insert(0);
                    let id = if *count == 0 { base.clone() } else { format!("{base}-{count}") };
                    *count += 1;
                    out.push(Heading { level, id, text });
                }
            }
            _ => {}
        }
    }

    out
}

fn level_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Where a link in a guide goes, once read against the repository.
enum Target {
    /// Off this repository: left as it is.
    Outside,
    /// A guide, and a heading in it, as a wiki address.
    Guide(String),
    /// Another file of the repository: shown as its path, since the repository is not public.
    File(String),
}

fn target(dest: &str, from: &Source, anchors: &HashMap<String, HashSet<String>>, by_path: &HashMap<PathBuf, String>, errors: &mut Vec<String>) -> Target {
    if dest.contains("://") || dest.starts_with("mailto:") {
        return Target::Outside;
    }

    let (path, fragment) = match dest.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (dest, None),
    };
    let id = if path.is_empty() {
        from.id.clone()
    } else {
        let joined = normalize(&from.path.parent().unwrap_or(Path::new("")).join(path));

        match by_path.get(&joined) {
            Some(id) => id.clone(),
            None => return Target::File(joined.to_string_lossy().into_owned()),
        }
    };

    if let Some(fragment) = fragment
        && !anchors.get(&id).is_some_and(|ids| ids.contains(fragment))
    {
        errors.push(format!("{}: the link {dest} names a heading {fragment} that {} does not have", from.path.display(), if path.is_empty() { "this page" } else { path }));
    }

    Target::Guide(match fragment {
        Some(fragment) => format!("#guide/{id}/{fragment}"),
        None => format!("#guide/{id}"),
    })
}

/// A path with its . and .. worked out, from the repository root.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();

    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }

    out
}

fn render(source: &Source, anchors: &HashMap<String, HashSet<String>>, by_path: &HashMap<PathBuf, String>, errors: &mut Vec<String>) -> Guide {
    let found = headings(&source.text);
    let mut heading_ids = found.iter().map(|heading| heading.id.clone());
    let mut events = Vec::new();
    // Whether each open link is one into a file, shown as its path, whose end closes that instead.
    let mut links: Vec<bool> = Vec::new();
    let mut text = String::new();
    let mut summary = None;
    let mut paragraph: Option<String> = None;
    let mut title = None;
    let mut in_heading = false;
    let mut first_heading = String::new();

    for event in Parser::new_ext(&source.text, options()) {
        match &event {
            Event::Text(part) | Event::Code(part) => {
                text.push_str(part);

                if let Some(paragraph) = &mut paragraph {
                    paragraph.push_str(part);
                }

                if in_heading && title.is_none() {
                    first_heading.push_str(part);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                text.push(' ');

                if let Some(paragraph) = &mut paragraph {
                    paragraph.push(' ');
                }
            }
            Event::Start(Tag::Paragraph) if summary.is_none() => paragraph = Some(String::new()),
            Event::End(TagEnd::Paragraph) => {
                text.push('\n');

                if let Some(done) = paragraph.take() {
                    summary = Some(done);
                }
            }
            Event::End(TagEnd::Heading(_)) | Event::End(TagEnd::Item) | Event::End(TagEnd::TableRow) => text.push('\n'),
            _ => {}
        }

        let event = match event {
            Event::Start(Tag::Heading { level, classes, attrs, .. }) => {
                in_heading = true;

                Event::Start(Tag::Heading { level, id: heading_ids.next().map(CowStr::from), classes, attrs })
            }
            Event::End(TagEnd::Heading(level)) => {
                in_heading = false;

                if title.is_none() && !first_heading.is_empty() {
                    title = Some(std::mem::take(&mut first_heading));
                }

                Event::End(TagEnd::Heading(level))
            }
            Event::Start(Tag::Link { link_type, dest_url, title: link_title, id }) => {
                let target = target(&dest_url, source, anchors, by_path, errors);
                links.push(matches!(target, Target::File(_)));

                match target {
                    Target::Outside => Event::Start(Tag::Link { link_type, dest_url, title: link_title, id }),
                    Target::Guide(address) => Event::Start(Tag::Link { link_type, dest_url: address.into(), title: link_title, id }),
                    Target::File(path) => Event::Html(format!("<span class=\"repo-path\" title=\"{}\">", escape(&path)).into()),
                }
            }
            Event::End(TagEnd::Link) if links.pop() == Some(true) => Event::Html("</span>".into()),
            other => other,
        };

        events.push(event);
    }

    let mut html = String::new();
    html::push_html(&mut html, events.into_iter());
    let (category, order) = placed(&source.id);
    let summary = summary.unwrap_or_default();

    Guide {
        id: source.id.clone(),
        lang: "guide",
        kind: "guide",
        parent: "",
        // The README's title is the project's name; in the sidebar it is the overview.
        name: if source.id == "readme" { "Overview".to_string() } else { title.unwrap_or_else(|| source.id.clone()) },
        category,
        order,
        summary: shorten(&summary, 240),
        html,
        headings: found.into_iter().filter(|heading| matches!(heading.level, 2 | 3)).collect(),
        text,
        params: [],
        returns: [],
        see_also: [],
        access: "public",
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

/// The first sentences of a paragraph, at most this many characters, cut at a word.
fn shorten(text: &str, most: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if text.chars().count() <= most {
        return text;
    }

    let cut: String = text.chars().take(most).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head).trim_end_matches([',', ';', ':']);

    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with a README and these docs, in a directory of its own.
    fn repo(name: &str, docs: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("wiki-guides-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("README.md"), "# chainv4\n\nRuns a chain. See [fees](docs/fees.md#the-gas-limit).\n\n## Your own chain\n").unwrap();

        for (file, text) in docs {
            fs::write(root.join("docs").join(file), text).unwrap();
        }

        root
    }

    #[test]
    fn anchors_are_spelled_as_github_spells_them() {
        assert_eq!(slug("Storage layouts"), "storage-layouts");
        assert_eq!(slug("`POST /offer`"), "post-offer");
        assert_eq!(slug("Who may call it?"), "who-may-call-it");
        assert_eq!(slug("u64_kind and More"), "u64_kind-and-more");

        let found = headings("# A\n\n## Steps\n\n## Steps\n\n### `Tx::Deploy`\n");

        assert_eq!(found.iter().map(|heading| heading.id.as_str()).collect::<Vec<_>>(), ["a", "steps", "steps-1", "txdeploy"]);
    }

    #[test]
    fn links_become_wiki_addresses_and_files_their_paths() {
        let root = repo("links", &[
            ("README.md", "# chainv4 docs\n\nStart with [the overview](../README.md).\n"),
            ("fees.md", "# Fees\n\nIntro paragraph, the summary.\n\n## The gas limit\n\nSee [layouts](contracts.md#storage-layouts), [above](#the-gas-limit), [the code](../src/chain/fees/mod.rs) and [Rust](https://www.rust-lang.org).\n\n```lua\nlocal x = 1\n```\n"),
            ("contracts.md", "# Contracts\n\n## Storage layouts\n"),
        ]);
        let guides = read(&root).unwrap();
        let fees = guides.iter().find(|guide| guide.id == "fees").unwrap();

        assert!(fees.html.contains("href=\"#guide/contracts/storage-layouts\""), "{}", fees.html);
        assert!(fees.html.contains("href=\"#guide/fees/the-gas-limit\""));
        assert!(fees.html.contains("<span class=\"repo-path\" title=\"src/chain/fees/mod.rs\">the code</span>"), "a file of the repository is its path, never a link");
        assert!(fees.html.contains("href=\"https://www.rust-lang.org\""));
        assert!(fees.html.contains("<h2 id=\"the-gas-limit\">"));
        assert!(fees.html.contains("<code class=\"language-lua\">"));
        assert_eq!(fees.name, "Fees");
        assert_eq!(fees.summary, "Intro paragraph, the summary.");
        assert_eq!(fees.headings, [Heading { level: 2, id: "the-gas-limit".to_string(), text: "The gas limit".to_string() }]);
        assert!(fees.text.contains("The gas limit") && fees.text.contains("local x = 1"));

        let home = guides.iter().find(|guide| guide.id == HOME).unwrap();
        let readme = guides.iter().find(|guide| guide.id == "readme").unwrap();

        assert!(home.html.contains("href=\"#guide/readme\""), "docs/README.md is the home, and ../README.md the overview");
        assert_eq!(readme.name, "Overview");
        assert!(readme.html.contains("href=\"#guide/fees/the-gas-limit\""), "the README's links reach into docs/");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_link_to_a_heading_that_is_not_there_fails() {
        let root = repo("broken", &[
            ("fees.md", "# Fees\n\nSee [nothing](contracts.md#no-such-heading) and [here](#neither).\n"),
            ("contracts.md", "# Contracts\n"),
        ]);
        let err = read(&root).unwrap_err();

        assert!(err.contains("docs/fees.md: the link contracts.md#no-such-heading names a heading no-such-heading that contracts.md does not have"), "{err}");
        assert!(err.contains("docs/fees.md: the link #neither names a heading neither that this page does not have"), "{err}");
        assert!(err.contains("README.md: the link docs/fees.md#the-gas-limit names a heading the-gas-limit that docs/fees.md does not have"), "the README's too: {err}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn guides_take_their_place_and_new_ones_go_last() {
        let root = repo("order", &[("fees.md", "# Fees\n\n## The gas limit\n"), ("zebra.md", "# Zebra\n"), ("wallet.md", "# Wallet\n")]);
        let guides = read(&root).unwrap();
        let place = |id: &str| guides.iter().find(|guide| guide.id == id).map(|guide| (guide.category.clone(), guide.order)).unwrap();

        assert_eq!(place("readme"), ("Start here".to_string(), 0));
        assert!(place("fees").1 < place("wallet").1);
        assert_eq!(place("zebra").0, "More", "a guide nobody placed still shows");
        assert!(place("zebra").1 > place("wallet").1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_repositorys_own_docs_render_with_every_link_whole() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let guides = read(&root).unwrap();

        assert!(guides.iter().any(|guide| guide.id == HOME) && guides.iter().any(|guide| guide.id == "contracts"));
        assert!(guides.iter().all(|guide| !guide.html.is_empty() && !guide.name.is_empty()));
    }
}
