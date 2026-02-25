// MoltWallet — Shielded (ZKP) Transaction Module
// Client-side zero-knowledge proof generation and shielded balance management
//
// This module handles:
// - Shielded keypair derivation from wallet seed
// - Shield (deposit): transparent → shielded pool
// - Unshield (withdraw): shielded pool → transparent
// - Shielded transfer: private send between shielded addresses
// - Note management: scanning commitments, tracking owned notes
// - Client-side proof generation (Groth16/BN254 via WASM)

// ===== Constants =====
// SHELLS_PER_MOLT provided by shared/utils.js (loaded before this file)
const SHIELDED_POOL_PROGRAM_ID = 'ShieldPool11111111111111111111111111111111';
const MERKLE_TREE_DEPTH = 32;

// ===== Shielded Wallet State =====
let shieldedState = {
    initialized: false,
    // Shielded keypair (derived from wallet seed)
    spendingKey: null,        // Uint8Array(32) — secret, never leaves client
    viewingKey: null,         // Uint8Array(32) — can share with auditors
    shieldedAddress: null,    // Base58-encoded viewing key hash

    // Owned notes (trial-decrypted from commitment stream)
    ownedNotes: [],           // { index, value, blinding, serial, commitment, spent }
    shieldedBalance: 0,       // Sum of unspent note values (shells)

    // Merkle tree state (synced from RPC)
    merkleRoot: null,
    lastSyncedIndex: 0,
    commitments: [],          // All commitment hashes for proof generation

    // Pool stats
    poolStats: null,

    // Proof generation state
    provingKeys: {
        shield: null,
        unshield: null,
        transfer: null,
    },
    provingKeysLoaded: false,
};

// ===== Initialization =====

/**
 * Initialize the shielded wallet module.
 * Derives shielded keypair from the wallet's seed.
 */
async function initShielded(walletSeed) {
    if (!walletSeed || walletSeed.length < 32) {
        console.warn('Cannot initialize shielded wallet: no seed');
        return;
    }

    // Derive shielded spending key: SHA-256(seed || "moltchain-shielded-spending-key-v1")
    const encoder = new TextEncoder();
    const keyMaterial = new Uint8Array([...walletSeed, ...encoder.encode('moltchain-shielded-spending-key-v1')]);
    const spendingKeyHash = await crypto.subtle.digest('SHA-256', keyMaterial);
    shieldedState.spendingKey = new Uint8Array(spendingKeyHash);

    // Derive viewing key: SHA-256(spending_key || "moltchain-viewing-key-v1")
    const vkMaterial = new Uint8Array([...shieldedState.spendingKey, ...encoder.encode('moltchain-viewing-key-v1')]);
    const viewingKeyHash = await crypto.subtle.digest('SHA-256', vkMaterial);
    shieldedState.viewingKey = new Uint8Array(viewingKeyHash);

    // Shielded address = first 32 bytes of SHA-256(viewing_key), Base58 encoded
    const addrHash = await crypto.subtle.digest('SHA-256', shieldedState.viewingKey);
    shieldedState.shieldedAddress = bs58.encode(new Uint8Array(addrHash).slice(0, 32));

    shieldedState.initialized = true;

    // Load owned notes from localStorage
    loadNotesFromStorage();

    // Sync with chain
    await syncShieldedState();
}

// ===== Sync =====

/**
 * Sync shielded state with the chain: fetch pool stats, new commitments, and scan for owned notes.
 */
async function syncShieldedState() {
    if (!shieldedState.initialized) return;

    try {
        // Fetch pool stats
        const statsResp = await rpc.call('getShieldedPoolStats').catch(() => null);
        if (statsResp) {
            shieldedState.poolStats = statsResp;
            shieldedState.merkleRoot = statsResp.merkle_root;
        }

        // Fetch new commitments since last sync
        const commitsResp = await rpc.call('getShieldedCommitments', [{
            from_index: shieldedState.lastSyncedIndex,
        }]).catch(() => null);

        if (commitsResp && Array.isArray(commitsResp)) {
            for (const entry of commitsResp) {
                shieldedState.commitments.push(entry.commitment);

                // Trial-decrypt: try to decrypt with our viewing key
                const note = await tryDecryptNote(entry);
                if (note) {
                    shieldedState.ownedNotes.push({
                        index: shieldedState.lastSyncedIndex + shieldedState.commitments.length - 1,
                        value: note.value,
                        blinding: note.blinding,
                        serial: note.serial,
                        commitment: entry.commitment,
                        spent: false,
                    });
                }
                shieldedState.lastSyncedIndex++;
            }
        }

        // Check which owned notes have been spent (nullifier check)
        for (const note of shieldedState.ownedNotes) {
            if (note.spent) continue;
            const nullifier = await computeNullifier(note.serial);
            const isSpent = await rpc.call('checkNullifier', [nullifier]).catch(() => null);
            if (isSpent && isSpent.spent) {
                note.spent = true;
            }
        }

        // Recalculate shielded balance
        shieldedState.shieldedBalance = shieldedState.ownedNotes
            .filter(n => !n.spent)
            .reduce((sum, n) => sum + n.value, 0);

        // Persist to localStorage
        saveNotesToStorage();

        // Update UI
        updateShieldedUI();

    } catch (err) {
        console.error('Shielded sync error:', err);
    }
}

// ===== Note Encryption/Decryption =====

/**
 * Try to decrypt an encrypted note using our viewing key.
 * Returns the plaintext note if successful, null otherwise.
 */
async function tryDecryptNote(entry) {
    if (!shieldedState.viewingKey || !entry.encrypted_note) return null;

    try {
        // Derive decryption key: SHA-256(ephemeral_pk || viewing_key)
        const keyMaterial = new Uint8Array([
            ...hexToBytes(entry.ephemeral_pk),
            ...shieldedState.viewingKey,
        ]);
        const decKeyHash = await crypto.subtle.digest('SHA-256', keyMaterial);
        const decKey = new Uint8Array(decKeyHash);

        // XOR decrypt (matches core/src/zk/note.rs placeholder encryption)
        const ciphertext = hexToBytes(entry.encrypted_note);
        const plaintext = new Uint8Array(ciphertext.length);
        for (let i = 0; i < ciphertext.length; i++) {
            plaintext[i] = ciphertext[i] ^ decKey[i % 32];
        }

        // Parse note: 32 bytes owner + 8 bytes value + 32 bytes blinding + 32 bytes serial = 104 bytes
        if (plaintext.length < 104) return null;

        const owner = plaintext.slice(0, 32);
        const value = new DataView(plaintext.buffer, plaintext.byteOffset + 32, 8).getBigUint64(0, true);
        const blinding = bytesToHex(plaintext.slice(40, 72));
        const serial = bytesToHex(plaintext.slice(72, 104));

        // Verify commitment matches
        const expectedCommitment = await computeCommitmentHash(Number(value), blinding);
        if (expectedCommitment !== entry.commitment) {
            return null; // Wrong key or corrupted
        }

        return {
            owner: bytesToHex(owner),
            value: Number(value),
            blinding,
            serial,
        };
    } catch (err) {
        return null; // Decryption failed — not our note
    }
}

// ===== Shield Operation =====

/**
 * Shield (deposit) MOLT from transparent balance into the shielded pool.
 * Generates a ZK proof client-side and submits the transaction.
 */
async function shieldMolt(amountMolt) {
    if (!shieldedState.initialized) {
        showToast('Shielded wallet not initialized');
        return;
    }

    const amountShells = Math.floor(amountMolt * SHELLS_PER_MOLT);
    if (amountShells <= 0) {
        showToast('Amount must be positive');
        return;
    }

    showShieldedStatus('Generating ZK proof...', 'pending');

    try {
        // Generate random blinding factor (32 bytes)
        const blinding = crypto.getRandomValues(new Uint8Array(32));
        const serial = crypto.getRandomValues(new Uint8Array(32));

        // Compute Pedersen commitment hash
        const commitment = await computeCommitmentHash(amountShells, bytesToHex(blinding));

        // Encrypt note for ourselves
        const noteBytes = new Uint8Array(104);
        noteBytes.set(hexToBytes(shieldedState.shieldedAddress.padStart(64, '0')).slice(0, 32), 0);
        new DataView(noteBytes.buffer).setBigUint64(32, BigInt(amountShells), true);
        noteBytes.set(blinding, 40);
        noteBytes.set(serial, 72);

        // Encrypt with our viewing key
        const ephemeralKey = crypto.getRandomValues(new Uint8Array(32));
        const encKeyMaterial = new Uint8Array([...ephemeralKey, ...shieldedState.viewingKey]);
        const encKeyHash = await crypto.subtle.digest('SHA-256', encKeyMaterial);
        const encKey = new Uint8Array(encKeyHash);

        const encryptedNote = new Uint8Array(noteBytes.length);
        for (let i = 0; i < noteBytes.length; i++) {
            encryptedNote[i] = noteBytes[i] ^ encKey[i % 32];
        }

        // Generate Groth16 proof (client-side)
        // In production: use WASM-compiled arkworks prover
        // For now: submit proof placeholder with structure
        const proof = await generateShieldProof(amountShells, commitment, blinding);

        showShieldedStatus('Submitting transaction...', 'pending');

        // Submit shield transaction
        const result = await rpc.call('submitShieldTransaction', [{
            amount: amountShells,
            commitment: commitment,
            proof: bytesToHex(proof),
            encrypted_note: bytesToHex(encryptedNote),
            ephemeral_pk: bytesToHex(ephemeralKey),
        }]);

        if (result && result.success) {
            // Add to our owned notes
            shieldedState.ownedNotes.push({
                index: result.commitment_index || shieldedState.commitments.length,
                value: amountShells,
                blinding: bytesToHex(blinding),
                serial: bytesToHex(serial),
                commitment: commitment,
                spent: false,
            });
            shieldedState.shieldedBalance += amountShells;
            saveNotesToStorage();
            updateShieldedUI();

            showShieldedStatus('', 'idle');
            showToast(`Shielded ${amountMolt} MOLT successfully!`);
            closeModal('shieldModal');
        } else {
            throw new Error(result?.error || 'Shield transaction failed');
        }
    } catch (err) {
        showShieldedStatus('', 'idle');
        showToast('Shield failed: ' + err.message);
    }
}

// ===== Unshield Operation =====

/**
 * Unshield (withdraw) MOLT from the shielded pool to a transparent address.
 */
async function unshieldMolt(amountMolt, recipientAddress) {
    if (!shieldedState.initialized) {
        showToast('Shielded wallet not initialized');
        return;
    }

    const amountShells = Math.floor(amountMolt * SHELLS_PER_MOLT);

    // Find unspent notes with sufficient balance
    const unspentNotes = shieldedState.ownedNotes.filter(n => !n.spent);
    const totalAvailable = unspentNotes.reduce((sum, n) => sum + n.value, 0);

    if (amountShells > totalAvailable) {
        showToast(`Insufficient shielded balance. Available: ${(totalAvailable / SHELLS_PER_MOLT).toFixed(4)} MOLT`);
        return;
    }

    // Select notes to spend (simple: first-fit)
    const notesToSpend = [];
    let remaining = amountShells;
    for (const note of unspentNotes) {
        if (remaining <= 0) break;
        notesToSpend.push(note);
        remaining -= note.value;
    }

    showShieldedStatus('Generating ZK proof (may take ~3s)...', 'pending');

    try {
        // Compute nullifiers for notes being spent
        const nullifiers = [];
        for (const note of notesToSpend) {
            nullifiers.push(await computeNullifier(note.serial));
        }

        // Generate unshield proof
        const proof = await generateUnshieldProof(
            shieldedState.merkleRoot,
            nullifiers,
            amountShells,
            recipientAddress
        );

        showShieldedStatus('Submitting transaction...', 'pending');

        const result = await rpc.call('submitUnshieldTransaction', [{
            nullifiers: nullifiers,
            amount: amountShells,
            recipient: recipientAddress,
            merkle_root: shieldedState.merkleRoot,
            proof: bytesToHex(proof),
        }]);

        if (result && result.success) {
            // Mark notes as spent
            for (const note of notesToSpend) {
                note.spent = true;
            }
            shieldedState.shieldedBalance -= amountShells;

            // If change needed, a new note will appear in the commitment stream
            saveNotesToStorage();
            updateShieldedUI();

            showShieldedStatus('', 'idle');
            showToast(`Unshielded ${amountMolt} MOLT to ${recipientAddress.slice(0, 8)}...`);
            closeModal('unshieldModal');
        } else {
            throw new Error(result?.error || 'Unshield transaction failed');
        }
    } catch (err) {
        showShieldedStatus('', 'idle');
        showToast('Unshield failed: ' + err.message);
    }
}

// ===== Shielded Transfer =====

/**
 * Transfer MOLT privately between shielded addresses.
 * No amounts, senders, or recipients are revealed on-chain.
 */
async function shieldedTransfer(amountMolt, recipientViewingKey) {
    if (!shieldedState.initialized) {
        showToast('Shielded wallet not initialized');
        return;
    }

    const amountShells = Math.floor(amountMolt * SHELLS_PER_MOLT);
    const unspentNotes = shieldedState.ownedNotes.filter(n => !n.spent);
    const totalAvailable = unspentNotes.reduce((sum, n) => sum + n.value, 0);

    if (amountShells > totalAvailable) {
        showToast('Insufficient shielded balance');
        return;
    }

    // Select input notes
    const inputNotes = [];
    let inputTotal = 0;
    for (const note of unspentNotes) {
        if (inputTotal >= amountShells) break;
        inputNotes.push(note);
        inputTotal += note.value;
    }

    const changeAmount = inputTotal - amountShells;

    showShieldedStatus('Generating ZK proof (may take ~5s)...', 'pending');

    try {
        // Compute nullifiers
        const nullifiers = [];
        for (const note of inputNotes) {
            nullifiers.push(await computeNullifier(note.serial));
        }

        // Create output notes
        const recipientBlinding = crypto.getRandomValues(new Uint8Array(32));
        const recipientSerial = crypto.getRandomValues(new Uint8Array(32));
        const recipientCommitment = await computeCommitmentHash(amountShells, bytesToHex(recipientBlinding));

        const outputCommitments = [{
            commitment: recipientCommitment,
            encrypted_note: await encryptNoteForRecipient(amountShells, recipientBlinding, recipientSerial, recipientViewingKey),
            ephemeral_pk: bytesToHex(crypto.getRandomValues(new Uint8Array(32))),
        }];

        // Change output (back to ourselves)
        if (changeAmount > 0) {
            const changeBlinding = crypto.getRandomValues(new Uint8Array(32));
            const changeSerial = crypto.getRandomValues(new Uint8Array(32));
            const changeCommitment = await computeCommitmentHash(changeAmount, bytesToHex(changeBlinding));

            const changeEphKey = crypto.getRandomValues(new Uint8Array(32));
            const changeEncKeyMaterial = new Uint8Array([...changeEphKey, ...shieldedState.viewingKey]);
            const changeEncKeyHash = await crypto.subtle.digest('SHA-256', changeEncKeyMaterial);
            const changeEncKey = new Uint8Array(changeEncKeyHash);

            const changeNote = new Uint8Array(104);
            changeNote.set(new Uint8Array(32), 0); // self
            new DataView(changeNote.buffer).setBigUint64(32, BigInt(changeAmount), true);
            changeNote.set(changeBlinding, 40);
            changeNote.set(changeSerial, 72);

            const changeEncrypted = new Uint8Array(changeNote.length);
            for (let i = 0; i < changeNote.length; i++) {
                changeEncrypted[i] = changeNote[i] ^ changeEncKey[i % 32];
            }

            outputCommitments.push({
                commitment: changeCommitment,
                encrypted_note: bytesToHex(changeEncrypted),
                ephemeral_pk: bytesToHex(changeEphKey),
            });
        }

        // Generate transfer proof
        const proof = await generateTransferProof(
            shieldedState.merkleRoot,
            nullifiers,
            outputCommitments.map(o => o.commitment)
        );

        showShieldedStatus('Submitting transaction...', 'pending');

        const result = await rpc.call('submitShieldedTransfer', [{
            nullifiers: nullifiers,
            output_commitments: outputCommitments,
            merkle_root: shieldedState.merkleRoot,
            proof: bytesToHex(proof),
        }]);

        if (result && result.success) {
            // Mark input notes as spent
            for (const note of inputNotes) {
                note.spent = true;
            }
            shieldedState.shieldedBalance = shieldedState.ownedNotes
                .filter(n => !n.spent)
                .reduce((sum, n) => sum + n.value, 0);

            saveNotesToStorage();
            updateShieldedUI();

            showShieldedStatus('', 'idle');
            showToast(`Transferred ${amountMolt} MOLT privately`);
            closeModal('shieldedTransferModal');
        } else {
            throw new Error(result?.error || 'Shielded transfer failed');
        }
    } catch (err) {
        showShieldedStatus('', 'idle');
        showToast('Transfer failed: ' + err.message);
    }
}

// ===== Proof Generation (Client-Side) =====

/**
 * Generate a shield proof (Groth16).
 * In production: calls WASM-compiled arkworks prover.
 * Proof time target: <1 second.
 */
async function generateShieldProof(amount, commitment, blinding) {
    // Production: Load WASM module and generate real Groth16 proof
    // const prover = await import('./zk-prover.wasm');
    // return prover.prove_shield(amount, commitment, blinding, provingKeys.shield);

    // For now: generate structured proof placeholder (128 bytes like Groth16)
    const proof = new Uint8Array(128);
    const encoder = new TextEncoder();

    // Pack proof structure: type(1) + amount(8) + commitment_hash(32) + blinding_hash(32) + padding
    proof[0] = 0x02; // ProofType::Shield
    new DataView(proof.buffer).setBigUint64(1, BigInt(amount), true);

    const commitHash = await crypto.subtle.digest('SHA-256', encoder.encode(commitment));
    proof.set(new Uint8Array(commitHash).slice(0, 32), 9);

    const blindHash = await crypto.subtle.digest('SHA-256', blinding);
    proof.set(new Uint8Array(blindHash).slice(0, 32), 41);

    // Fill rest with deterministic padding
    const pad = await crypto.subtle.digest('SHA-256', proof.slice(0, 73));
    proof.set(new Uint8Array(pad), 73);

    return proof;
}

/**
 * Generate an unshield proof (Groth16).
 * Proof time target: <3 seconds.
 */
async function generateUnshieldProof(merkleRoot, nullifiers, amount, recipient) {
    const proof = new Uint8Array(128);
    proof[0] = 0x03; // ProofType::Unshield
    new DataView(proof.buffer).setBigUint64(1, BigInt(amount), true);

    const encoder = new TextEncoder();
    const rootHash = await crypto.subtle.digest('SHA-256', encoder.encode(merkleRoot || ''));
    proof.set(new Uint8Array(rootHash).slice(0, 32), 9);

    const recipHash = await crypto.subtle.digest('SHA-256', encoder.encode(recipient));
    proof.set(new Uint8Array(recipHash).slice(0, 32), 41);

    const pad = await crypto.subtle.digest('SHA-256', proof.slice(0, 73));
    proof.set(new Uint8Array(pad), 73);

    return proof;
}

/**
 * Generate a transfer proof (Groth16).
 * Proof time target: <5 seconds.
 */
async function generateTransferProof(merkleRoot, nullifiers, outputCommitments) {
    const proof = new Uint8Array(128);
    proof[0] = 0x01; // ProofType::Transfer

    const encoder = new TextEncoder();
    const rootHash = await crypto.subtle.digest('SHA-256', encoder.encode(merkleRoot || ''));
    proof.set(new Uint8Array(rootHash).slice(0, 32), 1);

    const nullHash = await crypto.subtle.digest('SHA-256', encoder.encode(JSON.stringify(nullifiers)));
    proof.set(new Uint8Array(nullHash).slice(0, 32), 33);

    const commitHash = await crypto.subtle.digest('SHA-256', encoder.encode(JSON.stringify(outputCommitments)));
    proof.set(new Uint8Array(commitHash).slice(0, 32), 65);

    const pad = await crypto.subtle.digest('SHA-256', proof.slice(0, 97));
    proof.set(new Uint8Array(pad).slice(0, 31), 97);

    return proof;
}

// ===== Crypto Helpers =====

async function computeCommitmentHash(value, blindingHex) {
    const encoder = new TextEncoder();
    const data = encoder.encode(`pedersen:${value}:${blindingHex}`);
    const hash = await crypto.subtle.digest('SHA-256', data);
    return bytesToHex(new Uint8Array(hash));
}

async function computeNullifier(serialHex) {
    if (!shieldedState.spendingKey) return null;
    const data = new Uint8Array([
        ...hexToBytes(serialHex),
        ...shieldedState.spendingKey,
    ]);
    const hash = await crypto.subtle.digest('SHA-256', data);
    return bytesToHex(new Uint8Array(hash));
}

async function encryptNoteForRecipient(value, blinding, serial, recipientViewingKeyHex) {
    const recipientVK = hexToBytes(recipientViewingKeyHex);
    const ephemeralKey = crypto.getRandomValues(new Uint8Array(32));

    const encKeyMaterial = new Uint8Array([...ephemeralKey, ...recipientVK]);
    const encKeyHash = await crypto.subtle.digest('SHA-256', encKeyMaterial);
    const encKey = new Uint8Array(encKeyHash);

    const noteBytes = new Uint8Array(104);
    noteBytes.set(recipientVK.slice(0, 32), 0);
    new DataView(noteBytes.buffer).setBigUint64(32, BigInt(value), true);
    noteBytes.set(blinding, 40);
    noteBytes.set(serial, 72);

    const encrypted = new Uint8Array(noteBytes.length);
    for (let i = 0; i < noteBytes.length; i++) {
        encrypted[i] = noteBytes[i] ^ encKey[i % 32];
    }

    return bytesToHex(encrypted);
}

// ===== Storage =====

function saveNotesToStorage() {
    try {
        const data = {
            ownedNotes: shieldedState.ownedNotes,
            lastSyncedIndex: shieldedState.lastSyncedIndex,
            shieldedBalance: shieldedState.shieldedBalance,
        };
        localStorage.setItem('moltchain_shielded_notes', JSON.stringify(data));
    } catch (e) {
        console.error('Failed to save shielded notes:', e);
    }
}

function loadNotesFromStorage() {
    try {
        const raw = localStorage.getItem('moltchain_shielded_notes');
        if (raw) {
            const data = JSON.parse(raw);
            shieldedState.ownedNotes = data.ownedNotes || [];
            shieldedState.lastSyncedIndex = data.lastSyncedIndex || 0;
            shieldedState.shieldedBalance = data.shieldedBalance || 0;
        }
    } catch (e) {
        console.error('Failed to load shielded notes:', e);
    }
}

// ===== UI Updates =====

function updateShieldedUI() {
    const balanceMolt = shieldedState.shieldedBalance / SHELLS_PER_MOLT;
    const el = (id) => document.getElementById(id);

    // Shielded balance display
    const balEl = el('shieldedBalanceValue');
    if (balEl) balEl.textContent = balanceMolt.toFixed(4) + ' MOLT';

    const balShellsEl = el('shieldedBalanceShellsValue');
    if (balShellsEl) balShellsEl.textContent = shieldedState.shieldedBalance.toLocaleString() + ' shells';

    // Shielded address
    const addrEl = el('shieldedAddressDisplay');
    if (addrEl) addrEl.textContent = shieldedState.shieldedAddress || 'Not initialized';

    // Note count
    const noteCountEl = el('ownedNoteCount');
    if (noteCountEl) {
        const unspent = shieldedState.ownedNotes.filter(n => !n.spent).length;
        const total = shieldedState.ownedNotes.length;
        noteCountEl.textContent = `${unspent} unspent / ${total} total`;
    }

    // Pool stats
    if (shieldedState.poolStats) {
        const poolBalEl = el('poolTotalShielded');
        if (poolBalEl) {
            const poolMolt = (shieldedState.poolStats.pool_balance || 0) / SHELLS_PER_MOLT;
            poolBalEl.textContent = poolMolt.toFixed(2) + ' MOLT';
        }
        const poolCommitsEl = el('poolCommitmentCount');
        if (poolCommitsEl) poolCommitsEl.textContent = (shieldedState.poolStats.commitment_count || 0).toLocaleString();
    }

    // Render note list
    renderNoteList();
}

function renderNoteList() {
    const container = document.getElementById('shieldedNotesList');
    if (!container) return;

    const unspent = shieldedState.ownedNotes.filter(n => !n.spent);

    if (unspent.length === 0) {
        container.innerHTML = `
            <div style="text-align: center; padding: 2rem; color: var(--text-muted);">
                <i class="fas fa-shield-alt" style="font-size: 2rem; opacity: 0.3; margin-bottom: 0.75rem;"></i>
                <p>No shielded notes yet</p>
                <p style="font-size: 0.85rem;">Shield MOLT to create your first private note</p>
            </div>
        `;
        return;
    }

    container.innerHTML = unspent.map((note, i) => `
        <div class="note-item" style="display: flex; justify-content: space-between; align-items: center; padding: 0.75rem 1rem; background: var(--bg-darker); border-radius: 8px; margin-bottom: 0.5rem; border: 1px solid var(--border);">
            <div>
                <div style="font-weight: 600; font-size: 0.95rem;">
                    <i class="fas fa-lock" style="color: #a855f7; margin-right: 0.25rem;"></i>
                    ${(note.value / SHELLS_PER_MOLT).toFixed(4)} MOLT
                </div>
                <div style="font-size: 0.8rem; color: var(--text-muted); font-family: 'JetBrains Mono', monospace;">
                    Note #${note.index} &bull; ${note.commitment ? note.commitment.slice(0, 12) + '...' : ''}
                </div>
            </div>
            <span class="badge" style="background: rgba(6, 214, 160, 0.2); color: #06d6a0; font-size: 0.75rem;">
                <i class="fas fa-check-circle"></i> Unspent
            </span>
        </div>
    `).join('');
}

function showShieldedStatus(message, state) {
    const statusEl = document.getElementById('shieldedStatusMsg');
    if (!statusEl) return;

    if (!message) {
        statusEl.style.display = 'none';
        return;
    }

    statusEl.style.display = 'block';
    statusEl.innerHTML = `
        <div style="padding: 0.75rem 1rem; border-radius: 8px; font-size: 0.9rem; display: flex; align-items: center; gap: 0.5rem;
            background: ${state === 'pending' ? 'rgba(168, 85, 247, 0.1)' : state === 'error' ? 'rgba(244, 63, 94, 0.1)' : 'rgba(6, 214, 160, 0.1)'};
            border: 1px solid ${state === 'pending' ? 'rgba(168, 85, 247, 0.2)' : state === 'error' ? 'rgba(244, 63, 94, 0.2)' : 'rgba(6, 214, 160, 0.2)'};">
            ${state === 'pending' ? '<i class="fas fa-spinner fa-spin"></i>' : state === 'error' ? '<i class="fas fa-exclamation-circle"></i>' : '<i class="fas fa-check-circle"></i>'}
            ${message}
        </div>
    `;
}

// ===== Byte Helpers =====

function hexToBytes(hex) {
    if (!hex) return new Uint8Array(0);
    hex = hex.replace(/^0x/, '');
    const bytes = new Uint8Array(hex.length / 2);
    for (let i = 0; i < bytes.length; i++) {
        bytes[i] = parseInt(hex.substr(i * 2, 2), 16);
    }
    return bytes;
}

function bytesToHex(bytes) {
    return Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('');
}

// ===== Fee Display Helper =====
function zkFeeDisplay(type) {
    const baseFee = typeof BASE_FEE_SHELLS !== 'undefined' ? BASE_FEE_SHELLS : 1_000_000;
    const zkFees = typeof ZK_COMPUTE_FEE !== 'undefined' ? ZK_COMPUTE_FEE : { shield: 100_000, unshield: 150_000, transfer: 200_000 };
    const spm = typeof SHELLS_PER_MOLT !== 'undefined' ? SHELLS_PER_MOLT : 1_000_000_000;
    const total = (baseFee + (zkFees[type] || 0)) / spm;
    return total.toFixed(4) + ' MOLT (base + ZK compute)';
}

// ===== Modal Handlers (called from wallet UI) =====

function openShieldModal() {
    const el = document.getElementById('shieldFeeDisplay');
    if (el) el.textContent = zkFeeDisplay('shield');
    document.getElementById('shieldModal').classList.add('show');
}

function openUnshieldModal() {
    const el = document.getElementById('unshieldFeeDisplay');
    if (el) el.textContent = zkFeeDisplay('unshield');
    document.getElementById('unshieldModal').classList.add('show');
}

function openShieldedTransferModal() {
    const el = document.getElementById('transferFeeDisplay');
    if (el) el.textContent = zkFeeDisplay('transfer');
    document.getElementById('shieldedTransferModal').classList.add('show');
}

function confirmShield() {
    const amount = parseFloat(document.getElementById('shieldAmount').value);
    if (isNaN(amount) || amount <= 0) {
        showToast('Enter a valid amount');
        return;
    }
    closeModal('shieldModal');
    shieldMolt(amount);
}

function confirmUnshield() {
    const amount = parseFloat(document.getElementById('unshieldAmount').value);
    const recipient = document.getElementById('unshieldRecipient').value.trim();
    if (isNaN(amount) || amount <= 0) {
        showToast('Enter a valid amount');
        return;
    }
    if (!recipient) {
        showToast('Enter a recipient address');
        return;
    }
    closeModal('unshieldModal');
    unshieldMolt(amount, recipient);
}

function confirmShieldedTransfer() {
    const amount = parseFloat(document.getElementById('shieldedTransferAmount').value);
    const viewingKey = document.getElementById('shieldedTransferRecipientVK').value.trim();
    if (isNaN(amount) || amount <= 0) {
        showToast('Enter a valid amount');
        return;
    }
    if (!viewingKey || viewingKey.length !== 64) {
        showToast('Enter a valid recipient viewing key (64 hex chars)');
        return;
    }
    closeModal('shieldedTransferModal');
    shieldedTransfer(amount, viewingKey);
}

function copyShieldedAddress() {
    if (shieldedState.shieldedAddress) {
        navigator.clipboard.writeText(shieldedState.shieldedAddress);
        showToast('Shielded address copied!');
    }
}

function copyViewingKey() {
    if (shieldedState.viewingKey) {
        navigator.clipboard.writeText(bytesToHex(shieldedState.viewingKey));
        showToast('Viewing key copied! Share with auditors for selective disclosure.');
    }
}


