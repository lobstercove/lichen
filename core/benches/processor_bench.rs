// MoltChain Core — Performance Benchmarks
//
// Run:  cargo bench --bench processor_bench
// Quick: cargo bench --bench processor_bench -- --warmup-time 1 --measurement-time 3

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use moltchain_core::{
    Account, Block, Hash, Instruction, Keypair, Message, Pubkey, Transaction,
    TxProcessor,
};
use moltchain_core::StateStore;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Open a fresh StateStore backed by a temp directory.
fn fresh_state() -> (StateStore, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("open state");
    (state, dir)
}

/// Create a simple signed transfer transaction.
fn make_signed_transfer(sender: &Keypair, recent_blockhash: Hash) -> Transaction {
    let receiver = Keypair::generate();
    let ix = Instruction {
        program_id: Pubkey([0u8; 32]), // system program
        accounts: vec![sender.pubkey(), receiver.pubkey()],
        data: bincode::serialize(&(1_000_000u64)).unwrap(), // 0.001 MOLT
    };
    let msg = Message::new(vec![ix], recent_blockhash);
    let mut tx = Transaction::new(msg);
    let sig = sender.sign(&tx.message.serialize());
    tx.signatures.push(sig);
    tx
}

// ---------------------------------------------------------------------------
// 1. Transaction processing throughput
// ---------------------------------------------------------------------------

fn bench_process_transactions(c: &mut Criterion) {
    let mut group = c.benchmark_group("tx_processing");
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.measurement_time(std::time::Duration::from_secs(5));

    for &n in &[1, 10, 50] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("process_n_txs", n), &n, |b, &n| {
            // Setup outside the measurement loop
            let (state, _dir) = fresh_state();
            let sender = Keypair::generate();

            // Fund the sender account in state
            let acct = Account::new(1_000_000_000_000, sender.pubkey()); // 1000 MOLT
            state
                .put_account(&sender.pubkey(), &acct)
                .expect("fund sender");

            // Store a recent blockhash so the processor can validate it
            let genesis = Block::genesis(Hash::hash(b"bench"), 1, Vec::new());
            state.put_block(&genesis).expect("put genesis");

            let recent_hash = genesis.hash();

            let processor = TxProcessor::new(state);
            let validator = Pubkey([42u8; 32]);

            // Pre-generate transactions
            let txs: Vec<Transaction> = (0..n)
                .map(|_| make_signed_transfer(&sender, recent_hash))
                .collect();

            b.iter(|| {
                for tx in &txs {
                    let _ = processor.process_transaction(tx, &validator);
                }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// 2. Block creation
// ---------------------------------------------------------------------------

fn bench_block_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("block_creation");
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.measurement_time(std::time::Duration::from_secs(5));

    for &n in &[0, 10, 100, 500] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("new_block_txs", n), &n, |b, &n| {
            let sender = Keypair::generate();
            let recent_hash = Hash::hash(b"bench_block");
            let parent_hash = Hash::hash(b"parent");
            let state_root = Hash::hash(b"state");
            let validator_bytes = [42u8; 32];

            let txs: Vec<Transaction> = (0..n)
                .map(|_| make_signed_transfer(&sender, recent_hash))
                .collect();

            b.iter(|| {
                let _block = Block::new(
                    1,
                    parent_hash,
                    state_root,
                    validator_bytes,
                    txs.clone(),
                );
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// 3. Ed25519 signature verification
// ---------------------------------------------------------------------------

fn bench_signature_verification(c: &mut Criterion) {
    let mut group = c.benchmark_group("signature_verification");
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.measurement_time(std::time::Duration::from_secs(5));

    // --- Single signature verify ---
    group.bench_function("ed25519_verify_single", |b| {
        let kp = Keypair::generate();
        let message = b"benchmark payload for signature verification";
        let sig = kp.sign(message);
        let pubkey = kp.pubkey();

        b.iter(|| {
            let _ = Keypair::verify(&pubkey, message, &sig);
        });
    });

    // --- Block signature verify ---
    group.bench_function("block_verify_signature", |b| {
        let kp = Keypair::generate();
        let mut block = Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::hash(b"state"),
            kp.pubkey().0,
            Vec::new(),
            1000,
        );
        block.sign(&kp);

        b.iter(|| {
            let _ = block.verify_signature();
        });
    });

    // --- Batch signature verification (N signatures) ---
    for &n in &[10, 50, 100] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("ed25519_verify_batch", n), &n, |b, &n| {
            let pairs: Vec<(Keypair, [u8; 64])> = (0..n)
                .map(|i| {
                    let kp = Keypair::generate();
                    let msg = format!("message {}", i);
                    let sig = kp.sign(msg.as_bytes());
                    (kp, sig)
                })
                .collect();

            let messages: Vec<String> = (0..n).map(|i| format!("message {}", i)).collect();

            b.iter(|| {
                for (i, (kp, sig)) in pairs.iter().enumerate() {
                    let _ = Keypair::verify(&kp.pubkey(), messages[i].as_bytes(), sig);
                }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion harness
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_process_transactions,
    bench_block_creation,
    bench_signature_verification,
);
criterion_main!(benches);
