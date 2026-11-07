use super::*;
use std::cell::Cell;

#[test]
fn private_frame_fragments_share_one_decreasing_budget() {
    let start = Instant::now();
    let clock = Cell::new(start);
    let fragments: [&[u8]; 3] = [b"{\"value\":", b"\"ok\"", b"}\n"];
    let mut reads = 0;
    let mut requests = Vec::new();

    let result = read_private_frame_with(
        |max_bytes, timeout_ms| {
            requests.push((max_bytes, timeout_ms));
            let data = fragments
                .get(reads)
                .expect("a complete frame must stop further reads")
                .to_vec();
            reads += 1;
            clock.set(start + Duration::from_secs(4 * reads as u64));
            Ok(NetworkWorkerStreamRead { data, eof: false })
        },
        || clock.get(),
    );

    assert_eq!(result, Ok(b"{\"value\":\"ok\"}".to_vec()));
    assert_eq!(reads, 3);
    assert_eq!(
        requests,
        vec![
            (REMOTE_TARGET_FRAME_MAX_BYTES, 15_000),
            (REMOTE_TARGET_FRAME_MAX_BYTES - fragments[0].len(), 11_000),
            (
                REMOTE_TARGET_FRAME_MAX_BYTES - fragments[0].len() - fragments[1].len(),
                7_000,
            ),
        ]
    );
}

#[test]
fn private_frame_continued_fragments_cannot_extend_deadline() {
    let start = Instant::now();
    let clock = Cell::new(start);
    let mut reads = 0;
    let mut budgets = Vec::new();

    let result = read_private_frame_with(
        |_, timeout_ms| {
            reads += 1;
            assert!(reads <= 15, "fragments must not reset the frame deadline");
            budgets.push(timeout_ms);
            clock.set(start + Duration::from_secs(reads));
            // Both incomplete fragments and empty successful reads consume the
            // same frame budget; neither supplies an EOF or transport error.
            let data = if reads % 2 == 0 {
                Vec::new()
            } else {
                vec![b'x']
            };
            Ok(NetworkWorkerStreamRead { data, eof: false })
        },
        || clock.get(),
    );

    assert_eq!(result, Err(RemoteComputerTargetError::TransportFailed));
    assert_eq!(reads, 15);
    assert_eq!(
        budgets,
        (1_u64..=15).rev().map(|n| n * 1_000).collect::<Vec<_>>()
    );
}

#[test]
fn private_frame_rejects_complete_frame_at_or_after_deadline() {
    for arrival in [Duration::from_secs(15), Duration::from_millis(15_001)] {
        let start = Instant::now();
        let clock = Cell::new(start);
        let reads = Cell::new(0);

        let result = read_private_frame_with(
            |max_bytes, timeout_ms| {
                reads.set(reads.get() + 1);
                assert_eq!(max_bytes, REMOTE_TARGET_FRAME_MAX_BYTES);
                assert_eq!(timeout_ms, 15_000);
                clock.set(start + arrival);
                Ok(NetworkWorkerStreamRead {
                    data: b"{\"ok\":true}\n".to_vec(),
                    eof: true,
                })
            },
            || clock.get(),
        );

        assert_eq!(
            result,
            Err(RemoteComputerTargetError::TransportFailed),
            "complete frame arriving at {arrival:?} must not be admitted"
        );
        assert_eq!(reads.get(), 1);
    }
}

#[test]
fn private_frame_expiry_prevents_first_read() {
    for elapsed in [
        Duration::from_micros(14_999_500),
        Duration::from_secs(15),
        Duration::from_millis(15_001),
    ] {
        let start = Instant::now();
        let clock = Cell::new(start);
        let reads = Cell::new(0);

        let result = read_private_frame_with(
            |_, _| {
                reads.set(reads.get() + 1);
                Ok(NetworkWorkerStreamRead {
                    data: b"late\n".to_vec(),
                    eof: false,
                })
            },
            || {
                let current = clock.get();
                // The first observation establishes the deadline. Advance
                // before the reader can request its first chunk.
                clock.set(start + elapsed);
                current
            },
        );

        assert_eq!(result, Err(RemoteComputerTargetError::TransportFailed));
        assert_eq!(reads.get(), 0, "no whole millisecond remains at {elapsed:?}");
    }
}

#[test]
fn private_frame_accepts_bounded_newline_frames() {
    let largest_payload = vec![b'x'; REMOTE_TARGET_FRAME_MAX_BYTES - 1];
    let mut largest_frame = largest_payload.clone();
    largest_frame.push(b'\n');
    let cases = [
        (
            b"{\"ok\":true}\n \t\r\n".to_vec(),
            b"{\"ok\":true}".to_vec(),
            true,
        ),
        (b"{}\n".to_vec(), b"{}".to_vec(), false),
        (largest_frame, largest_payload, true),
    ];

    for (data, expected, eof) in cases {
        let start = Instant::now();
        let clock = Cell::new(start);
        let reads = Cell::new(0);

        let result = read_private_frame_with(
            |max_bytes, timeout_ms| {
                reads.set(reads.get() + 1);
                assert_eq!(max_bytes, REMOTE_TARGET_FRAME_MAX_BYTES);
                assert!(data.len() <= max_bytes);
                assert_eq!(timeout_ms, 15_000);
                clock.set(start + Duration::from_millis(14_999));
                Ok(NetworkWorkerStreamRead {
                    data: data.clone(),
                    eof,
                })
            },
            || clock.get(),
        );

        assert_eq!(result, Ok(expected));
        assert_eq!(reads.get(), 1);
    }
}

#[test]
fn private_frame_rejects_malformed_and_oversized_frames() {
    let cases = [
        ("empty", vec![(b"\n".to_vec(), false)]),
        (
            "EOF without newline",
            vec![(b"{\"ok\":true}".to_vec(), true)],
        ),
        (
            "non-whitespace trailing data",
            vec![(b"{}\nsecond".to_vec(), false)],
        ),
        (
            "oversized frame",
            vec![
                (vec![b'x'; REMOTE_TARGET_FRAME_MAX_BYTES], false),
                (b"\n".to_vec(), true),
            ],
        ),
    ];

    for (name, responses) in cases {
        let clock = Cell::new(Instant::now());
        let reads = Cell::new(0);
        let mut responses = responses.into_iter();

        let result = read_private_frame_with(
            |max_bytes, timeout_ms| {
                reads.set(reads.get() + 1);
                let (data, eof) = responses.next().expect("unexpected additional frame read");
                // Honor the real OwnedNetworkWorker transport bound. The
                // oversized case fills the allowance without a newline and
                // must be rejected before requesting the pending next byte.
                assert!(data.len() <= max_bytes, "{name}: transport read bound");
                assert_eq!(timeout_ms, 15_000);
                Ok(NetworkWorkerStreamRead { data, eof })
            },
            || clock.get(),
        );

        assert_eq!(
            result,
            Err(RemoteComputerTargetError::InvalidResponse),
            "{name}"
        );
        assert_eq!(reads.get(), 1, "{name}: reject before another read");
    }
}
