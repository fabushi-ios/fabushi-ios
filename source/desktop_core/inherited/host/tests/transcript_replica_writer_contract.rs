use mahayana_host_runtime::extensions::transcript::replica_writer::HostReplicaWriter;

#[test]
fn replica_writer_preserves_frozen_epoch_sequence_and_snapshot_contract() {
    let writer = HostReplicaWriter::with_epoch("epoch-a");
    assert_eq!(writer.process_epoch(), "epoch-a");
    assert_eq!(writer.last_sequence("roster"), 0);
    assert_eq!(writer.last_sequence("transcript:a"), 0);

    let first = writer.next_stamp("roster");
    let second = writer.next_stamp("roster");
    let independent = writer.next_stamp("transcript:a");
    assert_eq!(first.replica_key, "roster");
    assert_eq!(first.epoch, "epoch-a");
    assert_eq!(first.sequence, 1);
    assert_eq!(second.sequence, 2);
    assert_eq!(independent.sequence, 1);
    assert_eq!(writer.last_sequence("roster"), 2);
    assert_eq!(writer.last_sequence("transcript:a"), 1);

    let snapshot = writer.capture_snapshot(
        "roster",
        "complete-roster",
        vec!["a", "b"],
    );
    assert_eq!(snapshot.replica_key, "roster");
    assert_eq!(snapshot.epoch, "epoch-a");
    assert_eq!(snapshot.through_sequence, 2);
    assert_eq!(snapshot.coverage, "complete-roster");
    assert_eq!(snapshot.value, vec!["a", "b"]);
}

#[test]
fn default_writer_mints_a_non_empty_process_epoch() {
    let writer = HostReplicaWriter::new();
    assert!(!writer.process_epoch().is_empty());
    let stamp = writer.next_stamp("roster");
    assert_eq!(stamp.epoch, writer.process_epoch());
}
