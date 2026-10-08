use super::*;

#[test]
fn completed_and_processed_content_follow_block_indices_not_insertion_order() {
    let mut batch = Batch::new();
    for index in [23, 4, 17, 0, 9, 1] {
        batch.complete_item(
            index,
            MessageContent::MessageBlock {
                text: index.to_string(),
                phase: None,
            },
        );
    }
    let completed = batch.completed_content();
    let expected = ["0", "1", "4", "9", "17", "23"];
    assert_eq!(completed.len(), expected.len());
    for (content, text) in completed.iter().zip(expected) {
        assert!(
            matches!(content, MessageContent::MessageBlock { text: actual, .. } if actual == text)
        );
    }
    let processed = batch.extract_and_pre_process().unwrap();
    assert_eq!(processed.len(), expected.len());
    for (item, text) in processed.iter().zip(expected) {
        assert!(
            matches!(item, ProcessedItem::Content(MessageContent::MessageBlock { text: actual, .. }) if actual == text)
        );
    }
    assert!(batch.completed_content().is_empty());
}

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
