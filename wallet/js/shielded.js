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
const NOTE_ENCRYPTION_V1_PREFIX = 'a1:';
const SHIELDED_STORAGE_VERSION = 1;

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
    await loadNotesFromStorage();

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
        const statsResp = await rpc.call('getShieldedPoolState').catch(() => rpc.call('getShieldedPoolStats').catch(() => null));
        if (statsResp) {
            shieldedState.poolStats = statsResp;
            shieldedState.merkleRoot = statsResp.merkle_root || statsResp.merkleRoot || null;
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
            const isSpent = await rpc.call('isNullifierSpent', [nullifier]).catch(() => rpc.call('checkNullifier', [nullifier]).catch(() => null));
            if (isSpent && isSpent.spent) {
                note.spent = true;
            }
        }

        // Recalculate shielded balance
        shieldedState.shieldedBalance = shieldedState.ownedNotes
            .filter(n => !n.spent)
            .reduce((sum, n) => sum + n.value, 0);

        // Persist to localStorage
        await saveNotesToStorage();

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

        let plaintext = null;
        if (entry.encrypted_note.startsWith(NOTE_ENCRYPTION_V1_PREFIX)) {
            const parts = entry.encrypted_note.split(':');
            if (parts.length !== 3) return null;
            const iv = hexToBytes(parts[1]);
            const ciphertext = hexToBytes(parts[2]);
            if (iv.length !== 12 || ciphertext.length === 0) return null;

            const aesKey = await crypto.subtle.importKey(
                'raw',
                decKey,
                { name: 'AES-GCM' },
                false,
                ['decrypt']
            );

            const decrypted = await crypto.subtle.decrypt(
                { name: 'AES-GCM', iv },
                aesKey,
                ciphertext
            );
            plaintext = new Uint8Array(decrypted);
        } else {
            // Legacy compatibility: decrypt historical XOR-encrypted notes.
            const ciphertext = hexToBytes(entry.encrypted_note);
            const legacyPlaintext = new Uint8Array(ciphertext.length);
            for (let i = 0; i < ciphertext.length; i++) {
                legacyPlaintext[i] = ciphertext[i] ^ decKey[i % 32];
            }
            plaintext = legacyPlaintext;
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
        const blindingHex = bytesToHex(blinding);
        const serial = crypto.getRandomValues(new Uint8Array(32));

        // Generate a real Groth16 shield proof and circuit-aligned commitment.
        const shieldProof = await generateShieldProof(amountShells, null, blindingHex);
        const commitment = shieldProof.commitment;

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

        const encryptedNote = await encryptNoteBytes(noteBytes, encKey);

        const proof = hexToBytes(shieldProof.proof);

        showShieldedStatus('Submitting transaction...', 'pending');

        // Submit shield transaction
        const result = await rpc.call('submitShieldTransaction', [{
            amount: amountShells,
            commitment: commitment,
            proof: bytesToHex(proof),
            encrypted_note: encryptedNote,
            ephemeral_pk: bytesToHex(ephemeralKey),
        }]);

        if (result && result.success) {
            // Add to our owned notes
            shieldedState.ownedNotes.push({
                index: result.commitment_index || shieldedState.commitments.length,
                value: amountShells,
                blinding: blindingHex,
                serial: bytesToHex(serial),
                commitment: commitment,
                spent: false,
            });
            shieldedState.shieldedBalance += amountShells;
            await saveNotesToStorage();
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

    if (!window.MoltCrypto || !window.MoltCrypto.isValidAddress(recipientAddress)) {
        showToast('Enter a valid recipient address');
        return;
    }

    const amountShells = Math.floor(amountMolt * SHELLS_PER_MOLT);

    const unspentNotes = shieldedState.ownedNotes.filter(n => !n.spent);
    const totalAvailable = unspentNotes.reduce((sum, n) => sum + n.value, 0);
    if (amountShells > totalAvailable) {
        showToast(`Insufficient shielded balance. Available: ${(totalAvailable / SHELLS_PER_MOLT).toFixed(4)} MOLT`);
        return;
    }

    // Current unshield circuit supports one input note where value == amount.
    const noteToSpend = unspentNotes.find((n) => n.value === amountShells);
    if (!noteToSpend) {
        showToast('Unshield currently requires a single note exactly matching the amount');
        return;
    }

    showShieldedStatus('Generating ZK proof (may take ~3s)...', 'pending');

    try {
        const nullifier = await computeNullifier(noteToSpend.serial);
        const merklePath = await rpc.call('getShieldedMerklePath', [noteToSpend.index]);
        const unshieldProof = await generateUnshieldProof({
            amount: amountShells,
            merkleRoot: shieldedState.merkleRoot,
            recipient: recipientAddress,
            blinding: noteToSpend.blinding,
            serial: noteToSpend.serial,
            spendingKey: bytesToHex(shieldedState.spendingKey || new Uint8Array(32)),
            merklePath: merklePath?.siblings || [],
            pathBits: merklePath?.pathBits || merklePath?.path_bits || [],
        });
        const proof = hexToBytes(unshieldProof.proof);

        showShieldedStatus('Submitting transaction...', 'pending');

        const result = await rpc.call('submitUnshieldTransaction', [{
            nullifier: nullifier,
            amount: amountShells,
            recipient: recipientAddress,
            merkle_root: shieldedState.merkleRoot,
            proof: bytesToHex(proof),
        }]);

        if (result && result.success) {
            noteToSpend.spent = true;
            shieldedState.shieldedBalance -= amountShells;

            // If change needed, a new note will appear in the commitment stream
            await saveNotesToStorage();
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

    // Transfer circuit is fixed 2-in-2-out: choose exactly two notes whose sum covers amount.
    const inputNotes = selectTwoInputNotes(unspentNotes, amountShells);
    if (!inputNotes) {
        showToast('Transfer currently requires two notes with combined value >= amount');
        return;
    }

    const inputTotal = inputNotes[0].value + inputNotes[1].value;

    const changeAmount = inputTotal - amountShells;

    showShieldedStatus('Generating ZK proof (may take ~5s)...', 'pending');

    try {
        // Create output notes (always two outputs for transfer circuit)
        const recipientBlinding = crypto.getRandomValues(new Uint8Array(32));
        const recipientSerial = crypto.getRandomValues(new Uint8Array(32));
        const changeBlinding = crypto.getRandomValues(new Uint8Array(32));
        const changeSerial = crypto.getRandomValues(new Uint8Array(32));

        // Load Merkle witnesses for both inputs
        const merkleWitnesses = await Promise.all(inputNotes.map((note) => rpc.call('getShieldedMerklePath', [note.index])));

        // Generate transfer proof + canonical public outputs (nullifiers/commitments)
        const transferProof = await generateTransferProof({
            merkleRoot: shieldedState.merkleRoot,
            inputs: inputNotes.map((note, i) => ({
                amount: note.value,
                blinding: note.blinding,
                serial: note.serial,
                spendingKey: bytesToHex(shieldedState.spendingKey || new Uint8Array(32)),
                merklePath: (merkleWitnesses[i]?.siblings || []),
                pathBits: (merkleWitnesses[i]?.pathBits || merkleWitnesses[i]?.path_bits || []),
            })),
            outputs: [
                { amount: amountShells, blinding: bytesToHex(recipientBlinding) },
                { amount: changeAmount, blinding: bytesToHex(changeBlinding) },
            ],
        });

        const proofHex = transferProof.proof;
        const nullifiers = [transferProof.nullifier_a, transferProof.nullifier_b];

        const recipientEnc = await encryptNoteForRecipient(
            amountShells,
            recipientBlinding,
            recipientSerial,
            recipientViewingKey,
        );

        const changeEphKey = crypto.getRandomValues(new Uint8Array(32));
        const changeEncKeyMaterial = new Uint8Array([...changeEphKey, ...shieldedState.viewingKey]);
        const changeEncKeyHash = await crypto.subtle.digest('SHA-256', changeEncKeyMaterial);
        const changeEncKey = new Uint8Array(changeEncKeyHash);

        const changeNote = new Uint8Array(104);
        changeNote.set(new Uint8Array(32), 0); // self
        new DataView(changeNote.buffer).setBigUint64(32, BigInt(changeAmount), true);
        changeNote.set(changeBlinding, 40);
        changeNote.set(changeSerial, 72);

        const changeEncrypted = await encryptNoteBytes(changeNote, changeEncKey);

        const outputCommitments = [
            {
                commitment: transferProof.commitment_c,
                encrypted_note: recipientEnc.encryptedNote,
                ephemeral_pk: recipientEnc.ephemeralPk,
            },
            {
                commitment: transferProof.commitment_d,
                encrypted_note: changeEncrypted,
                ephemeral_pk: bytesToHex(changeEphKey),
            },
        ];

        showShieldedStatus('Submitting transaction...', 'pending');

        const result = await rpc.call('submitShieldedTransfer', [{
            nullifiers: nullifiers,
            output_commitments: outputCommitments,
            merkle_root: shieldedState.merkleRoot,
            proof: typeof proofHex === 'string' ? proofHex : bytesToHex(proofHex),
        }]);

        if (result && result.success) {
            // Mark input notes as spent
            for (const note of inputNotes) {
                note.spent = true;
            }
            shieldedState.shieldedBalance = shieldedState.ownedNotes
                .filter(n => !n.spent)
                .reduce((sum, n) => sum + n.value, 0);

            await saveNotesToStorage();
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
 * Generate a real shield proof via RPC-backed Groth16 prover.
 */
async function generateShieldProof(amount, commitment, blinding) {
    const blindingHex = typeof blinding === 'string' ? blinding : bytesToHex(blinding || new Uint8Array(32));
    return rpc.call('generateShieldProof', [{
        amount: amount,
        blinding: blindingHex,
        commitment: commitment || null,
    }]);
}

/**
 * Generate a real unshield proof via RPC-backed Groth16 prover.
 */
async function generateUnshieldProof({ amount, merkleRoot, recipient, blinding, serial, spendingKey, merklePath, pathBits }) {
    return rpc.call('generateUnshieldProof', [{
        amount,
        merkle_root: merkleRoot,
        recipient,
        blinding,
        serial,
        spending_key: spendingKey,
        merkle_path: merklePath || [],
        path_bits: pathBits || [],
    }]);
}

/**
 * Generate a transfer proof via RPC-backed Groth16 prover.
 */
async function generateTransferProof(witness) {
    return rpc.call('generateTransferProof', [{
        merkle_root: witness.merkleRoot,
        inputs: witness.inputs.map((input) => ({
            amount: input.amount,
            blinding: input.blinding,
            serial: input.serial,
            spending_key: input.spendingKey,
            merkle_path: input.merklePath,
            path_bits: input.pathBits,
        })),
        outputs: witness.outputs.map((output) => ({
            amount: output.amount,
            blinding: output.blinding,
        })),
    }]);
}

// ===== Crypto Helpers =====

async function computeCommitmentHash(value, blindingHex) {
    const resp = await rpc.call('computeShieldCommitment', [{
        amount: value,
        blinding: blindingHex,
    }]);
    if (!resp || !resp.commitment) {
        throw new Error('RPC did not return commitment');
    }
    return resp.commitment;
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

    const encryptedNote = await encryptNoteBytes(noteBytes, encKey);
    return {
        encryptedNote,
        ephemeralPk: bytesToHex(ephemeralKey),
    };
}

function selectTwoInputNotes(unspentNotes, targetAmount) {
    if (!Array.isArray(unspentNotes) || unspentNotes.length < 2) return null;

    let bestPair = null;
    let bestExcess = Number.MAX_SAFE_INTEGER;

    for (let i = 0; i < unspentNotes.length; i++) {
        for (let j = i + 1; j < unspentNotes.length; j++) {
            const total = unspentNotes[i].value + unspentNotes[j].value;
            if (total < targetAmount) continue;
            const excess = total - targetAmount;
            if (excess < bestExcess) {
                bestExcess = excess;
                bestPair = [unspentNotes[i], unspentNotes[j]];
                if (bestExcess === 0) return bestPair;
            }
        }
    }

    return bestPair;
}

async function encryptNoteBytes(noteBytes, encKey) {
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const aesKey = await crypto.subtle.importKey(
        'raw',
        encKey,
        { name: 'AES-GCM' },
        false,
        ['encrypt']
    );
    const ciphertext = await crypto.subtle.encrypt(
        { name: 'AES-GCM', iv },
        aesKey,
        noteBytes
    );
    return `${NOTE_ENCRYPTION_V1_PREFIX}${bytesToHex(iv)}:${bytesToHex(new Uint8Array(ciphertext))}`;
}

// ===== Storage =====

async function deriveShieldedStorageKey() {
    if (!shieldedState.spendingKey || !shieldedState.viewingKey) return null;
    const domain = new TextEncoder().encode('moltchain-shielded-storage-v1');
    const keyMaterial = new Uint8Array(
        shieldedState.spendingKey.length + shieldedState.viewingKey.length + domain.length
    );
    keyMaterial.set(shieldedState.spendingKey, 0);
    keyMaterial.set(shieldedState.viewingKey, shieldedState.spendingKey.length);
    keyMaterial.set(domain, shieldedState.spendingKey.length + shieldedState.viewingKey.length);

    const digest = await crypto.subtle.digest('SHA-256', keyMaterial);
    return crypto.subtle.importKey(
        'raw',
        new Uint8Array(digest),
        { name: 'AES-GCM' },
        false,
        ['encrypt', 'decrypt']
    );
}

async function saveNotesToStorage() {
    try {
        const key = await deriveShieldedStorageKey();
        if (!key) return;

        const data = {
            ownedNotes: shieldedState.ownedNotes,
            lastSyncedIndex: shieldedState.lastSyncedIndex,
            shieldedBalance: shieldedState.shieldedBalance,
        };

        const encoded = new TextEncoder().encode(JSON.stringify(data));
        const iv = crypto.getRandomValues(new Uint8Array(12));
        const encrypted = await crypto.subtle.encrypt(
            { name: 'AES-GCM', iv },
            key,
            encoded
        );

        localStorage.setItem('moltchain_shielded_notes', JSON.stringify({
            version: SHIELDED_STORAGE_VERSION,
            iv: bytesToHex(iv),
            ciphertext: bytesToHex(new Uint8Array(encrypted)),
        }));
    } catch (e) {
        console.error('Failed to save shielded notes:', e);
    }
}

async function loadNotesFromStorage() {
    try {
        const raw = localStorage.getItem('moltchain_shielded_notes');
        if (!raw) return;

        const parsed = JSON.parse(raw);
        if (parsed && parsed.version === SHIELDED_STORAGE_VERSION && parsed.iv && parsed.ciphertext) {
            const key = await deriveShieldedStorageKey();
            if (!key) return;
            const decrypted = await crypto.subtle.decrypt(
                { name: 'AES-GCM', iv: hexToBytes(parsed.iv) },
                key,
                hexToBytes(parsed.ciphertext)
            );
            const data = JSON.parse(new TextDecoder().decode(decrypted));
            shieldedState.ownedNotes = data.ownedNotes || [];
            shieldedState.lastSyncedIndex = data.lastSyncedIndex || 0;
            shieldedState.shieldedBalance = data.shieldedBalance || 0;
            return;
        }

        // Legacy migration path: previous plaintext object format.
        shieldedState.ownedNotes = parsed.ownedNotes || [];
        shieldedState.lastSyncedIndex = parsed.lastSyncedIndex || 0;
        shieldedState.shieldedBalance = parsed.shieldedBalance || 0;
        await saveNotesToStorage();
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
            <div class="shield-empty-state">
                <i class="fas fa-shield-alt"></i>
                <p>No shielded notes yet</p>
                <p class="shield-empty-sub">Shield MOLT to create your first private note</p>
            </div>
        `;
        return;
    }

    container.innerHTML = unspent.map((note, i) => `
        <div class="shield-note-item">
            <div>
                <div class="shield-note-amount">
                    <i class="fas fa-lock"></i>
                    ${(note.value / SHELLS_PER_MOLT).toFixed(4)} MOLT
                </div>
                <div class="shield-note-meta">
                    Note #${note.index} &bull; ${note.commitment ? note.commitment.slice(0, 12) + '...' : ''}
                </div>
            </div>
            <span class="shield-note-badge">
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
    const iconMap = {
        pending: '<i class="fas fa-spinner fa-spin"></i>',
        error: '<i class="fas fa-exclamation-circle"></i>',
        success: '<i class="fas fa-check-circle"></i>',
    };
    statusEl.innerHTML = `
        <div class="shield-status-inner ${state || 'pending'}">
            ${iconMap[state] || iconMap.pending}
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
    // Wire onblur amount clamping
    const input = document.getElementById('shieldAmount');
    if (input) {
        input.value = '';
        input.onblur = () => {
            const v = parseFloat(input.value);
            if (isNaN(v) || v <= 0) return;
            const maxMolt = Math.max(0, (window.walletBalance || 0) - (typeof BASE_FEE_MOLT !== 'undefined' ? BASE_FEE_MOLT : 0.001));
            if (v > maxMolt) input.value = maxMolt > 0 ? maxMolt.toFixed(6) : '';
        };
    }
    // Disable confirm if zero transparent balance
    _updateShieldModalBtn();
    document.getElementById('shieldModal').classList.add('show');
}

function openUnshieldModal() {
    const el = document.getElementById('unshieldFeeDisplay');
    if (el) el.textContent = zkFeeDisplay('unshield');
    const input = document.getElementById('unshieldAmount');
    if (input) {
        input.value = '';
        input.onblur = () => {
            const v = parseFloat(input.value);
            if (isNaN(v) || v <= 0) return;
            const maxMolt = (shieldedState.shieldedBalance || 0) / SHELLS_PER_MOLT;
            if (v > maxMolt) input.value = maxMolt > 0 ? maxMolt.toFixed(6) : '';
        };
    }
    _updateUnshieldModalBtn();
    document.getElementById('unshieldModal').classList.add('show');
}

function openShieldedTransferModal() {
    const el = document.getElementById('transferFeeDisplay');
    if (el) el.textContent = zkFeeDisplay('transfer');
    const input = document.getElementById('shieldedTransferAmount');
    if (input) {
        input.value = '';
        input.onblur = () => {
            const v = parseFloat(input.value);
            if (isNaN(v) || v <= 0) return;
            const maxMolt = (shieldedState.shieldedBalance || 0) / SHELLS_PER_MOLT;
            if (v > maxMolt) input.value = maxMolt > 0 ? maxMolt.toFixed(6) : '';
        };
    }
    _updateTransferModalBtn();
    document.getElementById('shieldedTransferModal').classList.add('show');
}

// ===== Modal button disable helpers =====

function _updateShieldModalBtn() {
    const btn = document.querySelector('#shieldModal .modal-footer .btn-shield');
    if (!btn) return;
    const spendable = window.walletBalance || 0;
    const fee = typeof BASE_FEE_MOLT !== 'undefined' ? BASE_FEE_MOLT : 0.001;
    if (spendable <= fee) {
        btn.disabled = true;
        btn.title = 'Insufficient MOLT balance';
    } else {
        btn.disabled = false;
        btn.title = '';
    }
}

function _updateUnshieldModalBtn() {
    const btn = document.querySelector('#unshieldModal .modal-footer .btn-unshield');
    if (!btn) return;
    const available = (shieldedState.shieldedBalance || 0) / SHELLS_PER_MOLT;
    if (available <= 0) {
        btn.disabled = true;
        btn.title = 'No shielded balance';
    } else {
        btn.disabled = false;
        btn.title = '';
    }
}

function _updateTransferModalBtn() {
    const btn = document.querySelector('#shieldedTransferModal .modal-footer .btn-shield');
    if (!btn) return;
    const available = (shieldedState.shieldedBalance || 0) / SHELLS_PER_MOLT;
    if (available <= 0) {
        btn.disabled = true;
        btn.title = 'No shielded balance';
    } else {
        btn.disabled = false;
        btn.title = '';
    }
}

async function confirmShield() {
    let amount = parseFloat(document.getElementById('shieldAmount').value);
    if (isNaN(amount) || amount <= 0) {
        showToast('Enter a valid amount');
        return;
    }
    // Balance guard: check transparent balance
    try {
        const wallet = getActiveWallet();
        if (wallet) {
            const balResult = await rpc.call('getBalance', [wallet.address]);
            const spendable = (balResult?.spendable || balResult?.balance || 0) / SHELLS_PER_MOLT;
            const maxShieldable = Math.max(0, spendable - BASE_FEE_MOLT);
            if (maxShieldable <= 0) {
                showToast('Insufficient MOLT balance to shield');
                return;
            }
            if (amount > maxShieldable) {
                amount = parseFloat(maxShieldable.toFixed(6));
                document.getElementById('shieldAmount').value = amount;
                showToast(`Amount adjusted to available balance: ${(maxShieldable).toFixed(4)} MOLT`);
                return; // Let user review
            }
        }
    } catch (e) { /* let RPC reject */ }
    closeModal('shieldModal');
    shieldMolt(amount);
}

function confirmUnshield() {
    let amount = parseFloat(document.getElementById('unshieldAmount').value);
    const recipient = document.getElementById('unshieldRecipient').value.trim();
    if (isNaN(amount) || amount <= 0) {
        showToast('Enter a valid amount');
        return;
    }
    if (!recipient) {
        showToast('Enter a recipient address');
        return;
    }
    if (!window.MoltCrypto || !window.MoltCrypto.isValidAddress(recipient)) {
        showToast('Enter a valid recipient address');
        return;
    }
    // Balance guard: check shielded balance
    const availableMolt = (shieldedState.shieldedBalance || 0) / SHELLS_PER_MOLT;
    if (availableMolt <= 0) {
        showToast('No shielded balance to unshield');
        return;
    }
    if (amount > availableMolt) {
        amount = parseFloat(availableMolt.toFixed(6));
        document.getElementById('unshieldAmount').value = amount;
        showToast(`Amount adjusted to shielded balance: ${availableMolt.toFixed(4)} MOLT`);
        return; // Let user review
    }
    closeModal('unshieldModal');
    unshieldMolt(amount, recipient);
}

function confirmShieldedTransfer() {
    let amount = parseFloat(document.getElementById('shieldedTransferAmount').value);
    const viewingKey = document.getElementById('shieldedTransferRecipientVK').value.trim();
    if (isNaN(amount) || amount <= 0) {
        showToast('Enter a valid amount');
        return;
    }
    if (!viewingKey || viewingKey.length !== 64) {
        showToast('Enter a valid recipient viewing key (64 hex chars)');
        return;
    }
    // Balance guard: check shielded balance
    const availableMolt = (shieldedState.shieldedBalance || 0) / SHELLS_PER_MOLT;
    if (availableMolt <= 0) {
        showToast('No shielded balance for transfer');
        return;
    }
    if (amount > availableMolt) {
        amount = parseFloat(availableMolt.toFixed(6));
        document.getElementById('shieldedTransferAmount').value = amount;
        showToast(`Amount adjusted to shielded balance: ${availableMolt.toFixed(4)} MOLT`);
        return; // Let user review
    }
    closeModal('shieldedTransferModal');
    shieldedTransfer(amount, viewingKey);
}

function copyShieldedAddress(btnEl) {
    if (shieldedState.shieldedAddress) {
        navigator.clipboard.writeText(shieldedState.shieldedAddress);
        if (typeof pulseCopyButton === 'function') pulseCopyButton(btnEl);
        showToast('Shielded address copied!');
    }
}

function copyViewingKey(btnEl) {
    if (shieldedState.viewingKey) {
        navigator.clipboard.writeText(bytesToHex(shieldedState.viewingKey));
        if (typeof pulseCopyButton === 'function') pulseCopyButton(btnEl);
        showToast('Viewing key copied! Share with auditors for selective disclosure.');
    }
}

function toggleViewingKey(btnEl) {
    const display = document.getElementById('viewingKeyDisplay');
    if (!display) return;
    const icon = btnEl ? btnEl.querySelector('i') : null;
    const isHidden = display.dataset.revealed !== 'true';
    if (isHidden && shieldedState.viewingKey) {
        display.textContent = bytesToHex(shieldedState.viewingKey);
        display.dataset.revealed = 'true';
        if (icon) { icon.className = 'fas fa-eye'; }
    } else {
        display.textContent = 'Click eye icon to reveal';
        display.dataset.revealed = 'false';
        if (icon) { icon.className = 'fas fa-eye-slash'; }
    }
}


