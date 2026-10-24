//! Storage layouts: the namespaces a contract's code says it keeps its state in, and what each
//! holds. Code that declares any may use no other namespace, so the declaration is what the code
//! does, and a new version must keep every namespace the old one declared, with what it holds,
//! or retire it on purpose.

use {
    super::MAX_NAMESPACE_LEN,
    r#macro::document,
    std::fmt,
    wincode::{SchemaRead, SchemaWrite},
};

/// The longest kind, in bytes.
pub const MAX_KIND_LEN: usize = 64;

#[document(
    summary = "The namespaces a contract's code keeps its state in, as it declares them: --@namespace and --@retired lines in Lua, the chainv4.layout custom section in WebAssembly. Code that declares any may use no other namespace; plain storage and its own space stay free. A new version, swapped in by Tx::Upgrade or by a proxy repointing, must keep every namespace with the same kind, or retire it.",
    category = "VM",
    see_also = "Layout::keeps, Space, LAYOUT_SECTION",
    example = "--@namespace balances u256\n--@namespace allowances u256\n--@retired fees",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Layout {
    #[document(summary = "Every namespace declared, sorted by name. Empty for code that declares none, which may use any.")]
    pub namespaces: Vec<Declared>,
}

#[document(summary = "One namespace a layout declares.", category = "VM", see_also = "Layout")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Declared {
    #[document(summary = "The namespace's name: 1 to 64 bytes, without spaces.")]
    pub name: String,
    #[document(summary = "What it holds, as one word the code picks, such as u256 or counter-v1: 1 to 64 bytes. None once it is retired: then it may be read and cleared, never written.")]
    pub kind: Option<String>,
}

impl Declared {
    #[document(
        parent = "Declared",
        summary = "Reads one entry as code writes it: `namespace NAME KIND`, or `retired NAME`. Words are separated by spaces.",
        returns = "The namespace, or what is wrong with the entry.",
    )]
    pub fn parse(text: &str) -> Result<Declared, String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let (name, kind) = match words[..] {
            ["namespace", name, kind] => (name, Some(kind)),
            ["retired", name] => (name, None),
            ["namespace", ..] => return Err("a namespace is declared as `namespace NAME KIND`".to_string()),
            ["retired", ..] => return Err("a namespace is retired as `retired NAME`".to_string()),
            _ => return Err(format!("unknown layout entry `{}`", text.trim())),
        };

        if name.len() > MAX_NAMESPACE_LEN {
            return Err(format!("namespace names are 1 to {MAX_NAMESPACE_LEN} bytes"));
        }

        if kind.is_some_and(|kind| kind.len() > MAX_KIND_LEN) {
            return Err(format!("kinds are 1 to {MAX_KIND_LEN} bytes"));
        }

        Ok(Declared {
            name: name.to_string(),
            kind: kind.map(str::to_string),
        })
    }
}

impl Layout {
    #[document(
        parent = "Layout",
        summary = "A layout of these namespaces, sorted by name.",
        returns = "The layout, or an error naming a namespace declared twice.",
    )]
    pub fn new(mut namespaces: Vec<Declared>) -> Result<Layout, String> {
        namespaces.sort_by(|a, b| a.name.cmp(&b.name));

        if let Some(twice) = namespaces.windows(2).find(|pair| pair[0].name == pair[1].name) {
            return Err(format!("namespace {} is declared twice", twice[0].name));
        }

        Ok(Layout { namespaces })
    }

    #[document(parent = "Layout", summary = "Whether the code declares any namespace, and so may use no other.")]
    pub fn is_declared(&self) -> bool {
        !self.namespaces.is_empty()
    }

    #[document(parent = "Layout", summary = "The namespace by this name, if the layout declares it.")]
    pub fn get(&self, name: &[u8]) -> Option<&Declared> {
        self.namespaces
            .binary_search_by(|declared| declared.name.as_bytes().cmp(name))
            .ok()
            .map(|at| &self.namespaces[at])
    }

    #[document(
        parent = "Layout",
        summary = "Whether code with this layout may use a namespace: any, when it declares none; otherwise those it declares, and of those it retired, only to read or clear.",
        params = {
            name = "The namespace's name.",
            write = "Whether the code writes a value, rather than reading or deleting one.",
        },
        returns = "Why not, when it may not.",
    )]
    pub fn reaches(&self, name: &[u8], write: bool) -> Result<(), String> {
        if !self.is_declared() {
            return Ok(());
        }

        let shown = String::from_utf8_lossy(name);

        match self.get(name) {
            None => Err(format!("namespace {shown} is not in this code's layout")),
            Some(Declared { kind: None, .. }) if write => {
                Err(format!("namespace {shown} is retired: it can be read and cleared, not written"))
            }
            Some(_) => Ok(()),
        }
    }

    #[document(
        parent = "Layout",
        summary = "Whether this layout, a new version's, keeps old: every namespace old declares is still declared, with the same kind, or retired; one old retired stays retired. New namespaces may come in. Anything keeps a layout that declares nothing.",
        params = { old = "The layout of the code it replaces." },
        returns = "Why not, naming the first namespace that breaks it.",
        see_also = "Contracts::upgrade, Host::check_layout",
    )]
    pub fn keeps(&self, old: &Layout) -> Result<(), String> {
        for before in &old.namespaces {
            let name = &before.name;
            let now = self.get(name.as_bytes()).map(|declared| declared.kind.as_deref());

            match (before.kind.as_deref(), now) {
                (_, None) => return Err(format!("namespace {name} is dropped: keep it, or retire it")),
                (None, Some(Some(_))) => return Err(format!("namespace {name} was retired, and stays retired")),
                (Some(was), Some(Some(is))) if was != is => {
                    return Err(format!("namespace {name} held {was}, and now holds {is}: retire it and declare another"));
                }
                _ => {}
            }
        }

        Ok(())
    }
}

/// One entry a line, as code declares them.
impl fmt::Display for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for declared in &self.namespaces {
            match &declared.kind {
                Some(kind) => writeln!(f, "namespace {} {kind}", declared.name)?,
                None => writeln!(f, "retired {}", declared.name)?,
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(entries: &[&str]) -> Layout {
        Layout::new(entries.iter().map(|entry| Declared::parse(entry).unwrap()).collect()).unwrap()
    }

    #[test]
    fn entries_read_as_code_writes_them() {
        assert_eq!(
            Declared::parse("  namespace  balances u256 ").unwrap(),
            Declared {
                name: "balances".into(),
                kind: Some("u256".into())
            }
        );
        assert_eq!(Declared::parse("retired fees").unwrap().kind, None);

        for (entry, expected) in [
            ("namespace balances", "NAME KIND"),
            ("retired fees u256", "retired NAME"),
            ("reentrant", "unknown"),
            (&format!("retired {}", "n".repeat(65)), "1 to 64"),
            (&format!("namespace n {}", "k".repeat(65)), "1 to 64"),
        ] {
            assert!(Declared::parse(entry).unwrap_err().contains(expected), "{entry}");
        }

        let twice = [Declared::parse("namespace a u256").unwrap(), Declared::parse("retired a").unwrap()];

        assert!(Layout::new(twice.to_vec()).unwrap_err().contains("declared twice"));
    }

    #[test]
    fn declared_code_reaches_only_its_namespaces() {
        let layout = layout(&["namespace balances u256", "retired fees"]);

        assert_eq!(layout.reaches(b"balances", true), Ok(()));
        assert_eq!(layout.reaches(b"fees", false), Ok(()), "read and cleared");
        assert!(layout.reaches(b"fees", true).unwrap_err().contains("retired"));
        assert!(layout.reaches(b"other", false).unwrap_err().contains("not in this code's layout"));
        assert_eq!(Layout::default().reaches(b"other", true), Ok(()), "code that declares nothing reaches any");
    }

    #[test]
    fn a_new_version_keeps_every_namespace_or_retires_it() {
        let v1 = layout(&["namespace balances u256", "namespace fees u256"]);

        assert_eq!(layout(&["namespace balances u256", "namespace fees u256", "namespace votes u64"]).keeps(&v1), Ok(()));
        assert_eq!(layout(&["namespace balances u256", "retired fees"]).keeps(&v1), Ok(()));
        assert_eq!(v1.keeps(&Layout::default()), Ok(()), "anything keeps no layout");

        let cases = [
            (layout(&["namespace balances u256"]), "fees is dropped"),
            (layout(&["namespace balances text", "namespace fees u256"]), "held u256, and now holds text"),
            (Layout::default(), "balances is dropped"),
        ];

        for (v2, expected) in cases {
            assert!(v2.keeps(&v1).unwrap_err().contains(expected), "{v2}");
        }

        let v2 = layout(&["namespace balances u256", "retired fees"]);

        assert_eq!(layout(&["namespace balances u256", "retired fees"]).keeps(&v2), Ok(()));
        assert!(v1.keeps(&v2).unwrap_err().contains("stays retired"), "no going back");
        assert!(layout(&["namespace balances u256"]).keeps(&v2).unwrap_err().contains("fees is dropped"));
    }
}
