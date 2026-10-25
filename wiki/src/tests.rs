use super::*;

const CREATE: &str = r#"
--[=[document
parent = "ents",
name = "create",
realm = "server",
summary = "Creates a scripted entity.",
params = {
    class = { ty = "string", desc = "Class name." },
},
returns = { ty = "Entity", desc = "The new entity." },
example = "local ent = ents.create(\"sent_blaster\")",
]=]
"#;

fn rust_pages(source: &str) -> Vec<Page> {
    parse_rust_source("api.rs", source).unwrap()
}

fn find<'a>(pages: &'a [Page], key: &str) -> &'a Page {
    pages
        .iter()
        .find(|page| page.key() == key)
        .unwrap_or_else(|| panic!("no page {key}"))
}

#[test]
fn lua_binding_documented_from_rust() {
    let pages = rust_pages(
        r#"
#[document(
    lua,
    parent = "ents",
    name = "create",
    realm = "server",
    summary = "Creates a scripted entity.",
    params = {
        class = { ty = "string", desc = "Class name." },
    },
    returns = { ty = "Entity", desc = "The new entity." },
    example = "local ent = ents.create(\"sent_blaster\")",
)]
fn ents_create() {}
"#,
    );

    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].id, "ents.create");
    assert_eq!(pages[0].lang, Lang::Lua);
    assert_eq!(pages[0].realm, Some(Realm::Server));
    assert_eq!(pages[0].params[0].name, "class");
    assert_eq!(pages[0].params[0].ty, "string");
    assert_eq!(pages[0].returns[0].ty, "Entity");
    assert_eq!(pages[0].syntax, "ents.create(string class)");
    assert!(pages[0].origin.starts_with("api.rs:"));
}

#[test]
fn wasm_imports_read_like_functions() {
    let pages = rust_pages(
        r#"
#[document(wasm, kind = "library", name = "chain", summary = "Host functions.")]
fn chain() {}

#[document(
    wasm,
    parent = "chain",
    name = "storage_get",
    summary = "Reads storage.",
    params = {
        key = { ty = "i32", desc = "Where the key is." },
        key_len = { ty = "i32", desc = "Its length." },
    },
    returns = { ty = "i32", desc = "The value's length, or -1." },
)]
fn chain_storage_get() {}
"#,
    );
    let module = find(&pages, "wasm/chain");
    let function = find(&pages, "wasm/chain.storage_get");

    assert_eq!(module.kind, Kind::Library);
    assert_eq!(module.syntax, "module chain");
    assert_eq!(function.lang, Lang::Wasm);
    assert_eq!(function.syntax, "chain.storage_get(key: i32, key_len: i32) -> i32");
}

#[test]
fn rust_source_defaults_to_rust_pages() {
    let pages = rust_pages(
        "#[document(lang = \"rust\", parent = \"ents\", name = \"create\", summary = \"Creates.\")]\nfn ents_create() {}\n",
    );

    assert_eq!(pages[0].lang, Lang::Rust);
    assert_eq!(pages[0].id, "ents::create");
    assert_eq!(pages[0].syntax, "fn ents::create()");
    assert_eq!(pages[0].realm, None);
}

#[test]
fn parses_lua_block() {
    let pages = parse_lua_source("ents.lua", CREATE).unwrap();

    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].id, "ents.create");
    assert_eq!(pages[0].kind, Kind::Function);
    assert_eq!(
        pages[0].example.as_deref(),
        Some("local ent = ents.create(\"sent_blaster\")")
    );
    assert!(pages[0].origin.contains("ents.lua:"));
    assert_eq!(pages[0].lang, Lang::Lua);
    assert_eq!(pages[0].access, Access::Public);
}

#[test]
fn rejects_missing_summary() {
    let source = "--[=[document\nparent = \"ents\",\nname = \"create\",\n]=]\n";
    let err = parse_lua_source("a.lua", source).unwrap_err();

    assert!(err.contains("a.lua:1"), "{err}");
    assert!(err.contains("missing summary"), "{err}");
}

#[test]
fn parses_internal_flag() {
    let pages = rust_pages(
        r#"
#[document(
    parent = "net",
    name = "call",
    summary = "Runs a callback.",
    internal,
)]
fn net_call() {}
"#,
    );

    assert_eq!(pages[0].access, Access::Internal);
}

#[test]
fn rejects_duplicate_id() {
    let first = parse_lua_source("a.lua", CREATE).unwrap();
    let second = parse_lua_source("b.lua", CREATE).unwrap();
    let mut pages = first;
    pages.extend(second);
    let err = assemble(pages).unwrap_err();

    assert!(err.contains("duplicate"), "{err}");
    assert!(err.contains("ents.create"), "{err}");
}

#[test]
fn the_same_id_in_rust_and_lua_is_two_pages() {
    let mut pages = rust_pages("#[document(summary = \"A transaction.\")]\npub struct Tx;\n");
    pages.extend(
        parse_lua_source(
            "tx.lua",
            "--[=[document\nkind = \"class\",\nname = \"Tx\",\nsummary = \"A transaction.\",\nsee_also = \"rust/Tx\",\n]=]\n",
        )
        .unwrap(),
    );
    let pages = assemble(pages).unwrap();
    let lua = find(&pages, "lua/Tx");

    assert_eq!(find(&pages, "rust/Tx").kind, Kind::Struct);
    assert_eq!(lua.kind, Kind::Class);
    assert_eq!(lua.see_also, vec!["rust/Tx".to_string()]);
}

#[test]
fn kinds_must_fit_their_language() {
    let lua = parse_lua_source(
        "a.lua",
        "--[=[document\nkind = \"struct\",\nname = \"Tx\",\nsummary = \"A.\",\n]=]\n",
    )
    .unwrap_err();
    let rust = parse_rust_source(
        "a.rs",
        "#[document(kind = \"class\", name = \"Tx\", summary = \"A.\")]\nfn tx() {}\n",
    )
    .unwrap_err();

    assert!(lua.contains("kind \"struct\" is not used for lua pages"), "{lua}");
    assert!(rust.contains("kind \"class\" is not used for rust pages"), "{rust}");
}

#[test]
fn rust_function_takes_its_signature() {
    let pages = rust_pages(
        r#"
#[document(
    summary = "Moves an amount between two addresses.",
    params = { amount = "How much to move." },
    returns = "The receipt.",
)]
pub fn transfer(from: &Address, mut amount: u128, memo: Option<&'static str>) -> Result<Receipt, Error> {}
"#,
    );
    let page = &pages[0];

    assert_eq!(page.id, "transfer");
    assert_eq!(page.kind, Kind::Function);
    assert_eq!(
        page.syntax,
        "fn transfer(from: &Address, amount: u128, memo: Option<&'static str>) -> Result<Receipt, Error>"
    );
    assert_eq!(page.params[1].desc, "How much to move.");
    assert_eq!(page.params[1].ty, "u128");
    assert_eq!(page.returns[0].ty, "Result<Receipt, Error>");
    assert_eq!(page.returns[0].desc, "The receipt.");
}

#[test]
fn rust_method_keeps_its_receiver() {
    let pages = rust_pages(
        r#"
impl Tx {
    #[document(parent = "Tx", summary = "Signs the transaction.", params = { key = "The signing key." })]
    pub fn sign(&mut self, key: &[u8; 32]) -> Signature {}
}
"#,
    );
    let page = &pages[0];

    assert_eq!(page.id, "Tx::sign");
    assert_eq!(page.kind, Kind::Method);
    assert_eq!(page.syntax, "fn Tx::sign(&mut self, key: &[u8; 32]) -> Signature");
    assert_eq!(page.params[0].name, "self");
    assert_eq!(page.params[1].desc, "The signing key.");
}

#[test]
fn documenting_a_param_the_signature_lacks_is_an_error() {
    let err = parse_rust_source(
        "a.rs",
        "#[document(summary = \"Hashes.\", params = { data = \"Bytes.\" })]\npub fn hash(bytes: &[u8]) {}\n",
    )
    .unwrap_err();

    assert!(err.contains("param data is not in the signature"), "{err}");
}

#[test]
fn renamed_pages_ignore_the_signature() {
    let pages = rust_pages(
        "#[document(name = \"send\", parent = \"net\", summary = \"Sends.\", params = { data = \"Bytes.\" })]\nfn net_send(lua: &Lua) {}\n",
    );

    assert_eq!(pages[0].syntax, "fn net::send(data)");
}

#[test]
fn trait_methods_are_documented() {
    let pages = rust_pages(
        r#"
#[document(summary = "Something that can be hashed.", category = "Crypto")]
pub trait Hashable {
    #[document(parent = "Hashable", summary = "The bytes that get hashed.")]
    fn hashable_bytes(&self) -> Cow<'_, [u8]>;
}
"#,
    );

    assert_eq!(find(&pages, "rust/Hashable").kind, Kind::Trait);
    assert_eq!(find(&pages, "rust/Hashable").syntax, "trait Hashable");
    assert_eq!(
        find(&pages, "rust/Hashable::hashable_bytes").syntax,
        "fn Hashable::hashable_bytes(&self) -> Cow<'_, [u8]>"
    );
}

#[test]
fn variants_and_fields_are_documented_inside_their_item() {
    let pages = rust_pages(
        r#"
#[document(summary = "A state change.", category = "Transactions")]
pub enum Tx {
    #[document(summary = "Moves an amount.", params = { from = "The sender." })]
    Transfer { from: Address, to: Address, amount: Amount },
    #[document(summary = "Raw bytes.")]
    Raw(Vec<u8>),
    Empty,
}

#[document(name = "Account", summary = "An account.")]
pub struct AccountState {
    #[document(summary = "Spendable balance.")]
    pub balance: u128,
}
"#,
    );
    let pages = assemble(pages).unwrap();
    let transfer = find(&pages, "rust/Tx::Transfer");

    assert_eq!(transfer.kind, Kind::Variant);
    assert_eq!(
        transfer.syntax,
        "Tx::Transfer { from: Address, to: Address, amount: Amount }"
    );
    assert_eq!(transfer.params[0].desc, "The sender.");
    assert_eq!(transfer.category.as_deref(), Some("Transactions"));
    assert_eq!(find(&pages, "rust/Tx::Raw").syntax, "Tx::Raw(Vec<u8>)");
    assert_eq!(find(&pages, "rust/Account.balance").syntax, "Account.balance: u128");
    assert_eq!(find(&pages, "rust/Account.balance").kind, Kind::Field);
    assert!(pages.iter().all(|page| page.name != "Empty"));
}

#[test]
fn constants_aliases_and_modules() {
    let pages = rust_pages(
        r#"
#[document(summary = "Blocks per epoch.")]
pub const EPOCH: u64 = 432;

#[document(summary = "A balance.")]
pub type Balance = u128;

#[document(summary = "Consensus rules.")]
pub mod consensus {}
"#,
    );

    assert_eq!(find(&pages, "rust/EPOCH").syntax, "const EPOCH: u64");
    assert_eq!(find(&pages, "rust/Balance").syntax, "type Balance = u128");
    assert_eq!(find(&pages, "rust/consensus").syntax, "mod consensus");
}

#[test]
fn members_inherit_category_and_realm() {
    let pages = parse_lua_source(
        "w.lua",
        "--[=[document\nkind = \"class\",\nname = \"Window\",\nrealm = \"client\",\ncategory = \"GUI\",\nsummary = \"A window.\",\n]=]\n\
         --[=[document\nparent = \"Window\",\nname = \"close\",\nsummary = \"Closes.\",\n]=]\n",
    )
    .unwrap();
    let pages = assemble(pages).unwrap();
    let close = find(&pages, "lua/Window:close");

    assert_eq!(close.realm, Some(Realm::Client));
    assert_eq!(close.category.as_deref(), Some("GUI"));
}

#[test]
fn missing_parents_are_generated_per_language() {
    let pages = rust_pages(
        "#[document(parent = \"Tx\", name = \"Transfer\", kind = \"variant\", summary = \"Moves.\")]\nfn tx_transfer() {}\n\
         #[document(parent = \"codec\", name = \"encode\", summary = \"Encodes.\")]\nfn codec_encode() {}\n",
    );
    let pages = assemble(pages).unwrap();

    assert_eq!(find(&pages, "rust/Tx").kind, Kind::Enum);
    assert_eq!(find(&pages, "rust/codec").kind, Kind::Module);
}

fn realm_of(realm: &str) -> Result<Realm, String> {
    let source = format!(
        "--[=[document\nparent = \"ents\",\nname = \"create\",\nrealm = \"{realm}\",\nsummary = \"Creates.\",\n]=]\n"
    );
    let pages = parse_lua_source("realm.lua", &source)?;

    Ok(pages[0].realm.unwrap())
}

#[test]
fn realms_can_be_combined() {
    assert_eq!(realm_of("client").unwrap(), Realm::Client);
    assert_eq!(realm_of("client, menu").unwrap(), Realm::ClientMenu);
    assert_eq!(realm_of("menu+client").unwrap(), Realm::ClientMenu);
    assert_eq!(realm_of("server,menu").unwrap(), Realm::ServerMenu);
    assert_eq!(realm_of("shared").unwrap(), Realm::Shared);
    assert_eq!(realm_of("shared, menu").unwrap(), Realm::All);
    assert_eq!(realm_of("client, server, menu").unwrap(), Realm::All);
    assert!(realm_of("client, bogus").is_err());
}

#[test]
fn combined_realms_serialize_as_a_list_the_page_splits() {
    let source = CREATE.replace("\"server\"", "\"client, menu\"");
    let pages = assemble(parse_lua_source("sample.lua", &source).unwrap()).unwrap();
    let html = render(&pages, &[]);

    assert!(html.contains("\"realm\":\"client,menu\""), "{html}");
    assert!(html.contains("realm.split(\",\")"), "page script must split combined realms");
}

fn page_with_see_also(source: &str) -> Vec<Page> {
    parse_lua_source("see.lua", source).unwrap()
}

#[test]
fn see_also_resolves_the_dot_spelling_of_a_method() {
    let mut pages = page_with_see_also(
        "--[=[document\nkind = \"class\",\nname = \"Thing\",\nsummary = \"A thing.\",\n]=]\n\
         --[=[document\nparent = \"Thing\",\nname = \"poke\",\nkind = \"method\",\nsummary = \"Pokes.\",\n]=]\n\
         --[=[document\nparent = \"Thing\",\nname = \"prod\",\nkind = \"method\",\nsummary = \"Prods.\",\nsee_also = \"Thing.poke, Thing\",\n]=]\n",
    );
    pages = assemble(pages).unwrap();
    let prod = find(&pages, "lua/Thing:prod");

    assert_eq!(
        prod.see_also,
        vec!["lua/Thing:poke".to_string(), "lua/Thing".to_string()]
    );
}

#[test]
fn see_also_to_a_missing_page_is_a_build_error() {
    let pages = page_with_see_also(
        "--[=[document\nkind = \"library\",\nname = \"lib\",\nsummary = \"A library.\",\nsee_also = \"lib.nothing\",\n]=]\n",
    );
    let error = assemble(pages).unwrap_err();

    assert!(error.contains("lib lists see_also \"lib.nothing\""), "{error}");
}

#[test]
fn tidy_spaces_types_like_rustfmt() {
    let cases = [
        "[u8; 32]",
        "Option<&'a mut Vec<u8>>",
        "HashMap<String, Vec<Vec<u8>>>",
        "Box<dyn Fn(u8) -> bool + Send>",
        "impl Iterator<Item = (u8, u8)>",
        "std::borrow::Cow<'_, [u8]>",
        "*const T",
        "fn(&[u8]) -> [u8; 32]",
        "<T as Trait>::Output",
    ];

    for case in cases {
        let ty: syn::Type = syn::parse_str(case).unwrap();

        assert_eq!(tidy(&ty), case);
    }
}

#[test]
fn page_ships_language_switch_realm_filter_search_and_copy_controls() {
    let pages = assemble(parse_lua_source("sample.lua", CREATE).unwrap()).unwrap();
    let html = render(&pages, &[]);

    for marker in [
        "id=\"lang-switch\"",
        "id=\"realm-filter\"",
        "function searchPages(",
        "function sections(",
        "ArrowDown",
        "class='codewrap'",
        "function typeHtml(",
        "function rustSignature(",
        "referrers[",
    ] {
        assert!(html.contains(marker), "missing {marker}");
    }
}

#[test]
fn sidebar_title_links_home() {
    let pages = assemble(parse_lua_source("sample.lua", CREATE).unwrap()).unwrap();
    let html = render(&pages, &[]);

    assert!(
        html.contains("<a class=\"brand\" href=\"#\""),
        "the Wiki title in the sidebar must link to the homepage"
    );
}

#[test]
fn render_contains_signature_realm_lang_and_example() {
    let pages = assemble(parse_lua_source("sample.lua", CREATE).unwrap()).unwrap();
    let html = render(&pages, &[]);

    assert!(html.contains("ents.create(string class)"), "{html}");
    assert!(html.contains("\"realm\":\"server\""), "{html}");
    assert!(html.contains("\"lang\":\"lua\""), "{html}");
    assert!(html.contains("\"access\":\"public\""), "{html}");
    assert!(html.contains("sent_blaster"), "{html}");
}

#[test]
fn scan_reads_rust_and_lua_files() {
    let dir = std::env::temp_dir().join(format!("wiki-scan-{}", std::process::id()));
    let nested = dir.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        dir.join("tx.rs"),
        "#[document(summary = \"A transaction.\", category = \"Chain\")]\npub enum Tx { #[document(summary = \"Mints.\")] Mint }\n",
    )
    .unwrap();
    fs::write(nested.join("ents.lua"), CREATE).unwrap();
    fs::write(dir.join("notes.txt"), "#[document(").unwrap();

    let pages = scan(&dir);
    fs::remove_dir_all(&dir).unwrap();
    let pages = pages.unwrap();
    let keys: Vec<String> = pages.iter().map(Page::key).collect();

    assert_eq!(keys, vec!["lua/ents", "lua/ents.create", "rust/Tx", "rust/Tx::Mint"]);
    assert_eq!(find(&pages, "rust/Tx::Mint").category.as_deref(), Some("Chain"));
}
