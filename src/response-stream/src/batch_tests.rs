use super::*;

#[test]
fn rejected_delta_preserves_pending_content_for_subsequent_updates() {
    let mut batch = Batch::new();
    batch.put(
        0,
        ContentBlock::new(ContentBlockInfo::Text {
            text: "First".into(),
        }),
    );

    assert!(
        batch
            .accum(
                &0,
                Delta::InputJsonDelta {
                    partial_json: "{}".into(),
                },
            )
            .is_err()
    );
    batch
        .accum(
            &0,
            Delta::TextDelta {
                text: " second".into(),
            },
        )
        .unwrap();
    batch.apply_reduce(&0, None).unwrap();

    let items = batch.extract_and_pre_process().unwrap();
    assert!(matches!(
        &items[0],
        ProcessedItem::Content(MessageContent::MessageBlock { text, .. })
            if text == "First second"
    ));
}

#[test]
fn rejected_delta_preserves_completed_content() {
    let mut batch = Batch::new();
    batch.complete_item(
        0,
        MessageContent::MessageBlock {
            text: "Final".into(),
            phase: None,
        },
    );

    assert!(
        batch
            .accum(
                &0,
                Delta::TextDelta {
                    text: " discarded".into(),
                },
            )
            .is_err()
    );

    let items = batch.extract_and_pre_process().unwrap();
    assert!(matches!(
        &items[0],
        ProcessedItem::Content(MessageContent::MessageBlock { text, .. })
            if text == "Final"
    ));
}
