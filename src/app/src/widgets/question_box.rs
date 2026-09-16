use crate::theme::{self, KeyHint};
use commands::command::{Answer, QuestionAnswer};
use common_models::interaction::Question;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

pub(crate) struct QuestionBox;

#[derive(Default)]
pub(crate) struct QuestionPickerState {
    forms: Vec<QuestionForm>,
    selected: usize,
}

struct QuestionForm {
    question: Question,
    choices: ListState,
    text: String,
    mode: AnswerMode,
    error: Option<String>,
    prompt_scroll: u16,
}

enum AnswerMode {
    Choosing,
    Text,
    Submitting(Answer),
}

pub(crate) enum QuestionAction {
    Stay,
    Close,
    Submit(QuestionAnswer),
}

impl QuestionPickerState {
    pub fn sync(&mut self, questions: &[Question]) -> bool {
        let selected_id = self
            .forms
            .get(self.selected)
            .map(|form| form.question.id.clone());
        let has_new = questions.iter().any(|question| {
            !self
                .forms
                .iter()
                .any(|form| form.question.id == question.id)
        });
        let mut previous = std::mem::take(&mut self.forms);
        self.forms = questions
            .iter()
            .map(
                |question| match previous.iter().position(|form| form.question == *question) {
                    Some(index) => previous.remove(index),
                    None => QuestionForm::new(question.clone()),
                },
            )
            .collect();
        self.selected = self
            .forms
            .iter()
            .position(|form| Some(&form.question.id) == selected_id.as_ref())
            .unwrap_or(0);
        has_new
    }

    pub fn is_empty(&self) -> bool {
        self.forms.is_empty()
    }

    pub fn answered(&mut self, answer: &QuestionAnswer, result: String) {
        if let Some(form) = self.forms.iter_mut().find(|form| {
            form.question.id == answer.id
                && matches!(&form.mode, AnswerMode::Submitting(pending) if pending == &answer.answer)
        }) {
            form.mode = match answer.answer {
                Answer::Choice { .. } => AnswerMode::Choosing,
                Answer::Text(_) => AnswerMode::Text,
            };
            form.error = Some(result);
        }
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(form) = self.forms.get_mut(self.selected)
            && matches!(form.mode, AnswerMode::Text)
        {
            form.text.extend(
                text.chars().filter(|character| {
                    !character.is_control() || matches!(character, '\n' | '\t')
                }),
            );
            form.error = None;
        }
    }

    pub fn key(&mut self, key: &KeyEvent) -> QuestionAction {
        match key.code {
            _ if key.kind == KeyEventKind::Release
                || (key.kind == KeyEventKind::Repeat && key.code == KeyCode::Enter) =>
            {
                QuestionAction::Stay
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                QuestionAction::Close
            }
            KeyCode::Tab | KeyCode::BackTab if !self.forms.is_empty() => {
                let offset = match key.code == KeyCode::BackTab
                    || key.modifiers.contains(KeyModifiers::SHIFT)
                {
                    true => self.forms.len() - 1,
                    false => 1,
                };
                self.selected = (self.selected + offset) % self.forms.len();
                QuestionAction::Stay
            }
            _ => self
                .forms
                .get_mut(self.selected)
                .map_or(QuestionAction::Close, |form| form.key(key)),
        }
    }

    pub fn hints(&self) -> Vec<KeyHint> {
        let mut hints = match self.forms.get(self.selected).map(|form| &form.mode) {
            Some(AnswerMode::Choosing) => vec![
                KeyHint::new("↑/↓ j/k", "select"),
                KeyHint::new("Enter", "answer"),
            ],
            Some(AnswerMode::Text) => vec![
                KeyHint::new("Enter", "answer"),
                KeyHint::new("Ctrl+n", "new line"),
            ],
            _ => Vec::new(),
        };
        hints.push(KeyHint::new("Esc", "back"));
        if self.forms.len() > 1 {
            hints.push(KeyHint::new("Tab/Shift+Tab", "question"));
        }
        hints
    }
}

impl QuestionForm {
    fn new(question: Question) -> Self {
        let mode = match question.choices.is_empty() {
            true => AnswerMode::Text,
            false => AnswerMode::Choosing,
        };
        Self {
            question,
            choices: ListState::default().with_selected(Some(0)),
            text: String::new(),
            mode,
            error: None,
            prompt_scroll: 0,
        }
    }

    fn key(&mut self, key: &KeyEvent) -> QuestionAction {
        match key.code {
            KeyCode::Esc => match self.mode {
                AnswerMode::Text if !self.question.choices.is_empty() => {
                    self.mode = AnswerMode::Choosing;
                    self.error = None;
                    QuestionAction::Stay
                }
                _ => QuestionAction::Close,
            },
            KeyCode::PageDown => {
                self.prompt_scroll = self.prompt_scroll.saturating_add(3);
                QuestionAction::Stay
            }
            KeyCode::PageUp => {
                self.prompt_scroll = self.prompt_scroll.saturating_sub(3);
                QuestionAction::Stay
            }
            _ => match self.mode {
                AnswerMode::Choosing => self.choice_key(key),
                AnswerMode::Text => self.text_key(key),
                AnswerMode::Submitting(_) => QuestionAction::Stay,
            },
        }
    }

    fn choice_key(&mut self, key: &KeyEvent) -> QuestionAction {
        let count = self.question.choices.len() + usize::from(self.question.allow_free_text);
        let selected = self.choices.selected().unwrap_or(0);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.choices.select(Some((selected + 1) % count));
                QuestionAction::Stay
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.choices.select(Some((selected + count - 1) % count));
                QuestionAction::Stay
            }
            KeyCode::Enter => match self.question.choices.get(selected) {
                Some(choice) => self.submit(Answer::Choice {
                    choice_id: choice.id.clone(),
                }),
                None if self.question.allow_free_text => {
                    self.mode = AnswerMode::Text;
                    self.error = None;
                    QuestionAction::Stay
                }
                None => QuestionAction::Stay,
            },
            _ => QuestionAction::Stay,
        }
    }

    fn text_key(&mut self, key: &KeyEvent) -> QuestionAction {
        match key.code {
            KeyCode::Enter if key.modifiers.is_empty() => {
                self.submit(Answer::Text(self.text.clone()))
            }
            KeyCode::Enter => {
                self.text.push('\n');
                QuestionAction::Stay
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.push('\n');
                QuestionAction::Stay
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.clear();
                self.error = None;
                QuestionAction::Stay
            }
            KeyCode::Backspace => {
                self.text.pop();
                self.error = None;
                QuestionAction::Stay
            }
            KeyCode::Char(character)
                if !character.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.text.push(character);
                self.error = None;
                QuestionAction::Stay
            }
            _ => QuestionAction::Stay,
        }
    }

    fn submit(&mut self, answer: Answer) -> QuestionAction {
        match self.question.answer(&answer) {
            Ok(_) => {
                self.mode = AnswerMode::Submitting(answer.clone());
                self.error = None;
                QuestionAction::Submit(QuestionAnswer {
                    id: self.question.id.clone(),
                    answer,
                })
            }
            Err(error) => {
                self.error = Some(error.to_string());
                QuestionAction::Stay
            }
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let prompt = wrapped(&self.question.prompt, area.width);
        let prompt_height = u16::try_from(prompt.len())
            .unwrap_or(u16::MAX)
            .min(area.height / 2);
        let [prompt_area, answer_area, error_area] = Layout::vertical([
            Constraint::Length(prompt_height),
            Constraint::Min(1),
            Constraint::Length(u16::from(self.error.is_some()) * 2),
        ])
        .areas(area);
        let max_scroll = u16::try_from(prompt.len())
            .unwrap_or(u16::MAX)
            .saturating_sub(prompt_area.height);
        self.prompt_scroll = self.prompt_scroll.min(max_scroll);
        Paragraph::new(prompt)
            .scroll((self.prompt_scroll, 0))
            .render(prompt_area, buf);
        match self.mode {
            AnswerMode::Choosing => {
                let items = self
                    .question
                    .choices
                    .iter()
                    .map(|choice| choice.label.as_str())
                    .chain(
                        self.question
                            .allow_free_text
                            .then_some("Other (type an answer)"),
                    )
                    .map(|label| {
                        ListItem::new(wrapped(label, answer_area.width.saturating_sub(3)))
                    });
                let list = List::new(items).highlight_symbol(" › ").highlight_style(
                    Style::default()
                        .fg(theme::ACCENT)
                        .bg(theme::SELECTION)
                        .add_modifier(Modifier::BOLD),
                );
                StatefulWidget::render(list, answer_area, buf, &mut self.choices);
            }
            AnswerMode::Text => {
                let lines = wrapped(&format!("Your answer: {}▏", self.text), answer_area.width);
                let scroll = u16::try_from(lines.len())
                    .unwrap_or(u16::MAX)
                    .saturating_sub(answer_area.height);
                Paragraph::new(lines)
                    .scroll((scroll, 0))
                    .style(Style::default().fg(theme::ACCENT))
                    .render(answer_area, buf);
            }
            AnswerMode::Submitting(_) => {
                Paragraph::new("Submitting answer…")
                    .style(Style::default().fg(theme::MUTED))
                    .render(answer_area, buf);
            }
        }
        if let Some(error) = &self.error {
            Paragraph::new(wrapped(error, error_area.width))
                .style(Style::default().fg(theme::RED))
                .render(error_area, buf);
        }
    }
}

fn wrapped(text: &str, width: u16) -> Vec<Line<'static>> {
    textwrap::wrap(text, usize::from(width.max(1)))
        .into_iter()
        .map(|line| Line::from(line.into_owned()))
        .collect()
}

impl StatefulWidget for QuestionBox {
    type State = QuestionPickerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let total = state.forms.len();
        if let Some(form) = state.forms.get_mut(state.selected) {
            let kind = match form.question.required {
                true => "required",
                false => "optional",
            };
            let block = theme::panel(
                format!(
                    "Question {}/{} · {} · {kind}",
                    state.selected + 1,
                    total,
                    form.question.id
                ),
                theme::AMBER,
            )
            .title_bottom(Line::from(theme::muted(" PgUp/PgDn: scroll prompt ")));
            let inner = block.inner(area);
            block.render(area, buf);
            form.render(inner, buf);
        }
    }
}
