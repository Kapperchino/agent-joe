use super::{CommandMenu, HomeMenu, InputMode, Progress, TUIApp};
use crate::branding;
use crate::theme::{self, KeyHint};
use common_models::interaction::{PlanReview, StepState, WorkMode};
use common_models::tui_models::{Lifecycle, ValidationState};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders},
};

impl TUIApp {
    pub(super) fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let block = Block::new()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(theme::BORDER));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let mut title = vec![Span::styled(
            format!(" {} ", branding::TITLE),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )];
        if area.width >= 64 {
            title.push(theme::muted(" /  THE RUST WORKSPACE"));
        }
        let title = Line::from(title);
        let [brand, model] = Layout::horizontal([
            Constraint::Length(u16::try_from(title.width()).unwrap_or(u16::MAX)),
            Constraint::Min(0),
        ])
        .areas(inner);
        frame.render_widget(title, brand);
        let config = self.config_context.get_config();
        let model_label = match area.width {
            0..64 => format!("{} ", config.get_model()),
            _ => format!("{}  ·  {} ", config.get_model(), config.get_effort()),
        };
        frame.render_widget(Line::from(theme::muted(model_label)).right_aligned(), model);
    }

    pub(super) fn progress_line(&self, width: u16) -> Line<'static> {
        let mode = match self.interaction.planning.mode {
            WorkMode::Plan => theme::badge("PLAN", theme::AMBER),
            WorkMode::Implement => theme::badge("BUILD", theme::ACCENT),
        };
        let color = match self.progress {
            Progress::Turn(Lifecycle::Failed)
            | Progress::Operation {
                state: Lifecycle::Failed,
                ..
            } => theme::RED,
            _ if self.root_busy => theme::AMBER,
            Progress::Turn(Lifecycle::Completed)
            | Progress::Operation {
                state: Lifecycle::Completed,
                ..
            } => theme::GREEN,
            _ => theme::ACCENT,
        };
        let label = match &self.progress {
            Progress::Input { kind, .. } => format!("{kind:?} input"),
            Progress::Operation { state, .. } => format!("{state:?}"),
            _ => self.progress.to_string(),
        };
        let status = Line::from(vec![
            mode,
            Span::styled(format!("  ● {label}"), Style::default().fg(color)),
        ]);
        let mut details = Vec::new();
        if self.interaction.planning.review() == PlanReview::Required {
            details.push(Span::styled(
                " · plan needs review",
                Style::default().fg(theme::AMBER),
            ));
        }
        if !self.interaction.questions.is_empty() {
            details.push(Span::styled(
                format!(" · questions {}", self.interaction.questions.len()),
                Style::default().fg(theme::AMBER),
            ));
        }
        if let Some(validation) = &self.validation {
            let color = match validation.state {
                ValidationState::Passed => theme::GREEN,
                ValidationState::Failed => theme::RED,
                ValidationState::NotRun => theme::MUTED,
            };
            details.push(Span::styled(
                format!(" · last {} {:?}", validation.operation, validation.state),
                Style::default().fg(color),
            ));
        }
        let steps = &self.interaction.planning.plan.steps;
        let completed = steps
            .iter()
            .filter(|step| step.state == StepState::Completed)
            .count();
        let workers = self
            .workers
            .values()
            .filter(|state| !state.terminal())
            .count();
        details.extend([
            theme::muted(format!(" · plan {completed}/{}", steps.len())),
            theme::muted(format!(" · queued {}", self.queued.len())),
            theme::muted(format!(" · workers {workers}")),
        ]);
        if let Progress::Operation { detail, .. } = &self.progress {
            details.push(theme::muted(format!(" · {detail}")));
        }
        details.into_iter().fold(status, |mut line, detail| {
            if line.width() + detail.width() <= usize::from(width) {
                line.spans.push(detail);
            }
            line
        })
    }

    pub(super) fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let [context, hints] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        frame.render_widget(self.context_line(context.width), context);
        let mut shortcuts = match self.input_mode {
            InputMode::HomeMenu(HomeMenu::Normal) | InputMode::None => vec![
                KeyHint::new("i", "write"),
                KeyHint::new("/", "commands"),
                KeyHint::new("Enter", "send"),
                KeyHint::new("q", "quit"),
            ],
            InputMode::HomeMenu(HomeMenu::Editing) => vec![
                KeyHint::new("Enter", "send"),
                KeyHint::new("Esc", "normal"),
                KeyHint::new("Ctrl+n", "new line"),
            ],
            InputMode::HomeMenu(HomeMenu::InputCommand) => vec![
                KeyHint::new("Tab", "complete"),
                KeyHint::new("Enter", "run"),
                KeyHint::new("Esc", "cancel"),
            ],
            InputMode::CommandMenu(CommandMenu::ModelSelector) => vec![
                KeyHint::new("j/k", "select"),
                KeyHint::new("Enter", "confirm"),
                KeyHint::new("Esc", "cancel"),
            ],
            InputMode::CommandMenu(CommandMenu::SessionSelector) => vec![
                KeyHint::new("↑/↓", "select"),
                KeyHint::new("Enter", "resume"),
                KeyHint::new("Esc", "cancel"),
            ],
        };
        if self.root_busy && matches!(self.input_mode, InputMode::HomeMenu(HomeMenu::Normal)) {
            shortcuts.insert(0, KeyHint::new("Ctrl+c", "interrupt"));
        }
        frame.render_widget(theme::hints(&shortcuts, hints.width), hints);
    }

    fn context_line(&self, width: u16) -> Line<'static> {
        let context = &self.request_context;
        let percent = context
            .estimated_tokens
            .saturating_mul(100)
            .checked_div(context.ceiling)
            .unwrap_or_default()
            .min(100);
        let filled = percent * 8 / 100;
        let color = match percent {
            0..70 => theme::ACCENT,
            70..90 => theme::AMBER,
            _ => theme::RED,
        };
        let label = match context.ceiling {
            0 => " context — ".to_string(),
            _ => format!(
                " context ~{}/{} ",
                theme::compact_number(u64::try_from(context.estimated_tokens).unwrap_or(u64::MAX)),
                theme::compact_number(u64::try_from(context.ceiling).unwrap_or(u64::MAX)),
            ),
        };
        let mut spans = vec![theme::muted(label)];
        if width >= 48 && context.ceiling > 0 {
            spans.extend([
                Span::styled("━".repeat(filled), Style::default().fg(color)),
                Span::styled("─".repeat(8 - filled), Style::default().fg(theme::BORDER)),
                theme::muted(format!(" {percent}%")),
            ]);
        }
        if width >= 70 {
            spans.extend([
                theme::muted("  ·  total "),
                Span::styled(
                    format!(
                        "↑ {}  ↓ {}",
                        theme::compact_number(u64::from(self.token_count.input_tokens)),
                        theme::compact_number(u64::from(self.token_count.output_tokens))
                    ),
                    Style::default().fg(theme::ACCENT),
                ),
            ]);
        }
        if width >= 100 && context.response_reserve > 0 {
            spans.push(theme::muted(format!(
                "  ·  {} response reserved",
                theme::compact_number(u64::from(context.response_reserve))
            )));
        }
        Line::from(spans)
    }
}
