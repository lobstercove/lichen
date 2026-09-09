use super::*;
use lichen_core::archive_v2::{
    ArchiveV2Catalog, ArchiveV2CodecConfig, ArchiveV2Error, ArchiveV2Identity,
    ArchiveV2ObjectSource, ArchiveV2Reader, ArchiveV2ReaderConfig, ArchiveV2Role,
    ArchiveV2SegmentCodec, ArchiveV2SegmentContents,
};

struct SlowSource {
    bytes: Vec<u8>,
    entered: std::sync::mpsc::SyncSender<()>,
}

struct UnavailableCountingSource(Arc<std::sync::atomic::AtomicUsize>);

impl ArchiveV2ObjectSource for UnavailableCountingSource {
    fn name(&self) -> &str {
        "unrelated-unavailable-segment"
    }
    fn authenticated(&self) -> bool {
        true
    }
    fn fetch(&self, _: &Hash) -> Result<Option<Vec<u8>>, ArchiveV2Error> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(None)
    }
}

#[tokio::test]
async fn genesis_block_missing_receipts_do_not_fetch_unrelated_segments() {
    let state_root = tempdir().unwrap();
    let archive_root = tempdir().unwrap();
    let cache_root = tempdir().unwrap();
    let state = StateStore::open(state_root.path()).unwrap();
    let tx = Transaction::new(lichen_core::Message::new(
        vec![lichen_core::Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![Pubkey([1; 32]), Pubkey([2; 32])],
            data: vec![0; 9],
        }],
        Hash::default(),
    ));
    let genesis =
        Block::new_with_timestamp(0, Hash::default(), Hash::default(), [0; 32], vec![tx], 1);
    let child =
        Block::new_with_timestamp(1, genesis.hash(), Hash::default(), [0; 32], Vec::new(), 2);
    let unrelated =
        Block::new_with_timestamp(2, child.hash(), Hash::default(), [0; 32], Vec::new(), 3);
    let identity = ArchiveV2Identity {
        network_id: "known-slot-rpc-test".to_string(),
        genesis_hash: genesis.hash(),
    };
    let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
    let mut previous_object = None;
    let mut previous_block = Hash::default();
    std::fs::create_dir_all(archive_root.path().join("objects")).unwrap();
    for block in [genesis.clone(), child, unrelated] {
        let slot = block.header.slot;
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            previous_object,
            previous_block,
            &ArchiveV2SegmentContents::from_blocks(vec![block]),
            &ArchiveV2CodecConfig::default(),
        )
        .unwrap();
        if slot < 2 {
            std::fs::write(
                archive_root
                    .path()
                    .join("objects")
                    .join(format!("{}.av2s", manifest.segment_object_hash)),
                bytes,
            )
            .unwrap();
        }
        previous_object = Some(manifest.segment_object_hash);
        previous_block = manifest.last_block_hash;
        catalog.append(manifest).unwrap();
    }
    let catalog_path = archive_root.path().join("catalog.av2");
    catalog.store_atomic(&catalog_path).unwrap();
    let fetches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    state.attach_archive_v2_reader(
        ArchiveV2Reader::open(
            identity,
            &catalog_path,
            ArchiveV2ReaderConfig {
                role: ArchiveV2Role::VerifiedCache,
                root: archive_root.path().to_path_buf(),
                cache_root: Some(cache_root.path().to_path_buf()),
                cache_quota_bytes: 1024 * 1024,
                max_decoded_segments: 1,
                allow_remote_fetch: true,
                sources: vec![Arc::new(UnavailableCountingSource(fetches.clone()))],
            },
        )
        .unwrap(),
    );
    let rpc = make_test_rpc_state(state);
    let block = super::super::handle_get_block(&rpc, Some(serde_json::json!([0])))
        .await
        .unwrap();
    assert_eq!(block["hash"], genesis.hash().to_hex());
    assert_eq!(block["transaction_count"], 1);
    for encoding in ["json", "base64", "base58"] {
        let block = super::super::handle_solana_get_block(
            &rpc,
            Some(serde_json::json!([0, {"encoding": encoding}])),
        )
        .await
        .unwrap();
        assert_eq!(block["transactions"].as_array().unwrap().len(), 1);
        assert!(block["transactions"][0]["meta"]["err"].is_null());
    }
    assert_eq!(
        fetches.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "missing legacy receipts must not fetch any unrelated archive segment"
    );
    // Verify that this fixture detects the original archive-wide lookup.
    assert!(rpc
        .state
        .get_tx_meta_full(&genesis.transactions[0].signature())
        .is_err());
    assert!(fetches.load(std::sync::atomic::Ordering::SeqCst) > 0);
}

impl ArchiveV2ObjectSource for SlowSource {
    fn name(&self) -> &str {
        "slow-test-source"
    }
    fn authenticated(&self) -> bool {
        true
    }
    fn fetch(&self, _: &Hash) -> Result<Option<Vec<u8>>, ArchiveV2Error> {
        let _ = self.entered.try_send(());
        std::thread::sleep(std::time::Duration::from_millis(700));
        Ok(Some(self.bytes.clone()))
    }
}

#[test]
fn slow_archive_query_does_not_stall_rpc_or_runtime_progress() {
    let state_root = tempdir().unwrap();
    let archive_root = tempdir().unwrap();
    let cache_root = tempdir().unwrap();
    let state = StateStore::open(state_root.path()).unwrap();
    let block =
        Block::new_with_timestamp(0, Hash::default(), Hash::default(), [0; 32], Vec::new(), 0);
    state.put_block(&block).unwrap();
    let recent = Block::new_with_timestamp(
        1,
        block.hash(),
        Hash::default(),
        [0; 32],
        Vec::new(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    );
    state.put_block(&recent).unwrap();
    state.set_last_slot(1).unwrap();
    assert_eq!(state.get_last_slot().unwrap(), 1);
    let identity = ArchiveV2Identity {
        network_id: "runtime-isolation-test".to_string(),
        genesis_hash: block.hash(),
    };
    let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
        identity.clone(),
        None,
        Hash::default(),
        &ArchiveV2SegmentContents::from_blocks(vec![block]),
        &ArchiveV2CodecConfig::default(),
    )
    .unwrap();
    let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
    catalog.append(manifest).unwrap();
    let catalog_path = archive_root.path().join("catalog.av2");
    catalog.store_atomic(&catalog_path).unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    state.attach_archive_v2_reader(
        ArchiveV2Reader::open(
            identity,
            &catalog_path,
            ArchiveV2ReaderConfig {
                role: ArchiveV2Role::VerifiedCache,
                root: archive_root.path().to_path_buf(),
                cache_root: Some(cache_root.path().to_path_buf()),
                cache_quota_bytes: bytes.len() as u64 + 1024,
                max_decoded_segments: 1,
                allow_remote_fetch: true,
                sources: vec![Arc::new(SlowSource {
                    bytes,
                    entered: entered_tx,
                })],
            },
        )
        .unwrap(),
    );
    state.mark_archive_v2_admitted_after_fresh_sync().unwrap();
    assert_eq!(
        state.get_block_by_slot(1).unwrap().unwrap().hash(),
        recent.hash()
    );
    assert_eq!(state.archive_v2_status().unwrap().remote_fetches, 0);
    let app = Router::new()
        .route("/", post(super::super::handle_rpc))
        .with_state(Arc::new(make_test_rpc_state(state)));
    let request = |method: &str, params: serde_json::Value| {
        Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "jsonrpc":"2.0", "id":1, "method":method, "params":params,
                })
                .to_string(),
            ))
            .unwrap()
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let slow_app = app.clone();
    let slow_request = request(
        "getAccountTxCount",
        serde_json::json!([Pubkey([1; 32]).to_base58()]),
    );
    let slow = runtime.spawn(async move { slow_app.oneshot(slow_request).await.unwrap() });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let (progress_tx, progress_rx) = std::sync::mpsc::sync_channel(1);
    let fast_request = request("getSlot", serde_json::json!([]));
    runtime.spawn(async move {
        let response = app.oneshot(fast_request).await.unwrap();
        let _ = progress_tx.send(response.status());
    });
    let progress = progress_rx.recv_timeout(std::time::Duration::from_millis(250));
    runtime.block_on(slow).unwrap();
    runtime.shutdown_timeout(std::time::Duration::from_secs(3));
    assert_eq!(
        progress.ok(),
        Some(StatusCode::OK),
        "slow authenticated archive reads must not occupy the async runtime worker"
    );
}

#[tokio::test]
async fn cancelled_client_keeps_capacity_reserved_until_blocking_work_finishes() {
    let root = tempdir().unwrap();
    let mut state = make_test_rpc_state(StateStore::open(root.path()).unwrap());
    state.blocking_requests = Arc::new(tokio::sync::Semaphore::new(1));
    let state = Arc::new(state);
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        super::super::execute_blocking_rpc(&task_state, serde_json::json!(1), async move {
            let _ = entered_tx.send(());
            release_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            axum::response::IntoResponse::into_response(StatusCode::OK)
        })
        .await
    });
    entered_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(state.blocking_requests.available_permits(), 0);
    let rejected = super::super::execute_blocking_rpc(&state, serde_json::json!(2), async {
        panic!("a saturated executor must not poll the rejected request")
    })
    .await;
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = to_bytes(rejected.into_body(), 4096).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["id"], 2);
    assert_eq!(value["error"]["code"], -32005);
    release_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state.blocking_requests.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let accepted = super::super::execute_blocking_rpc(&state, serde_json::json!(3), async {
        axum::response::IntoResponse::into_response(StatusCode::OK)
    })
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
}
