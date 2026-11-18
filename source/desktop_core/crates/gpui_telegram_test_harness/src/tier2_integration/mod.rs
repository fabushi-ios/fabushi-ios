#[cfg(test)]
mod tests {
    use crate::fixtures::make_test_message;
    use gpui_telegram_core::types::ChatId;
    use gpui_telegram_proto::sync::UpdateEvent;
    use gpui_telegram_server::sync::PtsEngine;

    #[test]
    fn test_pts_sequence_catchup() {
        let engine = PtsEngine::open_in_memory().expect("open in-memory pts engine");
        let chat = ChatId(100);

        let msg1 = make_test_message(1, 100, 42, "hello 1");
        let msg2 = make_test_message(2, 100, 42, "hello 2");

        let u1 = engine
            .append_update(chat, UpdateEvent::NewMessage(msg1))
            .expect("append update 1");
        let u2 = engine
            .append_update(chat, UpdateEvent::NewMessage(msg2))
            .expect("append update 2");

        assert_eq!(u1.pts, 1);
        assert_eq!(u2.pts, 2);

        let diff = engine.get_difference_legacy(chat, 0, 10).unwrap();
        assert_eq!(diff.updates.len(), 2);
        assert_eq!(diff.current_pts, 2);

        let diff_after_1 = engine.get_difference_legacy(chat, 1, 10).unwrap();
        assert_eq!(diff_after_1.updates.len(), 1);
        assert_eq!(diff_after_1.updates[0].pts, 2);
    }
}
