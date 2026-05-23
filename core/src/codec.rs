use serde::{de::DeserializeOwned, Serialize};

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
}
