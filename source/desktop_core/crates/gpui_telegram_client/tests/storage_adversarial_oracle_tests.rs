//! Adversarial stress test harness, boundary condition verification,
//! and cryptographic oracle validation against Module 03 Storage subsystem.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use gpui_telegram_client::storage::details::storage_file_utilities::*;
use gpui_telegram_client::storage::storage_account::*;
use gpui_telegram_client::storage::storage_sparse_ids_list::*;

// =========================================================================
// 1. AES-IGE Cipher Oracle Tests
// =========================================================================

#[test]
fn test_aes_ige_cipher_oracle_roundtrip_arbitrary_sizes() {
    let key = [0x5A; 32];
    let iv = [0x3C; 32];

    // Direct aes_ige_encrypt / decrypt with exact multiples of 16 bytes:
    let test_sizes_16 = [0, 16, 32, 64, 128, 512, 1024, 4096, 65536];
    for &size in &test_sizes_16 {
        let plaintext: Vec<u8> = (0..size).map(|i| ((i * 31 + 7) & 0xFF) as u8).collect();
        let ciphertext = aes_ige_encrypt(&plaintext, &key, &iv);
        assert_eq!(ciphertext.len(), plaintext.len());

        let decrypted = aes_ige_decrypt(&ciphertext, &key, &iv);
        assert_eq!(decrypted, plaintext, "AES-IGE direct failed at size {size}");
    }

    // High-level prepare_encrypted / decrypt_local with arbitrary sizes (including non-multiples):
    let arbitrary_sizes = [0, 1, 2, 7, 15, 16, 17, 31, 32, 33, 100, 255, 1024, 8191, 65535];
    for &size in &arbitrary_sizes {
        let payload: Vec<u8> = (0..size).map(|i| ((i * 47 + 13) & 0xFF) as u8).collect();
        let encrypted = prepare_encrypted(&payload, &key);

        // Encrypted length must be 16 (key128) + 16*k (padded length of 4-byte size + payload)
        assert!(encrypted.len() >= 32);
        assert_eq!((encrypted.len() - 16) % 16, 0);

        let decrypted = decrypt_local(&encrypted, &key).expect("decrypt_local should succeed");
        assert_eq!(decrypted, payload, "prepare_encrypted / decrypt_local failed at size {size}");
    }
}

#[test]
fn test_aes_ige_cipher_oracle_tamper_detection_bit_flip() {
    let key = [0x9Du8; 32];
    let original = b"The quick brown fox jumps over the lazy dog. Adversarial tamper validation payload.";
    let encrypted = prepare_encrypted(original, &key);

    // 1. Flip every single byte in the key128 header (bytes 0..16) one by one:
    for byte_idx in 0..16 {
        for bit in 0..8 {
            let mut tampered = encrypted.clone();
            tampered[byte_idx] ^= 1 << bit;
            let result = decrypt_local(&tampered, &key);
            assert!(
                result.is_err(),
                "Header bit flip at byte {byte_idx} bit {bit} was not detected!"
            );
        }
    }

    // 2. Flip bits in ciphertext body (bytes 16..encrypted.len()):
    let check_points = [16, 17, 24, 32, 40, encrypted.len() - 16, encrypted.len() - 1];
    for &byte_idx in &check_points {
        for bit in [0, 3, 7] {
            let mut tampered = encrypted.clone();
            tampered[byte_idx] ^= 1 << bit;
            let result = decrypt_local(&tampered, &key);
            assert!(
                result.is_err(),
                "Ciphertext bit flip at byte {byte_idx} bit {bit} was not detected!"
            );
        }
    }

    // 3. Decrypt with wrong key (1-bit flipped key):
    let mut wrong_key = key;
    wrong_key[0] ^= 1;
    let result_wrong_key = decrypt_local(&encrypted, &wrong_key);
    assert!(result_wrong_key.is_err(), "Decryption with 1-bit wrong key must fail");
}

#[test]
fn test_aes_ige_cipher_oracle_non_multiple_of_16_payload() {
    let key = [0x11u8; 32];
    let iv = [0x22u8; 32];

    // decrypt_local must return Err for non-16-byte multiple cipher lengths
    let invalid_lengths = [0, 1, 15, 16, 17, 31, 33, 47, 49];
    for &len in &invalid_lengths {
        let dummy = vec![0u8; len];
        let res = decrypt_local(&dummy, &key);
        assert!(res.is_err(), "decrypt_local should reject invalid length {len}");
    }

    // aes_ige_encrypt / decrypt assert multiple-of-16: verify panic unwinding
    let non_16_payload = vec![0xEEu8; 17];
    let enc_panic = panic::catch_unwind(AssertUnwindSafe(|| {
        aes_ige_encrypt(&non_16_payload, &key, &iv);
    }));
    assert!(enc_panic.is_err(), "aes_ige_encrypt must panic on non-16-byte slice");

    let dec_panic = panic::catch_unwind(AssertUnwindSafe(|| {
        aes_ige_decrypt(&non_16_payload, &key, &iv);
    }));
    assert!(dec_panic.is_err(), "aes_ige_decrypt must panic on non-16-byte slice");
}

#[test]
fn test_aes_ige_cipher_oracle_zero_key_and_zero_iv() {
    let zero_key = [0u8; 32];
    let zero_iv = [0u8; 32];
    let payload = b"Zero key and zero IV boundary test payload 16b!!";

    // Direct AES-IGE with all zeros
    let cipher = aes_ige_encrypt(payload, &zero_key, &zero_iv);
    assert_ne!(cipher, payload.as_slice(), "Ciphertext must not match plaintext even with zero key");
    let recovered = aes_ige_decrypt(&cipher, &zero_key, &zero_iv);
    assert_eq!(recovered, payload.as_slice());

    // High level prepare_encrypted / decrypt_local with zero key
    let enc = prepare_encrypted(payload, &zero_key);
    let dec = decrypt_local(&enc, &zero_key).expect("zero-key decryption");
    assert_eq!(dec, payload.as_slice());
}

// =========================================================================
// 2. Argon2id & PBKDF2 KDF Oracle Tests
// =========================================================================

#[test]
fn test_kdf_oracle_repeatability_and_determinism() {
    let passcode = b"ComplexPasscode#2026_EmpiricalTest";
    let salt = b"salt_for_deterministic_kdf_test_12345678";

    // 1. PBKDF2 Repeatability: 10 successive derivations must yield exact same 32 bytes
    let local_key_1 = create_local_key(passcode, salt);
    for _ in 0..10 {
        let local_key_n = create_local_key(passcode, salt);
        assert_eq!(local_key_1, local_key_n, "PBKDF2 derivation must be deterministic");
    }

    // 2. Argon2id Repeatability: 5 successive derivations must yield exact same 32 bytes
    let kdf = PasscodeKdf {
        kind: PASSCODE_KDF_ARGON2ID,
        memory: 1024,
        time: 2,
        parallel: 1,
    };
    let argon_key_1 = create_passcode_key(passcode, salt, &kdf).expect("Argon2id run 1");
    for _ in 0..5 {
        let argon_key_n = create_passcode_key(passcode, salt, &kdf).expect("Argon2id run n");
        assert_eq!(argon_key_1, argon_key_n, "Argon2id derivation must be deterministic");
    }
}

#[test]
fn test_kdf_oracle_salt_sensitivity_hamming_distance() {
    let passcode = b"SecureMasterPassword99";
    let base_salt = [0x42u8; 32];

    fn count_differing_bits(a: &[u8; 32], b: &[u8; 32]) -> usize {
        a.iter().zip(b.iter()).map(|(x, y)| (x ^ y).count_ones() as usize).sum()
    }

    // 1. PBKDF2 Salt Sensitivity: flip 1 bit in salt
    let key_orig = create_local_key(passcode, &base_salt);
    for bit_pos in 0..8 {
        let mut flipped_salt = base_salt;
        flipped_salt[0] ^= 1 << bit_pos;
        let key_flipped = create_local_key(passcode, &flipped_salt);

        let diff_bits = count_differing_bits(&key_orig, &key_flipped);
        // A cryptographically sound KDF should flip approximately 50% (128 bits) of the 256 output bits.
        // Assert at least 90 differing bits (> 35% avalanche)
        assert!(
            diff_bits >= 90,
            "PBKDF2 1-bit salt flip bit {bit_pos} had insufficient avalanche: {diff_bits}/256 bits"
        );
    }

    // 2. Argon2id Salt Sensitivity: flip 1 bit in salt
    let kdf = PasscodeKdf {
        kind: PASSCODE_KDF_ARGON2ID,
        memory: 1024,
        time: 2,
        parallel: 1,
    };
    let argon_orig = create_passcode_key(passcode, &base_salt, &kdf).expect("argon base");
    for bit_pos in 0..8 {
        let mut flipped_salt = base_salt;
        flipped_salt[15] ^= 1 << bit_pos;
        let argon_flipped = create_passcode_key(passcode, &flipped_salt, &kdf).expect("argon flipped");

        let diff_bits = count_differing_bits(&argon_orig, &argon_flipped);
        assert!(
            diff_bits >= 90,
            "Argon2id 1-bit salt flip bit {bit_pos} had insufficient avalanche: {diff_bits}/256 bits"
        );
    }
}

#[test]
fn test_kdf_oracle_empty_passcode_and_short_salt_handling() {
    let salt = [0x77u8; 16];
    let kdf = PasscodeKdf {
        kind: PASSCODE_KDF_ARGON2ID,
        memory: 1024,
        time: 2,
        parallel: 1,
    };

    // 1. PBKDF2 empty passcode: handles gracefully with 1 iteration (used for no-passcode local encryption)
    let pbkdf2_empty = create_local_key(b"", &salt);
    assert_ne!(pbkdf2_empty, [0u8; 32]);
    let pbkdf2_empty_2 = create_local_key(b"", &salt);
    assert_eq!(pbkdf2_empty, pbkdf2_empty_2);

    // 2. Argon2id empty passcode: must be rejected with Err
    let argon_empty = create_passcode_key(b"", &salt, &kdf);
    assert!(argon_empty.is_err(), "Argon2id must reject empty passcode");

    // 3. Short salt (< PASSCODE_SALT_MIN_SIZE = 8 bytes) must be rejected by create_passcode_key
    for short_len in 0..8 {
        let short_salt = vec![0x33u8; short_len];
        let res = create_passcode_key(b"ValidPasscode", &short_salt, &kdf);
        assert!(res.is_err(), "create_passcode_key must reject salt of len {short_len}");
    }

    // 4. Invalid KDF parameters (time=0, parallel=0, memory < 8 * parallel)
    let invalid_kdf_1 = PasscodeKdf { kind: PASSCODE_KDF_ARGON2ID, memory: 1024, time: 0, parallel: 1 };
    assert!(!invalid_kdf_1.valid());
    assert!(create_passcode_key(b"Passcode", &salt, &invalid_kdf_1).is_err());

    let invalid_kdf_2 = PasscodeKdf { kind: PASSCODE_KDF_ARGON2ID, memory: 4, time: 1, parallel: 2 };
    assert!(!invalid_kdf_2.valid());
    assert!(create_passcode_key(b"Passcode", &salt, &invalid_kdf_2).is_err());
}

// =========================================================================
// 3. TDF Atomic Serialization Oracle Tests
// =========================================================================

#[test]
fn test_tdf_oracle_magic_header_enforcement() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let base_path = temp_dir.path().to_str().unwrap();

    // Write a valid TDF file first
    let file_name = "magic_test";
    let mut writer = FileWriteDescriptor::new(file_name, base_path, true);
    writer.write_byte_array(b"Payload to test magic header");
    writer.finish().expect("write succeeds");

    // Verify it reads normally
    assert!(read_file(file_name, base_path).is_ok());

    // Corrupt magic header in the written file
    let path_s = temp_dir.path().join(format!("{}s", file_name));
    assert!(path_s.exists());

    let mut content = std::fs::read(&path_s).expect("read file");
    assert_eq!(&content[..4], &TDF_MAGIC);

    // Corrupt 1st byte 'T' -> 'X'
    content[0] = b'X';
    std::fs::write(&path_s, &content).expect("write corrupted");

    let read_result = read_file(file_name, base_path);
    assert!(read_result.is_err(), "read_file must reject file with invalid magic header");
}

#[test]
fn test_tdf_oracle_md5_trailer_verification_bit_flip() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let base_path = temp_dir.path().to_str().unwrap();

    let file_name = "md5_tamper_test";
    let payload = b"Critical configuration data that must not be tampered with!";

    let mut writer = FileWriteDescriptor::new(file_name, base_path, true);
    writer.write_byte_array(payload);
    writer.finish().expect("write succeeds");

    let path_s = temp_dir.path().join(format!("{}s", file_name));
    let orig_bytes = std::fs::read(&path_s).expect("read written file");
    let len = orig_bytes.len();
    assert!(len >= 24); // 4 magic + 4 version + data + 16 md5

    // 1. Flip 1 bit in data payload (between byte 8 and len - 16)
    let payload_byte_idx = 12;
    let mut tampered_payload = orig_bytes.clone();
    tampered_payload[payload_byte_idx] ^= 0x01;
    std::fs::write(&path_s, &tampered_payload).expect("write tampered");

    assert!(
        read_file(file_name, base_path).is_err(),
        "read_file must reject data payload bit tampering via MD5 checksum"
    );

    // 2. Flip 1 bit in MD5 trailer (last 16 bytes)
    let mut tampered_md5 = orig_bytes.clone();
    tampered_md5[len - 1] ^= 0x01;
    std::fs::write(&path_s, &tampered_md5).expect("write tampered md5");

    assert!(
        read_file(file_name, base_path).is_err(),
        "read_file must reject MD5 trailer bit tampering"
    );

    // 3. Truncate file (remove last 8 bytes)
    let truncated = orig_bytes[..len - 8].to_vec();
    std::fs::write(&path_s, &truncated).expect("write truncated");
    assert!(
        read_file(file_name, base_path).is_err(),
        "read_file must reject truncated file"
    );
}

#[test]
fn test_tdf_oracle_crash_recovery_incomplete_temp_swap_files() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let base_path = temp_dir.path().to_str().unwrap();
    let file_name = "crash_recovery_test";

    // Scenario A: Write version 0 (*0)
    let path_0 = temp_dir.path().join(format!("{}0", file_name));
    let path_s = temp_dir.path().join(format!("{}s", file_name));
    let path_tmp = temp_dir.path().join(format!("{}s.tmp", file_name));

    // Create a valid previous version directly in *0
    let mut writer_0 = FileWriteDescriptor::new(file_name, base_path, true);
    writer_0.write_byte_array(b"Previous Valid Version 0 Data");
    writer_0.finish().expect("write succeeds");
    // Move *s to *0 to simulate older committed version
    std::fs::rename(&path_s, &path_0).expect("rename to 0");
    assert!(path_0.exists());
    assert!(!path_s.exists());

    // 1. Verify read_file reads from *0 when *s doesn't exist
    let mut reader_0 = read_file(file_name, base_path).expect("read from *0");
    let payload_0 = reader_0.read_byte_array().unwrap();
    assert_eq!(payload_0, b"Previous Valid Version 0 Data");

    // 2. Simulate incomplete write crash: *s.tmp exists, *s does not exist yet
    std::fs::write(&path_tmp, b"Corrupted half-written crash data in tmp").expect("write tmp");
    let mut reader_after_crash = read_file(file_name, base_path).expect("read falls back to *0");
    assert_eq!(
        reader_after_crash.read_byte_array().unwrap(),
        b"Previous Valid Version 0 Data"
    );

    // 3. Simulate corrupted *s file (crash mid-flush after rename): *s exists but is invalid
    std::fs::write(&path_s, b"TDF$GarbageTruncated").expect("write invalid *s");
    // read_file should skip corrupted *s and recover data from *0!
    let mut reader_fallback = read_file(file_name, base_path).expect("read recovers from *0");
    assert_eq!(
        reader_fallback.read_byte_array().unwrap(),
        b"Previous Valid Version 0 Data"
    );

    // 4. Overwrite with valid new write: write_sync produces valid *s and removes old *0
    let mut writer_new = FileWriteDescriptor::new(file_name, base_path, true);
    writer_new.write_byte_array(b"Committed New Version Data");
    writer_new.finish().expect("new write succeeds");

    assert!(!path_0.exists(), "write_sync must clean up legacy *0 file");
    assert!(path_s.exists(), "write_sync must produce safe *s file");
    let mut reader_new = read_file(file_name, base_path).expect("read new version");
    assert_eq!(
        reader_new.read_byte_array().unwrap(),
        b"Committed New Version Data"
    );
}

// =========================================================================
// 4. SQLite Account DB Concurrency Stress Tests
// =========================================================================

#[test]
fn test_sqlite_account_db_wal_concurrency_stress() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_name = "account_concurrency";
    let base_path = temp_dir.path();

    // Open database in WAL mode
    let account = Arc::new(StorageAccount::open(db_name, base_path).expect("open account db"));

    let num_threads = 8;
    let ops_per_thread = 150;
    let mut handles = Vec::new();

    let error_counter = Arc::new(AtomicUsize::new(0));
    let success_counter = Arc::new(AtomicUsize::new(0));

    for thread_idx in 0..num_threads {
        let account = Arc::clone(&account);
        let error_counter = Arc::clone(&error_counter);
        let success_counter = Arc::clone(&success_counter);

        let handle = thread::spawn(move || {
            for i in 0..ops_per_thread {
                let peer_id = 1000 + (thread_idx as i64 * 100) + (i % 20) as i64;
                let topic_id = (i % 5) as i32;

                // 1. Insert / update draft
                let draft = MessageDraft {
                    peer_id,
                    topic_root_id: topic_id,
                    reply_to_id: Some(i as i32),
                    text_content: format!("Thread {thread_idx} draft iteration {i}"),
                    cursor_position: (i % 100) as i32,
                    updated_at: 1700000000 + (i as i64),
                };
                if let Err(e) = account.write_draft(&draft) {
                    eprintln!("Thread {thread_idx} write_draft error: {e}");
                    error_counter.fetch_add(1, Ordering::SeqCst);
                } else {
                    success_counter.fetch_add(1, Ordering::SeqCst);
                }

                // 2. Read draft
                match account.read_draft(peer_id, topic_id) {
                    Ok(Some(d)) => {
                        assert_eq!(d.peer_id, peer_id);
                        success_counter.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        eprintln!("Thread {thread_idx} read_draft error: {e}");
                        error_counter.fetch_add(1, Ordering::SeqCst);
                    }
                }

                // 3. Increment hashtag
                let tag = format!("tag_{}_{}", thread_idx, i % 10);
                if let Err(e) = account.increment_recent_hashtag(&tag) {
                    eprintln!("Thread {thread_idx} increment_hashtag error: {e}");
                    error_counter.fetch_add(1, Ordering::SeqCst);
                } else {
                    success_counter.fetch_add(1, Ordering::SeqCst);
                }

                // 4. Query hashtags
                if (i % 10) == 0 {
                    if let Err(e) = account.read_recent_hashtags(5) {
                        eprintln!("Thread {thread_idx} read_hashtags error: {e}");
                        error_counter.fetch_add(1, Ordering::SeqCst);
                    } else {
                        success_counter.fetch_add(1, Ordering::SeqCst);
                    }
                }

                // 5. Update sticker set
                let set_id = (thread_idx * 1000 + (i % 10)) as u64;
                let sticker = StickerSetRecord {
                    set_id,
                    access_hash: set_id * 3,
                    title: format!("StickerSet {set_id}"),
                    short_name: format!("stickers_{set_id}"),
                    count: 30,
                    installed_order: (i % 10) as i32,
                };
                if let Err(e) = account.write_sticker_set(&sticker) {
                    eprintln!("Thread {thread_idx} write_sticker_set error: {e}");
                    error_counter.fetch_add(1, Ordering::SeqCst);
                } else {
                    success_counter.fetch_add(1, Ordering::SeqCst);
                }

                // 6. Write file location
                let loc = FileLocation {
                    media_key: format!("doc_{thread_idx}_{i}"),
                    file_path: format!("/cache/doc_{thread_idx}_{i}.mp4"),
                    file_size: 1024 * (i as i64 + 1),
                    modified_time: 1700000000 + i as i64,
                };
                if let Err(e) = account.write_file_location(&loc) {
                    eprintln!("Thread {thread_idx} write_file_location error: {e}");
                    error_counter.fetch_add(1, Ordering::SeqCst);
                } else {
                    success_counter.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().expect("thread join");
    }

    assert_eq!(
        error_counter.load(Ordering::SeqCst),
        0,
        "Zero errors (including database locked) expected under concurrent WAL access"
    );
    assert!(
        success_counter.load(Ordering::SeqCst) >= (num_threads * ops_per_thread * 4),
        "Expected high number of completed concurrent operations"
    );

    // Final consistency verification
    let installed_stickers = account.read_installed_sticker_sets().expect("read stickers");
    assert!(!installed_stickers.is_empty());

    let hashtags = account.read_recent_hashtags(20).expect("read hashtags");
    assert!(!hashtags.is_empty());
}

// =========================================================================
// 5. SparseIdsList Edge Cases & Boundary Tests
// =========================================================================

#[test]
fn test_sparse_ids_list_edge_cases_empty_boundaries_monotonic() {
    // 1. Query completely empty list
    let empty_list = SparseIdsList::new();
    assert!(empty_list.is_empty());
    assert_eq!(empty_list.total_count(), None);

    let empty_res = empty_list.snapshot(&SparseIdsListQuery::new(50, 10, 10));
    assert!(empty_res.message_ids.is_empty());
    assert_eq!(empty_res.skipped_before, None);
    assert_eq!(empty_res.skipped_after, None);
    assert_eq!(empty_res.count, None);

    // 2. Query beyond boundaries of single slice [100..200]
    let mut list = SparseIdsList::new();
    let ids: Vec<MsgId> = (100..=200).step_by(5).collect(); // 100, 105, 110, ... 200
    list.add_slice(ids.clone(), MsgRange::new(100, 200), Some(ids.len()));
    assert_eq!(list.total_count(), Some(21));

    // Query strictly before boundary (around_id = 50)
    let res_before = list.snapshot(&SparseIdsListQuery::new(50, 10, 10));
    assert!(
        res_before.message_ids.is_empty(),
        "Querying before slice range must return empty message_ids"
    );

    // Query strictly after boundary (around_id = 300)
    let res_after = list.snapshot(&SparseIdsListQuery::new(300, 10, 10));
    assert!(
        res_after.message_ids.is_empty(),
        "Querying after slice range must return empty message_ids"
    );

    // Query exact start boundary (around_id = 100)
    let res_start = list.snapshot(&SparseIdsListQuery::new(100, 5, 5));
    assert_eq!(res_start.skipped_before, Some(0));
    assert_eq!(res_start.message_ids[0], 100);
    assert_eq!(res_start.message_ids.len(), 6); // 100 + 5 after

    // Query exact end boundary (around_id = 200)
    let res_end = list.snapshot(&SparseIdsListQuery::new(200, 5, 5));
    assert_eq!(res_end.skipped_after, Some(0));
    assert_eq!(*res_end.message_ids.last().unwrap(), 200);

    // 3. Monotonic ordering invariant across diverse query limits
    for around in [100, 105, 112, 150, 188, 200] {
        for limit_before in [0, 1, 3, 10, 50] {
            for limit_after in [0, 1, 3, 10, 50] {
                let snap = list.snapshot(&SparseIdsListQuery::new(around, limit_before, limit_after));
                for window in snap.message_ids.windows(2) {
                    assert!(
                        window[0] < window[1],
                        "Monotonic ordering violation: {} >= {} around {} limits ({}, {})",
                        window[0],
                        window[1],
                        around,
                        limit_before,
                        limit_after
                    );
                }
            }
        }
    }

    // 4. Negative message IDs support (local unsent messages in Telegram)
    let mut neg_list = SparseIdsList::new();
    let neg_ids = vec![-100, -90, -80, -50, -1];
    neg_list.add_slice(neg_ids.clone(), MsgRange::new(-100, -1), Some(5));

    let neg_res = neg_list.snapshot(&SparseIdsListQuery::new(-80, 2, 2));
    assert_eq!(neg_res.message_ids, vec![-100, -90, -80, -50, -1]);
    for window in neg_res.message_ids.windows(2) {
        assert!(window[0] < window[1]);
    }
}

#[test]
fn test_sparse_ids_list_overlapping_and_adjacent_slices_merge() {
    let mut list = SparseIdsList::new();

    // Slice 1: 10..20
    list.add_slice(vec![10, 12, 15, 20], MsgRange::new(10, 20), None);
    // Slice 2: Overlapping 18..30 (merges with 10..20 into 10..30)
    list.add_slice(vec![18, 22, 25, 30], MsgRange::new(18, 30), None);

    let res_merged = list.snapshot(&SparseIdsListQuery::new(20, 10, 10));
    assert_eq!(res_merged.message_ids, vec![10, 12, 15, 18, 20, 22, 25, 30]);

    // Slice 3: Adjacent 31..40 (since 31 == 30 + 1, overlaps_or_adjacent returns true -> merges into 10..40)
    list.add_slice(vec![31, 35, 40], MsgRange::new(31, 40), None);

    let res_adj = list.snapshot(&SparseIdsListQuery::new(25, 10, 10));
    assert_eq!(res_adj.message_ids, vec![10, 12, 15, 18, 20, 22, 25, 30, 31, 35, 40]);

    // Slice 4: Disjoint with a gap: 100..120
    list.add_slice(vec![100, 110, 120], MsgRange::new(100, 120), None);

    // Query in the gap (around_id = 70)
    let res_gap = list.snapshot(&SparseIdsListQuery::new(70, 5, 5));
    assert!(res_gap.message_ids.is_empty(), "Gap between slices must return empty");

    // Query second slice
    let res_slice2 = list.snapshot(&SparseIdsListQuery::new(110, 2, 2));
    assert_eq!(res_slice2.message_ids, vec![100, 110, 120]);

    // Test remove_one and remove_all
    list.remove_one(110);
    let after_remove = list.snapshot(&SparseIdsListQuery::new(110, 2, 2));
    assert_eq!(after_remove.message_ids, vec![100, 120]);

    list.remove_all();
    assert!(list.is_empty());
    assert_eq!(list.total_count(), Some(0));
}
