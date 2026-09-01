use crate::chain::{challenge_is_open, ChainClient, ChallengeState, StorageInfo};
use crate::config::Config;
use crate::content::{decode_hash, AssignmentRecord, ContentStore, ObjectRecord};
use crate::merkle::root_for_file;
use futures_util::{stream, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::{info, warn};

const SUBMISSION_RETRY_DELAY: Duration = Duration::from_secs(30);
const RECONCILE_CONCURRENCY: usize = 8;

pub async fn run(
    config: Arc<Config>,
    store: Arc<ContentStore>,
    chain: Arc<ChainClient>,
    notify: Arc<Notify>,
) {
    loop {
        if let Err(error) = reconcile_once(&config, &store, &chain).await {
            warn!(error = %error, "Moss reconciliation cycle failed");
        }
        tokio::select! {
            _ = tokio::time::sleep(config.reconcile_interval) => {}
            _ = notify.notified() => {}
        }
    }
}

async fn reconcile_once(
    config: &Config,
    store: &Arc<ContentStore>,
    chain: &Arc<ChainClient>,
) -> Result<(), String> {
    let records = store.list().await?;
    if records.is_empty() {
        return Ok(());
    }
    let accepting_assignments = match chain.provider_status().await? {
        Some(status) if status.operational() => status.accepting_assignments(),
        Some(_) => return Err("Moss provider is inactive or has no valid price".to_string()),
        None => return Err("Moss provider is not registered on-chain".to_string()),
    };
    let slot = chain.current_slot().await?;
    let failures = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    stream::iter(records)
        .for_each_concurrent(RECONCILE_CONCURRENCY, |record| {
            let failures = failures.clone();
            async move {
                if let Err(error) =
                    process_object(config, store, chain, slot, accepting_assignments, record).await
                {
                    let mut failures = failures.lock().await;
                    if failures.len() < 8 {
                        failures.push(error);
                    }
                }
            }
        })
        .await;
    let failures = failures.lock().await;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} Moss objects failed reconciliation; samples: {}",
            failures.len(),
            failures.join(" | ")
        ))
    }
}

async fn process_object(
    config: &Config,
    store: &ContentStore,
    chain: &ChainClient,
    slot: u64,
    accepting_assignments: bool,
    record: ObjectRecord,
) -> Result<(), String> {
    verify_object(store, &record).await?;
    if record.assignments.is_empty() {
        if record
            .modified
            .elapsed()
            .map(|age| age >= config.staged_ttl)
            .unwrap_or(false)
            && store.remove(&record.hash).await?
        {
            info!(content_hash = %record.hash, "removed expired uncommitted Moss upload");
        }
        return Ok(());
    }

    let mut failures = Vec::new();
    for assignment in record.assignments.clone() {
        if let Err(error) = process_assignment(
            config,
            store,
            chain,
            slot,
            accepting_assignments,
            &record,
            assignment,
        )
        .await
        {
            failures.push(error);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join(" | "))
    }
}

async fn process_assignment(
    config: &Config,
    store: &ContentStore,
    chain: &ChainClient,
    slot: u64,
    accepting_assignments: bool,
    record: &ObjectRecord,
    assignment: AssignmentRecord,
) -> Result<(), String> {
    let storage_id = &assignment.storage_id;
    let info = match chain.storage_info(storage_id).await? {
        Some(info) => info,
        None => {
            if assignment
                .modified
                .elapsed()
                .map(|age| age >= config.staged_ttl)
                .unwrap_or(false)
                && store.remove_assignment(&record.hash, storage_id).await?
            {
                info!(storage_id = %storage_id, content_hash = %record.hash, "removed expired uncommitted Moss assignment");
            }
            return Ok(());
        }
    };
    if info.owner.to_base58() != assignment.owner {
        return Err(format!(
            "{}: on-chain owner conflicts with signed upload owner",
            storage_id
        ));
    }
    let on_chain_content_hash = chain.storage_content_hash(storage_id).await?;
    if on_chain_content_hash != record.hash {
        return Err(format!(
            "{}: on-chain content hash {} conflicts with local {}",
            storage_id, on_chain_content_hash, record.hash
        ));
    }
    if info.size != record.size {
        return Err(format!(
            "{}: on-chain size {} conflicts with local content {} size {}",
            storage_id, info.size, record.hash, record.size
        ));
    }
    if info.replication == 0 || info.replication > 10 {
        return Err(format!("{}: invalid on-chain replication", storage_id));
    }
    if chain.is_closed(storage_id).await? {
        if store.remove_assignment(&record.hash, storage_id).await? {
            info!(storage_id = %storage_id, content_hash = %record.hash, "removed finalized Moss assignment");
        }
        return Ok(());
    }

    let provider = chain.provider();
    let provider_confirmed = info.providers.contains(&provider);
    if !provider_confirmed {
        if slot > info.expiry_slot {
            submit_close_if_due(store, chain, &record.hash, storage_id, &info).await?;
            return Ok(());
        }
        if !accepting_assignments {
            return Err(format!(
                "{}: provider collateral is below current obligations",
                storage_id
            ));
        }
        if !store
            .assignment_marker_is_recent(
                &record.hash,
                storage_id,
                "confirm_submitted",
                SUBMISSION_RETRY_DELAY,
            )
            .await
        {
            let signature = chain.confirm_storage(storage_id).await?;
            store
                .mark_assignment(
                    &record.hash,
                    storage_id,
                    "confirm_submitted",
                    signature.as_bytes(),
                )
                .await?;
            info!(storage_id = %storage_id, content_hash = %record.hash, tx = %signature, "submitted Moss storage confirmation");
        }
        return Ok(());
    }
    if !store
        .has_assignment_marker(&record.hash, storage_id, "confirmed")
        .await
    {
        store
            .mark_assignment(
                &record.hash,
                storage_id,
                "confirmed",
                slot.to_string().as_bytes(),
            )
            .await?;
        info!(storage_id = %storage_id, content_hash = %record.hash, "Moss storage confirmation observed on-chain");
    }

    let mut unresolved_challenge = false;
    match chain.challenge(storage_id).await? {
        ChallengeState::Missing => {}
        ChallengeState::WaitingForEntropy => unresolved_challenge = true,
        ChallengeState::Ready(challenge) if challenge_is_open(challenge) => {
            unresolved_challenge = true;
            if slot <= challenge.deadline_slot
                && !store
                    .assignment_marker_is_recent(
                        &record.hash,
                        storage_id,
                        "proof_submitted",
                        SUBMISSION_RETRY_DELAY,
                    )
                    .await
            {
                let signature = chain
                    .respond_to_challenge(
                        storage_id,
                        &record.hash,
                        &record.path,
                        record.size,
                        challenge.effective_nonce,
                    )
                    .await?;
                store
                    .mark_assignment(
                        &record.hash,
                        storage_id,
                        "proof_submitted",
                        signature.as_bytes(),
                    )
                    .await?;
                info!(storage_id = %storage_id, content_hash = %record.hash, tx = %signature, "submitted Moss challenge response");
            }
        }
        ChallengeState::Ready(_) => {}
    }

    if slot > info.expiry_slot && !unresolved_challenge {
        submit_close_if_due(store, chain, &record.hash, storage_id, &info).await?;
    }
    Ok(())
}

async fn verify_object(store: &ContentStore, record: &ObjectRecord) -> Result<(), String> {
    if store.has_marker(&record.hash, "verified").await {
        return Ok(());
    }
    let expected = decode_hash(&record.hash)?;
    let (actual, size) = root_for_file(&record.path).await?;
    if expected != actual || size != record.size {
        return Err(format!("{}: local content integrity failure", record.hash));
    }
    store
        .mark(&record.hash, "verified", b"sha256-merkle-v1")
        .await
}

async fn submit_close_if_due(
    store: &ContentStore,
    chain: &ChainClient,
    content_hash: &str,
    storage_id: &str,
    info: &StorageInfo,
) -> Result<(), String> {
    if store
        .assignment_marker_is_recent(
            content_hash,
            storage_id,
            "close_submitted",
            SUBMISSION_RETRY_DELAY,
        )
        .await
    {
        return Ok(());
    }
    let signature = chain.close_storage(&info.owner, storage_id).await?;
    store
        .mark_assignment(
            content_hash,
            storage_id,
            "close_submitted",
            signature.as_bytes(),
        )
        .await?;
    info!(storage_id = %storage_id, content_hash = %content_hash, tx = %signature, "submitted Moss storage finalization");
    Ok(())
}
