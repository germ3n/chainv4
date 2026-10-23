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

/// The chain's 256-bit unsigned integer. Everything imports it from here, so the implementation
/// can change in one place.
pub use ethnum::u256;

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
