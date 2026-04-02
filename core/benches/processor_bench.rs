// Lichen Core — Performance Benchmarks
//
// Run:  cargo bench --bench processor_bench
// Quick: cargo bench --bench processor_bench -- --warmup-time 1 --measurement-time 3

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
#[cfg(feature = "zk")]
use lichen_core::zk::circuits::shield::ShieldCircuit;
#[cfg(feature = "zk")]
use lichen_core::zk::{commitment_hash, random_scalar_bytes, Prover, Verifier};
use lichen_core::StateStore;
use lichen_core::{
    Account, Block, Hash, Instruction, Keypair, Message, Pubkey, Transaction, TxProcessor,
};
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
        data: bincode::serialize(&(1_000_000u64)).unwrap(), // 0.001 LICN
    };
    let msg = Message::new(vec![ix], recent_blockhash);
    let mut tx = Transaction::new(msg);
    let sig = sender.sign(&tx.message.serialize());
    tx.signatures.push(sig);
    tx
}

#[cfg(feature = "zk")]
fn make_shield_circuit(amount: u64) -> ShieldCircuit {
    let blinding = random_scalar_bytes();
    let commitment = commitment_hash(amount, &blinding);

    ShieldCircuit::new_bytes(amount, amount, blinding, commitment)
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
            let acct = Account::new(1_000_000_000_000, sender.pubkey()); // 1000 LICN
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
                let _block = Block::new(1, parent_hash, state_root, validator_bytes, txs.clone());
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// 3. ML-DSA-65 signature verification
// ---------------------------------------------------------------------------

fn bench_signature_verification(c: &mut Criterion) {
    let mut group = c.benchmark_group("signature_verification");
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.measurement_time(std::time::Duration::from_secs(5));

    // --- Single signature verify ---
    group.bench_function("mldsa65_verify_single", |b| {
        let kp = Keypair::generate();
        let message = b"benchmark payload for signature verification";
        let sig = kp.sign(message);
        let pubkey = kp.pubkey();

        b.iter(|| {
            let _ = Keypair::verify(black_box(&pubkey), black_box(message), black_box(&sig));
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
        group.bench_with_input(BenchmarkId::new("mldsa65_verify_batch", n), &n, |b, &n| {
            let pairs: Vec<_> = (0..n)
                .map(|i| {
                    let kp = Keypair::generate();
                    let msg = format!("message {}", i).into_bytes();
                    let sig = kp.sign(&msg);
                    (kp.pubkey(), msg, sig)
                })
                .collect();

            b.iter(|| {
                for (pubkey, message, sig) in &pairs {
                    let _ = Keypair::verify(
                        black_box(pubkey),
                        black_box(message.as_slice()),
                        black_box(sig),
                    );
                }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// 4. Plonky3 shield prove/verify
// ---------------------------------------------------------------------------

#[cfg(feature = "zk")]
fn bench_shielded_proofs(c: &mut Criterion) {
    let mut group = c.benchmark_group("shielded_proofs");
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.measurement_time(std::time::Duration::from_secs(5));
    group.sample_size(10);

    let amount = 500_000_000u64;

    group.bench_function("plonky3_shield_prove", |b| {
        let prover = Prover::new();

        b.iter(|| {
            let circuit = make_shield_circuit(amount);
            let _ = prover
                .prove_shield(black_box(circuit))
                .expect("prove shield benchmark");
        });
    });

    group.bench_function("plonky3_shield_verify", |b| {
        let prover = Prover::new();
        let proof = prover
            .prove_shield(make_shield_circuit(amount))
            .expect("seed shield proof benchmark");
        let verifier = Verifier::new();

        b.iter(|| {
            let _ = verifier
                .verify(black_box(&proof))
                .expect("verify shield benchmark");
        });
    });

    group.finish();
}

#[cfg(not(feature = "zk"))]
fn bench_shielded_proofs(_: &mut Criterion) {}

// ---------------------------------------------------------------------------
// Criterion harness
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_process_transactions,
    bench_block_creation,
    bench_signature_verification,
    bench_shielded_proofs,
);
criterion_main!(benches);
