//! The coin the EVM's own frames move, for the receipt's payments: every call below the top that
//! sends value, every contract a contract creates with value, and every SELFDESTRUCT that pays
//! its balance on. A frame that reverts takes what it and the frames under it paid back with it.
//! The top frame's value is the transaction's own, or what a native contract sent with evm.call,
//! which its host records; and what goes to the native precompile goes on to a native contract,
//! which the crossing records.
//!
//! The same inspector keeps the frames for the receipt's trace: each call, delegate call and
//! create, as a hop at the EVM's depth, with the gas it used, why it failed, and which of the logs
//! that stand it emitted. The native precompile puts each native call's own hops under the frame
//! that called it.

use {
    super::NATIVE_PRECOMPILE,
    crate::structs::trace::HopKind,
    revm::context_interface::{ContextTr, JournalTr},
    revm::inspector::Inspector,
    revm::interpreter::{CallInputs, CallOutcome, CallScheme, CreateInputs, CreateOutcome},
    revm::primitives::{Address as Word, Log, U256},
    std::cell::RefCell,
    std::rc::Rc,
};

/// One EVM frame, as the trace will tell it once its addresses are told as accounts.
#[derive(Clone, Debug)]
pub(super) struct Frame {
    pub depth: usize,
    pub kind: HopKind,
    pub from: [u8; 20],
    /// The contract called, whose code ran in a delegate call, or that was created.
    pub to: [u8; 20],
    /// The selector called, in hex; empty for a plain payment or a create.
    pub function: String,
    pub value: U256,
    pub gas: u64,
    pub error: Option<String>,
}

/// The run's frames so far, shared with the native precompile, which reads the frame calling it.
#[derive(Default)]
pub(super) struct Frames {
    pub frames: Vec<Frame>,
    /// The frames open now, innermost last.
    open: Vec<usize>,
    /// Each log that stands so far, by the frame that emitted it: a frame that fails takes its
    /// own back, and those of the frames under it.
    pub logs: Vec<usize>,
    /// How many logs stood as each open frame began.
    logs_at: Vec<usize>,
}

impl Frames {
    fn begin(&mut self, frame: Frame) {
        self.logs_at.push(self.logs.len());
        self.open.push(self.frames.len());
        self.frames.push(frame);
    }

    fn end(&mut self, gas: u64, error: Option<String>, created: Option<[u8; 20]>) {
        let (Some(at), Some(logs_at)) = (self.open.pop(), self.logs_at.pop()) else {
            return;
        };

        if error.is_some() {
            self.logs.truncate(logs_at);
        }

        let frame = &mut self.frames[at];
        frame.gas = gas;
        frame.error = error;

        if let Some(created) = created {
            frame.to = created;
        }
    }

    /// The innermost frame running now: the one calling the native precompile, when it runs.
    pub fn current(&self) -> Option<usize> {
        self.open.last().copied()
    }
}

/// Why a frame failed, as a hop says it: a revert's reason, or how it halted.
fn failure(result: &revm::interpreter::InterpreterResult) -> Option<String> {
    match result.result {
        outcome if outcome.is_ok() => None,
        outcome if outcome.is_revert() => Some(format!("reverted: {}", super::revert_reason(&result.output))),
        outcome => Some(format!("halted: {outcome:?}")),
    }
}

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

/// The inspector that keeps them, and the frames.
#[derive(Default)]
pub(super) struct Payments {
    pub paid: Paid,
    pub traced: Rc<RefCell<Frames>>,
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
        let depth = context.journal().depth();
        self.open();

        if depth > 0 && let Some(value) = inputs.transfer_value() {
            self.pay(inputs.caller, inputs.target_address, value);
        }

        let input = inputs.input.bytes(context);
        let (kind, to) = match inputs.scheme {
            CallScheme::DelegateCall | CallScheme::CallCode => (HopKind::Delegate, inputs.bytecode_address),
            CallScheme::Call | CallScheme::StaticCall => (HopKind::Call, inputs.target_address),
        };
        self.traced.borrow_mut().begin(Frame {
            depth,
            kind,
            from: inputs.caller.into_array(),
            to: to.into_array(),
            function: input.get(..4).map(crate::structs::hex::to_hex).unwrap_or_default(),
            value: inputs.transfer_value().unwrap_or_default(),
            gas: 0,
            error: None,
        });

        None
    }

    fn call_end(&mut self, _: &mut CTX, _: &CallInputs, outcome: &mut CallOutcome) {
        self.close(outcome.result.result.is_ok());
        self.traced.borrow_mut().end(outcome.result.gas.total_gas_spent(), failure(&outcome.result), None);
    }

    fn create(&mut self, context: &mut CTX, inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        self.open();
        self.traced.borrow_mut().begin(Frame {
            depth: context.journal().depth(),
            kind: HopKind::Create,
            from: inputs.caller().into_array(),
            to: [0; 20],
            function: String::new(),
            value: inputs.value(),
            gas: 0,
            error: None,
        });

        None
    }

    fn log(&mut self, _: &mut CTX, _: Log) {
        let mut frames = self.traced.borrow_mut();

        if let Some(at) = frames.current() {
            frames.logs.push(at);
        }
    }

    fn create_end(&mut self, context: &mut CTX, inputs: &CreateInputs, outcome: &mut CreateOutcome) {
        let succeeded = outcome.result.result.is_ok();
        self.traced.borrow_mut().end(outcome.result.gas.total_gas_spent(), failure(&outcome.result), outcome.address.filter(|_| succeeded).map(|created| created.into_array()));
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
