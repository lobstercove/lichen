use lichen_core::Pubkey;

fn build_named_export_args(layout: &[u8], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        1 + layout.len() + chunks.iter().map(|chunk| chunk.len()).sum::<usize>(),
    );
    out.push(0xAB);
    out.extend_from_slice(layout);
    for chunk in chunks {
        out.extend_from_slice(chunk);
    }
    out
}

pub(super) fn encode_single_address_arg(address: &Pubkey) -> Vec<u8> {
    build_named_export_args(&[0x20], &[address.0.to_vec()])
}

pub(super) fn encode_dual_address_amount_args(
    first: &Pubkey,
    second: &Pubkey,
    amount: u64,
) -> Vec<u8> {
    build_named_export_args(
        &[0x20, 0x20, 0x08],
        &[
            first.0.to_vec(),
            second.0.to_vec(),
            amount.to_le_bytes().to_vec(),
        ],
    )
}
