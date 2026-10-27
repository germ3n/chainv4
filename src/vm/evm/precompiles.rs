//! Ethereum's precompiles, and chainv4's beside them: the native precompile on every chain that
//! runs the EVM with native contracts to call, the names precompile on a chain with names, and the
//! crypto precompile on every chain.

#[cfg(feature = "names")]
use super::names::{NAMES_PRECOMPILE, NamesAt};
use {
    super::crypto::{CRYPTO_PRECOMPILE, CryptoAt},
    super::native::{NATIVE_PRECOMPILE, NativeAt},
    crate::db::DbError,
    revm::context_interface::{Cfg, ContextTr, JournalTr},
    revm::handler::{EthPrecompiles, PrecompileProvider, precompile_output_to_interpreter_result},
    revm::interpreter::{CallInputs, InterpreterResult},
    revm::primitives::{Address as Word, AddressSet},
    revm::state::EvmState,
};

pub(super) struct ChainPrecompiles<'a> {
    eth: EthPrecompiles,
    #[cfg(feature = "names")]
    names: Option<NamesAt<'a>>,
    native: Option<NativeAt<'a>>,
    crypto: CryptoAt,
    warm: AddressSet,
}

impl<'a> ChainPrecompiles<'a> {
    pub fn new(eth: EthPrecompiles, #[cfg(feature = "names")] names: Option<NamesAt<'a>>, native: Option<NativeAt<'a>>, crypto: CryptoAt) -> ChainPrecompiles<'a> {
        let mut precompiles = ChainPrecompiles {
            eth,
            #[cfg(feature = "names")]
            names,
            native,
            crypto,
            warm: AddressSet::default(),
        };
        precompiles.warm = precompiles.ours();

        precompiles
    }

    /// The addresses a precompile answers at, which start warm, as Ethereum's own do.
    fn ours(&self) -> AddressSet {
        let mut warm = self.eth.warm_addresses().clone();

        #[cfg(feature = "names")]
        if self.names.is_some() {
            warm.insert(Word::from(NAMES_PRECOMPILE));
        }

        if self.native.is_some() {
            warm.insert(Word::from(NATIVE_PRECOMPILE));
        }

        warm.insert(Word::from(CRYPTO_PRECOMPILE));

        warm
    }

    /// The database failure a precompile met, if any.
    pub fn fault(&self) -> Option<DbError> {
        #[cfg(feature = "names")]
        if let Some(err) = self.names.as_ref().and_then(NamesAt::fault) {
            return Some(err);
        }

        self.native.as_ref().and_then(|native| native.fault.borrow_mut().take())
    }

    /// The native calls made, and the precompile's fault taken.
    pub fn native(&mut self) -> Option<&mut NativeAt<'a>> {
        self.native.as_mut()
    }
}

impl<CTX> PrecompileProvider<CTX> for ChainPrecompiles<'_>
where
    CTX: ContextTr<Journal: JournalTr<State = EvmState>>,
{
    type Output = InterpreterResult;

    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) -> bool {
        let changed = <EthPrecompiles as PrecompileProvider<CTX>>::set_spec(&mut self.eth, spec);

        if changed {
            self.warm = self.ours();
        }

        changed
    }

    fn run(&mut self, context: &mut CTX, inputs: &CallInputs) -> Result<Option<InterpreterResult>, String> {
        #[cfg(feature = "names")]
        if let Some(names) = &self.names
            && inputs.bytecode_address == Word::from(NAMES_PRECOMPILE)
        {
            let input = inputs.input.as_bytes(context);
            let output = names.answer(&input, inputs.gas_limit, inputs.reservoir);

            return Ok(Some(precompile_output_to_interpreter_result(output, inputs.gas_limit)));
        }

        if let Some(native) = &self.native
            && inputs.bytecode_address == Word::from(NATIVE_PRECOMPILE)
        {
            let output = native.answer(context, inputs);

            return Ok(Some(precompile_output_to_interpreter_result(output, inputs.gas_limit)));
        }

        if inputs.bytecode_address == Word::from(CRYPTO_PRECOMPILE) {
            let input = inputs.input.as_bytes(context);
            let output = self.crypto.answer(&input, inputs.gas_limit, inputs.reservoir);

            return Ok(Some(precompile_output_to_interpreter_result(output, inputs.gas_limit)));
        }

        self.eth.run(context, inputs)
    }

    fn warm_addresses(&self) -> &AddressSet {
        &self.warm
    }

    fn contains(&self, address: &Word) -> bool {
        self.warm.contains(address)
    }
}
