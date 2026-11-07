use gpui_telegram_client::storage::details::storage_file_utilities::*;
use gpui_telegram_client::storage::details::storage_settings_scheme::*;
use gpui_telegram_client::storage::storage_account::*;
use gpui_telegram_client::storage::storage_cloud_blob::*;
use gpui_telegram_client::storage::storage_domain::*;
use gpui_telegram_client::storage::storage_file_lock::*;
use gpui_telegram_client::storage::storage_folder_archive::*;
use gpui_telegram_client::storage::storage_media_prepare::*;
use gpui_telegram_client::storage::storage_shared_media::*;
use gpui_telegram_client::storage::storage_sparse_ids_list::*;
use gpui_telegram_client::storage::storage_user_photos::*;
use gpui_telegram_client::storage::streamed_file_downloader::*;
use gpui_telegram_client::storage::file_download::*;
use gpui_telegram_client::storage::file_upload::*;
use gpui_telegram_client::storage::localimageloader::*;
use gpui_telegram_client::storage::localstorage::*;
use gpui_telegram_client::storage::serialize_common::*;
use gpui_telegram_client::storage::serialize_document::*;
use gpui_telegram_client::storage::serialize_peer::*;

#[test]
fn test_to_file_part_and_key_generation() {
    let key: FileKey = 0x0123456789ABCDEF;
    let part = to_file_part(key);
    assert_eq!(part.len(), 16);
    assert_eq!(part, "FEDCBA9876543210");

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let base_path = temp_dir.path().to_str().unwrap();
    let generated = generate_key(base_path);
    assert_ne!(generated, 0);

    let part_gen = to_file_part(generated);
    assert!(!key_already_used(base_path, &part_gen));
}

#[test]
fn test_passcode_kdf_argon2id_and_pbkdf2() {
    let passcode = b"SuperSecretPasscode123!";
    let salt = b"salt_for_storage_testing_12345";

    // PBKDF2-HMAC-SHA512
    let local_key = create_local_key(passcode, salt);
    assert_eq!(local_key.len(), 32);
    assert_ne!(local_key, [0u8; 32]);

    // Argon2id KDF
    let kdf = PasscodeKdf {
        kind: PASSCODE_KDF_ARGON2ID,
        memory: 1024, // 1MB for fast unit test
        time: 2,
        parallel: 1,
    };
    assert!(kdf.valid());
    assert!(kdf.cost_within_limits());

    let argon2_key = create_passcode_key(passcode, salt, &kdf).expect("argon2 derivation");
    assert_eq!(argon2_key.len(), 32);
    assert_ne!(argon2_key, [0u8; 32]);
    assert_ne!(argon2_key, local_key);
}

#[test]
fn test_aes_ige_encryption_decryption_roundtrip() {
    let key = [0x42u8; 32];
    let original_payload = b"Hello Telegram Desktop Storage Subsystem! 1:1 Rust GPUI Rewrite";

    let encrypted = prepare_encrypted(original_payload, &key);
    assert!(encrypted.len() > original_payload.len());
    // First 16 bytes is key128 / IV checksum
    assert_eq!((encrypted.len() - 16) % 16, 0);

    let decrypted = decrypt_local(&encrypted, &key).expect("decryption succeeds");
    assert_eq!(decrypted, original_payload);

    // Test tamper resistance: corrupting a byte fails decryption
    let mut tampered = encrypted.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xFF;
    let tamper_res = decrypt_local(&tampered, &key);
    assert!(tamper_res.is_err());
}

#[test]
fn test_tdf_file_atomic_write_and_read() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let base_path = temp_dir.path().to_str().unwrap();

    let key = [0x77u8; 32];
    let test_data = b"TDF Atomic File Persistence Test Payload";

    // 1. Plain file write
    let mut writer = FileWriteDescriptor::new("test_plain", base_path, true);
    writer.write_byte_array(test_data);
    writer.finish().expect("write succeeds");

    let mut reader = read_file("test_plain", base_path).expect("read succeeds");
    assert_eq!(reader.version, APP_VERSION);
    let read_payload = reader.read_byte_array().expect("read byte array");
    assert_eq!(read_payload, test_data);

    // 2. Encrypted file write
    let mut enc_writer = FileWriteDescriptor::new("test_enc", base_path, true);
    enc_writer.write_encrypted(test_data, &key);
    enc_writer.finish().expect("write encrypted succeeds");

    let enc_reader = read_encrypted_file("test_enc", base_path, &key).expect("read encrypted succeeds");
    assert_eq!(enc_reader.version, APP_VERSION);
    assert_eq!(enc_reader.data, test_data);
}

#[test]
fn test_settings_scheme_serialization() {
    let mut writer = SettingsSchemeWriter::new();
    let app_settings = b"{\"theme\":\"dark\",\"scale\":125}";
    writer.write_block(DBI_APPLICATION_SETTINGS, app_settings);

    let theme_buf = [1u8; 16];
    writer.write_block(DBI_THEME_KEY, &theme_buf);

    let encoded = writer.finish();
    assert!(!encoded.is_empty());

    let mut details = StorageSettingsSchemeDetails::new();
    let context = details.parse_settings(&encoded, APP_VERSION);

    assert_eq!(details.last_parsed_blocks, 2);
    assert_eq!(context.application_settings_raw, app_settings);
    assert_eq!(context.theme_key_day, u64::from_le_bytes([1u8; 8]));
    assert_eq!(context.theme_key_night, u64::from_le_bytes([1u8; 8]));
}

#[test]
fn test_sqlite_account_database_operations() {
    let account = StorageAccount::open_in_memory("account_test").expect("open in-memory db");

    // 1. File locations
    let loc = FileLocation {
        media_key: "doc_12345".to_string(),
        file_path: "/tmp/telegram_cache/doc_12345.mp4".to_string(),
        file_size: 1048576,
        modified_time: 1700000000,
    };
    account.write_file_location(&loc).expect("write file location");

    let read_loc = account.read_file_location("doc_12345").expect("read file location");
    assert_eq!(read_loc, Some(loc));

    // 2. Message drafts
    let draft = MessageDraft {
        peer_id: 987654321,
        topic_root_id: 42,
        reply_to_id: Some(101),
        text_content: "Draft message for testing".to_string(),
        cursor_position: 12,
        updated_at: 1700000100,
    };
    account.write_draft(&draft).expect("write draft");

    let read_draft = account.read_draft(987654321, 42).expect("read draft");
    assert_eq!(read_draft, Some(draft));

    account.clear_draft(987654321, 42).expect("clear draft");
    let after_clear = account.read_draft(987654321, 42).expect("read draft after clear");
    assert_eq!(after_clear, None);

    // 3. Sticker sets
    let sticker = StickerSetRecord {
        set_id: 11223344,
        access_hash: 55667788,
        title: "Animated Pepe".to_string(),
        short_name: "pepe_anim".to_string(),
        count: 50,
        installed_order: 1,
    };
    account.write_sticker_set(&sticker).expect("write sticker");

    let sets = account.read_installed_sticker_sets().expect("read sticker sets");
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0], sticker);

    // 4. Recent hashtags
    account.increment_recent_hashtag("telegram").expect("inc hashtag");
    account.increment_recent_hashtag("telegram").expect("inc hashtag");
    account.increment_recent_hashtag("rust").expect("inc hashtag");

    let top = account.read_recent_hashtags(10).expect("read hashtags");
    assert_eq!(top.len(), 2);
    assert_eq!(top[0], "telegram");
    assert_eq!(top[1], "rust");
}

#[test]
fn test_sparse_ids_list_and_shared_media() {
    let mut list = SparseIdsList::new();
    assert!(list.is_empty());

    // Add slice of message IDs: 10, 11, 12, 13, 14, 15
    list.add_slice(vec![10, 11, 12, 13, 14, 15], MsgRange::new(10, 15), Some(6));
    assert_eq!(list.total_count(), Some(6));

    let query = SparseIdsListQuery::new(12, 2, 2);
    let result = list.snapshot(&query);
    assert_eq!(result.message_ids, vec![10, 11, 12, 13, 14]);
    assert_eq!(result.skipped_before, Some(0));
    assert_eq!(result.skipped_after, Some(1));

    // SharedMedia multi-type partition
    let mut shared_media = SharedMedia::new();
    shared_media.add_new(1001, 0, 0, SharedMediaType::Photo, 50);
    shared_media.add_new(1001, 0, 0, SharedMediaType::Photo, 51);
    shared_media.add_new(1001, 0, 0, SharedMediaType::Video, 70);

    let photo_res = shared_media.snapshot(1001, 0, 0, SharedMediaType::Photo, 50, 0, 5);
    assert_eq!(photo_res.message_ids, vec![50, 51]);

    let video_res = shared_media.snapshot(1001, 0, 0, SharedMediaType::Video, 70, 0, 5);
    assert_eq!(video_res.message_ids, vec![70]);
}

#[test]
fn test_user_photos_cache() {
    let mut photos = UserPhotos::new();
    photos.add_new(42, 1001);
    photos.add_new(42, 1002);
    photos.add_new(42, 1003);

    let res = photos.query(42, 1003, 1, 1);
    assert_eq!(res.photo_ids.len(), 2);
    assert!(res.photo_ids.contains(&1003));
}

#[test]
fn test_folder_archive_and_cloud_blob_unpacking() {
    let temp_src = tempfile::tempdir().expect("temp_src");
    let file1_path = temp_src.path().join("doc.txt");
    std::fs::write(&file1_path, b"Document content for archive test").expect("write file1");

    let sub_dir = temp_src.path().join("sub");
    std::fs::create_dir_all(&sub_dir).expect("create sub");
    let file2_path = sub_dir.join("image.png");
    std::fs::write(&file2_path, b"\x89PNG\r\n\x1a\nFakePngData").expect("write file2");

    let entries = gather_archive_entries(temp_src.path()).expect("gather entries");
    assert_eq!(entries.list.len(), 3); // 2 files + 1 dir

    let temp_dest = tempfile::tempdir().expect("temp_dest");
    let zip_path = temp_dest.path().join("bundle.zip");

    let result = write_archive(&entries, &zip_path, 100 * 1024 * 1024, |_| true);
    assert_eq!(result.status, ArchiveStatus::Done);
    assert!(result.size > 0);

    // Unpack with storage_cloud_blob unpack_blob
    let unpack_dir = temp_dest.path().join("unpacked");
    let unpacked = unpack_blob(&zip_path, &unpack_dir, |_| true).expect("unpack blob");
    assert_eq!(unpacked, 2); // 2 files unpacked

    let read_back = std::fs::read_to_string(unpack_dir.join("doc.txt")).expect("read doc.txt");
    assert_eq!(read_back, "Document content for archive test");
}

#[test]
fn test_streamed_file_downloader_progressive_read() {
    let downloader = StreamedFileDownloader::new(300);
    assert_eq!(downloader.total_size(), 300);
    assert_eq!(downloader.downloaded_bytes(), 0);

    // Chunk 0..100
    downloader.feed_part(0, vec![1u8; 100]);
    assert_eq!(downloader.downloaded_bytes(), 100);
    assert!(downloader.is_range_ready(0, 50));
    assert!(downloader.is_range_ready(0, 100));
    assert!(!downloader.is_range_ready(50, 100)); // missing 100..150

    // Chunk 100..200
    downloader.feed_part(100, vec![2u8; 100]);
    assert!(downloader.is_range_ready(50, 100));

    let stream = downloader.read_stream(90, 20).expect("read stream");
    assert_eq!(stream.len(), 20);
    assert_eq!(&stream[..10], &[1u8; 10]);
    assert_eq!(&stream[10..], &[2u8; 10]);
}

#[test]
fn test_serialize_common_document_and_peer() {
    // 1. SerializeCommon
    let mut writer = ByteArrayWriter::new();
    writer.write_string("Hello Telegram");
    writer.write_u64(987654321);
    writer.write_bool(true);
    let bytes = writer.finish();

    let mut reader = ByteArrayReader::new(&bytes);
    assert_eq!(reader.read_string(), "Hello Telegram");
    assert_eq!(reader.read_u64(), 987654321);
    assert!(reader.read_bool());
    assert!(reader.at_end());

    // 2. SerializeDocument
    let doc = SerializedDocument {
        id: 1234567,
        access_hash: 7654321,
        date: 1700000000,
        name: "test.pdf".to_string(),
        mime_type: "application/pdf".to_string(),
        size: 54321,
        dc_id: 2,
        thumbnail_data: vec![0xDE, 0xAD, 0xBE, 0xEF],
        sticker_info: Some(StickerSetInfo::new(999, 888, "stickers")),
    };
    let mut doc_serializer = SerializeDocument::new();
    let doc_bytes = doc_serializer.serialize(&doc);
    let decoded_doc = doc_serializer.deserialize(&doc_bytes).expect("deserialize doc");
    assert_eq!(decoded_doc, doc);

    // 3. SerializePeer
    let peer = SerializedPeer {
        peer_type: PeerType::User,
        id: 777,
        access_hash: 888,
        name: "Alice Bob".to_string(),
        username: "alice_bob".to_string(),
        phone: "+1234567890".to_string(),
    };
    let mut peer_serializer = SerializePeer::new();
    let peer_bytes = peer_serializer.serialize_peer(&peer);
    let decoded_peer = peer_serializer.deserialize_peer(&peer_bytes).expect("deserialize peer");
    assert_eq!(decoded_peer, peer);
}

#[test]
fn test_file_upload_and_download_flow() {
    let mut upload = FileUpload::new();
    let payload = vec![0xAAu8; 1024 * 1024]; // 1 MB payload
    let file_id = upload.queue_upload(payload.clone());

    let mut uploaded_chunks = Vec::new();
    while let Some((idx, part)) = upload.take_part(file_id) {
        uploaded_chunks.push((idx, part));
    }
    assert!(upload.is_finished(file_id));
    assert_eq!(uploaded_chunks.len(), 2); // 1 MB / 512 KB = 2 parts

    // FileLoader download test
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let dest_file = temp_dir.path().join("downloaded.bin");

    let mut loader = FileLoader::new(&dest_file, 1024 * 1024, false, 0);
    loader.start().expect("start loader");

    let mut offset = 0;
    for (_idx, chunk) in uploaded_chunks {
        loader.write_part(offset, &chunk).expect("write part");
        offset += chunk.len() as i64;
    }
    assert!(loader.finished);
    assert_eq!(loader.current_progress(), 1.0);

    let downloaded_data = std::fs::read(&dest_file).expect("read downloaded file");
    assert_eq!(downloaded_data, payload);
}

#[test]
fn test_storage_file_lock_lifecycle() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let lock_file = temp_dir.path().join("tdata.lock");

    let mut lock = FileLock::new();
    let locked = lock.lock(&lock_file).expect("lock result");
    assert!(locked);
    assert!(lock.is_locked());

    lock.unlock();
    assert!(!lock.is_locked());
}

#[test]
fn test_storage_domain_passcode_and_derivation() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let mut domain = StorageDomain::new(temp_dir.path());

    let salt = [0x55u8; 32].to_vec();
    let passcode = b"MySecurePasscode123".to_vec();
    let kdf = PasscodeKdf {
        kind: PASSCODE_KDF_ARGON2ID,
        memory: 1024,
        time: 2,
        parallel: 1,
    };

    let derivation = PasscodeDerivation::new(passcode.clone(), salt.clone(), kdf);
    let start_res = domain.start(derivation);
    assert_eq!(start_res, StartResult::Success);

    // Verify correct passcode generates a nonced verification token
    let verification = domain.verify_passcode(&passcode, &salt, &kdf);
    assert!(verification.is_some());
    let token = verification.unwrap();
    assert!(token.nonce() != 0);

    // Verify incorrect passcode fails
    let bad_verification = domain.verify_passcode(b"wrong", &salt, &kdf);
    assert!(bad_verification.is_none());
}

#[test]
fn test_storage_media_prepare_and_task_queue() {
    // 1. MIME detection
    let p_jpg = std::path::Path::new("image.jpg");
    assert_eq!(detect_mime_type(p_jpg), "image/jpeg");
    let p_png = std::path::Path::new("photo.png");
    assert_eq!(detect_mime_type(p_png), "image/png");
    let p_mp4 = std::path::Path::new("video.mp4");
    assert_eq!(detect_mime_type(p_mp4), "video/mp4");
    let p_unknown = std::path::Path::new("archive.xyz");
    assert_eq!(detect_mime_type(p_unknown), "application/octet-stream");

    // 2. TaskQueue
    let queue = TaskQueue::new();
    let id1 = queue.next_id();
    let id2 = queue.next_id();
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let test_img = temp_dir.path().join("sample.jpg");
    std::fs::write(&test_img, b"sample image bytes").expect("write img");

    let task = ImagePrepareTask::new(id1, test_img);
    let queued_id = queue.add_task(Box::new(task));
    assert_eq!(queued_id, id1);

    let processed = queue.process_one();
    assert_eq!(processed, Some(id1));
}

#[test]
fn test_localstorage_persistence_cycle() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let mut local = Localstorage::new(temp_dir.path());

    local.write_autoupdate_prefix("https://updates.telegram.org/tdesktop".to_string());
    assert_eq!(local.read_autoupdate_prefix(), "https://updates.telegram.org/tdesktop");

    // Persist settings
    local.write_settings().expect("write settings");

    // Read back in new instance
    let mut reader = Localstorage::new(temp_dir.path());
    let _ctx = reader.read_settings().expect("read settings");
}

