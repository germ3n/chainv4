//! A field added to a hashed type after chains launched, encoding as nothing while unset.

use {
    r#macro::document,
    std::mem::MaybeUninit,
    wincode::config::{Config, ConfigCore},
    wincode::io::{Reader, Writer},
    wincode::{ReadResult, SchemaRead, SchemaWrite, WriteResult},
};

#[document(
    summary = "Encodes a field added after chains launched to a type whose encoding is hashed: as nothing at all while it is None, so a value without it encodes and hashes as it did before the field existed, and as Option's encoding, a 1 and the value, once it is Some. Only for a type's last fields, read by none of its readers but a trailing one: decoding takes whatever bytes follow as the field, so it can only be the last thing in a message.",
    category = "Chain",
    see_also = "Genesis::max_block_bytes, Genesis::pow_window, LightParams",
)]
pub struct Unset;

// SAFETY: size_of is exactly what write writes: nothing for None, nine bytes for Some.
unsafe impl<C: ConfigCore> SchemaWrite<C> for Unset {
    type Src = Option<u64>;

    fn size_of(src: &Option<u64>) -> WriteResult<usize> {
        Ok(if src.is_some() { 9 } else { 0 })
    }

    fn write(writer: impl Writer, src: &Option<u64>) -> WriteResult<()> {
        let Some(value) = src else {
            return Ok(());
        };
        let mut bytes = [1; 9];
        bytes[1..].copy_from_slice(&value.to_le_bytes());

        <[u8; 9] as SchemaWrite<C>>::write(writer, &bytes)
    }
}

// SAFETY: read initializes dst before it returns Ok. An exhausted reader is None, which is how an
// encoding from before the field reads; so this only ever ends a message.
unsafe impl<'de, C: ConfigCore> SchemaRead<'de, C> for Unset {
    type Dst = Option<u64>;

    fn read(mut reader: impl Reader<'de>, dst: &mut MaybeUninit<Option<u64>>) -> ReadResult<()> {
        let value = match reader.take_byte() {
            Ok(1) => Some(u64::from_le_bytes(reader.take_array::<8>()?)),
            Ok(_) => None,
            Err(_) => None,
        };
        dst.write(value);

        Ok(())
    }
}

#[document(
    summary = "Encodes a list of strings added after chains launched to a hashed type: as nothing at all while empty, so a value without it hashes as before, and as a list once it holds anything. As Unset, only for a type's last field, read by nothing but a trailing reader.",
    category = "Chain",
    see_also = "Unset, ContractScorer::kinds",
)]
pub struct UnsetList;

// SAFETY: size_of is exactly what write writes: nothing when empty, the list's encoding otherwise.
unsafe impl<C: Config> SchemaWrite<C> for UnsetList {
    type Src = Vec<String>;

    fn size_of(src: &Vec<String>) -> WriteResult<usize> {
        match src.is_empty() {
            true => Ok(0),
            false => <Vec<String> as SchemaWrite<C>>::size_of(src),
        }
    }

    fn write(writer: impl Writer, src: &Vec<String>) -> WriteResult<()> {
        match src.is_empty() {
            true => Ok(()),
            false => <Vec<String> as SchemaWrite<C>>::write(writer, src),
        }
    }
}

// SAFETY: read initializes dst before it returns Ok. An exhausted reader is the empty list, which
// is how an encoding from before the field reads; so this only ever ends a message.
unsafe impl<'de, C: Config> SchemaRead<'de, C> for UnsetList {
    type Dst = Vec<String>;

    fn read(reader: impl Reader<'de>, dst: &mut MaybeUninit<Vec<String>>) -> ReadResult<()> {
        dst.write(<Vec<String> as SchemaRead<'de, C>>::get(reader).unwrap_or_default());

        Ok(())
    }
}

#[document(
    summary = "Encodes any optional value added after chains launched to a type that is hashed but never decoded, such as Genesis: as nothing at all while None, and as a 1 and the value once Some. As Unset, only for a type's last fields.",
    category = "Chain",
    see_also = "Unset, Genesis::names",
)]
pub struct UnsetOption<T>(std::marker::PhantomData<T>);

// SAFETY: size_of is exactly what write writes: nothing for None, a byte and the encoding for Some.
unsafe impl<C: Config, T: SchemaWrite<C, Src = T>> SchemaWrite<C> for UnsetOption<T> {
    type Src = Option<T>;

    fn size_of(src: &Option<T>) -> WriteResult<usize> {
        match src {
            Some(value) => Ok(1 + T::size_of(value)?),
            None => Ok(0),
        }
    }

    fn write(mut writer: impl Writer, src: &Option<T>) -> WriteResult<()> {
        let Some(value) = src else {
            return Ok(());
        };
        <[u8; 1] as SchemaWrite<C>>::write(&mut writer, &[1])?;

        T::write(writer, value)
    }
}

#[document(
    summary = "Encodes a value framed: its encoding's length, eight bytes little-endian, then the encoding, read back as a message of its own. For a type whose last fields are Unset ones, which read only at a message's end, carried in the middle of another: the finality config a governance change names.",
    category = "Chain",
    see_also = "Unset, Change::Finality",
)]
pub struct Framed<T>(std::marker::PhantomData<T>);

// SAFETY: size_of is exactly what write writes: the eight-byte length, then the encoding.
unsafe impl<C: Config, T: SchemaWrite<C, Src = T>> SchemaWrite<C> for Framed<T> {
    type Src = T;

    fn size_of(src: &T) -> WriteResult<usize> {
        Ok(8 + T::size_of(src)?)
    }

    fn write(mut writer: impl Writer, src: &T) -> WriteResult<()> {
        let len = T::size_of(src)? as u64;
        <[u8; 8] as SchemaWrite<C>>::write(&mut writer, &len.to_le_bytes())?;

        T::write(writer, src)
    }
}

// SAFETY: read initializes dst before it returns Ok, from the framed bytes alone, read to their end.
unsafe impl<'de, C: Config, T> SchemaRead<'de, C> for Framed<T>
where
    T: for<'a> SchemaRead<'a, C, Dst = T>,
{
    type Dst = T;

    fn read(mut reader: impl Reader<'de>, dst: &mut MaybeUninit<T>) -> ReadResult<()> {
        let len = u64::from_le_bytes(reader.take_array::<8>()?) as usize;
        let framed = reader.take_scoped(len)?;
        dst.write(T::get(framed)?);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value whose last field reads only at a message's end, as FfgConfig's leak does.
    #[derive(SchemaWrite, SchemaRead, PartialEq, Debug)]
    struct Trailing {
        first: u32,
        #[wincode(with = "Unset")]
        last: Option<u64>,
    }

    #[derive(SchemaWrite, SchemaRead, PartialEq, Debug)]
    struct Carrying {
        #[wincode(with = "Framed<Trailing>")]
        inner: Trailing,
        after: u32,
    }

    #[test]
    fn a_framed_value_reads_back_in_the_middle_of_another() {
        for last in [None, Some(9)] {
            let carrying = Carrying { inner: Trailing { first: 1, last }, after: 7 };

            assert_eq!(wincode::deserialize::<Carrying>(&wincode::serialize(&carrying).unwrap()).unwrap(), carrying);
        }
    }

    #[derive(SchemaWrite, SchemaRead, PartialEq, Debug)]
    struct Listed {
        first: u32,
        #[wincode(with = "UnsetList")]
        added: Vec<String>,
    }

    #[test]
    fn an_unset_list_encodes_as_nothing_while_empty() {
        let before = wincode::serialize(&7u32).unwrap();
        let without = Listed { first: 7, added: Vec::new() };
        let with = Listed { first: 7, added: vec!["call".to_string()] };

        assert_eq!(wincode::serialize(&without).unwrap(), before);
        assert_eq!(wincode::deserialize::<Listed>(&before).unwrap(), without);
        assert_eq!(wincode::deserialize::<Listed>(&wincode::serialize(&with).unwrap()).unwrap(), with);
    }

    #[derive(SchemaWrite)]
    struct Optional {
        first: u32,
        #[wincode(with = "UnsetOption<String>")]
        added: Option<String>,
    }

    #[test]
    fn an_unset_option_encodes_as_nothing_while_none_and_as_a_one_and_the_value_once_set() {
        let before = wincode::serialize(&7u32).unwrap();
        let with = wincode::serialize(&Optional { first: 7, added: Some("dev".to_string()) }).unwrap();

        assert_eq!(wincode::serialize(&Optional { first: 7, added: None }).unwrap(), before);
        assert_eq!(with, [before, vec![1], wincode::serialize(&"dev".to_string()).unwrap()].concat());
    }

    #[derive(SchemaWrite, SchemaRead, PartialEq, Debug)]
    struct Tail {
        first: u32,
        #[wincode(with = "Unset")]
        added: Option<u64>,
    }

    #[test]
    fn unset_encodes_as_nothing_and_reads_back_either_way() {
        let before = wincode::serialize(&7u32).unwrap();
        let without = Tail { first: 7, added: None };
        let with = Tail { first: 7, added: Some(300) };

        assert_eq!(wincode::serialize(&without).unwrap(), before, "as it was before the field");
        assert_eq!(wincode::serialize(&with).unwrap().len(), before.len() + 9);
        assert_eq!(wincode::deserialize::<Tail>(&before).unwrap(), without);
        assert_eq!(wincode::deserialize::<Tail>(&wincode::serialize(&with).unwrap()).unwrap(), with);
    }
}
