use crate::{Client, Error, Keypair, Pubkey, Result};
use serde::{Deserialize, Serialize};

const PROGRAM_SYMBOL_CANDIDATES: [&str; 2] = ["SPOREPAY", "sporepay"];
const STREAM_SIZE: usize = 105;
const STREAM_INFO_SIZE: usize = 113;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SporePayStream {
    pub stream_id: u64,
    pub sender: String,
    pub recipient: String,
    pub total_amount: u64,
    pub withdrawn_amount: u64,
    pub start_slot: u64,
    pub end_slot: u64,
    pub cancelled: bool,
    pub created_slot: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SporePayStreamInfo {
    #[serde(flatten)]
    pub stream: SporePayStream,
    pub cliff_slot: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SporePayStats {
    pub stream_count: u64,
    pub total_streamed: u64,
    pub total_withdrawn: u64,
    pub cancel_count: u64,
    pub paused: bool,
}

#[derive(Debug, Clone)]
pub struct CreateStreamParams {
    pub recipient: Pubkey,
    pub total_amount: u64,
    pub start_slot: u64,
    pub end_slot: u64,
}

#[derive(Debug, Clone)]
pub struct CreateStreamWithCliffParams {
    pub recipient: Pubkey,
    pub total_amount: u64,
    pub start_slot: u64,
    pub end_slot: u64,
    pub cliff_slot: u64,
}

#[derive(Debug, Clone)]
pub struct WithdrawFromStreamParams {
    pub stream_id: u64,
    pub amount: u64,
}

#[derive(Debug, Clone)]
pub struct TransferStreamParams {
    pub stream_id: u64,
    pub new_recipient: Pubkey,
}

#[derive(Debug, Clone)]
pub struct SporePayClient {
    client: Client,
    program_id: std::sync::Arc<std::sync::Mutex<Option<Pubkey>>>,
}

fn build_layout_args(layout: &[u8], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + layout.len() + chunks.iter().map(|chunk| chunk.len()).sum::<usize>());
    out.push(0xAB);
    out.extend_from_slice(layout);
    for chunk in chunks {
        out.extend_from_slice(chunk);
    }
    out
}

fn encode_create_stream_args(sender: &Pubkey, params: &CreateStreamParams) -> Vec<u8> {
    build_layout_args(&[0x20, 0x20, 0x08, 0x08, 0x08], &[
        sender.as_ref().to_vec(),
        params.recipient.as_ref().to_vec(),
        params.total_amount.to_le_bytes().to_vec(),
        params.start_slot.to_le_bytes().to_vec(),
        params.end_slot.to_le_bytes().to_vec(),
    ])
}

fn encode_create_stream_with_cliff_args(sender: &Pubkey, params: &CreateStreamWithCliffParams) -> Vec<u8> {
    build_layout_args(&[0x20, 0x20, 0x08, 0x08, 0x08, 0x08], &[
        sender.as_ref().to_vec(),
        params.recipient.as_ref().to_vec(),
        params.total_amount.to_le_bytes().to_vec(),
        params.start_slot.to_le_bytes().to_vec(),
        params.end_slot.to_le_bytes().to_vec(),
        params.cliff_slot.to_le_bytes().to_vec(),
    ])
}

fn encode_withdraw_args(caller: &Pubkey, params: &WithdrawFromStreamParams) -> Vec<u8> {
    build_layout_args(&[0x20, 0x08, 0x08], &[
        caller.as_ref().to_vec(),
        params.stream_id.to_le_bytes().to_vec(),
        params.amount.to_le_bytes().to_vec(),
    ])
}

fn encode_cancel_args(caller: &Pubkey, stream_id: u64) -> Vec<u8> {
    build_layout_args(&[0x20, 0x08], &[
        caller.as_ref().to_vec(),
        stream_id.to_le_bytes().to_vec(),
    ])
}

fn encode_transfer_args(caller: &Pubkey, params: &TransferStreamParams) -> Vec<u8> {
    build_layout_args(&[0x20, 0x20, 0x08], &[
        caller.as_ref().to_vec(),
        params.new_recipient.as_ref().to_vec(),
        params.stream_id.to_le_bytes().to_vec(),
    ])
}

fn encode_stream_lookup_args(stream_id: u64) -> Vec<u8> {
    build_layout_args(&[0x08], &[stream_id.to_le_bytes().to_vec()])
}

fn ensure_readonly_success(
    result: &crate::client::ReadonlyContractResult,
    function_name: &str,
) -> Result<()> {
    let code = result.return_code.unwrap_or(0);
    if code != 0 {
        return Err(Error::RpcError(
            result
                .error
                .clone()
                .unwrap_or_else(|| format!("SporePay {} returned code {}", function_name, code)),
        ));
    }
    if !result.success {
        return Err(Error::RpcError(
            result
                .error
                .clone()
                .unwrap_or_else(|| format!("SporePay {} failed", function_name)),
        ));
    }
    Ok(())
}

fn decode_return_data(result: &crate::client::ReadonlyContractResult, function_name: &str) -> Result<Vec<u8>> {
    let Some(return_data) = &result.return_data else {
        return Err(Error::ParseError(format!(
            "SporePay {} did not return payload data",
            function_name,
        )));
    };

    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, return_data)
        .map_err(|err| Error::ParseError(err.to_string()))
}

fn decode_stream(stream_id: u64, bytes: &[u8]) -> Result<SporePayStream> {
    if bytes.len() < STREAM_SIZE {
        return Err(Error::ParseError(
            "SporePay stream payload was shorter than expected".into(),
        ));
    }

    Ok(SporePayStream {
        stream_id,
        sender: Pubkey(bytes[0..32].try_into().unwrap()).to_base58(),
        recipient: Pubkey(bytes[32..64].try_into().unwrap()).to_base58(),
        total_amount: u64::from_le_bytes(bytes[64..72].try_into().unwrap()),
        withdrawn_amount: u64::from_le_bytes(bytes[72..80].try_into().unwrap()),
        start_slot: u64::from_le_bytes(bytes[80..88].try_into().unwrap()),
        end_slot: u64::from_le_bytes(bytes[88..96].try_into().unwrap()),
        cancelled: bytes[96] == 1,
        created_slot: u64::from_le_bytes(bytes[97..105].try_into().unwrap()),
    })
}

fn decode_stream_info(stream_id: u64, bytes: &[u8]) -> Result<SporePayStreamInfo> {
    if bytes.len() < STREAM_INFO_SIZE {
        return Err(Error::ParseError(
            "SporePay stream-info payload was shorter than expected".into(),
        ));
    }

    Ok(SporePayStreamInfo {
        stream: decode_stream(stream_id, bytes)?,
        cliff_slot: u64::from_le_bytes(bytes[105..113].try_into().unwrap()),
    })
}

impl SporePayClient {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            program_id: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub fn with_program_id(client: Client, program_id: Pubkey) -> Self {
        Self {
            client,
            program_id: std::sync::Arc::new(std::sync::Mutex::new(Some(program_id))),
        }
    }

    pub async fn get_program_id(&self) -> Result<Pubkey> {
        if let Some(program_id) = self
            .program_id
            .lock()
            .map_err(|_| Error::ConfigError("SporePayClient program cache lock poisoned".into()))?
            .clone()
        {
            return Ok(program_id);
        }

        for symbol in PROGRAM_SYMBOL_CANDIDATES {
            let entry = match self.client.get_symbol_registry(symbol).await {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let Some(program) = entry.get("program").and_then(|value| value.as_str()) else {
                continue;
            };
            let program_id = Pubkey::from_base58(program).map_err(Error::ParseError)?;
            *self
                .program_id
                .lock()
                .map_err(|_| Error::ConfigError("SporePayClient program cache lock poisoned".into()))? = Some(program_id);
            return Ok(program_id);
        }

        Err(Error::ConfigError(
            "Unable to resolve the SporePay program via getSymbolRegistry(\"SPOREPAY\")".into(),
        ))
    }

    pub async fn get_stream(&self, stream_id: u64) -> Result<Option<SporePayStream>> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_stream",
                encode_stream_lookup_args(stream_id),
                None,
            )
            .await?;

        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }

        ensure_readonly_success(&result, "get_stream")?;
        let bytes = decode_return_data(&result, "get_stream")?;
        decode_stream(stream_id, &bytes).map(Some)
    }

    pub async fn get_stream_info(&self, stream_id: u64) -> Result<Option<SporePayStreamInfo>> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_stream_info",
                encode_stream_lookup_args(stream_id),
                None,
            )
            .await?;

        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }

        ensure_readonly_success(&result, "get_stream_info")?;
        let bytes = decode_return_data(&result, "get_stream_info")?;
        decode_stream_info(stream_id, &bytes).map(Some)
    }

    pub async fn get_withdrawable(&self, stream_id: u64) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_withdrawable",
                encode_stream_lookup_args(stream_id),
                None,
            )
            .await?;

        ensure_readonly_success(&result, "get_withdrawable")?;
        let bytes = decode_return_data(&result, "get_withdrawable")?;
        if bytes.len() < 8 {
            return Err(Error::ParseError(
                "SporePay withdrawable payload was shorter than expected".into(),
            ));
        }
        Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
    }

    pub async fn get_stats(&self) -> Result<SporePayStats> {
        let value = self.client.get_sporepay_stats().await?;
        serde_json::from_value(value).map_err(|err| Error::ParseError(err.to_string()))
    }

    pub async fn create_stream(&self, sender: &Keypair, params: CreateStreamParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_create_stream_args(&sender.pubkey(), &params);
        self.client
            .call_contract(sender, &program_id, "create_stream", args, 0)
            .await
    }

    pub async fn create_stream_with_cliff(&self, sender: &Keypair, params: CreateStreamWithCliffParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_create_stream_with_cliff_args(&sender.pubkey(), &params);
        self.client
            .call_contract(sender, &program_id, "create_stream_with_cliff", args, 0)
            .await
    }

    pub async fn withdraw_from_stream(&self, recipient: &Keypair, params: WithdrawFromStreamParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_withdraw_args(&recipient.pubkey(), &params);
        self.client
            .call_contract(recipient, &program_id, "withdraw_from_stream", args, 0)
            .await
    }

    pub async fn cancel_stream(&self, sender: &Keypair, stream_id: u64) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_cancel_args(&sender.pubkey(), stream_id);
        self.client
            .call_contract(sender, &program_id, "cancel_stream", args, 0)
            .await
    }

    pub async fn transfer_stream(&self, recipient: &Keypair, params: TransferStreamParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_transfer_args(&recipient.pubkey(), &params);
        self.client
            .call_contract(recipient, &program_id, "transfer_stream", args, 0)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_stream_encoding_includes_all_u64_layout_entries() {
        let sender = Pubkey([1u8; 32]);
        let encoded = encode_create_stream_args(
            &sender,
            &CreateStreamParams {
                recipient: Pubkey([2u8; 32]),
                total_amount: 100,
                start_slot: 10,
                end_slot: 20,
            },
        );
        assert_eq!(&encoded[..6], &[0xAB, 0x20, 0x20, 0x08, 0x08, 0x08]);
    }

    #[test]
    fn decode_stream_info_reads_cliff_slot() {
        let mut payload = vec![0u8; STREAM_INFO_SIZE];
        payload[64..72].copy_from_slice(&100u64.to_le_bytes());
        payload[72..80].copy_from_slice(&25u64.to_le_bytes());
        payload[80..88].copy_from_slice(&10u64.to_le_bytes());
        payload[88..96].copy_from_slice(&20u64.to_le_bytes());
        payload[97..105].copy_from_slice(&9u64.to_le_bytes());
        payload[105..113].copy_from_slice(&12u64.to_le_bytes());

        let info = decode_stream_info(7, &payload).unwrap();
        assert_eq!(info.stream.stream_id, 7);
        assert_eq!(info.cliff_slot, 12);
    }
}