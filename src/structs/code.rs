use {
    crate::crypto::hashable::Hashable,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[document(
    summary = "A kind of contract, and the engine that runs it. A chain lists the engines it allows in its genesis, and a node refuses to follow a chain whose engines it was built without.",
    category = "VM",
    see_also = "Code, ScriptEngine, Genesis",
    note = "Each engine sits behind a cargo feature: lvm for Lua and wasm for WebAssembly, both on by default. Tags are pinned, and every variant exists whatever is compiled in, so any node can decode any block.",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Engine {
    #[document(summary = "Luau scripts, on the forked Luau with fuel metering.")]
    #[wincode(tag = 0)]
    Lua,
    #[document(summary = "WebAssembly modules, on wasmi.")]
    #[wincode(tag = 1)]
    Wasm,
}

impl Engine {
    #[document(
        parent = "Engine",
        summary = "The engines this node was built with.",
    )]
    pub fn compiled() -> Vec<Engine> {
        vec![
            #[cfg(feature = "lvm")]
            Engine::Lua,
            #[cfg(feature = "wasm")]
            Engine::Wasm,
        ]
    }

    #[document(
        parent = "Engine",
        summary = "The engine's name: lua or wasm.",
    )]
    pub fn name(&self) -> &'static str {
        match self {
            Engine::Lua => "lua",
            Engine::Wasm => "wasm",
        }
    }
}

impl Hashable for Engine {
    const TAG_LEN: usize = 1;
}

#[document(
    summary = "A contract as it is deployed, tagged with its engine.",
    category = "VM",
    see_also = "Tx::Deploy, Engine",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Code {
    #[document(summary = "Luau source. Nodes compile it themselves, since Luau does not check bytecode.")]
    #[wincode(tag = 0)]
    Lua(String),
    #[document(summary = "A WebAssembly module in the binary format. Nodes validate it before storing it.")]
    #[wincode(tag = 1)]
    Wasm(Vec<u8>),
}

impl Code {
    #[document(
        parent = "Code",
        summary = "The engine that runs this code.",
    )]
    pub fn engine(&self) -> Engine {
        match self {
            Code::Lua(_) => Engine::Lua,
            Code::Wasm(_) => Engine::Wasm,
        }
    }

    #[document(
        parent = "Code",
        summary = "The code itself: the source's bytes, or the module.",
    )]
    pub fn bytes(&self) -> &[u8] {
        match self {
            Code::Lua(source) => source.as_bytes(),
            Code::Wasm(module) => module,
        }
    }
}

impl Hashable for Code {
    const TAG_LEN: usize = 1;
}
