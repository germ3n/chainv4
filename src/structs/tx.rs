use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::{Keypair, PublicKey, Signature},
    crate::crypto::verifiable::Verifiable,
    crate::crypto::verifiable::VerifyError,
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::structs::code::Code,
    crate::structs::hash::Hash,
    crate::structs::u256::U256Le,
    crate::structs::u256::u256,
    crate::structs::vote::SignedVote,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[document(
    summary = "A state change submitted to the chain.",
    category = "Transactions",
    see_also = "Address, Amount",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Tx {
    #[document(
        summary = "Moves an amount from one address to another.",
        params = {
            from = "The address the amount is taken from.",
            to = "The address the amount is credited to.",
            amount = "How much to move.",
        },
    )]
    #[wincode(tag = 0)]
    Transfer {
        from: Address,
        to: Address,
        amount: Amount,
    },
    #[document(
        summary = "Creates new supply and credits it to an address.",
        params = {
            to = "The address that receives the new supply.",
            amount = "How much to create.",
        },
        see_also = "Tx::Burn",
    )]
    #[wincode(tag = 1)]
    Mint {
        to: Address,
        amount: Amount,
    },
    #[document(
        summary = "Destroys supply held by an address.",
        params = {
            from = "The address the supply is taken from.",
            amount = "How much to destroy.",
        },
    )]
    #[wincode(tag = 2)]
    Burn {
        from: Address,
        amount: Amount,
    },
    #[document(
        summary = "Calls a function on a contract.",
        params = {
            to = "The address of the contract to call.",
            func = "Which function to call: the XXH3-64 of its name, from Hasher::digest.",
            amount = "How much to send to the contract.",
            data = "The data to send to the contract.",
        },
        see_also = "Hasher::digest, TxCallArgumentType",
    )]
    #[wincode(tag = 3)]
    ContractCall {
        to: Address,
        func: Hash,
        amount: Amount,
        data: Vec<TxCallArgumentType>,
    },
    #[document(
        summary = "Deploys a contract. Nodes compile or validate the code themselves; the contract lands at an address derived from the sender and this transaction's nonce.",
        params = { code = "The contract, tagged with the engine that runs it." },
        see_also = "Contracts::deploy, contract_address, Code",
    )]
    #[wincode(tag = 4)]
    Deploy { code: Code },
    #[document(
        summary = "Replaces a contract's code, keeping its address, storage and balance. The contract decides: its check_perms must return true for \"upgrade\" and the sender. Then the new code's migrate runs, if it has one.",
        params = {
            contract = "The contract to upgrade.",
            code = "The new code, for either engine.",
        },
        see_also = "Contracts::upgrade, CHECK_PERMS",
    )]
    #[wincode(tag = 5)]
    Upgrade { contract: Address, code: Code },
    #[document(
        summary = "Bonds funds from the sender to become a finality validator. The funds are locked, the voting key starts voting a few epochs later, and only the sender can unbond and withdraw.",
        params = {
            voter = "The key that will sign the validator's votes. It can stay on the validating node while the sender's key stays offline.",
            amount = "How much to bond: at least the chain's min_bond. Votes weigh by bond.",
        },
        see_also = "Tx::Unbond, Tx::Withdraw, FinalityConfig",
    )]
    #[wincode(tag = 6)]
    Bond { voter: PublicKey, amount: Amount },
    #[document(
        summary = "Stops a validator voting, a few epochs later. Its bond stays locked, and slashable, for withdrawal_delay epochs more.",
        params = { voter = "The validator's voting key. Only the account that bonded may unbond it." },
        see_also = "Tx::Withdraw",
    )]
    #[wincode(tag = 7)]
    Unbond { voter: PublicKey },
    #[document(
        summary = "Pays an unbonded validator's bond back to the account that bonded it, once withdrawal_delay has passed, and forgets the validator.",
        params = { voter = "The validator's voting key." },
    )]
    #[wincode(tag = 8)]
    Withdraw { voter: PublicKey },
    #[document(
        summary = "Reports a validator that signed two conflicting votes. Its whole bond is taken: a share goes to the sender, the rest is burned.",
        params = {
            first = "One of the votes.",
            second = "The other, by the same voter, conflicting with the first.",
        },
        see_also = "Vote::conflicts",
    )]
    #[wincode(tag = 9)]
    Slash { first: Box<SignedVote>, second: Box<SignedVote> },
}

#[document(
    summary = "The type of an argument to a contract call.",
    category = "Transactions",
    see_also = "Tx::ContractCall",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum TxCallArgumentType {
    #[document(summary = "An address.")]
    #[wincode(tag = 0)]
    Address(Address),
    #[document(summary = "An amount.")]
    #[wincode(tag = 1)]
    Amount(Amount),
    #[document(summary = "A vector of bytes.")]
    #[wincode(tag = 2)]
    Bytes(Vec<u8>),
    #[document(summary = "A boolean.")]
    #[wincode(tag = 3)]
    Bool(bool),
    #[document(summary = "A string.")]
    #[wincode(tag = 4)]
    String(String),
    #[document(summary = "A number.")]
    #[wincode(tag = 5)]
    Number(f64),
    #[document(summary = "A signed 64-bit integer.")]
    #[wincode(tag = 6)]
    I64(i64),
    #[document(summary = "An unsigned 64-bit integer.")]
    #[wincode(tag = 7)]
    U64(u64),
    #[document(summary = "A signed 128-bit integer.")]
    #[wincode(tag = 8)]
    I128(i128),
    #[document(summary = "An unsigned 128-bit integer.")]
    #[wincode(tag = 9)]
    U128(u128),
    #[document(summary = "An unsigned 256-bit integer.")]
    #[wincode(tag = 10)]
    U256(#[wincode(with = "U256Le")] u256),
}

impl Verifiable for Tx {
    #[document(
        parent = "Tx",
        summary = "Checks every address and argument inside, that a transfer, mint or burn moves more than zero, and that a contract call names its function by XXH3-64. A contract call may send zero.",
        see_also = "VerifyError, Hasher::digest",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        match self {
            Tx::Transfer { from, to, amount } => {
                from.verify()?;
                to.verify()?;
                amount.verify()?;

                nonzero(amount)
            }
            Tx::Mint { to, amount } => {
                to.verify()?;
                amount.verify()?;

                nonzero(amount)
            }
            Tx::Burn { from, amount } => {
                from.verify()?;
                amount.verify()?;

                nonzero(amount)
            }
            Tx::ContractCall {
                to,
                func,
                amount,
                data,
            } => {
                to.verify()?;
                func.verify()?;
                amount.verify()?;
                function_hash(func)?;

                for argument in data {
                    argument.verify()?;
                }

                Ok(())
            }
            // Whether the source compiles is decided when the transaction runs.
            Tx::Deploy { .. } | Tx::Upgrade { .. } => Ok(()),
            Tx::Bond { voter, amount } => {
                voter.verify()?;
                amount.verify()?;

                nonzero(amount)
            }
            Tx::Unbond { voter } | Tx::Withdraw { voter } => voter.verify(),
            // The signatures are checked when it runs, against the chain's genesis id.
            Tx::Slash { first, second } => {
                if !first.vote.conflicts(&second.vote) {
                    return Err(VerifyError::NotConflicting);
                }

                Ok(())
            }
        }
    }
}

fn nonzero(amount: &Amount) -> Result<(), VerifyError> {
    match amount {
        Amount::U128(0) => Err(VerifyError::ZeroAmount),
        Amount::U256(value) if *value == u256::ZERO => Err(VerifyError::ZeroAmount),
        Amount::U128(_) | Amount::U256(_) => Ok(()),
    }
}

/// Functions are named by the XXH3-64 of their name, so every client computes the same hash. A
/// build without the xxh3 feature cannot check that and rejects every contract call.
fn function_hash(func: &Hash) -> Result<(), VerifyError> {
    match func {
        #[cfg(feature = "xxh3")]
        Hash::Xxh3_64(_) => Ok(()),
        #[allow(unreachable_patterns)]
        other => Err(VerifyError::FunctionHash(other.algorithm())),
    }
}

impl Verifiable for TxCallArgumentType {
    #[document(
        parent = "TxCallArgumentType",
        summary = "Checks address arguments, and that number arguments are finite: NaN has many encodings, which would let the same call go out under different hashes.",
        see_also = "VerifyError::NonFiniteNumber",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        // Every variant is listed, so a new one has to decide its own rule.
        match self {
            TxCallArgumentType::Address(address) => address.verify(),
            TxCallArgumentType::Amount(amount) => amount.verify(),
            TxCallArgumentType::Number(number) if !number.is_finite() => {
                Err(VerifyError::NonFiniteNumber)
            }
            TxCallArgumentType::Number(_)
            | TxCallArgumentType::Bytes(_)
            | TxCallArgumentType::Bool(_)
            | TxCallArgumentType::String(_)
            | TxCallArgumentType::I64(_)
            | TxCallArgumentType::U64(_)
            | TxCallArgumentType::I128(_)
            | TxCallArgumentType::U128(_)
            | TxCallArgumentType::U256(_) => Ok(()),
        }
    }
}

impl Hashable for Tx {
    const TAG_LEN: usize = 1;
}

impl Hashable for TxCallArgumentType {
    const TAG_LEN: usize = 1;
}

/// Separates transaction signatures from anything else a key might sign.
const SIGNING_DOMAIN: &[u8] = b"chainv4 transaction";

#[document(
    summary = "A transaction as it travels and lands in blocks: the transaction, the signer's next nonce, the signer's public key and their signature over all three. The sender is the signer's address.",
    category = "Transactions",
    see_also = "Tx, Keypair::sign",
    example = "let signed = SignedTx::sign(tx, accounts.get(&keypair.public().address())?.nonce, &keypair);",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct SignedTx {
    #[document(summary = "What to do.")]
    pub tx: Tx,
    #[document(summary = "The sender's nonce at the time; each one is usable once, so the transaction cannot be replayed.")]
    pub nonce: u64,
    #[document(summary = "Who signed it. Their address is the sender.")]
    pub signer: PublicKey,
    #[document(summary = "The signer's signature over the transaction, nonce and signer.")]
    pub signature: Signature,
}

impl SignedTx {
    #[document(
        parent = "SignedTx",
        summary = "Signs a transaction.",
        params = {
            tx = "The transaction.",
            nonce = "The sender's current nonce.",
            keypair = "The sender's keys.",
        },
    )]
    pub fn sign(tx: Tx, nonce: u64, keypair: &Keypair) -> SignedTx {
        let signer = keypair.public();
        let signature = keypair.sign(&signing_bytes(&tx, nonce, &signer));

        SignedTx {
            tx,
            nonce,
            signer,
            signature,
        }
    }

    #[document(
        parent = "SignedTx",
        summary = "The address sending the transaction: the signer's.",
    )]
    pub fn sender(&self) -> Address {
        self.signer.address()
    }

    #[document(
        parent = "SignedTx",
        summary = "The transaction's id: its whole encoding, signature included, hashed with the chain's hash.",
        params = { hash = "The chain's hash, from genesis." },
    )]
    pub fn id(&self, hash: crate::structs::hash::HashType) -> Hash {
        self.hash_with_tag(hash)
    }
}

fn signing_bytes(tx: &Tx, nonce: u64, signer: &PublicKey) -> Vec<u8> {
    let mut bytes = SIGNING_DOMAIN.to_vec();
    bytes.extend_from_slice(&tx.hashable_bytes_with_tag());
    bytes.extend_from_slice(&nonce.to_le_bytes());
    bytes.extend_from_slice(&signer.hashable_bytes_with_tag());

    bytes
}

impl Verifiable for SignedTx {
    #[document(
        parent = "SignedTx",
        summary = "Checks the transaction itself, the signer's key, the signature, and that a transfer or burn only takes from the sender. Whether the nonce is the next one depends on chain state, so the executor checks that.",
        see_also = "Tx::verify, VerifyError::NotSender",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        self.tx.verify()?;
        self.signer.verify()?;
        self.signer.verify_signature(
            &signing_bytes(&self.tx, self.nonce, &self.signer),
            &self.signature,
        )?;

        match &self.tx {
            Tx::Transfer { from, .. } | Tx::Burn { from, .. } if *from != self.sender() => {
                Err(VerifyError::NotSender)
            }
            _ => Ok(()),
        }
    }
}

impl Hashable for SignedTx {}

