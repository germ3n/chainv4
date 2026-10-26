//! The names precompile: chainv4's names, such as alice.dev, for EVM contracts, as ENS's resolver
//! gives Ethereum's. Solidity calls it like any contract, through an interface:
//!
//! ```solidity
//! interface Names {
//!     function resolve(string calldata name) external view returns (address);
//!     function name(address account) external view returns (string memory);
//! }
//! ```
//!
//! It lives at NAMES_PRECOMPILE on a chain with names alone, judges names at the block's time,
//! and gives each account at its EVM address: an Ed25519 account's alias, which resolving writes
//! down, so the EVM can pay the account at once.

use {
    super::{account_of, evm_alias, register_alias},
    crate::db::{DbError, State},
    crate::registry::names::{Names, NamesConfig, name_problem},
    r#macro::document,
    revm::context_interface::{Cfg, ContextTr},
    revm::handler::{EthPrecompiles, PrecompileProvider, precompile_output_to_interpreter_result},
    revm::interpreter::{CallInputs, InterpreterResult},
    revm::precompile::{PrecompileHalt, PrecompileOutput},
    revm::primitives::{Address as Word, AddressSet, Bytes, U256, keccak256},
    std::cell::RefCell,
    std::sync::Arc,
};

#[document(
    summary = "Where the names precompile lives, on a chain with names: 0x…4e53, NS in ASCII, far above Ethereum's own precompiles.",
    category = "VM",
    see_also = "evm_alias",
)]
pub const NAMES_PRECOMPILE: [u8; 20] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x4e, 0x53];

/// What a lookup costs in EVM gas: about a cold storage read for each record it reads.
const LOOKUP_GAS: u64 = 3_000;
/// And for every 32 bytes of name read or written.
const WORD_GAS: u64 = 3;
/// The longest name the precompile reads.
const MAX_INPUT_NAME: usize = 1_024;

/// The names one transaction resolves through the precompile: the state it reads, the chain's
/// names and the block's time.
pub(super) struct NamesAt<'a> {
    pub state: &'a Arc<State>,
    pub config: NamesConfig,
    pub now: u64,
    /// The first database failure, which stops the node rather than the transaction.
    pub fault: RefCell<Option<DbError>>,
}

/// Ethereum's precompiles, and on a chain with names, the names precompile beside them.
pub(super) struct ChainPrecompiles<'a> {
    eth: EthPrecompiles,
    names: Option<NamesAt<'a>>,
    warm: AddressSet,
}

impl<'a> ChainPrecompiles<'a> {
    pub fn new(eth: EthPrecompiles, names: Option<NamesAt<'a>>) -> ChainPrecompiles<'a> {
        let warm = warm(&eth, names.is_some());

        ChainPrecompiles { eth, names, warm }
    }

    /// The database failure a lookup met, if any.
    pub fn fault(&self) -> Option<DbError> {
        self.names.as_ref().and_then(|names| names.fault.borrow_mut().take())
    }
}

fn warm(eth: &EthPrecompiles, names: bool) -> AddressSet {
    let mut warm = eth.warm_addresses().clone();

    if names {
        warm.insert(Word::from(NAMES_PRECOMPILE));
    }

    warm
}

impl<CTX: ContextTr> PrecompileProvider<CTX> for ChainPrecompiles<'_> {
    type Output = InterpreterResult;

    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) -> bool {
        let changed = <EthPrecompiles as PrecompileProvider<CTX>>::set_spec(&mut self.eth, spec);

        if changed {
            self.warm = warm(&self.eth, self.names.is_some());
        }

        changed
    }

    fn run(&mut self, context: &mut CTX, inputs: &CallInputs) -> Result<Option<InterpreterResult>, String> {
        match &self.names {
            Some(names) if inputs.bytecode_address == Word::from(NAMES_PRECOMPILE) => {
                let input = inputs.input.as_bytes(context);
                let output = names.answer(&input, inputs.gas_limit, inputs.reservoir);

                Ok(Some(precompile_output_to_interpreter_result(output, inputs.gas_limit)))
            }
            _ => self.eth.run(context, inputs),
        }
    }

    fn warm_addresses(&self) -> &AddressSet {
        &self.warm
    }

    fn contains(&self, address: &Word) -> bool {
        self.warm.contains(address)
    }
}

fn selector(signature: &str) -> [u8; 4] {
    keccak256(signature.as_bytes())[..4].try_into().expect("4 bytes")
}

impl NamesAt<'_> {
    /// Answers one call: resolve(string) or name(address). Bad input reverts; too little gas halts.
    fn answer(&self, input: &[u8], gas_limit: u64, reservoir: u64) -> PrecompileOutput {
        let gas = LOOKUP_GAS + WORD_GAS * (input.len() as u64).div_ceil(32);

        if gas > gas_limit {
            return PrecompileOutput::halt(PrecompileHalt::OutOfGas, reservoir);
        }

        let answered = match input.get(..4) {
            Some(called) if *called == selector("resolve(string)") => read_string(&input[4..]).map(|name| self.resolve(&name)),
            Some(called) if *called == selector("name(address)") => input.get(4..36).map(|word| self.name_of(word)),
            _ => None,
        };

        match answered {
            Some(Ok(bytes)) => PrecompileOutput::new(gas, Bytes::from(bytes), reservoir),
            Some(Err(err)) => {
                self.fault.borrow_mut().get_or_insert(err);

                PrecompileOutput::halt(PrecompileHalt::OutOfGas, reservoir)
            }
            None => PrecompileOutput::revert(gas, Bytes::new(), reservoir),
        }
    }

    /// A name's target as one ABI word, or zero; and its alias written down.
    fn resolve(&self, name: &str) -> Result<Vec<u8>, DbError> {
        let name = self.config.strip(name);
        let target = match name_problem(&name) {
            Some(_) => None,
            None => Names::new(self.state).resolve(&name, self.now, self.config.grace_ms)?,
        };
        let mut word = vec![0; 32];

        if let Some(target) = target {
            register_alias(self.state, &target);
            word[12..].copy_from_slice(&evm_alias(&target));
        }

        Ok(word)
    }

    /// An EVM address's primary name, ABI-encoded as a string, empty for none.
    fn name_of(&self, word: &[u8]) -> Result<Vec<u8>, DbError> {
        let account = account_of(self.state, word[12..].try_into().expect("20 bytes"))?;
        let name = Names::new(self.state)
            .primary(&account, self.now, self.config.grace_ms)?
            .map(|name| self.config.display(&name))
            .unwrap_or_default();

        Ok(write_string(&name))
    }
}

/// A string argument, ABI-encoded: its offset, then at the offset its length and bytes. None for
/// anything else, or one longer than MAX_INPUT_NAME.
fn read_string(args: &[u8]) -> Option<String> {
    let word = |at: usize| -> Option<usize> {
        let word = U256::from_be_slice(args.get(at..at.checked_add(32)?)?);

        usize::try_from(word).ok()
    };
    let offset = word(0)?;
    let len = word(offset)?;

    if len > MAX_INPUT_NAME {
        return None;
    }

    let start = offset.checked_add(32)?;
    let bytes = args.get(start..start.checked_add(len)?)?;

    String::from_utf8(bytes.to_vec()).ok()
}

/// A string as an ABI return value: its offset, its length, and its bytes padded to a word.
fn write_string(text: &str) -> Vec<u8> {
    let mut out = U256::from(32).to_be_bytes::<32>().to_vec();
    out.extend(U256::from(text.len()).to_be_bytes::<32>());
    out.extend(text.as_bytes());
    out.resize(64 + text.len().div_ceil(32) * 32, 0);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_cross_as_solidity_encodes_them() {
        let encoded = write_string("alice.dev");

        assert_eq!(encoded.len(), 96);
        assert_eq!(read_string(&encoded), Some("alice.dev".to_string()));
        assert_eq!(write_string("").len(), 64);
        assert_eq!(read_string(&encoded[..70]), None, "cut short");

        let mut long = U256::from(32).to_be_bytes::<32>().to_vec();
        long.extend(U256::from(MAX_INPUT_NAME + 1).to_be_bytes::<32>());
        long.resize(64 + MAX_INPUT_NAME + 1, b'a');

        assert_eq!(read_string(&long), None, "too long to be a name");
        assert_eq!(selector("resolve(string)"), [0x46, 0x1a, 0x44, 0x78]);
        assert_eq!(selector("name(address)"), [0x01, 0x98, 0x48, 0x92], "as solc --hashes gives them");
    }
}
