//! Dense, authenticated index-only cache. This is a disposable reader cache,
//! not a new archive segment format or a substitute for a complete object.
use super::*;

const INDEX_CACHE_MAGIC: &[u8; 8] = b"AV2IC1\0\0";

fn layout(
    manifest: &ArchiveV2Manifest,
) -> Result<(usize, u64, &ArchiveV2FrameDescriptor), ArchiveV2Error> {
    manifest.validate()?;
    let last = manifest.frames.last().ok_or_else(|| {
        ArchiveV2Error::Ordering("index cache manifest has no frames".to_string())
    })?;
    let object_bytes = last
        .file_offset
        .checked_add(last.compressed_bytes as u64)
        .and_then(|bytes| bytes.checked_add(SEGMENT_TRAILER_BYTES as u64))
        .ok_or_else(|| ArchiveV2Error::Bounds("index cache object length overflow".to_string()))?;
    let header_bytes = validate_seekable_layout(manifest, object_bytes)?;
    let mut indexes = manifest
        .frames
        .iter()
        .filter(|frame| frame.kind == ArchiveV2FrameKind::PublicIndexes);
    let index = indexes.next().ok_or_else(|| {
        ArchiveV2Error::Ordering("index cache manifest has no public index frame".to_string())
    })?;
    if indexes.next().is_some() || index.record_count != 1 {
        return Err(ArchiveV2Error::Ordering(
            "index cache requires exactly one public index frame and record".to_string(),
        ));
    }
    Ok((header_bytes, object_bytes, index))
}

impl ArchiveV2SegmentCodec {
    pub fn public_index_cache_size(manifest: &ArchiveV2Manifest) -> Result<usize, ArchiveV2Error> {
        let (header_bytes, _, index) = layout(manifest)?;
        Ok(INDEX_CACHE_MAGIC.len()
            + header_bytes
            + SEGMENT_TRAILER_BYTES
            + FRAME_HEADER_BYTES
            + index.compressed_bytes as usize)
    }

    pub fn public_index_cache_from_bytes(
        object: &[u8],
        manifest: &ArchiveV2Manifest,
        identity: &ArchiveV2Identity,
    ) -> Result<Vec<u8>, ArchiveV2Error> {
        let (header_bytes, object_bytes, index) = layout(manifest)?;
        if object.len() as u64 != object_bytes {
            return Err(ArchiveV2Error::Truncated("index cache source object"));
        }
        let frame_start = index.file_offset as usize - FRAME_HEADER_BYTES;
        let frame_end = index.file_offset as usize + index.compressed_bytes as usize;
        let mut bytes = Vec::with_capacity(Self::public_index_cache_size(manifest)?);
        bytes.extend_from_slice(INDEX_CACHE_MAGIC);
        bytes.extend_from_slice(&object[..header_bytes]);
        bytes.extend_from_slice(&object[object.len() - SEGMENT_TRAILER_BYTES..]);
        bytes.extend_from_slice(&object[frame_start..frame_end]);
        Self::decode_public_index_cache(&bytes, manifest, identity)?;
        Ok(bytes)
    }

    /// Extract only the original envelope and public-index frame. The result
    /// authenticates against the supplied manifest without reading transaction
    /// or block frames. It never asserts whole-object availability or validity.
    pub fn public_index_cache_at_path(
        path: &Path,
        manifest: &ArchiveV2Manifest,
        identity: &ArchiveV2Identity,
    ) -> Result<Vec<u8>, ArchiveV2Error> {
        let (header_bytes, object_bytes, index) = layout(manifest)?;
        let mut file = File::open(path)?;
        if file.metadata()?.len() != object_bytes {
            return Err(ArchiveV2Error::Truncated("index cache source object"));
        }
        let frame_bytes = FRAME_HEADER_BYTES + index.compressed_bytes as usize;
        let mut bytes =
            vec![0u8; INDEX_CACHE_MAGIC.len() + header_bytes + SEGMENT_TRAILER_BYTES + frame_bytes];
        bytes[..INDEX_CACHE_MAGIC.len()].copy_from_slice(INDEX_CACHE_MAGIC);
        let header_end = INDEX_CACHE_MAGIC.len() + header_bytes;
        file.read_exact(&mut bytes[INDEX_CACHE_MAGIC.len()..header_end])?;
        file.seek(SeekFrom::Start(object_bytes - SEGMENT_TRAILER_BYTES as u64))?;
        let trailer_end = header_end + SEGMENT_TRAILER_BYTES;
        file.read_exact(&mut bytes[header_end..trailer_end])?;
        file.seek(SeekFrom::Start(
            index.file_offset - FRAME_HEADER_BYTES as u64,
        ))?;
        file.read_exact(&mut bytes[trailer_end..])?;
        Self::decode_public_index_cache(&bytes, manifest, identity)?;
        Ok(bytes)
    }

    /// Decode only after the original envelope, dictionary and frame content
    /// hash match the caller's independently authenticated catalog manifest.
    pub fn decode_public_index_cache(
        bytes: &[u8],
        manifest: &ArchiveV2Manifest,
        identity: &ArchiveV2Identity,
    ) -> Result<ArchiveV2PublicIndexes, ArchiveV2Error> {
        Self::decode_public_index_cache_selected(bytes, manifest, identity, None)
    }

    pub(crate) fn decode_public_index_cache_selected(
        bytes: &[u8],
        manifest: &ArchiveV2Manifest,
        identity: &ArchiveV2Identity,
        query: Option<ArchiveV2IndexRowQuery<'_>>,
    ) -> Result<ArchiveV2PublicIndexes, ArchiveV2Error> {
        let (header_bytes, _, index) = layout(manifest)?;
        let header_end = INDEX_CACHE_MAGIC.len() + header_bytes;
        let trailer_end = header_end + SEGMENT_TRAILER_BYTES;
        let expected = trailer_end + FRAME_HEADER_BYTES + index.compressed_bytes as usize;
        if bytes.len() != expected || !bytes.starts_with(INDEX_CACHE_MAGIC) {
            return Err(ArchiveV2Error::Malformed(
                "invalid index cache envelope".to_string(),
            ));
        }
        let config = verify_seekable_envelope(
            &bytes[INDEX_CACHE_MAGIC.len()..header_end],
            &bytes[header_end..trailer_end],
            manifest,
            identity,
        )?;
        let raw = decode_seekable_frame_encoding(&bytes[trailer_end..], index, &config.dictionary)?;
        let indexes =
            decode_compact_indexes_selected(record_at(&raw, 0, 1)?, query, Some(manifest))?;
        if query.is_none() {
            indexes.validate(manifest.start_slot, manifest.end_slot)?;
            validate_index_frame_references(&indexes, &manifest.frames)?;
        }
        Ok(indexes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_index_decoding_keeps_exact_rows_and_validates_skipped_data() {
        let block =
            Block::new_with_timestamp(0, Hash::default(), Hash::default(), [0; 32], Vec::new(), 0);
        let identity = ArchiveV2Identity {
            network_id: "selected-index-test".to_string(),
            genesis_hash: block.hash(),
        };
        let mut contents = super::super::super::ArchiveV2SegmentContents::from_blocks(vec![block]);
        let rows = vec![
            ArchiveV2PublicRow {
                slot: 0,
                key: b"account-a/1".to_vec(),
                value: vec![1; 16],
            },
            ArchiveV2PublicRow {
                slot: 0,
                key: b"account-b/1".to_vec(),
                value: vec![2; 16],
            },
        ];
        contents
            .public_categories
            .insert("events".to_string(), rows.clone());
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &contents,
            &ArchiveV2CodecConfig::default(),
        )
        .unwrap();
        let cache =
            ArchiveV2SegmentCodec::public_index_cache_from_bytes(&bytes, &manifest, &identity)
                .unwrap();
        let query = ArchiveV2IndexRowQuery {
            category: "events",
            prefix: b"account-b",
            start_slot: 0,
            end_slot: 0,
        };
        let selected = ArchiveV2SegmentCodec::decode_public_index_cache_selected(
            &cache,
            &manifest,
            &identity,
            Some(query),
        )
        .unwrap();
        assert_eq!(selected.categories["events"], vec![rows[1].clone()]);
        assert!(selected.blocks_by_slot.is_empty());
        assert!(selected.transactions_by_signature.is_empty());
        let complete =
            ArchiveV2SegmentCodec::decode_public_index_cache(&cache, &manifest, &identity).unwrap();
        let mut corrupt = complete.clone();
        corrupt.categories.get_mut("events").unwrap()[0].slot = 1;
        assert!(decode_compact_indexes_selected(
            &encode_compact_indexes(&corrupt).unwrap(),
            Some(query),
            Some(&manifest)
        )
        .unwrap_err()
        .to_string()
        .contains("outside the segment range"));
        let mut corrupt = complete.clone();
        corrupt.categories.get_mut("events").unwrap()[0].key = rows[1].key.clone();
        assert!(decode_compact_indexes_selected(
            &encode_compact_indexes(&corrupt).unwrap(),
            Some(query),
            Some(&manifest)
        )
        .is_err());
        let mut corrupt = complete;
        corrupt.blocks_by_slot.get_mut(&0).unwrap().0 = u32::MAX;
        assert!(decode_compact_indexes_selected(
            &encode_compact_indexes(&corrupt).unwrap(),
            Some(query),
            Some(&manifest)
        )
        .unwrap_err()
        .to_string()
        .contains("invalid frame reference"));
        let mut corrupt = cache;
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(ArchiveV2SegmentCodec::decode_public_index_cache_selected(
            &corrupt,
            &manifest,
            &identity,
            Some(query)
        )
        .is_err());
    }

    #[test]
    fn cache_authenticates_used_frames_without_claiming_complete_object() {
        let root = tempfile::tempdir().unwrap();
        let block =
            Block::new_with_timestamp(0, Hash::default(), Hash::default(), [0; 32], Vec::new(), 0);
        let identity = ArchiveV2Identity {
            network_id: "index-cache-test".to_string(),
            genesis_hash: block.hash(),
        };
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &super::super::super::ArchiveV2SegmentContents::from_blocks(vec![block]),
            &ArchiveV2CodecConfig::default(),
        )
        .unwrap();
        let path = root.path().join("source.av2s");
        std::fs::write(&path, &bytes).unwrap();
        let cache =
            ArchiveV2SegmentCodec::public_index_cache_at_path(&path, &manifest, &identity).unwrap();
        assert_eq!(
            ArchiveV2SegmentCodec::public_index_cache_from_bytes(&bytes, &manifest, &identity)
                .unwrap(),
            cache
        );
        let decoded = ArchiveV2SegmentCodec::decode(&bytes, &manifest, &identity).unwrap();
        assert_eq!(
            ArchiveV2SegmentCodec::decode_public_index_cache(&cache, &manifest, &identity).unwrap(),
            decoded.indexes
        );
        assert!(cache.len() < bytes.len());
        for offset in [0, 8, cache.len() - 1] {
            let mut corrupt = cache.clone();
            corrupt[offset] ^= 0x80;
            assert!(ArchiveV2SegmentCodec::decode_public_index_cache(
                &corrupt, &manifest, &identity
            )
            .is_err());
        }
        let mut extra = cache.clone();
        extra.push(0);
        assert!(
            ArchiveV2SegmentCodec::decode_public_index_cache(&extra, &manifest, &identity).is_err()
        );
        assert!(ArchiveV2SegmentCodec::decode_public_index_cache(
            &cache[..cache.len() - 1],
            &manifest,
            &identity
        )
        .is_err());
        let wrong = ArchiveV2Identity {
            network_id: "wrong".to_string(),
            ..identity.clone()
        };
        assert!(
            ArchiveV2SegmentCodec::decode_public_index_cache(&cache, &manifest, &wrong).is_err()
        );
        // Unused body corruption must not be mislabeled as a complete-object
        // verification. The independently authenticated public index remains usable.
        let mut damaged_body = bytes.clone();
        let body = manifest
            .frames
            .iter()
            .find(|f| f.kind == ArchiveV2FrameKind::Blocks)
            .unwrap();
        damaged_body[body.file_offset as usize] ^= 1;
        std::fs::write(&path, &damaged_body).unwrap();
        assert!(ArchiveV2SegmentCodec::decode(&damaged_body, &manifest, &identity).is_err());
        assert_eq!(
            ArchiveV2SegmentCodec::public_index_cache_at_path(&path, &manifest, &identity).unwrap(),
            cache
        );
    }
}
