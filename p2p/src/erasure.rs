/// P3-4: Reed-Solomon erasure coding for parallelized block downloads.
///
/// Blocks are split into K data shards + M parity shards. Any K shards from
/// any source are sufficient to reconstruct the original data, enabling
/// download parallelism across multiple peers.
use reed_solomon_erasure::galois_8::ReedSolomon;
use serde::{Deserialize, Serialize};

/// Number of data shards (original pieces of the block).
pub const DATA_SHARDS: usize = 4;

/// Number of parity shards (redundancy).
pub const PARITY_SHARDS: usize = 2;

/// Total shards = DATA + PARITY.
pub const TOTAL_SHARDS: usize = DATA_SHARDS + PARITY_SHARDS;

/// An individual shard from an erasure-coded block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErasureShard {
    /// Block slot this shard belongs to
    pub slot: u64,
    /// Shard index (0..TOTAL_SHARDS)
    pub index: usize,
    /// Total number of shards
    pub total: usize,
    /// Original (unsharded) data length in bytes
    pub data_len: usize,
    /// Shard payload
    pub data: Vec<u8>,
}

/// Encode a block's serialized bytes into `TOTAL_SHARDS` shards.
///
/// The input is split into `DATA_SHARDS` equal-sized pieces (zero-padded to
/// align), then `PARITY_SHARDS` additional parity pieces are generated.
pub fn encode_shards(slot: u64, data: &[u8]) -> Result<Vec<ErasureShard>, String> {
    let rs = ReedSolomon::new(DATA_SHARDS, PARITY_SHARDS)
        .map_err(|e| format!("Reed-Solomon init failed: {}", e))?;

    let shard_size = data.len().div_ceil(DATA_SHARDS);

    let mut shards: Vec<Vec<u8>> = Vec::with_capacity(TOTAL_SHARDS);
    for i in 0..DATA_SHARDS {
        let start = i * shard_size;
        let end = std::cmp::min(start + shard_size, data.len());
        let mut shard = vec![0u8; shard_size];
        if start < data.len() {
            shard[..end - start].copy_from_slice(&data[start..end]);
        }
        shards.push(shard);
    }
    // Add empty parity shards
    for _ in 0..PARITY_SHARDS {
        shards.push(vec![0u8; shard_size]);
    }

    rs.encode(&mut shards)
        .map_err(|e| format!("Reed-Solomon encode failed: {}", e))?;

    let result = shards
        .into_iter()
        .enumerate()
        .map(|(i, shard_data)| ErasureShard {
            slot,
            index: i,
            total: TOTAL_SHARDS,
            data_len: data.len(),
            data: shard_data,
        })
        .collect();

    Ok(result)
}

/// Reconstruct original data from at least `DATA_SHARDS` of the `TOTAL_SHARDS`.
///
/// Missing shards should be `None`. Returns the original data (trimmed to
/// `data_len`) on success.
pub fn decode_shards(shards: &[Option<ErasureShard>]) -> Result<Vec<u8>, String> {
    if shards.len() != TOTAL_SHARDS {
        return Err(format!(
            "Expected {} shards, got {}",
            TOTAL_SHARDS,
            shards.len()
        ));
    }

    let present = shards.iter().filter(|s| s.is_some()).count();
    if present < DATA_SHARDS {
        return Err(format!(
            "Need at least {} shards, only have {}",
            DATA_SHARDS, present
        ));
    }

    // Determine shard size and data_len from any present shard
    let reference = shards.iter().flatten().next().ok_or("No shards present")?;
    let shard_size = reference.data.len();
    let data_len = reference.data_len;

    let rs = ReedSolomon::new(DATA_SHARDS, PARITY_SHARDS)
        .map_err(|e| format!("Reed-Solomon init failed: {}", e))?;

    let mut shard_data: Vec<Option<Vec<u8>>> = shards
        .iter()
        .map(|opt| opt.as_ref().map(|s| s.data.clone()))
        .collect();

    // Ensure missing shards are None with correct size placeholder
    for item in shard_data.iter_mut() {
        if item.is_none() {
            *item = None; // already None, but be explicit
        }
    }

    // Create the format reed-solomon-erasure expects: Vec<Option<Vec<u8>>>
    // and reconstruct
    let mut shards_for_reconstruct: Vec<Option<Vec<u8>>> = Vec::with_capacity(TOTAL_SHARDS);
    for item in &shard_data {
        match item {
            Some(d) => shards_for_reconstruct.push(Some(d.clone())),
            None => shards_for_reconstruct.push(None),
        }
    }

    // Convert to the format the library expects
    let mut owned_shards: Vec<Vec<u8>> = Vec::with_capacity(TOTAL_SHARDS);
    let mut shard_present: Vec<bool> = Vec::with_capacity(TOTAL_SHARDS);
    for item in &shard_data {
        match item {
            Some(d) => {
                owned_shards.push(d.clone());
                shard_present.push(true);
            }
            None => {
                owned_shards.push(vec![0u8; shard_size]);
                shard_present.push(false);
            }
        }
    }

    // Build mut slice refs for reconstruction
    let mut shard_refs: Vec<(&mut [u8], bool)> = owned_shards
        .iter_mut()
        .zip(shard_present.iter())
        .map(|(s, &p)| (s.as_mut_slice(), p))
        .collect();

    rs.reconstruct(&mut shard_refs)
        .map_err(|e| format!("Reed-Solomon reconstruct failed: {}", e))?;

    // Concatenate data shards and trim to original length
    let mut result = Vec::with_capacity(data_len);
    for shard in owned_shards.iter().take(DATA_SHARDS) {
        result.extend_from_slice(shard);
    }
    result.truncate(data_len);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_and_decode_all_shards() {
        let data = b"Hello, erasure coding! This is block data for slot 42.";
        let shards = encode_shards(42, data).unwrap();
        assert_eq!(shards.len(), TOTAL_SHARDS);
        for (i, s) in shards.iter().enumerate() {
            assert_eq!(s.index, i);
            assert_eq!(s.slot, 42);
            assert_eq!(s.total, TOTAL_SHARDS);
            assert_eq!(s.data_len, data.len());
        }

        // Decode with all shards present
        let all: Vec<Option<ErasureShard>> = shards.into_iter().map(Some).collect();
        let recovered = decode_shards(&all).unwrap();
        assert_eq!(&recovered, data);
    }

    #[test]
    fn test_decode_with_missing_parity_shards() {
        let data = b"Recover me even with missing parity shards!";
        let shards = encode_shards(10, data).unwrap();

        // Drop all parity shards (keep only data shards)
        let mut partial: Vec<Option<ErasureShard>> = shards.into_iter().map(Some).collect();
        for shard in &mut partial[DATA_SHARDS..TOTAL_SHARDS] {
            *shard = None;
        }

        let recovered = decode_shards(&partial).unwrap();
        assert_eq!(&recovered, data);
    }

    #[test]
    fn test_decode_with_missing_data_shards() {
        let data = b"Even missing data shards can be recovered with parity!";
        let shards = encode_shards(20, data).unwrap();

        // Drop first 2 data shards, keep remaining data + all parity
        let mut partial: Vec<Option<ErasureShard>> = shards.into_iter().map(Some).collect();
        partial[0] = None;
        partial[1] = None;

        let recovered = decode_shards(&partial).unwrap();
        assert_eq!(&recovered, data);
    }

    #[test]
    fn test_insufficient_shards_fails() {
        let data = b"Not enough shards to recover!";
        let shards = encode_shards(30, data).unwrap();

        // Keep only DATA_SHARDS - 1 (not enough)
        let mut partial: Vec<Option<ErasureShard>> = vec![None; TOTAL_SHARDS];
        for i in 0..(DATA_SHARDS - 1) {
            partial[i] = Some(shards[i].clone());
        }

        let result = decode_shards(&partial);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Need at least"));
    }

    #[test]
    fn test_wrong_shard_count_fails() {
        let result = decode_shards(&[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Expected"));
    }

    #[test]
    fn test_large_data() {
        // Test with larger data to ensure shard splitting works correctly
        let data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
        let shards = encode_shards(100, &data).unwrap();
        assert_eq!(shards.len(), TOTAL_SHARDS);

        // Drop one data shard and one parity shard
        let mut partial: Vec<Option<ErasureShard>> = shards.into_iter().map(Some).collect();
        partial[1] = None;
        partial[DATA_SHARDS] = None;

        let recovered = decode_shards(&partial).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn test_shard_serialization_roundtrip() {
        let data = b"Shard serialization test";
        let shards = encode_shards(77, data).unwrap();
        let shard = &shards[0];

        // Serialize and deserialize with bincode
        let encoded = bincode::serialize(shard).unwrap();
        let decoded: ErasureShard = bincode::deserialize(&encoded).unwrap();
        assert_eq!(decoded.slot, 77);
        assert_eq!(decoded.index, 0);
        assert_eq!(decoded.data, shard.data);
    }
}
