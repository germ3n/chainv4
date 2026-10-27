//! The coin the EVM's own frames move, for the receipt's payments: every call below the top that
//! sends value, every contract a contract creates with value, and every SELFDESTRUCT that pays
//! its balance on. A frame that reverts takes what it and the frames under it paid back with it.
//! The top frame's value is the transaction's own, or what a native contract sent with evm.call,
//! which its host records; and what goes to the native precompile goes on to a native contract,
//! which the crossing records.

use {
    super::NATIVE_PRECOMPILE,
    revm::context_interface::{ContextTr, JournalTr},
    revm::inspector::Inspector,
    revm::interpreter::{CallInputs, CallOutcome, CreateInputs, CreateOutcome},
    revm::primitives::{Address as Word, U256},
    std::cell::RefCell,
    std::rc::Rc,
};

/// A move between two EVM addresses, as the EVM sees them, before they are told as accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Moved {
    pub from: [u8; 20],
    pub to: [u8; 20],
    pub amount: U256,
}

/// What the run's frames paid so far, shared with the native precompile, which places the native
/// calls' payments among them by how many there were when each returned.
pub(super) type Paid = Rc<RefCell<Vec<Moved>>>;

/// The inspector that keeps them.
#[derive(Default)]
pub(super) struct Payments {
    pub paid: Paid,
    /// How many moves there were as each open frame began: what a revert truncates to.
    frames: Vec<usize>,
}

impl Payments {
    fn open(&mut self) {
        self.frames.push(self.paid.borrow().len());
    }

    /// Ends the innermost frame: what it paid stays only if it succeeded.
    fn close(&mut self, succeeded: bool) {
        if let Some(start) = self.frames.pop()
            && !succeeded
        {
            self.paid.borrow_mut().truncate(start);
        }
    }

    fn pay(&self, from: Word, to: Word, amount: U256) {
        if amount > U256::ZERO && from != to && to != Word::from(NATIVE_PRECOMPILE) {
            self.paid.borrow_mut().push(Moved { from: from.into_array(), to: to.into_array(), amount });
        }
    }
}

impl<CTX: ContextTr> Inspector<CTX> for Payments {
    fn call(&mut self, context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        let below_the_top = context.journal().depth() > 0;
        self.open();

        if below_the_top && let Some(value) = inputs.transfer_value() {
            self.pay(inputs.caller, inputs.target_address, value);
        }

        None
    }

    fn call_end(&mut self, _: &mut CTX, _: &CallInputs, outcome: &mut CallOutcome) {
        self.close(outcome.result.result.is_ok());
    }

    fn create(&mut self, _: &mut CTX, _: &mut CreateInputs) -> Option<CreateOutcome> {
        self.open();

        None
    }

    fn create_end(&mut self, context: &mut CTX, inputs: &CreateInputs, outcome: &mut CreateOutcome) {
        let succeeded = outcome.result.result.is_ok();
        // What it sent the new contract goes before what the contract's init paid on, as a
        // call's does: put in at the frame's start.
        let start = *self.frames.last().unwrap_or(&0);
        self.close(succeeded);

        if succeeded
            && context.journal().depth() > 0
            && let Some(created) = outcome.address
            && inputs.value() > U256::ZERO
        {
            let at = start.min(self.paid.borrow().len());
            self.paid.borrow_mut().insert(at, Moved { from: inputs.caller().into_array(), to: created.into_array(), amount: inputs.value() });
        }
    }

    fn selfdestruct(&mut self, contract: Word, target: Word, value: U256) {
        self.pay(contract, target, value);
    }
}
