//! `chainv4 bridge refund`: a transfer refused for good at its destination, back to who sent it. The
//! order is what keeps it from paying twice: the federation signs to void it where it was going, which
//! takes its nonce there for good, and only once its signers read that, final, does it sign to refund
//! it where it left. Nothing moves on an operator's word alone: each signer's tool reads both chains
//! itself before it signs, and it takes the federation's threshold. Each run does the next step.

use {
    super::{Bridge, btc::raw, calldata},
    crate::abi::{AbiType, AbiValue, decode_values},
    crate::bridge::{AssetKind, Transfer, refund_input},
    crate::cli::CliResult,
    crate::cli::send::typed_yes,
    crate::cli::swap::rpc::Rpc,
    crate::structs::address::Address,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::json::argument,
    crate::structs::u256::u256,
    crate::swap::evm::{Call, parse_address},
    serde_json::{Value, json},
};

/// What both chains say of a transfer, each read for itself: what a signer goes by.
struct Standing {
    transfer: Transfer,
    /// Whom its source gateway says sent it, as that chain writes an address: what a refund signs.
    sender: Vec<u8>,
    /// Who a refund pays: that, or where they said, shown as that chain shows an address.
    pays: String,
    /// It left in a block that is final there, so it will still have left.
    final_at_source: bool,
    delivered: bool,
    /// It was voided at its destination, and whether that block is final there.
    voided: Option<bool>,
    refunded: bool,
}

/// The next step for a transfer, from what both chains say.
#[derive(Debug, PartialEq, Eq)]
enum RefundStep {
    /// Refunded: nothing to do.
    Done,
    /// It arrived, so there is nothing to refund.
    Delivered,
    /// It has not left in a block that is final: a reorganization could yet take it away.
    NotFinal,
    Void,
    /// Voided, but in a block that could yet be undone, when a delivery could then land.
    VoidNotFinal,
    Refund,
}

impl Standing {
    fn step(&self) -> RefundStep {
        match (self.refunded, self.delivered, self.voided) {
            (true, _, _) => RefundStep::Done,
            (_, true, _) => RefundStep::Delivered,
            _ if !self.final_at_source => RefundStep::NotFinal,
            (_, _, Some(true)) => RefundStep::Refund,
            (_, _, Some(false)) => RefundStep::VoidNotFinal,
            (_, _, None) => RefundStep::Void,
        }
    }
}

/// What a transfer moves, as a person reads it.
fn what_moves(transfer: &Transfer) -> String {
    let symbol = if transfer.symbol.is_empty() { "the asset" } else { transfer.symbol.as_str() };

    match transfer.kind {
        AssetKind::Coin | AssetKind::Token => format!("{} of {symbol}, in its smallest units", transfer.amount),
        AssetKind::Nft => format!("NFT {} of {symbol}", transfer.id),
        AssetKind::Multi => format!("{} of id {} of {symbol}", transfer.amount, transfer.id),
    }
}

impl Bridge {
    /// The height that is final here: at or below the finalized checkpoint, or chain_depth deep.
    fn safe_here(&self) -> CliResult<u64> {
        let status = self.status()?;
        let tip = status["height"].as_u64().unwrap_or(0);

        Ok(match status["finality"]["finalized"]["height"].as_u64() {
            Some(finalized) => finalized,
            None => tip.saturating_sub(status["federation"]["chain_depth"].as_u64().unwrap_or(20)),
        })
    }

    /// The block that is final on an EVM chain: its RPC's finalized one, or evm_depth deep.
    fn safe_there(&self, rpc: &Rpc) -> CliResult<u64> {
        let (tip, finalized) = rpc.heads()?;

        Ok(finalized.unwrap_or_else(|| tip.saturating_sub(self.status().ok().and_then(|status| status["federation"]["evm_depth"].as_u64()).unwrap_or(12))))
    }

    /// One value a contract on an EVM chain answers.
    fn evm_word(&self, rpc: &Rpc, at: &[u8; 20], signature: &str, value: AbiValue, returns: &[AbiType]) -> CliResult<Vec<AbiValue>> {
        Ok(decode_values(returns, &rpc.call(at, &calldata(signature, &[value])?)?)?)
    }

    /// A transfer that left here for this EVM chain, against what that chain's gateway says. The gateway's
    /// own digest must be the one worked out from the gateway the directory names: if its owner repointed
    /// its peer since, this is about some other contract's word, and no signer takes that.
    #[allow(clippy::too_many_arguments)]
    fn standing_here(&self, gateway: &Address, evm_gateway: &[u8; 20], rpc: &Rpc, identity: &[u8], here: u64, chain: u64, nonce: u64) -> CliResult<Standing> {
        let values = self.read(gateway, "sent", json!([{ "type": "u64", "value": nonce.to_string() }]))?;

        if values.first().is_none_or(|value| value["value"].is_null()) {
            return Err(format!("no transfer {nonce} left the gateway here").into());
        }

        let transfer = self.left_here(gateway, identity, here, nonce)?;

        if transfer.target != chain {
            return Err(format!("transfer {nonce} went to chain {}, and --eth-rpc is chain {chain}", transfer.target).into());
        }

        if values.get(11).map(raw).as_deref() != Some(transfer.digest().as_slice()) {
            return Err(format!("transfer {nonce} did not leave for the gateway named on chain {chain}: it stays where it is until its peer is that one again").into());
        }

        let sender: Address = values.get(12).and_then(|value| value["value"].as_str()).and_then(|text| text.parse().ok()).ok_or("the gateway does not say who sent it")?;
        let left_at = values.get(13).and_then(|value| value["value"].as_str()).and_then(|text| text.parse::<u64>().ok()).ok_or("the gateway does not say when it left")?;
        let refund = self.read(gateway, "refund_of", json!([{ "type": "u64", "value": nonce.to_string() }]))?;
        let key = AbiValue::FixedBytes(transfer.delivered_key());
        let delivered = self.evm_word(rpc, evm_gateway, "delivered(bytes32)", key.clone(), &[AbiType::Bool])?[0] == AbiValue::Bool(true);
        let voided_at = self.evm_word(rpc, evm_gateway, "voidedAt(bytes32)", key, &[AbiType::Uint(256)])?;
        let voided_at = match voided_at[0] {
            AbiValue::Uint(height) if height != u256::ZERO => Some(u64::try_from(height).map_err(|_| "a height past 64 bits")?),
            _ => None,
        };
        let safe_there = self.safe_there(rpc)?;

        Ok(Standing {
            sender: sender.to_abi().to_vec(),
            pays: refund.first().and_then(|value| value["value"].as_str()).unwrap_or_default().to_string(),
            final_at_source: left_at <= self.safe_here()?,
            delivered,
            voided: voided_at.map(|height| height <= safe_there),
            refunded: refund.get(1).is_some_and(|value| value["value"] == true),
            transfer,
        })
    }

    /// A transfer that left this EVM chain for here, against what the gateway here says.
    #[allow(clippy::too_many_arguments)]
    fn standing_there(&self, gateway: &Address, evm_gateway: &[u8; 20], rpc: &Rpc, identity: &[u8], here: u64, chain: u64, nonce: u64) -> CliResult<Standing> {
        let (_, types) = crate::abi::parse_signature(&format!("({})", crate::bridge::TRANSFER_TUPLE))?;
        let transfer = decode_values(&types, &rpc.call(evm_gateway, &calldata("sent(uint64)", &[AbiValue::Uint(u256::from(nonce))])?)?).map_err(|err| format!("Gateway.sol's sent answered what does not read: {err}"))?;
        let transfer = Transfer::from_evm(&transfer[0]).ok_or("Gateway.sol's sent answered what is no transfer")?;

        if transfer.nonce != nonce {
            return Err(format!("no transfer {nonce} left the gateway on chain {chain}").into());
        }

        if !transfer.arrives_at(here, identity, evm_gateway) {
            return Err(format!("transfer {nonce} left for another gateway than this one, as one from before a reset did: it arrives nowhere, and this cannot void it").into());
        }

        let left = self.evm_word(rpc, evm_gateway, "leftBy(uint64)", AbiValue::Uint(u256::from(nonce)), &[AbiType::Address, AbiType::Uint(64)])?;
        let [AbiValue::Address(sender), AbiValue::Uint(left_at)] = left[..] else {
            return Err("Gateway.sol does not say who sent it".into());
        };
        let refund_to = self.evm_word(rpc, evm_gateway, "refundTo(uint64)", AbiValue::Uint(u256::from(nonce)), &[AbiType::Address])?;
        let refunded = self.evm_word(rpc, evm_gateway, "refunded(uint64)", AbiValue::Uint(u256::from(nonce)), &[AbiType::Bool])?[0] == AbiValue::Bool(true);
        let at = json!([{ "type": "u64", "value": chain.to_string() }, { "type": "u64", "value": nonce.to_string() }]);
        let delivered = self.read(gateway, "received", at.clone())?.first().is_some_and(|value| value["value"] == true);
        let voided = self.read(gateway, "voided", at)?;
        let voided = match voided.first() {
            Some(digest) if !digest["value"].is_null() => {
                if raw(digest) != transfer.digest() {
                    return Err(format!("the gateway here voided another transfer at nonce {nonce} from chain {chain}").into());
                }

                let height = voided.get(1).and_then(|value| value["value"].as_str()).and_then(|text| text.parse::<u64>().ok()).ok_or("the gateway does not say when it was voided")?;

                Some(height <= self.safe_here()?)
            }
            _ => None,
        };
        let pays = match refund_to[0] {
            AbiValue::Address(named) if named != [0; 20] => named,
            _ => sender,
        };

        Ok(Standing {
            sender: sender.to_vec(),
            pays: to_hex(&pays),
            final_at_source: u64::try_from(left_at).map_err(|_| "a height past 64 bits")? <= self.safe_there(rpc)?,
            delivered,
            voided,
            refunded,
            transfer,
        })
    }

    /// Why it does not arrive, as far as a chain will say: the refusal a delivery gets if it were tried
    /// now, or that there are not the signatures to try. An error if it would arrive, as then it should
    /// just be delivered, and if the chain could not be asked, which says nothing about it.
    fn why_refused(&self, gateway: &Address, evm_gateway: &[u8; 20], rpc: &Rpc, transfer: &Transfer, from_evm: bool, secret: Option<&[u8; 32]>) -> CliResult<String> {
        let posted = self.read(gateway, "signatures", json!([{ "type": "bytes", "value": to_hex(&transfer.digest()) }]))?;
        let threshold = posted.first().and_then(|value| value["value"].as_str()?.parse::<usize>().ok()).unwrap_or(usize::MAX);
        let signatures: Vec<[u8; 65]> = posted.iter().skip(1).filter_map(|value| value["value"].as_str().and_then(from_hex)?.try_into().ok()).collect();
        let short = format!("the board holds {} of the {threshold} signatures a delivery takes, so none can be tried", signatures.len());

        let (answer, refusal): (Result<(), String>, &str) = match from_evm {
            false if signatures.len() < threshold => return Ok(short),
            false => (rpc.call(evm_gateway, &transfer.deliver_input(&signatures)).map(|_| ()).map_err(|err| err.to_string()), "eth_call:"),
            // This signer's own signature is what would take it to the threshold here, on a throwaway
            // copy of the state.
            true => {
                let Some(secret) = secret.filter(|_| signatures.len() + 1 >= threshold) else {
                    return Ok(short);
                };
                let args: Vec<Value> = transfer.attest_args(&transfer.sign(secret)).iter().map(argument).collect();

                (self.read(gateway, "attest", json!(args)).map(|_| ()).map_err(|err| err.to_string()), "attest:")
            }
        };

        match answer {
            Ok(()) => Err(format!("transfer {} would be taken at its destination now: deliver it, rather than refund it", transfer.nonce).into()),
            Err(why) if why.starts_with(refusal) => Ok(format!("its destination refuses it: {why}")),
            Err(why) => Err(format!("its destination could not be asked: {why}").into()),
        }
    }

    /// A step's signature, on the board here unless it is already, and what the board then holds, if
    /// that is enough for a relayer to take over there.
    fn post_step(&self, gateway: &Address, digest: &[u8; 32], secret: Option<&[u8; 32]>, function: &str, args: impl FnOnce(&[u8; 65]) -> Vec<crate::structs::tx::TxCallArgumentType>) -> CliResult<Option<Vec<[u8; 65]>>> {
        let read = || self.read(gateway, "signatures", json!([{ "type": "bytes", "value": to_hex(digest) }]));

        if let Some(secret) = secret {
            let signature = crate::bridge::sign_digest(secret, digest);

            if !read()?.iter().skip(1).any(|value| value["value"].as_str() == Some(&to_hex(&signature))) {
                self.call(*gateway, function, u256::ZERO, args(&signature))?;
            }
        }

        let posted = read()?;
        let threshold = posted.first().and_then(|value| value["value"].as_str()?.parse::<usize>().ok()).unwrap_or(usize::MAX);
        let signatures: Vec<[u8; 65]> = posted.iter().skip(1).filter_map(|value| value["value"].as_str().and_then(from_hex)?.try_into().ok()).collect();
        println!("the board holds {} of the {threshold} signatures it takes", signatures.len());

        Ok((signatures.len() >= threshold).then_some(signatures))
    }

    /// The next step of a refund, on what this tool reads for itself. Only a signer's --evm-key signs;
    /// anyone else's only takes what the board has enough of over to the EVM chain, paying its gas.
    pub(crate) fn refund(&self, nonce: u64, from_evm: bool, why: Option<&str>, yes: bool, input: &mut dyn std::io::BufRead) -> CliResult<()> {
        let rpc = self.rpc()?;
        let gateway = self.gateway()?;
        let (here, chain) = (self.here()?, rpc.chain_id()?);
        let evm_gateway = self.evm_gateway(&gateway, chain)?;
        let identity = self.identity(&gateway)?;
        let evm_secret = self.evm_secret()?;
        let secret = self.is_signer(&evm_secret)?.then_some(evm_secret);
        let standing = match from_evm {
            false => self.standing_here(&gateway, &evm_gateway, &rpc, &identity, here, chain, nonce)?,
            true => self.standing_there(&gateway, &evm_gateway, &rpc, &identity, here, chain, nonce)?,
        };
        let transfer = &standing.transfer;
        let path = format!("transfer {nonce}, from chain {} to chain {}", transfer.source, transfer.target);

        match standing.step() {
            RefundStep::Done => {
                println!("{path} was refunded already");

                Ok(())
            }
            RefundStep::Delivered => Err(format!("{path} was delivered, so there is nothing to refund").into()),
            RefundStep::NotFinal => Err(format!("{path} is not final where it left yet: try again once it is").into()),
            RefundStep::VoidNotFinal => {
                println!("{path} is voided, in a block not yet final there: run this again once it is, to refund it");

                Ok(())
            }
            RefundStep::Void => {
                let refused = self.why_refused(&gateway, &evm_gateway, &rpc, transfer, from_evm, secret.as_ref())?;
                self.preview(&path, "void it at its destination, so it can never arrive", transfer, &standing.pays, why, &refused);
                typed_yes("Void this?", yes, input, "nothing is signed")?;
                let enough = self.post_step(&gateway, &transfer.void_digest(), secret.as_ref(), "attest_void", |signature| transfer.attest_void_args(signature))?;

                match (from_evm, enough) {
                    (false, Some(signatures)) => {
                        rpc.send(&Call { to: evm_gateway, value: u256::ZERO, input: transfer.void_input(&signatures), what: "Void a transfer" }, false, &evm_secret)?;
                    }
                    (true, Some(_)) => println!("voided here"),
                    (_, None) => {}
                }

                println!("run this again once it is voided and final there, to refund it");

                Ok(())
            }
            RefundStep::Refund => {
                self.preview(&path, "refund it where it left, as it is voided where it was going", transfer, &standing.pays, why, "it was voided");
                typed_yes("Refund this?", yes, input, "nothing is signed")?;
                let digest = transfer.refund_digest(&standing.sender);
                let enough = self.post_step(&gateway, &digest, secret.as_ref(), "attest_refund", |signature| transfer.attest_refund_args(&standing.sender, signature))?;

                if from_evm && let Some(signatures) = enough {
                    rpc.send(&Call { to: evm_gateway, value: u256::ZERO, input: refund_input(nonce, &signatures), what: "Refund a transfer" }, false, &evm_secret)?;
                }

                Ok(())
            }
        }
    }

    fn preview(&self, path: &str, does: &str, transfer: &Transfer, pays: &str, why: Option<&str>, refused: &str) {
        println!("{path}");
        println!("  this step: {does}");
        println!("  it moved:  {}", what_moves(transfer));
        println!("  it pays:   {pays}, as its source gateway recorded, once refunded");
        println!("  why:       {refused}");

        if let Some(why) = why {
            println!("  and:       {why}, as this operator says");
        }
    }

    /// Names where a refund goes, by who sent it.
    pub(crate) fn refund_to(&self, nonce: u64, recipient: &str, from_evm: bool) -> CliResult<()> {
        let gateway = self.gateway()?;

        match from_evm {
            false => {
                let recipient: Address = recipient.parse().map_err(|_| format!("{recipient} is not an address here"))?;

                self.call(gateway, "set_refund_to", u256::ZERO, vec![crate::structs::tx::TxCallArgumentType::U64(nonce), crate::structs::tx::TxCallArgumentType::Address(recipient)])
            }
            true => {
                let (rpc, secret) = (self.rpc()?, self.evm_secret()?);
                let evm_gateway = self.evm_gateway(&gateway, rpc.chain_id()?)?;
                let input = calldata("setRefundTo(uint64,address)", &[AbiValue::Uint(u256::from(nonce)), AbiValue::Address(parse_address(recipient)?)])?;
                rpc.send(&Call { to: evm_gateway, value: u256::ZERO, input, what: "Name where a refund goes" }, false, &secret)?;

                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing(refunded: bool, delivered: bool, voided: Option<bool>, final_at_source: bool) -> Standing {
        let transfer = Transfer {
            from: vec![1; 33],
            to: vec![2; 20],
            source: 1,
            target: 2,
            nonce: 3,
            kind: AssetKind::Coin,
            origin: 1,
            asset: Vec::new(),
            id: u256::ZERO,
            amount: u256::from(5u64),
            recipient: vec![4; 20],
            name: String::new(),
            symbol: "DEV".to_string(),
            decimals: 18,
            uri: String::new(),
        };

        Standing { transfer, sender: vec![1; 33], pays: String::new(), final_at_source, delivered, voided, refunded }
    }

    #[test]
    fn a_signer_voids_only_what_did_not_arrive_and_refunds_only_what_is_voided_and_final() {
        assert_eq!(standing(false, false, None, false).step(), RefundStep::NotFinal, "it could yet not have left");
        assert_eq!(standing(false, false, Some(true), false).step(), RefundStep::NotFinal, "nor refunded, however it was voided");
        assert_eq!(standing(false, false, None, true).step(), RefundStep::Void);
        assert_eq!(standing(false, true, None, true).step(), RefundStep::Delivered, "what arrived is never voided");
        assert_eq!(standing(false, false, Some(false), true).step(), RefundStep::VoidNotFinal, "a void that could be undone refunds nothing");
        assert_eq!(standing(false, false, Some(true), true).step(), RefundStep::Refund);
        assert_eq!(standing(true, false, Some(true), true).step(), RefundStep::Done, "once");
    }

    #[test]
    fn what_a_refund_shows_says_what_moved() {
        let coin = standing(false, false, None, true).transfer;

        assert_eq!(what_moves(&coin), "5 of DEV, in its smallest units");
        assert_eq!(what_moves(&Transfer { kind: AssetKind::Nft, id: u256::from(7u64), ..coin.clone() }), "NFT 7 of DEV");
        assert_eq!(what_moves(&Transfer { kind: AssetKind::Multi, id: u256::from(7u64), ..coin }), "5 of id 7 of DEV");
    }
}
