#![no_main]
use libfuzzer_sys::fuzz_target;
use lichen_core::codec::serialize_legacy_bincode;
use lichen_core::Hash;
use lichen_p2p::{MessageType, P2PMessage, P2P_PROTOCOL_VERSION};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fuzz_target!(|data: &[u8]| {
    let _ = P2PMessage::deserialize(data);

    let mut uncompressed = Vec::with_capacity(data.len() + 1);
    uncompressed.push(0x00);
    uncompressed.extend_from_slice(data);
    let _ = P2PMessage::deserialize(&uncompressed);

    let mut compressed = Vec::with_capacity(data.len() + 5);
    compressed.push(0xFF);
    compressed.extend_from_slice(&(data.len() as u32).to_le_bytes());
    compressed.extend_from_slice(data);
    let _ = P2PMessage::deserialize(&compressed);

    let message = P2PMessage::new(message_type_from_bytes(data), loopback_addr(data));
    let encoded = message
        .serialize()
        .expect("valid P2P message must serialize");
    let decoded = P2PMessage::deserialize(&encoded).expect("valid P2P message must round-trip");
    assert_eq!(decoded.version, P2P_PROTOCOL_VERSION);

    let mut legacy = message.clone();
    legacy.version = if data.first().copied().unwrap_or(0) & 1 == 0 {
        P2P_PROTOCOL_VERSION
    } else {
        P2P_PROTOCOL_VERSION.saturating_add(1)
    };
    let legacy_wire =
        serialize_legacy_bincode(&legacy, "fuzz P2P legacy message").expect("legacy P2P encode");
    let legacy_result = P2PMessage::deserialize(&legacy_wire);
    if legacy.version == P2P_PROTOCOL_VERSION {
        assert!(legacy_result.is_ok());
    } else {
        assert!(legacy_result.is_err());
    }

    if let Ok(decoded) = P2PMessage::deserialize(data) {
        assert_eq!(decoded.version, P2P_PROTOCOL_VERSION);
        let reencoded = decoded.serialize().expect("decoded P2P message re-encodes");
        assert!(P2PMessage::deserialize(&reencoded).is_ok());
    }
});

fn loopback_addr(data: &[u8]) -> SocketAddr {
    let port = u16::from_le_bytes([
        data.get(1).copied().unwrap_or(0),
        data.get(2).copied().unwrap_or(0),
    ])
    .max(1);
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

fn message_type_from_bytes(data: &[u8]) -> MessageType {
    match data.first().copied().unwrap_or(0) % 6 {
        0 => MessageType::Ping,
        1 => MessageType::Pong,
        2 => MessageType::StatusRequest,
        3 => MessageType::StatusResponse {
            current_slot: read_u64(data, 1),
            total_blocks: read_u64(data, 9),
        },
        4 => MessageType::BlockRequest {
            slot: read_u64(data, 1),
        },
        _ => MessageType::ConsistencyReport {
            current_slot: read_u64(data, 1),
            validator_set_hash: Hash::hash(data),
            stake_pool_hash: Hash::hash(&data.iter().rev().copied().collect::<Vec<_>>()),
        },
    }
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    for (idx, byte) in bytes.iter_mut().enumerate() {
        *byte = data.get(offset + idx).copied().unwrap_or(0);
    }
    u64::from_le_bytes(bytes)
}
