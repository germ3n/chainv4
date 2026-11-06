//! What contracts run under in a block, as genesis sets it and governance may replace it: what
//! each host operation costs in fuel and the EVM precompiles in gas (Change::Fees), the limits a
//! contract keeps within (Change::Limits), the EVM's spec (Change::EvmSpec) and the DEX's fee
//! (Change::Dex). Plain data, in every build; the engines read it from the call's block, so every
//! call in a block runs by the same, and a change counts from the block after its decision.

use {
    crate::crypto::hashable::Hashable,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "What host operations cost in fuel, the same in every engine, and what the EVM's native and names precompiles cost in gas. Genesis's fees, or a Change::Fees, may name them as fees.costs; unset, each is HostCosts::DEFAULT's. Every divisor, the bytes per fuel, is at least 1.",
    category = "Fees",
    see_also = "FeeConfig::costs, Change::Fees",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
pub struct HostCosts {
    #[document(summary = "A storage read, plus 1 for every storage_bytes_per_fuel bytes of key and value.")]
    pub storage_read: u64,
    #[document(summary = "A storage write or delete. Dearer than a read: every node keeps what is written.")]
    pub storage_write: u64,
    #[document(summary = "Bytes of key and value a fuel pays for, in storage reads, writes and scans.")]
    pub storage_bytes_per_fuel: u64,
    #[document(summary = "A storage scan's seek.")]
    pub storage_scan: u64,
    #[document(summary = "Each entry a scan gives: cheaper than a read, as a scan reads them in order.")]
    pub storage_scan_entry: u64,
    #[document(summary = "One hash, plus 1 for every hash_bytes_per_fuel bytes hashed.")]
    pub hash: u64,
    #[document(summary = "Bytes a fuel hashes, or checks a signature over.")]
    pub hash_bytes_per_fuel: u64,
    #[document(summary = "Calling another contract, on top of the fuel the callee burns.")]
    pub call: u64,
    #[document(summary = "Reading a balance.")]
    pub balance: u64,
    #[document(summary = "Moving funds, by transfer or with a call.")]
    pub transfer: u64,
    #[document(summary = "Emitting an event, plus 1 for every event_bytes_per_fuel bytes of name and data.")]
    pub event: u64,
    #[document(summary = "Bytes of an event's name and data a fuel pays for.")]
    pub event_bytes_per_fuel: u64,
    #[document(summary = "Bytes a fuel copies between a WebAssembly contract's memory and the host, or encodes in Solidity's ABI.")]
    pub copy_bytes_per_fuel: u64,
    #[document(summary = "Compiling and checking a contract's code at deploy or upgrade, per byte of it.")]
    pub load_byte: u64,
    #[document(summary = "Asking the token registry to ask a token again.")]
    pub token_refresh: u64,
    #[document(summary = "Comparing two contracts' storage layouts.")]
    pub check_layout: u64,
    #[document(summary = "Cloning a contract, plus its code's bytes as storage.")]
    pub clone: u64,
    #[document(summary = "Resolving a name, or an address's primary name.")]
    pub name: u64,
    #[document(summary = "Encoding or decoding values in Solidity's ABI, plus its bytes as copies.")]
    pub abi: u64,
    #[document(summary = "Checking a signature, of any scheme, plus its message's bytes as hashes.")]
    pub verify: u64,
    #[document(summary = "Recovering a secp256k1 key from a signature.")]
    pub recover: u64,
    #[document(summary = "The address a key controls.")]
    pub key_address: u64,
    #[document(summary = "Checking a state proof or a Merkle path, plus proof_sibling for each of its siblings.")]
    pub check_proof: u64,
    #[document(summary = "Each sibling of a proof checked.")]
    pub proof_sibling: u64,
    #[document(summary = "Reading the federation in force, or another of the chain's settings, such as the DEX's fee.")]
    pub federation: u64,
    #[document(summary = "Reading the chain's id.")]
    pub chain_id: u64,
    #[document(summary = "EVM gas for a call through the native precompile, before the fuel the native contract burns.")]
    pub evm_native_call: u64,
    #[document(summary = "EVM gas for each 32-byte word of calldata the native precompile reads.")]
    pub evm_native_word: u64,
    #[document(summary = "EVM gas for a lookup through the names precompile.")]
    pub evm_name_lookup: u64,
    #[document(summary = "Checking an Ethereum BLS aggregate signature (FastAggregateVerify): hashing the message to G2, decompressing the signature and the pairing check, plus bls_key for each public key and 1 for every hash_bytes_per_fuel bytes of message.")]
    pub bls_verify: u64,
    #[document(summary = "Each public key a BLS check aggregates: decompressing it, checking it is in its group, and adding it.")]
    pub bls_key: u64,
}

impl Hashable for HostCosts {}

impl HostCosts {
    #[document(parent = "HostCosts", summary = "What every chain charged before costs were governed, and what one that names none still does.")]
    pub const DEFAULT: HostCosts = HostCosts {
        storage_read: 100,
        storage_write: 500,
        storage_bytes_per_fuel: 16,
        storage_scan: 100,
        storage_scan_entry: 40,
        hash: 20,
        hash_bytes_per_fuel: 64,
        call: 1_000,
        balance: 100,
        transfer: 1_000,
        event: 200,
        event_bytes_per_fuel: 16,
        copy_bytes_per_fuel: 64,
        load_byte: 1,
        token_refresh: 100,
        check_layout: 2_000,
        clone: 5_000,
        name: 500,
        abi: 50,
        verify: 3_000,
        recover: 3_000,
        key_address: 100,
        check_proof: 200,
        proof_sibling: 60,
        federation: 200,
        chain_id: 20,
        evm_native_call: 2_600,
        evm_native_word: 3,
        evm_name_lookup: 3_000,
        // Measured against check_signature: secp256k1 verifies in 58.8 µs for 3,000 fuel, so 51 a µs;
        // FastAggregateVerify took 2,017 µs plus 97 µs a key (src/vm/crypto/tests.rs, release).
        bls_verify: 103_000,
        bls_key: 5_000,
    };

    #[document(parent = "HostCosts", summary = "Why no chain could charge them: a divisor of 0.")]
    pub fn problem(&self) -> Option<&'static str> {
        let divisors = [self.storage_bytes_per_fuel, self.hash_bytes_per_fuel, self.event_bytes_per_fuel, self.copy_bytes_per_fuel];

        divisors.contains(&0).then_some("fees.costs' bytes per fuel are at least 1")
    }
}

impl Default for HostCosts {
    fn default() -> HostCosts {
        HostCosts::DEFAULT
    }
}

#[document(
    summary = "The limits a contract keeps within, as genesis's contract_limits or a Change::Limits names them; unset, ContractLimits::DEFAULT's. Raising any is safe. Lowering what deployed contracts may already rely on, how deep calls nest, a key's, a value's and an event's size, how many events and a scan's page, could leave one unable to run as it was written, so a change refuses to lower those; the module and source sizes bound only new deploys, and may go either way.",
    category = "VM",
    see_also = "BlockLimits::contract, Change::Limits",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
pub struct ContractLimits {
    #[document(summary = "The deepest calls between contracts may nest, across engines. A transaction's call is depth 1.")]
    pub call_depth: u32,
    #[document(summary = "The longest storage key, in bytes.")]
    pub key_len: u32,
    #[document(summary = "The largest storage value, in bytes.")]
    pub value_len: u32,
    #[document(summary = "The most entries one storage scan gives.")]
    pub scan: u32,
    #[document(summary = "The largest event data, encoded, in bytes.")]
    pub event_data: u32,
    #[document(summary = "The most events one call may emit itself.")]
    pub events: u32,
    #[document(summary = "The largest WebAssembly module a deploy or upgrade may carry, in bytes. Compiling is not metered by fuel, so this bounds the work one deploy can cause.")]
    pub module_len: u32,
    #[document(summary = "The longest Lua source a deploy or upgrade may carry, in bytes.")]
    pub source_len: u32,
}

impl Hashable for ContractLimits {}

impl ContractLimits {
    #[document(parent = "ContractLimits", summary = "What every chain kept to before the limits were governed, and what one that names none still does.")]
    pub const DEFAULT: ContractLimits = ContractLimits {
        call_depth: 8,
        key_len: 256,
        value_len: 64 * 1024,
        scan: 256,
        event_data: 16 * 1024,
        events: 256,
        module_len: 512 * 1024,
        source_len: 128 * 1024,
    };

    #[document(parent = "ContractLimits", summary = "Why no chain could run under them: a call depth of 0 or past 64, a key, value, scan, event or code size of 0, or a module past what the least block, MIN_BLOCK_BYTES, holds.")]
    pub fn problem(&self) -> Option<&'static str> {
        if self.call_depth == 0 || self.call_depth > 64 {
            return Some("contract_limits.call_depth is 1 to 64");
        }

        if [self.key_len, self.value_len, self.scan, self.event_data, self.events, self.module_len, self.source_len].contains(&0) {
            return Some("contract_limits are each at least 1");
        }

        if u64::from(self.module_len.max(self.source_len)) > super::MIN_BLOCK_BYTES / 2 {
            return Some("contract_limits.module_len and source_len are at most 512 KiB, so a deploy fits the least block");
        }

        None
    }

    #[document(
        parent = "ContractLimits",
        summary = "Why a change may not move from these to others: it lowers a limit a deployed contract may rely on.",
        params = { new = "The limits a change proposes." },
    )]
    pub fn lowering_problem(&self, new: &ContractLimits) -> Option<&'static str> {
        let lowered = new.call_depth < self.call_depth
            || new.key_len < self.key_len
            || new.value_len < self.value_len
            || new.scan < self.scan
            || new.event_data < self.event_data
            || new.events < self.events;

        lowered.then_some("a change of contract_limits never lowers call_depth, key_len, value_len, scan, event_data or events, which deployed contracts may rely on")
    }
}

impl Default for ContractLimits {
    fn default() -> ContractLimits {
        ContractLimits::DEFAULT
    }
}

#[document(
    summary = "Which Ethereum hard fork the chain's EVM follows: its opcodes, their gas and its code-size limits. Genesis's evm_spec sets the first, Prague when unset, and Change::EvmSpec moves to another from a height, so blocks below it replay as they ran.",
    category = "VM",
    see_also = "Change::EvmSpec, Genesis::evm_spec",
    note = "Tags are pinned, so encodings match across builds. The spec also pins revm's: a revm release that changes a spec's meaning is a fork of its own.",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum EvmSpec {
    #[document(summary = "Shanghai.")]
    #[wincode(tag = 0)]
    Shanghai,
    #[document(summary = "Cancun.")]
    #[wincode(tag = 1)]
    Cancun,
    #[document(summary = "Prague: what every chain ran before the spec was governed.")]
    #[default]
    #[wincode(tag = 2)]
    Prague,
    #[document(summary = "Osaka.")]
    #[wincode(tag = 3)]
    Osaka,
}

impl Hashable for EvmSpec {
    const TAG_LEN: usize = 1;
}

impl EvmSpec {
    #[document(parent = "EvmSpec", summary = "Every spec, oldest first.")]
    pub const ALL: [EvmSpec; 4] = [EvmSpec::Shanghai, EvmSpec::Cancun, EvmSpec::Prague, EvmSpec::Osaka];

    #[document(parent = "EvmSpec", summary = "Its name, as genesis files and the API write it: shanghai, cancun, prague or osaka.")]
    pub fn name(self) -> &'static str {
        match self {
            EvmSpec::Shanghai => "shanghai",
            EvmSpec::Cancun => "cancun",
            EvmSpec::Prague => "prague",
            EvmSpec::Osaka => "osaka",
        }
    }
}

impl std::str::FromStr for EvmSpec {
    type Err = String;

    fn from_str(text: &str) -> Result<EvmSpec, String> {
        EvmSpec::ALL
            .into_iter()
            .find(|spec| spec.name() == text)
            .ok_or_else(|| format!("{text} is not an EVM spec; one of {}", EvmSpec::ALL.map(EvmSpec::name).join(", ")))
    }
}

#[document(summary = "How many fee tiers the DEXes' settings may offer.", category = "DEX", see_also = "DexSettings")]
pub const DEX_TIERS: usize = 4;

#[document(
    summary = "The DEXes' settings, as genesis's dex or a Change::Dex sets them: the fee tiers a new pool may pick from, in hundredths of a percent, and the share of each trade's fee the protocol takes and burns. A pool's tier is fixed when it opens, one pool for each pair and tier, so a change of tiers only bounds the pools opened after it. Both DEXes read them as they trade, the native one through the chain library, the EVM one through the native precompile, and the apps quote by them.",
    category = "DEX",
    see_also = "Change::Dex, Genesis::dex",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
pub struct DexSettings {
    #[document(summary = "The tiers, in basis points, ascending, each 1 to 1000, 10%, and 0 for each slot past the last.")]
    pub tiers: [u32; DEX_TIERS],
    #[document(summary = "The share of every trade's fee the protocol takes and burns, in thousandths: 0, the default, leaves it all to the pool's providers, 1000 burns all of it.")]
    pub protocol_permille: u32,
}

impl Hashable for DexSettings {}

impl DexSettings {
    #[document(parent = "DexSettings", summary = "Three tiers, 0.05%, 0.3% and 1%, as Uniswap V3 offers them, and no protocol share.")]
    pub const DEFAULT: DexSettings = DexSettings { tiers: [5, 30, 100, 0], protocol_permille: 0 };

    #[document(parent = "DexSettings", summary = "The tiers it offers, in order, without the unused slots.")]
    pub fn offered(&self) -> Vec<u32> {
        self.tiers.iter().copied().take_while(|tier| *tier != 0).collect()
    }

    #[document(parent = "DexSettings", summary = "Whether a new pool may pick this fee.", params = { fee_bps = "The fee, in basis points." })]
    pub fn offers(&self, fee_bps: u32) -> bool {
        fee_bps != 0 && self.tiers.contains(&fee_bps)
    }

    #[document(parent = "DexSettings", summary = "The protocol's part of a fee taken in, burned: the fee times protocol_permille, in thousandths, rounded down.", params = { fee = "What a trade paid in fees." })]
    pub fn protocol_cut(&self, fee: crate::structs::u256::u256) -> crate::structs::u256::u256 {
        fee.checked_mul(crate::structs::u256::u256::from(self.protocol_permille)).map_or(crate::structs::u256::u256::ZERO, |all| all / crate::structs::u256::u256::from(1000u64))
    }

    #[document(parent = "DexSettings", summary = "Why no DEX could run by them: no tier, a tier of 0 before another, a tier past 10%, tiers out of order or twice, or a protocol share past 1000.")]
    pub fn problem(&self) -> Option<&'static str> {
        let offered = self.offered();

        if offered.is_empty() {
            return Some("dex.tiers offers at least one fee tier");
        }

        if self.tiers[offered.len()..].iter().any(|tier| *tier != 0) {
            return Some("dex.tiers lists its tiers first, with none left out between");
        }

        if offered.iter().any(|tier| *tier > 1000) {
            return Some("dex.tiers are 1 to 1000 basis points each, at most 10%");
        }

        if offered.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Some("dex.tiers go up, each once");
        }

        (self.protocol_permille > 1000).then_some("dex.protocol_permille is 0 to 1000")
    }
}

impl Default for DexSettings {
    fn default() -> DexSettings {
        DexSettings::DEFAULT
    }
}

#[document(
    summary = "Which engines a block's contracts may deploy on and run on: genesis's engines, or those a Change::Engines decided last. A contract of an engine turned off is frozen: nothing calls it and nothing deploys on its engine, but its storage and balance stay, readable, and turning the engine on again brings it back as it was.",
    category = "VM",
    see_also = "Change::Engines, Genesis::engines",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EngineSet(u8);

impl EngineSet {
    #[document(parent = "EngineSet", summary = "The set of these engines.")]
    pub fn of(engines: &[crate::structs::code::Engine]) -> EngineSet {
        EngineSet(engines.iter().fold(0, |bits, engine| bits | EngineSet::bit(*engine)))
    }

    fn bit(engine: crate::structs::code::Engine) -> u8 {
        1 << (engine as u8)
    }

    #[document(parent = "EngineSet", summary = "Whether it holds an engine.")]
    pub fn contains(&self, engine: crate::structs::code::Engine) -> bool {
        self.0 & EngineSet::bit(engine) != 0
    }
}

#[document(
    summary = "What Lua contracts' fuel costs, slot by slot, as the Luau fork charges it: each opcode's cost at its number, then the library's rates, bytes and elements a unit of fuel pays for, a sort comparison and a pattern-matching step. Genesis's fees, or a Change::Fees, may name any of them as fees.lua_costs, by LUA_FUEL_SLOT_NAMES; the rest are LuaCosts::DEFAULT's, which are the fork's own. chainv4 calibrate measures what each costs on this machine.",
    category = "Fees",
    see_also = "FeeConfig::lua_costs, LUA_FUEL_SLOT_NAMES, HostCosts",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
pub struct LuaCosts {
    #[document(summary = "Each slot's cost, LUA_FUEL_SLOTS of them.")]
    pub slots: [u64; LUA_FUEL_SLOTS],
}

#[document(summary = "How many fuel cost slots the Luau fork keeps: 128 for opcodes, then 4 rates.", category = "Fees", see_also = "LuaCosts")]
pub const LUA_FUEL_SLOTS: usize = 132;

#[document(summary = "How many of the Lua fuel cost slots are for opcodes, the first 128; the rates follow.", category = "Fees", see_also = "LuaCosts")]
pub const LUA_FUEL_OPCODES: usize = 128;

#[document(
    summary = "Each Lua fuel cost slot's name, as genesis's fees.lua_costs names it: an opcode's, such as ADD or GETTABLEKS, or a rate's, bytes_per_fuel, elements_per_fuel, sort_comparison and match_step. Empty for a slot no opcode uses.",
    category = "Fees",
    see_also = "LuaCosts",
)]
pub const LUA_FUEL_SLOT_NAMES: [&str; LUA_FUEL_SLOTS] = ["NOP", "BREAK", "LOADNIL", "LOADB", "LOADN", "LOADK", "MOVE", "GETGLOBAL", "SETGLOBAL", "GETUPVAL", "SETUPVAL", "CLOSEUPVALS", "GETIMPORT", "GETTABLE", "SETTABLE", "GETTABLEKS", "SETTABLEKS", "GETTABLEN", "SETTABLEN", "NEWCLOSURE", "NAMECALL", "CALL", "RETURN", "JUMP", "JUMPBACK", "JUMPIF", "JUMPIFNOT", "JUMPIFEQ", "JUMPIFLE", "JUMPIFLT", "JUMPIFNOTEQ", "JUMPIFNOTLE", "JUMPIFNOTLT", "ADD", "SUB", "MUL", "DIV", "MOD", "POW", "ADDK", "SUBK", "MULK", "DIVK", "MODK", "POWK", "AND", "OR", "ANDK", "ORK", "CONCAT", "NOT", "MINUS", "LENGTH", "NEWTABLE", "DUPTABLE", "SETLIST", "FORNPREP", "FORNLOOP", "FORGLOOP", "FORGPREP_INEXT", "FASTCALL3", "FORGPREP_NEXT", "NATIVECALL", "GETVARARGS", "DUPCLOSURE", "PREPVARARGS", "LOADKX", "JUMPX", "FASTCALL", "COVERAGE", "CAPTURE", "SUBRK", "DIVRK", "FASTCALL1", "FASTCALL2", "FASTCALL2K", "FORGPREP", "JUMPXEQKNIL", "JUMPXEQKB", "JUMPXEQKN", "JUMPXEQKS", "IDIV", "IDIVK", "GETUDATAKS", "SETUDATAKS", "NAMECALLUDATA", "NEWCLASSMEMBER", "CALLFB", "CMPPROTO", "FASTPCALL", "NEWCLASS", "CONSTRUCT", "FINCONSTRUCT", "GETPRIVATEFIELD", "SETPRIVATEFIELD", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "bytes_per_fuel", "elements_per_fuel", "sort_comparison", "match_step"];

impl LuaCosts {
    #[document(parent = "LuaCosts", summary = "The Luau fork's own costs, as a state starts with them.")]
    pub const DEFAULT: LuaCosts = LuaCosts { slots: [1, 1, 1, 1, 1, 1, 1, 5, 5, 2, 2, 3, 5, 4, 4, 4, 4, 4, 4, 10, 5, 5, 2, 2, 3, 3, 3, 4, 4, 4, 4, 4, 4, 2, 2, 3, 4, 4, 5, 1, 1, 3, 4, 4, 5, 2, 2, 2, 2, 4, 1, 1, 2, 6, 8, 4, 3, 3, 4, 4, 3, 4, 10, 3, 8, 2, 1, 3, 3, 1, 2, 2, 4, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 6, 5, 4, 3, 8, 8, 3, 4, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 64, 8, 1, 1] };

    #[document(
        parent = "LuaCosts",
        summary = "The slot a name is for.",
        params = { name = "An opcode's name or a rate's, as LUA_FUEL_SLOT_NAMES has it." },
        returns = "Its slot, or None for a name no slot has.",
    )]
    pub fn slot(name: &str) -> Option<usize> {
        (!name.is_empty()).then(|| LUA_FUEL_SLOT_NAMES.iter().position(|slot| *slot == name)).flatten()
    }

    #[document(parent = "LuaCosts", summary = "Each named slot whose cost is not the default's, with its name: what genesis and a change of fees write.")]
    pub fn changed(&self) -> Vec<(&'static str, u64)> {
        LUA_FUEL_SLOT_NAMES
            .iter()
            .zip(self.slots.iter().zip(LuaCosts::DEFAULT.slots.iter()))
            .filter(|(name, (cost, default))| !name.is_empty() && cost != default)
            .map(|(name, (cost, _))| (*name, *cost))
            .collect()
    }

    #[document(parent = "LuaCosts", summary = "Why no chain could charge them: a rate, bytes or elements a fuel pays for, of 0.")]
    pub fn problem(&self) -> Option<&'static str> {
        let rate = |name| self.slots[LuaCosts::slot(name).expect("a rate's slot")];

        (rate("bytes_per_fuel") == 0 || rate("elements_per_fuel") == 0).then_some("fees.lua_costs' bytes_per_fuel and elements_per_fuel are at least 1")
    }
}

impl Default for LuaCosts {
    fn default() -> LuaCosts {
        LuaCosts::DEFAULT
    }
}

#[document(
    summary = "What a block's contracts run under, from its parent's state: the host costs, Lua's costs, the contract limits, the EVM's spec and the DEX's fee. Every call in the block reads the same.",
    category = "VM",
    see_also = "HostCosts, ContractLimits, EvmSpec, DexSettings, BlockAt",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VmRules {
    #[document(summary = "What host operations cost.")]
    pub costs: HostCosts,
    #[document(summary = "What Lua contracts' instructions and library work cost.")]
    pub lua_costs: LuaCosts,
    #[document(summary = "The limits a contract keeps within.")]
    pub limits: ContractLimits,
    #[document(summary = "The engines contracts deploy and run on.")]
    pub engines: EngineSet,
    #[document(summary = "The EVM's spec at the block's height.")]
    pub evm_spec: EvmSpec,
    #[document(summary = "The DEX's fee.")]
    pub dex: DexSettings,
    #[document(summary = "The most bytes one transaction may take, encoded: None leaves native ones bounded by the block alone, and Ethereum-signed ones by ETHEREUM_TX_BYTES.")]
    pub max_tx_bytes: Option<u64>,
    #[document(summary = "The most members a native multisig may have.")]
    pub multisig_members: u32,
}

#[document(
    summary = "The most bytes an Ethereum-signed transaction's raw bytes take on a chain whose limits name no max_tx_bytes: what Ethereum's clients take, 128 KiB.",
    category = "Chain",
    see_also = "VmRules::max_tx_bytes, Tx::Ethereum",
)]
pub const ETHEREUM_TX_BYTES: u64 = 128 * 1024;

impl Default for VmRules {
    fn default() -> VmRules {
        VmRules {
            costs: HostCosts::DEFAULT,
            lua_costs: LuaCosts::DEFAULT,
            limits: ContractLimits::DEFAULT,
            // Every engine this build has, for tools and tests that run no chain.
            engines: EngineSet::of(&crate::structs::code::Engine::compiled()),
            evm_spec: EvmSpec::default(),
            dex: DexSettings::DEFAULT,
            max_tx_bytes: None,
            multisig_members: super::MULTISIG_MEMBERS,
        }
    }
}

impl VmRules {
    #[document(
        parent = "VmRules",
        summary = "The most bytes a transaction may take: max_tx_bytes, or for one Ethereum-signed, whose raw bytes are what is measured, ETHEREUM_TX_BYTES while that is unset.",
        params = { ethereum = "Whether it is Ethereum-signed." },
    )]
    pub fn tx_bytes_limit(&self, ethereum: bool) -> Option<u64> {
        match (self.max_tx_bytes, ethereum) {
            (Some(limit), _) => Some(limit),
            (None, true) => Some(ETHEREUM_TX_BYTES),
            (None, false) => None,
        }
    }
}
