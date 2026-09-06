use super::*;
use crate::archive_v2::ArchiveV2PublicIndexes;

fn index_path(root: &Path, hash: &Hash) -> PathBuf {
    root.join("indexes").join(format!("{}.av2i", hash.to_hex()))
}

impl ArchiveV2Reader {
    pub(super) fn public_indexes(
        &self,
        manifest: &ArchiveV2Manifest,
        query: super::super::codec::ArchiveV2IndexRowQuery<'_>,
    ) -> Result<ArchiveV2PublicIndexes, ArchiveV2Error> {
        self.ensure_deep_history()?;
        if let Some(cache) = &self.config.cache_root {
            let _guard = self.cache_io_lock.lock().unwrap_or_else(|p| p.into_inner());
            let path = index_path(cache, &manifest.segment_object_hash);
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    let decoded = if metadata.is_file()
                        && metadata.len() as usize
                            == ArchiveV2SegmentCodec::public_index_cache_size(manifest)?
                    {
                        ArchiveV2SegmentCodec::decode_public_index_cache_selected(
                            &fs::read(&path)?,
                            manifest,
                            &self.identity,
                            Some(query),
                        )
                    } else {
                        Err(ArchiveV2Error::Malformed(
                            "invalid cached index file".to_string(),
                        ))
                    };
                    match decoded {
                        Ok(indexes) => {
                            let mut status = self.status_lock();
                            status.cache_hits = status.cache_hits.saturating_add(1);
                            return Ok(indexes);
                        }
                        Err(error) => {
                            self.quarantine_path(&path, &manifest.segment_object_hash)?;
                            let mut status = self.status_lock();
                            status.quarantined_objects =
                                status.quarantined_objects.saturating_add(1);
                            status.last_error = Some(error.to_string());
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let local = object_path(&self.config.root, &manifest.segment_object_hash);
        let bytes = if local.exists() {
            ArchiveV2SegmentCodec::public_index_cache_at_path(&local, manifest, &self.identity)?
        } else {
            let object = self.acquire_object(manifest)?;
            ArchiveV2SegmentCodec::public_index_cache_from_bytes(&object, manifest, &self.identity)?
        };
        let indexes = ArchiveV2SegmentCodec::decode_public_index_cache_selected(
            &bytes,
            manifest,
            &self.identity,
            Some(query),
        )?;
        if self.config.cache_root.is_some() && self.config.cache_quota_bytes >= bytes.len() as u64 {
            self.persist_index_cache(manifest, &bytes)?;
        }
        Ok(indexes)
    }

    fn persist_index_cache(
        &self,
        manifest: &ArchiveV2Manifest,
        bytes: &[u8],
    ) -> Result<(), ArchiveV2Error> {
        let cache = self
            .config
            .cache_root
            .as_ref()
            .ok_or_else(|| ArchiveV2Error::Role("index cache is not configured".to_string()))?;
        if bytes.len() as u64 > self.config.cache_quota_bytes {
            return Err(ArchiveV2Error::Bounds(
                "public index exceeds shared cache quota".to_string(),
            ));
        }
        let _guard = self.cache_io_lock.lock().unwrap_or_else(|p| p.into_inner());
        let path = index_path(cache, &manifest.segment_object_hash);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && metadata.len() == bytes.len() as u64 => {
                if fs::read(&path)? == bytes {
                    return Ok(());
                }
                return Err(ArchiveV2Error::Malformed(
                    "conflicting cached index bytes".to_string(),
                ));
            }
            Ok(_) => {
                return Err(ArchiveV2Error::Malformed(
                    "invalid cached index file".to_string(),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        evict_cache_until(cache, self.config.cache_quota_bytes - bytes.len() as u64)?;
        write_new_synced(&path, bytes)?;
        self.status_lock().cache_bytes = cache_size(cache)?;
        Ok(())
    }

    /// Import authenticated index bundles from an offline object source. This
    /// populates only the bounded cache and never proves whole-object inventory.
    pub fn prewarm_public_indexes(&self, source_root: &Path) -> Result<u64, ArchiveV2Error> {
        self.ensure_deep_history()?;
        if self.config.cache_root.is_none() {
            return Err(ArchiveV2Error::Role(
                "index cache is not configured".to_string(),
            ));
        }
        let mut required_bytes = 0_u64;
        for entry in &self.catalog.entries {
            let manifest = self
                .catalog
                .active_manifest(&entry.manifest.segment_object_hash)?;
            required_bytes = required_bytes
                .checked_add(ArchiveV2SegmentCodec::public_index_cache_size(manifest)? as u64)
                .ok_or_else(|| {
                    ArchiveV2Error::Bounds("public index cache size overflow".to_string())
                })?;
        }
        if required_bytes > self.config.cache_quota_bytes {
            return Err(ArchiveV2Error::Bounds(
                "catalog indexes exceed shared cache quota".to_string(),
            ));
        }
        let mut count = 0;
        for entry in &self.catalog.entries {
            let manifest = self
                .catalog
                .active_manifest(&entry.manifest.segment_object_hash)?;
            let bytes = ArchiveV2SegmentCodec::public_index_cache_at_path(
                &object_path(source_root, &manifest.segment_object_hash),
                manifest,
                &self.identity,
            )?;
            self.persist_index_cache(manifest, &bytes)?;
            count += 1;
        }
        // A concurrent body fetch may evict indexes between imports. Report
        // completion only if every authenticated bundle is still present under
        // the shared cache lock. Operators prewarm while the validator is stopped.
        let cache = self
            .config
            .cache_root
            .as_ref()
            .expect("cache checked above");
        let _guard = self.cache_io_lock.lock().unwrap_or_else(|p| p.into_inner());
        for entry in &self.catalog.entries {
            let manifest = self
                .catalog
                .active_manifest(&entry.manifest.segment_object_hash)?;
            let path = index_path(cache, &manifest.segment_object_hash);
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_file()
                || metadata.len()
                    != ArchiveV2SegmentCodec::public_index_cache_size(manifest)? as u64
            {
                return Err(ArchiveV2Error::Malformed(
                    "prewarmed index cache is incomplete".to_string(),
                ));
            }
            ArchiveV2SegmentCodec::decode_public_index_cache(
                &fs::read(path)?,
                manifest,
                &self.identity,
            )?;
        }
        if cache_size(cache)? > self.config.cache_quota_bytes {
            return Err(ArchiveV2Error::Bounds(
                "shared cache exceeds quota after prewarm".to_string(),
            ));
        }
        Ok(count)
    }
}
