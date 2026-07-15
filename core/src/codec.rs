use serde::{de::DeserializeOwned, Serialize};
use std::io::{Read, Write};

pub const LEGACY_BINCODE_FORMAT_NAME: &str = "bincode-1-fixedint-little-endian";
pub const LEGACY_BINCODE_FORMAT_VERSION: u8 = 1;

pub fn serialize_legacy_bincode<T>(value: &T, context: &str) -> Result<Vec<u8>, String>
where
    T: Serialize + ?Sized,
{
    bincode::serialize(value)
        .map_err(|err| format!("{} legacy bincode serialize failed: {}", context, err))
}

pub fn serialize_legacy_bincode_limited<T>(
    value: &T,
    limit: u64,
    context: &str,
) -> Result<Vec<u8>, String>
where
    T: Serialize + ?Sized,
{
    use bincode::Options;

    bincode::options()
        .with_limit(limit)
        .with_fixint_encoding()
        .serialize(value)
        .map_err(|err| format!("{} legacy bincode serialize failed: {}", context, err))
}

pub fn serialize_legacy_bincode_into<W, T>(
    writer: &mut W,
    value: &T,
    limit: u64,
    context: &str,
) -> Result<(), String>
where
    W: Write,
    T: Serialize + ?Sized,
{
    use bincode::Options;

    bincode::options()
        .with_limit(limit)
        .with_fixint_encoding()
        .serialize_into(writer, value)
        .map_err(|err| format!("{} legacy bincode serialize failed: {}", context, err))
}

pub fn serialized_size_legacy_bincode<T>(value: &T, context: &str) -> Result<u64, String>
where
    T: Serialize + ?Sized,
{
    bincode::serialized_size(value)
        .map_err(|err| format!("{} legacy bincode size failed: {}", context, err))
}

pub fn append_legacy_bincode<T>(out: &mut Vec<u8>, value: &T, context: &str) -> Result<(), String>
where
    T: Serialize + ?Sized,
{
    let bytes = serialize_legacy_bincode(value, context)?;
    out.extend_from_slice(&bytes);
    Ok(())
}

pub fn deserialize_legacy_bincode<T>(bytes: &[u8], context: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    deserialize_legacy_bincode_strict(bytes, bytes.len() as u64, context)
}

pub fn deserialize_legacy_bincode_strict<T>(
    bytes: &[u8],
    limit: u64,
    context: &str,
) -> Result<T, String>
where
    T: DeserializeOwned,
{
    use bincode::Options;

    if bytes.len() as u64 > limit {
        return Err(format!(
            "{} legacy bincode payload too large: {} bytes (max {})",
            context,
            bytes.len(),
            limit
        ));
    }

    match std::panic::catch_unwind(|| {
        bincode::options()
            .with_limit(limit)
            .with_fixint_encoding()
            .reject_trailing_bytes()
            .deserialize(bytes)
    }) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(format!(
            "{} legacy bincode deserialize failed: {}",
            context, err
        )),
        Err(_) => Err(format!("{} legacy bincode deserialize panicked", context)),
    }
}

pub fn deserialize_legacy_bincode_from<R, T>(
    reader: &mut R,
    limit: u64,
    context: &str,
) -> Result<T, String>
where
    R: Read,
    T: DeserializeOwned,
{
    use bincode::Options;

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        bincode::options()
            .with_limit(limit)
            .with_fixint_encoding()
            .deserialize_from(reader)
    })) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(format!(
            "{} legacy bincode deserialize failed: {}",
            context, err
        )),
        Err(_) => Err(format!("{} legacy bincode deserialize panicked", context)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Fixture {
        value: u64,
        payload: Vec<u8>,
    }

    #[test]
    fn legacy_bincode_serialize_matches_bincode_1_defaults() {
        let fixture = Fixture {
            value: 42,
            payload: vec![1, 2, 3, 4],
        };

        let wrapped = serialize_legacy_bincode(&fixture, "fixture").unwrap();
        let limited = serialize_legacy_bincode_limited(&fixture, 1024, "fixture").unwrap();
        let mut appended = vec![0xAB];
        append_legacy_bincode(&mut appended, &fixture, "fixture").unwrap();
        let direct = bincode::serialize(&fixture).unwrap();

        assert_eq!(wrapped, direct);
        assert_eq!(limited, direct);
        assert_eq!(&appended[1..], direct.as_slice());
        assert_eq!(
            LEGACY_BINCODE_FORMAT_NAME,
            "bincode-1-fixedint-little-endian"
        );
        assert_eq!(LEGACY_BINCODE_FORMAT_VERSION, 1);
    }

    #[test]
    fn legacy_bincode_strict_deserialize_rejects_trailing_bytes() {
        let fixture = Fixture {
            value: 7,
            payload: vec![9],
        };
        let mut bytes = serialize_legacy_bincode(&fixture, "fixture").unwrap();
        bytes.push(0);

        assert_eq!(
            deserialize_legacy_bincode::<Fixture>(&bytes[..bytes.len() - 1], "fixture").unwrap(),
            fixture
        );

        let err = deserialize_legacy_bincode_strict::<Fixture>(&bytes, 1024, "fixture")
            .expect_err("trailing bytes must be rejected");

        assert!(err.contains("legacy bincode deserialize failed"));
    }

    #[test]
    fn legacy_bincode_strict_deserialize_obeys_limit() {
        let fixture = Fixture {
            value: 7,
            payload: vec![9; 32],
        };
        let bytes = serialize_legacy_bincode(&fixture, "fixture").unwrap();

        let err = deserialize_legacy_bincode_strict::<Fixture>(&bytes, 4, "fixture")
            .expect_err("low byte limit must be rejected");

        assert!(err.contains("legacy bincode payload too large"));
    }

    #[test]
    fn legacy_bincode_stream_helpers_roundtrip_sequential_values() {
        let first = Fixture {
            value: 7,
            payload: vec![1, 2, 3],
        };
        let second = Fixture {
            value: 8,
            payload: vec![4, 5],
        };
        let mut encoded = Vec::new();
        serialize_legacy_bincode_into(&mut encoded, &first, 1024, "first fixture").unwrap();
        serialize_legacy_bincode_into(&mut encoded, &second, 1024, "second fixture").unwrap();

        let mut cursor = std::io::Cursor::new(encoded.as_slice());
        let decoded_first: Fixture =
            deserialize_legacy_bincode_from(&mut cursor, 1024, "first fixture").unwrap();
        let decoded_second: Fixture =
            deserialize_legacy_bincode_from(&mut cursor, 1024, "second fixture").unwrap();

        assert_eq!(decoded_first, first);
        assert_eq!(decoded_second, second);
        assert_eq!(cursor.position(), encoded.len() as u64);
    }

    #[test]
    fn legacy_bincode_stream_helpers_enforce_limits() {
        let fixture = Fixture {
            value: 7,
            payload: vec![9; 32],
        };
        let mut encoded = Vec::new();
        serialize_legacy_bincode_into(&mut encoded, &fixture, 1024, "fixture").unwrap();

        let serialize_err = serialize_legacy_bincode_into(&mut Vec::new(), &fixture, 4, "fixture")
            .expect_err("serialize limit");
        assert!(serialize_err.contains("legacy bincode serialize failed"));

        let deserialize_err = deserialize_legacy_bincode_from::<_, Fixture>(
            &mut std::io::Cursor::new(encoded),
            4,
            "fixture",
        )
        .expect_err("deserialize limit");
        assert!(deserialize_err.contains("legacy bincode deserialize failed"));
    }
}
