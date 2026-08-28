use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::{AsyncReadExt, BufReader};

pub const CHUNK_BYTES: usize = 65_536;

fn hash(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

#[derive(Default)]
pub struct MerkleAccumulator {
    chunk: Vec<u8>,
    leaves: Vec<[u8; 32]>,
    size: u64,
}

impl MerkleAccumulator {
    pub fn update(&mut self, mut bytes: &[u8]) -> Result<(), String> {
        self.size = self
            .size
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| "object size overflow".to_string())?;
        while !bytes.is_empty() {
            let available = CHUNK_BYTES - self.chunk.len();
            let take = available.min(bytes.len());
            self.chunk.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.chunk.len() == CHUNK_BYTES {
                self.leaves.push(hash(&self.chunk));
                self.chunk.clear();
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<([u8; 32], u64), String> {
        if !self.chunk.is_empty() {
            self.leaves.push(hash(&self.chunk));
        }
        if self.leaves.is_empty() {
            return Err("empty objects are not supported".to_string());
        }
        Ok((root_from_leaves(self.leaves), self.size))
    }
}

fn root_from_leaves(mut nodes: Vec<[u8; 32]>) -> [u8; 32] {
    while nodes.len() > 1 {
        let mut next = Vec::with_capacity(nodes.len().div_ceil(2));
        for pair in nodes.chunks(2) {
            let right = if pair.len() == 2 { &pair[1] } else { &pair[0] };
            next.push(hash_pair(&pair[0], right));
        }
        nodes = next;
    }
    nodes[0]
}

pub async fn root_for_file(path: &Path) -> Result<([u8; 32], u64), String> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("open object: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut accumulator = MerkleAccumulator::default();
    let mut buffer = vec![0u8; CHUNK_BYTES];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| format!("read object: {error}"))?;
        if read == 0 {
            break;
        }
        accumulator.update(&buffer[..read])?;
    }
    accumulator.finish()
}

pub async fn proof_for_file(
    path: &Path,
    target_index: u64,
) -> Result<(Vec<u8>, Vec<u8>, u64), String> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("open object: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut chunks = Vec::new();
    let mut leaves = Vec::new();
    let mut size = 0u64;
    loop {
        let mut chunk = vec![0u8; CHUNK_BYTES];
        let mut filled = 0usize;
        while filled < CHUNK_BYTES {
            let read = reader
                .read(&mut chunk[filled..])
                .await
                .map_err(|error| format!("read object: {error}"))?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled == 0 {
            break;
        }
        chunk.truncate(filled);
        size = size
            .checked_add(filled as u64)
            .ok_or_else(|| "object size overflow".to_string())?;
        leaves.push(hash(&chunk));
        chunks.push(chunk);
    }
    let target = usize::try_from(target_index).map_err(|_| "challenge index overflow")?;
    let challenged_chunk = chunks
        .get(target)
        .cloned()
        .ok_or_else(|| "challenge index outside object".to_string())?;

    let mut proof = Vec::new();
    let mut index = target;
    let mut nodes = leaves;
    while nodes.len() > 1 {
        let sibling_index = if index.is_multiple_of(2) {
            (index + 1).min(nodes.len() - 1)
        } else {
            index - 1
        };
        proof.extend_from_slice(&nodes[sibling_index]);
        let mut next = Vec::with_capacity(nodes.len().div_ceil(2));
        for pair in nodes.chunks(2) {
            let right = if pair.len() == 2 { &pair[1] } else { &pair[0] };
            next.push(hash_pair(&pair[0], right));
        }
        nodes = next;
        index /= 2;
    }
    Ok((challenged_chunk, proof, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_root_matches_contract_tree() {
        let mut accumulator = MerkleAccumulator::default();
        accumulator.update(&vec![0x11; CHUNK_BYTES]).unwrap();
        accumulator.update(&vec![0x22; CHUNK_BYTES]).unwrap();
        accumulator.update(&[0x33; 123]).unwrap();
        let (root, size) = accumulator.finish().unwrap();

        let leaves = vec![
            hash(&vec![0x11; CHUNK_BYTES]),
            hash(&vec![0x22; CHUNK_BYTES]),
            hash(&[0x33; 123]),
        ];
        assert_eq!(root, root_from_leaves(leaves));
        assert_eq!(size, (CHUNK_BYTES * 2 + 123) as u64);
    }

    #[test]
    fn single_chunk_root_is_sha256() {
        let data = b"lichen moss";
        let mut accumulator = MerkleAccumulator::default();
        accumulator.update(data).unwrap();
        assert_eq!(accumulator.finish().unwrap().0, hash(data));
    }
}
