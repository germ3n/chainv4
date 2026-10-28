//! chainv4 update and chainv4 release (docs/updates.md). update fetches the newest release from a
//! node or the manifest's URL, checks a threshold of the release keys built in signed it, and puts
//! its binary for this machine in place of this one. release is the publisher's side: new writes a
//! product's release into a directory for any static host, beside its binaries, sign adds a release
//! key's signature, and verify shows what a manifest says and who signed it.

use {
    super::{CliResult, signer},
    crate::node::{download, fetch_release},
    crate::release::{Breaking, Product, ReleaseFile, ReleaseKeys, SignedRelease, Standing, TARGET, sha256},
    crate::structs::hex::from_hex,
    crate::structs::version::{Upgrade, Version},
    std::io::Write,
    std::path::{Path, PathBuf},
};

#[derive(clap::Args)]
pub struct UpdateArgs {
    /// A node to ask for the newest release, at its GET /release: it relays the manifest, whose
    /// signatures this checks itself.
    #[arg(long)]
    pub node: Option<String>,
    /// The manifest's URL, when not asking a node. Defaults to the one the release keys built in
    /// name.
    #[arg(long, conflicts_with = "node")]
    pub updates: Option<String>,
    /// A SOCKS5 proxy to fetch through, such as Tor's socks5h://127.0.0.1:9050.
    #[arg(long)]
    pub proxy: Option<String>,
    /// Only say whether a newer chainv4 is out.
    #[arg(long)]
    pub check: bool,
    /// Replace this binary without asking.
    #[arg(long)]
    pub yes: bool,
}

pub fn update(args: UpdateArgs) -> CliResult<()> {
    update_reading(args, &ReleaseKeys::built_in()?, &std::env::current_exe()?, &mut std::io::stdin().lock())
}

/// update under these release keys, replacing the binary at path and reading the answer to its
/// question from input.
pub(super) fn update_reading(args: UpdateArgs, keys: &ReleaseKeys, path: &Path, input: &mut dyn std::io::BufRead) -> CliResult<()> {

    if !keys.on() {
        return Err("this build trusts no release keys, so it takes no releases: build it with release/keys.json naming them".into());
    }

    let url = match (&args.node, &args.updates, &keys.updates) {
        (Some(node), _, _) => format!("{}/release", node.trim_end_matches('/')),
        (None, Some(url), _) | (None, None, Some(url)) => url.clone(),
        (None, None, None) => return Err("give --node or --updates: the release keys built in name no manifest".into()),
    };
    let signed = fetch_release(&url, args.proxy.as_deref())?;
    let signers = signed.verify(keys).map_err(|err| format!("the release from {url} is refused: {err}"))?;
    let ours = Version::ours();
    let product = signed.release.product("chainv4").ok_or("the release names no chainv4")?;
    println!("chainv4 {} is the newest, signed by {signers} of the {} release keys; this is {ours}", product.version, keys.keys.len());

    if !product.notes.is_empty() {
        println!("  {}", product.notes);
    }

    match product.standing(ours) {
        Standing::Current => {
            println!("up to date");

            return Ok(());
        }
        Standing::Behind { .. } => {}
        Standing::Unsupported { least, .. } => println!("versions older than {least} are no longer supported"),
    }

    if args.check {
        return Ok(());
    }

    let file = product.file(TARGET).ok_or_else(|| format!("chainv4 {} has no binary for {TARGET}", product.version))?;

    if !args.yes {
        print!("This replaces {} with chainv4 {}, {} bytes from {}. Type yes to go on: ", path.display(), product.version, file.size, file.url);
        std::io::stdout().flush()?;
        let mut answer = String::new();
        input.read_line(&mut answer)?;

        if answer.trim() != "yes" {
            return Err(format!("{} is left as it was", path.display()).into());
        }
    }

    let bytes = download(file, args.proxy.as_deref())?;
    crate::release::install(file, &bytes, path)?;
    println!("{} is chainv4 {} now: start running nodes and workers again to run it", path.display(), product.version);

    Ok(())
}

#[derive(clap::Args)]
pub struct ReleaseArgs {
    #[command(subcommand)]
    pub command: ReleaseCommand,
}

#[derive(clap::Subcommand)]
pub enum ReleaseCommand {
    /// Writes a product's release into a directory for a static host: each binary at
    /// PRODUCT/VERSION/TARGET/NAME, and manifest.json naming it beside the other products and
    /// upgrades already there, unsigned until the release keys sign it.
    New(NewArgs),
    /// Signs a manifest with a release key, beside the signatures it has.
    Sign(SignArgs),
    /// Shows what a manifest says and how many of the release keys built in signed it.
    Verify(VerifyArgs),
}

#[derive(clap::Args)]
pub struct NewArgs {
    /// The directory the host serves: manifest.json and the binaries go in it.
    #[arg(long)]
    pub out: PathBuf,
    /// Where the host serves that directory, such as https://example.org/releases.
    #[arg(long)]
    pub base_url: String,
    /// chainv4, wallet or explorer.
    #[arg(long)]
    pub product: String,
    /// The version released, such as 0.2.0.
    #[arg(long)]
    pub version: Version,
    /// The oldest version that still works with the network: older apps stop and ask to update.
    /// Defaults to the one the manifest had, or this version for a new product.
    #[arg(long)]
    pub least: Option<Version>,
    /// What changed, in a line.
    #[arg(long, default_value = "")]
    pub notes: String,
    /// A binary, as TARGET=PATH, such as x86_64-unknown-linux-gnu=target/release/chainv4. Repeat
    /// for each target.
    #[arg(long, required = true, value_parser = parse_target_file)]
    pub file: Vec<(String, PathBuf)>,
    /// An upgrade to announce, for a chain without governance: GENESIS:LEAST:FROM, the chain's
    /// genesis id, the oldest version that runs past block FROM. Repeat for several.
    #[arg(long, value_parser = parse_breaking)]
    pub breaking: Vec<Breaking>,
    /// Drop the upgrades the manifest announced before.
    #[arg(long)]
    pub clear_breaking: bool,
}

#[derive(clap::Args)]
pub struct SignArgs {
    /// The release key's file, from chainv4 keygen.
    #[arg(long)]
    pub key: PathBuf,
    /// The manifest to sign, written back with the signature added.
    pub manifest: PathBuf,
}

#[derive(clap::Args)]
pub struct VerifyArgs {
    /// The manifest to check.
    pub manifest: PathBuf,
}

pub fn release(args: ReleaseArgs) -> CliResult<()> {
    match args.command {
        ReleaseCommand::New(args) => new(args),
        ReleaseCommand::Sign(args) => sign(args),
        ReleaseCommand::Verify(args) => verify(args),
    }
}

fn read_manifest(path: &Path) -> CliResult<SignedRelease> {
    let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;

    Ok(SignedRelease::from_json(&serde_json::from_slice(&bytes).map_err(|err| format!("{} is not JSON: {err}", path.display()))?)?)
}

fn write_manifest(path: &Path, signed: &SignedRelease) -> CliResult<()> {
    std::fs::write(path, serde_json::to_string_pretty(&signed.to_json())? + "\n").map_err(|err| format!("{}: {err}", path.display()).into())
}

fn new(args: NewArgs) -> CliResult<()> {
    let manifest = args.out.join("manifest.json");
    let mut release = match manifest.exists() {
        true => read_manifest(&manifest)?.release,
        false => Default::default(),
    };
    let base = args.base_url.trim_end_matches('/');
    let mut files = Vec::new();

    for (target, path) in &args.file {
        let name = path.file_name().ok_or_else(|| format!("{} names no file", path.display()))?.to_string_lossy().to_string();
        let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
        let at = format!("{}/{}/{target}/{name}", args.product, args.version);
        let dest = args.out.join(&at);
        std::fs::create_dir_all(dest.parent().expect("a file's directory"))?;
        std::fs::write(&dest, &bytes).map_err(|err| format!("{}: {err}", dest.display()))?;
        files.push(ReleaseFile { target: target.clone(), url: format!("{base}/{at}"), sha256: sha256(&bytes), size: bytes.len() as u64 });
        println!("{target}: {} ({} bytes)", dest.display(), bytes.len());
    }

    files.sort_by(|one, other| one.target.cmp(&other.target));
    let least = args.least.or_else(|| release.product(&args.product).map(|product| product.least)).unwrap_or(args.version);
    release.products.retain(|product| product.name != args.product);
    release.products.push(Product { name: args.product.clone(), version: args.version, least, notes: args.notes, files });
    release.products.sort_by(|one, other| one.name.cmp(&other.name));

    if args.clear_breaking {
        release.breaking.clear();
    }

    release.breaking.extend(args.breaking);
    // Issued after the release it replaces, so nodes that took that one take this.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    release.issued = now.max(release.issued + 1);

    if let Some(problem) = release.problem() {
        return Err(problem.into());
    }

    write_manifest(&manifest, &SignedRelease { release, signatures: Vec::new() })?;
    println!("wrote {}: sign it with chainv4 release sign --key KEY {}", manifest.display(), manifest.display());

    Ok(())
}

fn sign(args: SignArgs) -> CliResult<()> {
    let mut signed = read_manifest(&args.manifest)?;
    let key = signer(Some(&args.key))?;
    signed.sign(&key);
    write_manifest(&args.manifest, &signed)?;
    let keys = ReleaseKeys::built_in()?;
    let trusted = keys.keys.contains(&key.public());
    println!("signed {} with {}{}", args.manifest.display(), key.public(), if trusted { "" } else { ", which this build does not trust" });

    match signed.verify(&keys) {
        Ok(signers) => println!("{signers} of the {} release keys signed it: it counts", keys.keys.len()),
        Err(err) => println!("{err}"),
    }

    Ok(())
}

fn verify(args: VerifyArgs) -> CliResult<()> {
    let signed = read_manifest(&args.manifest)?;

    for product in &signed.release.products {
        let targets: Vec<&str> = product.files.iter().map(|file| file.target.as_str()).collect();
        println!("{} {} (least {}): {}", product.name, product.version, product.least, targets.join(", "));
    }

    for breaking in &signed.release.breaking {
        println!("upgrade of {}: older than {} stop at block {}", crate::structs::hex::to_hex(&breaking.chain), breaking.upgrade.least, breaking.upgrade.from);
    }

    let keys = ReleaseKeys::built_in()?;

    match signed.verify(&keys) {
        Ok(signers) => println!("{signers} of the {} release keys signed it, of {} needed: it counts", keys.keys.len(), keys.threshold),
        Err(err) => return Err(err.to_string().into()),
    }

    Ok(())
}

fn parse_target_file(text: &str) -> Result<(String, PathBuf), String> {
    match text.split_once('=') {
        Some((target, path)) if !target.is_empty() && !path.is_empty() => Ok((target.to_string(), PathBuf::from(path))),
        _ => Err(format!("{text} is not TARGET=PATH")),
    }
}

fn parse_breaking(text: &str) -> Result<Breaking, String> {
    let wrong = || format!("{text} is not GENESIS:LEAST:FROM, such as 0x3b6a…:0.2.0:150000");
    let mut parts = text.split(':');
    let (Some(chain), Some(least), Some(from), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(wrong());
    };

    Ok(Breaking {
        chain: from_hex(chain).filter(|bytes| !bytes.is_empty()).ok_or_else(wrong)?,
        upgrade: Upgrade { least: least.parse()?, from: from.parse().map_err(|_| wrong())? },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_and_files_read_from_their_flags() {
        assert_eq!(parse_breaking("0x0a0b:0.2.0:150"), Ok(Breaking { chain: vec![10, 11], upgrade: Upgrade { least: Version::new(0, 2, 0), from: 150 } }));
        assert!(parse_breaking("0x0a0b:0.2.0").is_err());
        assert!(parse_breaking("zz:0.2.0:1").is_err());
        assert_eq!(parse_target_file("aarch64-apple-darwin=bin/chainv4"), Ok(("aarch64-apple-darwin".to_string(), PathBuf::from("bin/chainv4"))));
        assert!(parse_target_file("bin/chainv4").is_err());
    }

    #[test]
    fn new_keeps_the_other_products_and_issues_after_the_last() {
        let dir = std::env::temp_dir().join(format!("chainv4-release-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("chainv4");
        std::fs::write(&binary, b"a node").unwrap();
        let args = |product: &str, version: Version| NewArgs {
            out: dir.join("out"),
            base_url: "https://example.org/releases/".to_string(),
            product: product.to_string(),
            version,
            least: None,
            notes: String::new(),
            file: vec![(TARGET.to_string(), binary.clone())],
            breaking: Vec::new(),
            clear_breaking: false,
        };

        new(args("wallet", Version::new(0, 1, 0))).unwrap();
        let first = read_manifest(&dir.join("out/manifest.json")).unwrap().release;
        new(args("chainv4", Version::new(0, 2, 0))).unwrap();
        let second = read_manifest(&dir.join("out/manifest.json")).unwrap().release;

        assert_eq!(second.products.iter().map(|product| product.name.as_str()).collect::<Vec<_>>(), ["chainv4", "wallet"]);
        assert!(second.issued > first.issued);
        let file = second.product("chainv4").unwrap().file(TARGET).unwrap();
        assert_eq!(file.url, format!("https://example.org/releases/chainv4/0.2.0/{TARGET}/chainv4"));
        assert_eq!(std::fs::read(dir.join(format!("out/chainv4/0.2.0/{TARGET}/chainv4"))).unwrap(), b"a node");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
