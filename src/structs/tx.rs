use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::{Keypair, PublicKey, Signature},
    crate::crypto::verifiable::Verifiable,
    crate::crypto::verifiable::VerifyError,
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::registry::signers::Signer,
    crate::governance::Change,
    crate::registry::names::{NameAction, name_problem},
    crate::structs::code::Code,
    crate::structs::hash::Hash,
    crate::structs::u256::{I256Le, U256Le, i256},
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
        summary = "Reports a voter that signed two conflicting votes. Under FFG its whole bond is taken: a share goes to the sender, the rest is burned. Under authority finality the signer is thrown out of the decided set and the sets already fixed two epochs on.",
        params = {
            first = "One of the votes.",
            second = "The other, by the same voter, conflicting with the first.",
        },
        see_also = "Vote::conflicts",
    )]
    #[wincode(tag = 9)]
    Slash { first: Box<SignedVote>, second: Box<SignedVote> },
    #[document(
        summary = "Approves a new set of signers for authority finality. Only one of the decided signers may send it, from its account. Once change_threshold of them approve the same set for the same version, it is decided, and the checkpoints that follow fix it for epochs activation_delay on.",
        params = {
            version = "The version of the decided set this changes, so an approval never counts toward a later change.",
            signers = "Every signer of the new set.",
            threshold = "How many of them justify a checkpoint: more than half, and at most all.",
            change_threshold = "How many of them change the set after that: more than half, and at most all.",
        },
        see_also = "Signers::approve, AuthorityConfig",
    )]
    #[wincode(tag = 10)]
    ApproveSigners {
        version: u64,
        signers: Vec<Signer>,
        threshold: u32,
        change_threshold: u32,
    },
    #[document(
        summary = "Takes back the sender's approval of a change of signers, so it backs no change. Only one of the decided signers may send it, while its approval still counts.",
        params = { version = "The version of the decided set its approval was for." },
        see_also = "Signers::withdraw, Tx::ApproveSigners",
    )]
    #[wincode(tag = 11)]
    WithdrawApproval { version: u64 },
    #[document(
        summary = "Approves a change to what genesis set, under the chain's governance: today, a new consensus. Only an account governance lets approve may send it: a validator's or signer's account under the electorate, a member's under a council. Once the approvals for the same change weigh enough, it is decided, and the next checkpoint fixes it for the epoch activation_delay ahead.",
        params = {
            version = "The version of the decision this changes: how many changes were decided before, as GET /governance reports.",
            change = "The change it approves.",
        },
        see_also = "Changes::approve, GovernanceConfig, Change",
    )]
    #[wincode(tag = 12)]
    ApproveChange { version: u64, change: Change },
    #[document(
        summary = "Takes back the sender's approval of a change, so it backs none. Only an account governance lets approve may send it, while its approval still counts.",
        params = { version = "The version of the decision its approval was for." },
        see_also = "Changes::withdraw, Tx::ApproveChange",
    )]
    #[wincode(tag = 13)]
    WithdrawChange { version: u64 },
    #[document(
        summary = "Calls an EVM contract with raw calldata, as Ethereum does, or sends the coin to any address through the EVM. The sender takes part in the EVM at its 20-byte address: a secp256k1 account as itself, any other under its alias.",
        params = {
            to = "The EVM contract, or any account, which then simply receives value.",
            value = "What the call sends, in the coin's smallest units.",
            input = "The calldata: a function selector and ABI-encoded arguments, as Solidity expects.",
        },
        see_also = "Code::Evm, evm_alias",
    )]
    #[wincode(tag = 14)]
    EvmCall { to: Address, value: Amount, input: Vec<u8> },
    #[document(
        summary = "Registers, renews, transfers or changes a name, such as alice.dev, in the chain's name registry: on a chain whose genesis has names.",
        params = {
            name = "The name, without the chain's suffix: alice, or pay.alice for a subname.",
            action = "What to do with it.",
        },
        see_also = "NameAction, Names, NamesConfig",
    )]
    #[wincode(tag = 15)]
    Name { name: String, action: NameAction },
    #[document(
        summary = "Sets the sender's primary name, the one reverse lookups give for its address, or clears it. The name must resolve to the sender, and stops being its primary name once it no longer does.",
        params = { name = "The name, without the chain's suffix, or None to clear it." },
        see_also = "Names::primary, Tx::Name",
    )]
    #[wincode(tag = 16)]
    PrimaryName { name: Option<String> },
    #[document(
        summary = "An Ethereum transaction, as MetaMask, Foundry or ethers signed it: its signed bytes as they are, which the EVM runs as Ethereum would, a call or a create. Its SignedTx carries Signature::Ethereum, the key it recovers as the signer, and the nonce and fee offer its bytes say; verify_on checks them against the bytes, and its chain id against this chain's.",
        params = { raw = "The signed bytes, as eth_sendRawTransaction takes them: a legacy transaction under EIP-155, or one of type 1 or 2." },
        see_also = "EthereumTx, SignedTx::from_ethereum",
    )]
    #[wincode(tag = 17)]
    Ethereum { raw: Vec<u8> },
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
    #[document(summary = "No value: Lua's nil, JSON's null. A function that returns nil returns it, and one that is given it sees nil.")]
    #[wincode(tag = 11)]
    Nil,
    #[document(summary = "A signed 256-bit integer.")]
    #[wincode(tag = 12)]
    I256(#[wincode(with = "I256Le")] i256),
}

impl Verifiable for Tx {
    #[document(
        parent = "Tx",
        summary = "Checks every address and argument inside, that a transfer, mint or burn moves more than zero, that a contract call names its function by XXH3-64, and that a name transaction's name and what it sets could be a name's. A contract call may send zero.",
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
            Tx::EvmCall { to, value, .. } => {
                to.verify()?;
                value.verify()
            }
            Tx::Name { name, action } => {
                if name_problem(name).or_else(|| action.problem()).is_some() {
                    return Err(VerifyError::BadName);
                }

                match action {
                    NameAction::Transfer { to } | NameAction::Subname { owner: to, .. } => to.verify(),
                    NameAction::Target { target: Some(target) } => target.verify(),
                    _ => Ok(()),
                }
            }
            Tx::PrimaryName { name: Some(name) } if name_problem(name).is_some() => Err(VerifyError::BadName),
            Tx::PrimaryName { .. } => Ok(()),
            // Its signed bytes are read and checked with the chain in hand, by SignedTx::verify_on.
            Tx::Ethereum { .. } => Ok(()),
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
            Tx::ApproveSigners { signers, .. } => signers.iter().try_for_each(|signer| signer.key.verify()),
            Tx::WithdrawApproval { .. } | Tx::WithdrawChange { .. } => Ok(()),
            // Whether the consensus fits the chain is decided when the transaction runs; an
            // authority's key is checked here.
            Tx::ApproveChange { change: Change::Consensus(consensus), .. } => match consensus {
                crate::consensus::ConsensusConfig::Authority { key, .. } => key.verify(),
                _ => Ok(()),
            },
            // Whether the list is one a chain takes is decided when the transaction runs, and so are
            // name prices and collection standards.
            Tx::ApproveChange { change: Change::Bootnodes(_) | Change::NamePrices(_) | Change::Collections(_) | Change::Federation(_) | Change::Directory(_) | Change::Fees(_) | Change::Reward(_) | Change::Limits(_) | Change::Finality(_), .. } => Ok(()),
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
            | TxCallArgumentType::U256(_)
            | TxCallArgumentType::I256(_)
            | TxCallArgumentType::Nil => Ok(()),
        }
    }
}

impl Hashable for Tx {
    const TAG_LEN: usize = 1;
}

impl TxCallArgumentType {
    #[document(
        parent = "TxCallArgumentType",
        summary = "An argument as written for a person: type:value, such as i64:-5, string:hi, u256:1000, bytes:0x00ff or address:ed25519:0x…, or nil alone. What chainv4 send call and the apps take.",
        returns = "The argument, or why the text is not one.",
    )]
    pub fn parse_typed(text: &str) -> Result<Self, String> {
        let whole = |value: &str| u256::from_str_radix(value, 10).map_err(|_| format!("{value} is not a whole amount"));

        if text == "nil" {
            return Ok(Self::Nil);
        }

        let (kind, value) = text
            .split_once(':')
            .ok_or_else(|| format!("{text} needs a type, such as i64:{text}"))?;
        let bad = |_| format!("{value} is not a valid {kind}");
        let argument = match kind {
            "address" => Self::Address(value.parse().map_err(|err: crate::structs::address::ParseAddressError| err.to_string())?),
            "amount" => {
                let amount = whole(value)?;

                Self::Amount(match u128::try_from(amount) {
                    Ok(narrow) => Amount::U128(narrow),
                    Err(_) => Amount::U256(amount),
                })
            }
            "bytes" => Self::Bytes(crate::structs::hex::from_hex(value).ok_or_else(|| format!("{value} is not hex"))?),
            "bool" => Self::Bool(value.parse().map_err(|_| format!("{value} is not true or false"))?),
            "string" => Self::String(value.to_string()),
            "number" => Self::Number(value.parse().map_err(|_| format!("{value} is not a number"))?),
            "i64" => Self::I64(value.parse().map_err(bad)?),
            "u64" => Self::U64(value.parse().map_err(bad)?),
            "i128" => Self::I128(value.parse().map_err(bad)?),
            "u128" => Self::U128(value.parse().map_err(bad)?),
            "u256" => Self::U256(whole(value)?),
            "i256" => Self::I256(i256::from_str_radix(value, 10).map_err(bad)?),
            other => {
                return Err(format!(
                    "unknown type {other}; use address, amount, bytes, bool, string, number, i64, u64, i128, u128, i256, u256, or nil alone"
                ));
            }
        };

        Ok(argument)
    }
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
    example = "let signed = SignedTx::sign(tx, accounts.get(&keypair.public().address())?.nonce, FeeOffer::NONE, &keypair);",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct SignedTx {
    #[document(summary = "What to do.")]
    pub tx: Tx,
    #[document(summary = "The sender's nonce at the time; each one is usable once, so the transaction cannot be replayed.")]
    pub nonce: u64,
    #[document(summary = "What the sender offers to pay for it. Ignored on a chain without fees.")]
    pub fee: FeeOffer,
    #[document(summary = "Who signed it. Their address is the sender.")]
    pub signer: PublicKey,
    #[document(summary = "The signer's signature over the transaction, nonce, fee offer and signer.")]
    pub signature: Signature,
}

#[document(
    summary = "What a sender offers to pay for a transaction: the most gas it may use, and the most it pays for each. It pays for the gas it uses, and gets the rest back.",
    category = "Transactions",
    see_also = "SignedTx, FeeConfig",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct FeeOffer {
    #[document(summary = "The most gas the transaction may use. Its fee is reserved from the sender up front, at max_price, and what it does not use is refunded.")]
    pub gas_limit: u64,
    #[document(summary = "The most the sender pays per gas: the whole price when senders bid, the cap on base price plus tip under a base fee.")]
    #[wincode(with = "U256Le")]
    pub max_price: u256,
    #[document(summary = "Under a base fee, what the sender adds per gas for the producer, as far as max_price allows. Ignored when senders bid.")]
    #[wincode(with = "U256Le")]
    pub tip: u256,
}

impl Hashable for FeeOffer {}

impl FeeOffer {
    #[document(parent = "FeeOffer", summary = "No offer, for a chain without fees.")]
    pub const NONE: FeeOffer = FeeOffer {
        gas_limit: 0,
        max_price: u256::ZERO,
        tip: u256::ZERO,
    };

    #[document(
        parent = "FeeOffer",
        summary = "The most the transaction can cost: its gas limit at its max price. None when that passes the largest u256.",
    )]
    pub fn reserve(&self) -> Option<u256> {
        u256::from(self.gas_limit).checked_mul(self.max_price)
    }
}

impl SignedTx {
    #[document(
        parent = "SignedTx",
        summary = "Signs a transaction.",
        params = {
            tx = "The transaction.",
            nonce = "The sender's current nonce.",
            fee = "What the sender offers to pay; FeeOffer::NONE on a chain without fees.",
            keypair = "The sender's keys.",
        },
    )]
    pub fn sign(tx: Tx, nonce: u64, fee: FeeOffer, keypair: &Keypair) -> SignedTx {
        let signer = keypair.public();
        let signature = keypair.sign(&signing_bytes(&tx, nonce, &fee, &signer));

        SignedTx {
            tx,
            nonce,
            fee,
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

    #[document(
        parent = "SignedTx",
        summary = "Its leaf in a block's tx_root: its whole encoding, hashed as a Merkle leaf.",
        params = { hash = "The chain's hash, from genesis." },
        see_also = "Block::tx_root, leaf",
    )]
    pub fn leaf(&self, hash: crate::structs::hash::HashType) -> Hash {
        crate::crypto::merkle::leaf(hash, &self.hashable_bytes_with_tag())
    }
}

fn signing_bytes(tx: &Tx, nonce: u64, fee: &FeeOffer, signer: &PublicKey) -> Vec<u8> {
    let mut bytes = SIGNING_DOMAIN.to_vec();
    bytes.extend_from_slice(&tx.hashable_bytes_with_tag());
    bytes.extend_from_slice(&nonce.to_le_bytes());
    bytes.extend_from_slice(&fee.hashable_bytes_with_tag());
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
        // An Ethereum-signed transaction holds only with its chain, as verify_on checks it.
        if matches!(self.tx, Tx::Ethereum { .. }) {
            return Err(VerifyError::BadEthereum);
        }

        self.tx.verify()?;
        self.signer.verify()?;
        self.signer.verify_signature(
            &signing_bytes(&self.tx, self.nonce, &self.fee, &self.signer),
            &self.signature,
        )?;

        self.check_sender()
    }
}

impl SignedTx {
    #[document(
        parent = "SignedTx",
        summary = "Checks it as verify does, on a chain: chainv4's own signatures as verify checks them, and an EIP-712 one over the transaction's typed hash under this chain's domain (crypto::eip712), so one signed for another chain does not hold here. Every node's check of a transaction is this one.",
        params = { genesis = "The chain's genesis id, which an EIP-712 signature is bound to." },
    )]
    pub fn verify_on(&self, genesis: &crate::structs::hash::Hash) -> Result<(), VerifyError> {
        #[cfg(feature = "secp256k1")]
        if let (Signature::Ethereum, Tx::Ethereum { raw }) = (&self.signature, &self.tx) {
            // What the bytes say, signer and all, must be what the transaction says, on this chain.
            let held = SignedTx::from_ethereum(raw.clone()).map_err(|_| VerifyError::BadEthereum)?;
            let ethereum = crate::crypto::ethereum::decode(raw).map_err(|_| VerifyError::BadEthereum)?;

            if held != *self || ethereum.chain_id != crate::crypto::ethereum::chain_id(genesis) {
                return Err(VerifyError::BadEthereum);
            }

            return self.signer.verify();
        }

        #[cfg(feature = "secp256k1")]
        if let Signature::Secp256k1Eip712(signature) = &self.signature {
            self.tx.verify()?;
            self.signer.verify()?;
            let key = match &self.signer {
                PublicKey::Secp256k1(key) => key,
                #[allow(unreachable_patterns)]
                _ => return Err(VerifyError::InvalidSignature),
            };
            let digest = crate::crypto::eip712::transaction_digest(&self.tx, self.nonce, &self.fee, genesis);

            if !crate::crypto::secp256k1::verify_hash(key, &digest, signature) {
                return Err(VerifyError::InvalidSignature);
            }

            return self.check_sender();
        }

        let _ = genesis;

        self.verify()
    }

    #[document(
        parent = "SignedTx",
        summary = "Signs a transaction in its EIP-712 typed form for this chain, as MetaMask or a hardware wallet signs it: what chainv4 send --eip712 does with a key of its own.",
        params = {
            tx = "What to do.",
            nonce = "The sender's next nonce.",
            fee = "What the sender offers to pay.",
            keypair = "The sender's keys: secp256k1, Ethereum's, alone sign typed data.",
            genesis = "The chain's genesis id, which the signature is bound to.",
        },
        returns = "The signed transaction, or None for a key of another scheme.",
    )]
    #[cfg(feature = "secp256k1")]
    pub fn sign_eip712(tx: Tx, nonce: u64, fee: FeeOffer, keypair: &Keypair, genesis: &crate::structs::hash::Hash) -> Option<SignedTx> {
        let secret = match keypair.private() {
            crate::crypto::keypair::PrivateKey::Secp256k1(secret) => secret,
            #[allow(unreachable_patterns)]
            _ => return None,
        };
        let digest = crate::crypto::eip712::transaction_digest(&tx, nonce, &fee, genesis);

        Some(SignedTx {
            tx,
            nonce,
            fee,
            signer: keypair.public(),
            signature: Signature::Secp256k1Eip712(crate::crypto::secp256k1::sign_hash(secret, &digest)),
        })
    }

    #[document(
        parent = "SignedTx",
        summary = "An Ethereum transaction as chainv4 carries it, from its signed bytes: Tx::Ethereum with them, the nonce and fee offer they say, the key their signature recovers as the signer, and Signature::Ethereum. An EIP-1559 one offers its max fee per gas and its priority fee as the tip; the others, their gas price as both.",
        returns = "The transaction, or why the bytes are not one: they do not read, or their signature recovers no key.",
        see_also = "Tx::Ethereum, EthereumTx",
    )]
    #[cfg(feature = "secp256k1")]
    pub fn from_ethereum(raw: Vec<u8>) -> Result<SignedTx, String> {
        let ethereum = crate::crypto::ethereum::decode(&raw)?;
        let key = ethereum.signer().ok_or("its signature recovers no key, or has a high s")?;

        Ok(SignedTx {
            tx: Tx::Ethereum { raw },
            nonce: ethereum.nonce,
            fee: FeeOffer {
                gas_limit: ethereum.gas_limit,
                max_price: ethereum.max_price,
                tip: ethereum.tip,
            },
            signer: PublicKey::Secp256k1(key),
            signature: Signature::Ethereum,
        })
    }

    /// Whether a transfer or burn takes from its sender alone.
    fn check_sender(&self) -> Result<(), VerifyError> {
        match &self.tx {
            Tx::Transfer { from, .. } | Tx::Burn { from, .. } if *from != self.sender() => Err(VerifyError::NotSender),
            _ => Ok(()),
        }
    }
}

impl Hashable for SignedTx {}

