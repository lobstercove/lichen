use super::*;

pub(crate) async fn webhook_dispatcher_loop(
    state: CustodyState,
    event_rx: &mut broadcast::Receiver<CustodyWebhookEvent>,
) {
    info!("🔔 Webhook dispatcher started");

    loop {
        match event_rx.recv().await {
            Ok(event) => {
                let webhooks = match super::storage::list_all_webhooks(&state.db) {
                    Ok(webhooks) => webhooks,
                    Err(error) => {
                        tracing::warn!("failed to list webhooks: {}", error);
                        continue;
                    }
                };

                for webhook in webhooks {
                    if !webhook.active {
                        continue;
                    }
                    if !webhook.event_filter.is_empty()
                        && !webhook.event_filter.contains(&event.event_type)
                    {
                        continue;
                    }

                    let client = state.http.clone();
                    let event_clone = event.clone();
                    let webhook_clone = webhook.clone();
                    let permit = match state.webhook_delivery_limiter.clone().acquire_owned().await
                    {
                        Ok(permit) => permit,
                        Err(_) => {
                            tracing::warn!("webhook delivery limiter closed");
                            continue;
                        }
                    };

                    tokio::spawn(async move {
                        let _permit = permit;
                        deliver_webhook(&client, &webhook_clone, &event_clone).await;
                    });
                }
            }
            Err(broadcast::error::RecvError::Lagged(dropped)) => {
                tracing::warn!("webhook dispatcher lagged, dropped {} events", dropped);
            }
            Err(broadcast::error::RecvError::Closed) => {
                tracing::warn!("webhook dispatcher channel closed");
                break;
            }
        }
    }
}

async fn deliver_webhook(
    client: &reqwest::Client,
    webhook: &WebhookRegistration,
    event: &CustodyWebhookEvent,
) {
    let payload = match serde_json::to_vec(event) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::warn!("webhook payload encode failed: {}", error);
            return;
        }
    };

    let signature = super::validation::compute_webhook_signature(&payload, &webhook.secret);

    for attempt in 0..3u32 {
        if attempt > 0 {
            sleep(Duration::from_secs(1 << attempt)).await;
        }

        let result = client
            .post(&webhook.url)
            .header("Content-Type", "application/json")
            .header("X-Custody-Signature", &signature)
            .header("X-Custody-Event", &event.event_type)
            .header("X-Custody-Delivery", &event.event_id)
            .header("X-Custody-Timestamp", event.timestamp.to_string())
            .body(payload.clone())
            .send()
            .await;

        match result {
            Ok(response) => {
                let status = response.status();
                if status.is_success() || status == reqwest::StatusCode::NO_CONTENT {
                    tracing::debug!(
                        "webhook delivered: {} → {} (event={})",
                        event.event_type,
                        webhook.url,
                        event.event_id
                    );
                    return;
                }
                tracing::warn!(
                    "webhook {} returned HTTP {} (attempt {}/3, event={})",
                    webhook.url,
                    status,
                    attempt + 1,
                    event.event_type
                );
            }
            Err(error) => {
                tracing::warn!(
                    "webhook {} delivery failed (attempt {}/3): {}",
                    webhook.url,
                    attempt + 1,
                    error
                );
            }
        }
    }

    tracing::error!(
        "webhook delivery exhausted all retries: {} → {} (event={}, entity={})",
        event.event_type,
        webhook.url,
        event.event_id,
        event.entity_id,
    );
}
