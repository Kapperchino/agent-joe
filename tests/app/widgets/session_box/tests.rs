use super::*;
use common_models::tui_models::Lifecycle;

fn summary(id: &str, title: &str) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        title: title.into(),
        preview: "Saved response".into(),
        updated_at: None,
        status: Lifecycle::Completed,
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn search_navigation_and_selection_keep_the_correct_session_id() {
    let mut picker = SessionPickerState::Loading;
    picker.load(Ok(vec![
        summary("first", "Fix parser"),
        summary("second", "Fix LMDB storage"),
        summary("third", "Résumé search"),
    ]));
    picker.paste("FIX\nSTORAGE");
    assert!(
        matches!(&picker, SessionPickerState::Selecting(selection) if selection.matches.len() == 1 && selection.selected().unwrap().id == "second")
    );
    picker.key(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    picker.key(&key(KeyCode::Down));
    picker.key(&key(KeyCode::Down));
    picker.key(&key(KeyCode::Down));
    assert_eq!(
        picker.key(&KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        )),
        PickerAction::Stay
    );
    assert_eq!(
        picker.key(&key(KeyCode::Enter)),
        PickerAction::Resume { id: "third".into() }
    );
    picker.load(Ok(vec![]));
    assert!(matches!(picker, SessionPickerState::Resuming));
    assert_eq!(picker.key(&key(KeyCode::Enter)), PickerAction::Stay);
    assert_eq!(picker.key(&key(KeyCode::Esc)), PickerAction::Stay);
    assert_eq!(
        picker
            .resumed(Ok(SessionTranscript {
                id: "third".into(),
                messages: vec![],
            }))
            .unwrap()
            .id,
        "third"
    );
    assert!(matches!(picker, SessionPickerState::Closed));
    assert!(picker.resumed(Err("Late error".into())).is_none());
    assert!(matches!(picker, SessionPickerState::Closed));
}

#[test]
fn empty_results_cancel_and_unicode_search_are_safe() {
    let mut picker = SessionPickerState::Loading;
    assert_eq!(picker.key(&key(KeyCode::Esc)), PickerAction::Cancel);
    picker.load(Ok(vec![summary("late", "Late result")]));
    picker.load(Err("Late error".into()));
    assert!(matches!(picker, SessionPickerState::Closed));
    picker = SessionPickerState::new(&ResumeTarget::Picker);
    picker.load(Ok(vec![]));
    picker.key(&key(KeyCode::Down));
    assert_eq!(picker.key(&key(KeyCode::Enter)), PickerAction::Stay);
    picker = SessionPickerState::Loading;
    picker.load(Ok(vec![summary("unicode-id", "Résumé search")]));
    picker.paste("missing");
    assert_eq!(picker.key(&key(KeyCode::Enter)), PickerAction::Stay);
    picker.key(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    picker.key(&key(KeyCode::Char('é')));
    picker.key(&key(KeyCode::Backspace));
    picker.paste("UNICODE-ID");
    assert_eq!(
        picker.key(&key(KeyCode::Enter)),
        PickerAction::Resume {
            id: "unicode-id".into()
        }
    );
}

#[test]
fn picker_renders_results_empty_and_error_states_at_small_sizes() {
    for width in [1, 24, 100] {
        for height in [1, 6, 18] {
            let area = Rect::new(0, 0, width, height);
            let mut buffer = Buffer::empty(area);
            let mut picker = SessionPickerState::Loading;
            SessionBox.render(area, &mut buffer, &mut picker);
            picker.load(Ok(vec![summary("saved-id", "Résumé 🦀")]));
            SessionBox.render(area, &mut buffer, &mut picker);
            picker.paste("no match");
            SessionBox.render(area, &mut buffer, &mut picker);
            picker =
                SessionPickerState::Failed("Session is already open in another process".into());
            SessionBox.render(area, &mut buffer, &mut picker);
        }
    }
}
