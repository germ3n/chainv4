//! `chainv4 nft`: the chain's NFT and multi-token collections from the command line. collections
//! lists the registry; factory deploys contracts/lua/nft_factory.lua with both templates, from
//! sources built into the binary; create makes a collection through it, owned by the signer; mint,
//! send and operator send what the collections export; show and held read them.

use {
    super::send::{SendArgs, TlsArgs, TxArgs, agent, ok, send},
    super::{CliResult, deployed_at, parse_amount, signer},
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    serde_json::{Value, json},
    std::path::PathBuf,
};

const NFT_LUA: &str = include_str!("../../contracts/lua/nft.lua");
const MULTI_LUA: &str = include_str!("../../contracts/lua/multi.lua");
const FACTORY_LUA: &str = include_str!("../../contracts/lua/nft_factory.lua");

/// The most ids tokens_of returns at once, as the reference collections answer.
const PAGE: u64 = 100;

#[derive(clap::Args)]
pub struct NftArgs {
    #[command(subcommand)]
    pub action: NftAction,
    /// The node's HTTP API.
    #[arg(long, global = true, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    #[command(flatten)]
    pub tls: TlsArgs,
    /// The key file to sign with: the development key when none is given.
    #[arg(long, global = true)]
    pub key: Option<PathBuf>,
    /// The collection factory to create through, rather than the one the name nfts resolves to.
    #[arg(long, global = true)]
    pub factory: Option<Address>,
}

#[derive(clap::Subcommand)]
pub enum NftAction {
    /// Lists the chain's collections by symbol: their kind, name and address.
    Collections {
        /// Only symbols starting with this, ignoring case.
        #[arg(long)]
        symbol: Option<String>,
    },
    /// Deploys a collection factory with both templates, printing where.
    Factory,
    /// Creates a collection through the factory, owned by the signer, printing where.
    Create {
        /// nft, one owner a token, or multi, amounts of each id.
        kind: String,
        name: String,
        symbol: String,
        /// What token_uri starts with, the id following: https://…/, ipfs://CID/ or empty.
        #[arg(long, default_value = "")]
        base_uri: String,
    },
    /// Mints to an address: the next id of an NFT collection, or --amount of --id of a
    /// multi-token one. Only the collection's owner may.
    Mint {
        collection: Address,
        to: Address,
        /// A multi-token collection's id.
        #[arg(long, value_parser = parse_amount)]
        id: Option<u256>,
        /// How many, for a multi-token collection.
        #[arg(long, value_parser = parse_amount, default_value = "1")]
        amount: u256,
        /// The token's own URI, rather than the base one and its id.
        #[arg(long)]
        uri: Option<String>,
    },
    /// Shows a collection, or one of its tokens: who owns it, or how many exist, and its URI.
    Show {
        collection: Address,
        #[arg(value_parser = parse_amount)]
        id: Option<u256>,
    },
    /// What an address holds of a collection.
    Held { collection: Address, owner: Address },
    /// Sends a token, or --amount of a multi-token id.
    Send {
        collection: Address,
        to: Address,
        #[arg(value_parser = parse_amount)]
        id: u256,
        #[arg(long, value_parser = parse_amount, default_value = "1")]
        amount: u256,
        /// Ask a receiving contract first, with on_nft_received: what to send to contracts with.
        #[arg(long)]
        safe: bool,
    },
    /// Lets an operator move everything the signer holds of a collection, or with --revoke stops it.
    Operator {
        collection: Address,
        operator: Address,
        #[arg(long)]
        revoke: bool,
    },
}

/// What a command reaches the node with.
struct Nft {
    agent: ureq::Agent,
    node: String,
    args: NftArgs,
}

/// A collection's kind, as the registry says.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Nft,
    Multi,
}

pub fn nft(args: NftArgs) -> CliResult<()> {
    let nft = Nft { agent: agent(&args.tls)?, node: args.node.trim_end_matches('/').to_string(), args };
    let address = TxCallArgumentType::Address;
    let wide = TxCallArgumentType::U256;

    match &nft.args.action {
        NftAction::Collections { symbol } => {
            let mut url = format!("{}/collections?limit=500", nft.node);

            if let Some(symbol) = symbol {
                url.push_str(&format!("&symbol={}", symbol.trim()));
            }

            for collection in ok(nft.agent.get(url).call())?["collections"].as_array().into_iter().flatten() {
                let text = |key: &str| collection[key].as_str().unwrap_or("-").to_string();
                println!("{} {} {:<6} {}", text("address"), text("kind"), text("symbol"), text("name"));
            }
        }
        NftAction::Factory => {
            let me = signer(nft.args.key.as_deref())?.public().address();
            let single = nft.deploy_lua(&me, "nft.lua", NFT_LUA)?;
            let multi = nft.deploy_lua(&me, "multi.lua", MULTI_LUA)?;
            let factory = nft.deploy_lua(&me, "nft_factory.lua", FACTORY_LUA)?;
            nft.call(factory, "setup", vec![address(single), address(multi)])?;
            println!("nft template {single}");
            println!("multi template {multi}");
            println!("factory {factory}");
        }
        NftAction::Create { kind, name, symbol, base_uri } => {
            if kind != "nft" && kind != "multi" {
                return Err("the kind is nft or multi".into());
            }

            let factory = nft.factory()?;
            let made = nft.read(&factory, "count", &[])?.first().and_then(|count| count["value"].as_str().map(str::to_string)).ok_or("the factory does not say how many it made")?;
            let text = |text: &str| TxCallArgumentType::String(text.to_string());
            nft.call(factory, "create", vec![text(kind), text(name), text(symbol), text(base_uri)])?;
            let created = nft.read(&factory, "at", &[json!({ "type": "u256", "value": made })])?;
            println!("{kind} collection {}", created.first().and_then(|value| value["value"].as_str()).ok_or("the factory lost what it made")?);
        }
        NftAction::Mint { collection, to, id, amount, uri } => {
            let uri = uri.clone().map(TxCallArgumentType::String);

            match (nft.kind(collection)?, id) {
                (Kind::Nft, None) => nft.call(*collection, "mint", [vec![address(*to)], uri.into_iter().collect()].concat())?,
                (Kind::Nft, Some(_)) => return Err("an NFT collection picks the next id itself: leave out --id".into()),
                (Kind::Multi, Some(id)) => nft.call(*collection, "mint", [vec![address(*to), wide(*id), wide(*amount)], uri.into_iter().collect()].concat())?,
                (Kind::Multi, None) => return Err("a multi-token collection mints an --id".into()),
            }
        }
        NftAction::Show { collection, id: None } => {
            let found = ok(nft.agent.get(format!("{}/contracts/{collection}", nft.node)).call())?;
            let found = &found["collection"];

            if found.is_null() {
                return Err(format!("{collection} is not in the collection registry").into());
            }

            println!("{} {} ({})", found["kind"].as_str().unwrap_or("-"), found["name"].as_str().unwrap_or("-"), found["symbol"].as_str().unwrap_or("-"));
        }
        NftAction::Show { collection, id: Some(id) } => {
            let id_arg = json!({ "type": "u256", "value": id.to_string() });

            match nft.kind(collection)? {
                Kind::Nft => println!("owner {}", first(&nft.read(collection, "owner_of", std::slice::from_ref(&id_arg))?)),
                Kind::Multi => println!("supply {}", first(&nft.read(collection, "total_supply", std::slice::from_ref(&id_arg))?)),
            }

            println!("uri {}", first(&nft.read(collection, "token_uri", &[id_arg])?));
        }
        NftAction::Held { collection, owner } => {
            let kind = nft.kind(collection)?;

            for id in nft.held(collection, owner)? {
                match kind {
                    Kind::Nft => println!("{id}"),
                    Kind::Multi => {
                        let args = [json!({ "type": "address", "value": owner.to_string() }), json!({ "type": "u256", "value": id })];
                        println!("{id} × {}", first(&nft.read(collection, "balance_of", &args)?));
                    }
                }
            }
        }
        NftAction::Send { collection, to, id, amount, safe } => {
            let function = if *safe { "safe_transfer" } else { "transfer" };

            match nft.kind(collection)? {
                Kind::Nft => nft.call(*collection, function, vec![address(*to), wide(*id)])?,
                Kind::Multi => nft.call(*collection, function, vec![address(*to), wide(*id), wide(*amount)])?,
            }
        }
        NftAction::Operator { collection, operator, revoke } => {
            nft.call(*collection, "set_approval_for_all", vec![address(*operator), TxCallArgumentType::Bool(!revoke)])?;
        }
    }

    Ok(())
}

/// A read's first value as text.
fn first(values: &[Value]) -> String {
    values.first().map_or_else(|| "-".to_string(), |value| value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string))
}

impl Nft {
    fn call(&self, to: Address, function: &str, args: Vec<TxCallArgumentType>) -> CliResult<()> {
        send(SendArgs {
            node: self.node.clone(),
            tls: TlsArgs { ca_cert: self.args.tls.ca_cert.clone(), insecure: self.args.tls.insecure },
            key: self.args.key.clone(),
            no_wait: false,
            timeout: 120,
            confirmations: 1,
            finalized: false,
            gas_limit: None,
            anyway: false,
            eip712: false,
            max_price: None,
            tip: None,
            tx: TxArgs::Call { to, function: function.to_string(), amount: u256::ZERO, args },
        })
    }

    fn deploy_lua(&self, me: &Address, name: &str, source: &str) -> CliResult<Address> {
        let account = ok(self.agent.get(format!("{}/accounts/{me}", self.node)).call())?;
        let at = deployed_at(me, account["nonce"].as_u64().unwrap_or(0), HashType::default_chain());
        let dir = std::env::temp_dir().join(format!("chainv4-nft-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(name);
        std::fs::write(&path, source)?;
        let sent = send(SendArgs {
            node: self.node.clone(),
            tls: TlsArgs { ca_cert: self.args.tls.ca_cert.clone(), insecure: self.args.tls.insecure },
            key: self.args.key.clone(),
            no_wait: false,
            timeout: 120,
            confirmations: 1,
            finalized: false,
            gas_limit: None,
            anyway: false,
            eip712: false,
            max_price: None,
            tip: None,
            tx: TxArgs::Deploy { lua: Some(path), wasm: None, evm: None, constructor: None, constructor_args: Vec::new() },
        });
        let _ = std::fs::remove_dir_all(&dir);
        sent?;

        Ok(at)
    }

    /// A read-only call's values, each as the API writes it: {type, value}.
    fn read(&self, contract: &Address, function: &str, args: &[Value]) -> CliResult<Vec<Value>> {
        let body = json!({ "contract": contract.to_string(), "function": function, "args": args });
        let answer = ok(self.agent.post(format!("{}/call", self.node)).header("content-type", "application/json").send(body.to_string()))?;

        match answer["status"].as_str() {
            Some("returned") => Ok(answer["values"].as_array().cloned().unwrap_or_default()),
            _ => Err(format!("{function}: {}", answer["error"].as_str().unwrap_or("failed")).into()),
        }
    }

    fn kind(&self, collection: &Address) -> CliResult<Kind> {
        let found = ok(self.agent.get(format!("{}/contracts/{collection}", self.node)).call())?;

        match found["collection"]["kind"].as_str() {
            Some("nft") => Ok(Kind::Nft),
            Some("multi") => Ok(Kind::Multi),
            _ => Err(format!("{collection} is not in the collection registry").into()),
        }
    }

    /// Every id an owner holds of a collection, a page at a time.
    fn held(&self, collection: &Address, owner: &Address) -> CliResult<Vec<String>> {
        let mut ids = Vec::new();

        loop {
            let args = [
                json!({ "type": "address", "value": owner.to_string() }),
                json!({ "type": "u256", "value": ids.len().to_string() }),
                json!({ "type": "u256", "value": PAGE.to_string() }),
            ];
            let page = self.read(collection, "tokens_of", &args)?;
            ids.extend(page.iter().map(|value| first(std::slice::from_ref(value))));

            if (page.len() as u64) < PAGE {
                return Ok(ids);
            }
        }
    }

    /// The factory: given, or the one the chain's directory names.
    fn factory(&self) -> CliResult<Address> {
        match self.args.factory {
            Some(factory) => Ok(factory),
            None => super::send::announced(&self.agent, &self.node, "nft_factory")?
                .ok_or_else(|| "no factory: the chain's directory names none. Give --factory, or deploy one with chainv4 nft factory".into()),
        }
    }
}
