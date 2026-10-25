use {
    r#macro::document,
    std::mem::MaybeUninit,
    wincode::ReadResult,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    wincode::WriteResult,
    wincode::config::ConfigCore,
    wincode::io::Reader,
    wincode::io::Writer,
};

/// The chain's 256-bit integers, unsigned and signed. Everything imports them from here, so the
/// implementation can change in one place.
pub use ethnum::{i256, u256};

#[document(
    summary = "Encodes a u256 as its 32 bytes, little-endian, the way wincode writes a u128 as 16. Put #[wincode(with = \"U256Le\")] on any u256 field of a type that derives SchemaWrite or SchemaRead.",
    category = "Primitives",
    see_also = "Amount::U256",
)]
pub struct U256Le;

// SAFETY: both methods defer to the [u8; 32] schema, so size_of always matches what write writes.
unsafe impl<C: ConfigCore> SchemaWrite<C> for U256Le {
    type Src = u256;

    fn size_of(src: &u256) -> WriteResult<usize> {
        <[u8; 32] as SchemaWrite<C>>::size_of(&src.to_le_bytes())
    }

    fn write(writer: impl Writer, src: &u256) -> WriteResult<()> {
        <[u8; 32] as SchemaWrite<C>>::write(writer, &src.to_le_bytes())
    }
}

// SAFETY: read initializes dst before it returns Ok.
unsafe impl<'de, C: ConfigCore> SchemaRead<'de, C> for U256Le {
    type Dst = u256;

    fn read(reader: impl Reader<'de>, dst: &mut MaybeUninit<u256>) -> ReadResult<()> {
        let bytes = <[u8; 32] as SchemaRead<'de, C>>::get(reader)?;
        dst.write(u256::from_le_bytes(bytes));

        Ok(())
    }
}

#[document(
    summary = "Encodes an i256 as its 32 bytes, two's complement and little-endian, the way wincode writes an i128 as 16. Put #[wincode(with = \"I256Le\")] on any i256 field of a type that derives SchemaWrite or SchemaRead.",
    category = "Primitives",
    see_also = "TxCallArgumentType::I256",
)]
pub struct I256Le;

// SAFETY: both methods defer to the [u8; 32] schema, so size_of always matches what write writes.
unsafe impl<C: ConfigCore> SchemaWrite<C> for I256Le {
    type Src = i256;

    fn size_of(src: &i256) -> WriteResult<usize> {
        <[u8; 32] as SchemaWrite<C>>::size_of(&src.to_le_bytes())
    }

    fn write(writer: impl Writer, src: &i256) -> WriteResult<()> {
        <[u8; 32] as SchemaWrite<C>>::write(writer, &src.to_le_bytes())
    }
}

// SAFETY: read initializes dst before it returns Ok.
unsafe impl<'de, C: ConfigCore> SchemaRead<'de, C> for I256Le {
    type Dst = i256;

    fn read(reader: impl Reader<'de>, dst: &mut MaybeUninit<i256>) -> ReadResult<()> {
        let bytes = <[u8; 32] as SchemaRead<'de, C>>::get(reader)?;
        dst.write(i256::from_le_bytes(bytes));

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {
        super::u256,
        crate::crypto::hashable::Hashable,
        crate::structs::amount::Amount,
    };

    #[test]
    fn encodes_as_32_little_endian_bytes() {
        let mut expected = vec![1]; // Amount: U256
        expected.extend_from_slice(&[2, 1]);
        expected.extend_from_slice(&[0; 30]);

        assert_eq!(Amount::U256(u256::new(258)).hashable_bytes_with_tag(), expected);

        let mut max = vec![1];
        max.extend_from_slice(&[0xff; 32]);

        assert_eq!(Amount::U256(u256::MAX).hashable_bytes_with_tag(), max);
    }

    #[test]
    fn i256_arguments_encode_as_32_twos_complement_bytes_on_tag_12() {
        use {super::i256, crate::structs::tx::TxCallArgumentType};

        let mut expected = vec![12, 0xfb];
        expected.extend_from_slice(&[0xff; 31]);

        // what the SDK's Value::I256 encodes, byte for byte
        assert_eq!(wincode::serialize(&TxCallArgumentType::I256(i256::new(-5))).unwrap(), expected);

        for value in [i256::MIN, i256::MAX, i256::ZERO, i256::new(-1)] {
            let bytes = wincode::serialize(&TxCallArgumentType::I256(value)).unwrap();

            assert_eq!(bytes.len(), 33);
            assert_eq!(wincode::deserialize::<TxCallArgumentType>(&bytes).unwrap(), TxCallArgumentType::I256(value));
        }
    }

    #[test]
    fn round_trips() {
        let value = u256::MAX - u256::new(u128::MAX) * u256::new(3);
        let bytes = Amount::U256(value).hashable_bytes_with_tag();
        let decoded: Amount = wincode::deserialize(&bytes).unwrap();

        match decoded {
            Amount::U256(decoded) => assert_eq!(decoded, value),
            Amount::U128(_) => panic!("decoded the wrong variant"),
        }
    }
}
