//! chainv4 layout: the storage layout contract code declares, and whether it keeps an older
//! version's, worked out here before anything is sent.

use {
    super::CliResult,
    crate::chain::DEV_TX_LIMITS,
    crate::structs::address::Address,
    crate::structs::code::Engine,
    crate::structs::hash::HashType,
    crate::vm::{Layout, Vm},
    std::path::{Path, PathBuf},
};

#[derive(clap::Args)]
pub struct LayoutArgs {
    /// The code: a Luau source (.lua or .luau) or a WebAssembly module (.wasm).
    pub(super) code: PathBuf,
    /// What the code would replace: older code, as a file, or a deployed contract, such as a
    /// proxy's current implementation, read from the node. Without it, the layout is only shown.
    #[arg(long)]
    pub(super) from: Option<String>,
    /// The node's HTTP API, for --from a contract.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub(super) node: String,
}

/// Shows the layout code declares, and with --from, fails unless it keeps the older one, by the
/// rule an upgrade, and a proxy repointing with check_layout, holds new code to.
pub fn layout(args: LayoutArgs) -> CliResult<()> {
    let shown = args.code.display().to_string();
    let new = read(&args.code)?;
    print(&shown, &new);

    let Some(from) = args.from else {
        return Ok(());
    };
    let (older, old) = match from.parse::<Address>() {
        Ok(address) => (address.to_string(), deployed(&args.node, &address)?),
        Err(_) => (from.clone(), read(Path::new(&from))?),
    };
    print(&older, &old);
    new.keeps(&old).map_err(|why| format!("{shown} does not keep the layout of {older}: {why}"))?;
    println!("{shown} keeps the layout of {older}");

    Ok(())
}

fn print(what: &str, layout: &Layout) {
    match layout.is_declared() {
        true => print!("{what} declares\n{}", indent(layout)),
        false => println!("{what} declares no layout, and may use any namespace"),
    }
}

fn indent(layout: &Layout) -> String {
    layout.to_string().lines().map(|line| format!("  {line}\n")).collect()
}

/// The layout a file's code declares, loaded the way a node loads it at deploy, so code a node
/// would refuse is refused here too.
fn read(path: &Path) -> CliResult<Layout> {
    let engine = match path.extension().and_then(|extension| extension.to_str()) {
        Some("lua" | "luau") => Engine::Lua,
        Some("wasm") => Engine::Wasm,
        _ => return Err(format!("{}: give a .lua, .luau or .wasm file", path.display()).into()),
    };
    let code = std::fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let vm = Vm::new(&[engine], HashType::default_chain())
        .map_err(|engine| format!("this build has no {} engine", engine.name()))?;
    let loaded = vm
        .engine(engine)
        .expect("the vm was built with it")
        .load(&code, DEV_TX_LIMITS)
        .result
        .map_err(|err| format!("{}: {err}", path.display()))?;

    Ok(loaded.layout)
}

/// The layout of a deployed contract, from the node's GET /contracts/{address}.
#[cfg(feature = "client")]
fn deployed(node: &str, address: &Address) -> CliResult<Layout> {
    use crate::vm::Declared;

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into();
    let contract = super::send::ok(agent.get(format!("{}/contracts/{address}", node.trim_end_matches('/'))).call())?;
    let entries = contract["layout"].as_array().ok_or("the node sent no layout; is it older than layouts?")?;
    let declared = entries
        .iter()
        .map(|entry| {
            let name = entry["name"].as_str().ok_or("the node sent a namespace without a name")?;

            Ok(Declared {
                name: name.to_string(),
                kind: entry["kind"].as_str().map(str::to_string),
            })
        })
        .collect::<CliResult<Vec<_>>>()?;

    Ok(Layout::new(declared)?)
}

#[cfg(not(feature = "client"))]
fn deployed(_: &str, _: &Address) -> CliResult<Layout> {
    Err("this build has no client to ask a node with; give the older code as a file".into())
}
