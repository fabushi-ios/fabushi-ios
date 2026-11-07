use mahayana_host_runtime::runner::bounded_line_framer::BoundedLineFramer;

#[test]
fn frames_split_opaque_bytes_without_consuming_partial_line() {
    let mut framer = BoundedLineFramer::new(32, "overflow");
    assert_eq!(framer.push(b"alpha\nbe"), Ok(vec![b"alpha".to_vec()]));
    assert_eq!(framer.pending_byte_length(), 2);
    assert_eq!(framer.push(b"ta\n"), Ok(vec![b"beta".to_vec()]));
    assert_eq!(framer.pending_byte_length(), 0);
}

#[test]
fn rejects_unbounded_line_before_accumulating_more_bytes() {
    let mut framer = BoundedLineFramer::new(4, "line too long");
    assert_eq!(framer.push(b"1234"), Ok(Vec::new()));
    assert_eq!(framer.pending_byte_length(), 4);
    assert_eq!(framer.push(b"5"), Err("line too long".into()));
    assert_eq!(framer.pending_byte_length(), 0);
}
