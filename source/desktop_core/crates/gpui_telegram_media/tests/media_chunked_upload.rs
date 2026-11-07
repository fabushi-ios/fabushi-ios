use gpui_telegram_media::chunked_upload::{recommend_part_size, ChunkedUploadSession};
use sha2::{Digest, Sha256};

#[test]
fn test_chunked_upload_multi_part_flow() {
    let file_size = 500_000u64;
    let part_size = 131_072u32; // 128KB
    let mut session = ChunkedUploadSession::new("up_01", file_size, part_size).unwrap();

    assert_eq!(session.total_parts, 4);
    assert_eq!(session.expected_part_size(0), 131_072);
    assert_eq!(session.expected_part_size(1), 131_072);
    assert_eq!(session.expected_part_size(2), 131_072);
    assert_eq!(session.expected_part_size(3), (500_000 % 131_072) as usize);

    // Ingest out of order
    let p3 = vec![4u8; session.expected_part_size(3)];
    let p1 = vec![2u8; 131_072];
    let p0 = vec![1u8; 131_072];
    let p2 = vec![3u8; 131_072];

    session.receive_part(3, &p3).unwrap();
    assert_eq!(session.missing_parts(), vec![0, 1, 2]);

    session.receive_part(1, &p1).unwrap();
    assert_eq!(session.missing_parts(), vec![0, 2]);

    session.receive_part(0, &p0).unwrap();
    assert_eq!(session.missing_parts(), vec![2]);

    session.receive_part(2, &p2).unwrap();
    assert!(session.missing_parts().is_empty());
    assert!(session.is_complete());

    let payload = session.assemble_payload().unwrap();
    assert_eq!(payload.len(), 500_000);
}

#[test]
fn test_chunked_upload_checksum() {
    let data = b"Hello, resilient Telegram chunked media upload world!";
    let mut hasher = Sha256::new();
    hasher.update(data);
    let expected = format!("{:x}", hasher.finalize());

    let mut session = ChunkedUploadSession::new("chk_01", data.len() as u64, 10)
        .unwrap()
        .with_expected_sha256(&expected);

    for (i, chunk) in data.chunks(10).enumerate() {
        session.receive_part(i as u32, chunk).unwrap();
    }

    assert!(session.finalize_and_verify().unwrap());
}

#[test]
fn test_session_serialization_resume() {
    let mut session = ChunkedUploadSession::new("pause_doc", 300, 100).unwrap();
    session.receive_part(0, &vec![0xAA; 100]).unwrap();

    let json = serde_json::to_string(&session).unwrap();
    let mut resumed: ChunkedUploadSession = serde_json::from_str(&json).unwrap();

    assert_eq!(resumed.missing_parts(), vec![1, 2]);
    resumed.receive_part(1, &vec![0xBB; 100]).unwrap();
    resumed.receive_part(2, &vec![0xCC; 100]).unwrap();

    assert!(resumed.is_complete());
    let payload = resumed.assemble_payload().unwrap();
    assert_eq!(payload.len(), 300);
}

#[test]
fn test_recommended_part_sizes() {
    assert_eq!(recommend_part_size(100_000), 32 * 1024);
    assert_eq!(recommend_part_size(5_000_000), 64 * 1024);
    assert_eq!(recommend_part_size(50_000_000), 128 * 1024);
}
