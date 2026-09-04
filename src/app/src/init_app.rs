use anyhow::{Context, Result, anyhow};
use clients::config::Config;
use clients::openai_codex_auth::{
    REDIRECT_URI, create_authorization_flow, exchange_authorization_code, parse_redirect_input,
};
use clients::{
    ClaudeAuthConfig, ClaudeConfig, ClaudeEffort, ClaudeKeyConfig, LocalOpenAIConfig,
    OpenAIAuthConfig, OpenAICodexConfig, OpenAIConfig, OpenAIEffort, OpenAIKeyConfig,
    OpenRouterConfig,
};
use crossterm::event::{EventStream, KeyCode, KeyEvent};
use futures::StreamExt;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::Event,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};
use std::process::Command;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use url::Url;
// I will desloppify this eventually, trust

pub struct InitApp {
    provider: Provider,
    openai_auth_mode: OpenAIAuthMode,
    selected_field: InitField,
    api_key: String,
    url: String,
    model: String,
    character_index: usize,
    error: Option<String>,
    status: Option<String>,
    do_quit: bool,
    codex_login: Option<CodexLoginState>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Provider {
    Claude,
    OpenAI,
    Local,
    OpenRouter,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenAIAuthMode {
    ApiKey,
    Codex,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InitField {
    Provider,
    OpenAIAuth,
    Credential,
    Url,
    RedirectInput,
    Model,
    Action,
}

struct CodexLoginState {
    auth_url: String,
    verifier: String,
    expected_state: String,
    redirect_input: String,
    callback_rx: Option<mpsc::UnboundedReceiver<CodexCallbackEvent>>,
    browser_opened: bool,
    server_ready: bool,
}

enum CodexCallbackEvent {
    Code(String),
    Error(String),
}

impl Default for InitApp {
    fn default() -> Self {
        let provider = Provider::Claude;
        let openai_auth_mode = OpenAIAuthMode::ApiKey;
        Self {
            provider,
            openai_auth_mode,
            selected_field: InitField::Provider,
            api_key: String::new(),
            url: default_url(provider, openai_auth_mode).to_string(),
            model: default_model(provider, openai_auth_mode).to_string(),
            character_index: 0,
            error: None,
            status: None,
            do_quit: false,
            codex_login: None,
        }
    }
}

impl InitApp {
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<DefaultTerminal> {
        let mut events = EventStream::new();
        let period = Duration::from_secs_f32(1.0 / 30.0);
        let mut interval = tokio::time::interval(period);

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                Some(Ok(event)) = events.next() => {
                    if let Some(_) = self.handle_term_event(&event).await? {
                        terminal.clear()?;
                        return Ok(terminal);
                    }
                }
            }

            if let Some(_) = self.poll_codex_callback().await? {
                terminal.clear()?;
                return Ok(terminal);
            }

            terminal.draw(|frame| self.draw(frame))?;

            if self.do_quit {
                return Err(anyhow!("Setup cancelled"));
            }
        }
    }

    async fn handle_term_event(&mut self, event: &Event) -> Result<Option<Config>> {
        match event {
            Event::Key(key) => self.handle_key_event(key).await,
            Event::Paste(text) => {
                if self.selected_field.is_text_input() {
                    self.paste(text);
                }
                Ok(None)
            }
            Event::FocusGained | Event::FocusLost | Event::Mouse(_) | Event::Resize(_, _) => {
                Ok(None)
            }
        }
    }

    async fn handle_key_event(&mut self, key: &KeyEvent) -> Result<Option<Config>> {
        match key.code {
            KeyCode::Char('q') if !self.selected_field.is_text_input() => {
                self.do_quit = true;
                Ok(None)
            }
            KeyCode::Esc => {
                self.do_quit = true;
                Ok(None)
            }
            KeyCode::Tab | KeyCode::Down => {
                self.select_next_field();
                Ok(None)
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.select_prev_field();
                Ok(None)
            }
            KeyCode::Left => {
                match self.selected_field {
                    InitField::Provider => self.set_provider(self.provider.previous()),
                    InitField::OpenAIAuth => {
                        self.set_openai_auth_mode(self.openai_auth_mode.previous())
                    }
                    field if field.is_text_input() => self.move_cursor_left(),
                    _ => {}
                }
                Ok(None)
            }
            KeyCode::Right => {
                match self.selected_field {
                    InitField::Provider => self.set_provider(self.provider.next()),
                    InitField::OpenAIAuth => {
                        self.set_openai_auth_mode(self.openai_auth_mode.next())
                    }
                    field if field.is_text_input() => self.move_cursor_right(),
                    _ => {}
                }
                Ok(None)
            }
            KeyCode::Enter => match self.selected_field {
                InitField::Action => self.submit_action().await,
                InitField::RedirectInput => self.submit_codex_manual_redirect().await,
                _ => {
                    self.select_next_field();
                    Ok(None)
                }
            },
            KeyCode::Backspace if self.selected_field.is_text_input() => {
                self.delete_char();
                Ok(None)
            }
            KeyCode::Char(to_insert) if self.selected_field.is_text_input() => {
                self.enter_char(to_insert);
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    async fn poll_codex_callback(&mut self) -> Result<Option<Config>> {
        let event = match self
            .codex_login
            .as_mut()
            .and_then(|login| login.callback_rx.as_mut())
        {
            Some(rx) => match rx.try_recv() {
                Ok(event) => Some(event),
                Err(mpsc::error::TryRecvError::Empty)
                | Err(mpsc::error::TryRecvError::Disconnected) => None,
            },
            None => None,
        };

        match event {
            Some(CodexCallbackEvent::Code(code)) => self.complete_codex_login(code).await,
            Some(CodexCallbackEvent::Error(err)) => {
                self.error = Some(err);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    async fn submit_action(&mut self) -> Result<Option<Config>> {
        match self.provider {
            Provider::Claude => self.save_claude_config().await,
            Provider::Local => self.save_local_config().await,
            Provider::OpenRouter => self.save_openrouter_config().await,
            Provider::OpenAI => match self.openai_auth_mode {
                OpenAIAuthMode::ApiKey => self.save_openai_api_key_config().await,
                OpenAIAuthMode::Codex => {
                    if self.codex_login.is_some() {
                        self.submit_codex_manual_redirect().await
                    } else {
                        self.start_codex_login()?;
                        Ok(None)
                    }
                }
            },
        }
    }

    async fn save_claude_config(&mut self) -> Result<Option<Config>> {
        let api_key = self.api_key.trim();
        if api_key.is_empty() {
            self.error = Some("API key is required".to_string());
            return Ok(None);
        }

        let model = self.model.trim();
        if model.is_empty() {
            self.error = Some("Model is required".to_string());
            return Ok(None);
        }

        let config = Config::Claude(ClaudeConfig {
            auth: ClaudeAuthConfig::APIKey(ClaudeKeyConfig {
                api_key: api_key.to_string(),
            }),
            model: model.to_string(),
            effort: ClaudeEffort::Med,
        });
        config.save().await?;
        Ok(Some(config))
    }

    async fn save_openai_api_key_config(&mut self) -> Result<Option<Config>> {
        let api_key = self.api_key.trim();
        if api_key.is_empty() {
            self.error = Some("API key is required".to_string());
            return Ok(None);
        }

        let model = self.model.trim();
        if model.is_empty() {
            self.error = Some("Model is required".to_string());
            return Ok(None);
        }

        let config = Config::OpenAI(OpenAIConfig {
            request_encrypted_reasoning: None,
            auth: OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
                api_key: api_key.to_string(),
                url: None,
            }),
            model: model.to_string(),
            effort: OpenAIEffort::Medium,
        });
        config.save().await?;
        Ok(Some(config))
    }

    async fn save_local_config(&mut self) -> Result<Option<Config>> {
        let model = self.model.trim();
        if model.is_empty() {
            self.error = Some("Model is required".to_string());
            return Ok(None);
        }

        let url = self.url.trim();
        if url.is_empty() {
            self.error = Some("URL is required".to_string());
            return Ok(None);
        }

        let api_key = self.api_key.trim();
        let config = Config::OpenAI(OpenAIConfig {
            request_encrypted_reasoning: None,
            auth: OpenAIAuthConfig::Local(LocalOpenAIConfig {
                api_key: (!api_key.is_empty()).then(|| api_key.to_string()),
                url: url.to_string(),
            }),
            model: model.to_string(),
            effort: OpenAIEffort::Medium,
        });
        config.save().await?;
        Ok(Some(config))
    }

    async fn save_openrouter_config(&mut self) -> Result<Option<Config>> {
        let api_key = self.api_key.trim();
        if api_key.is_empty() {
            self.error = Some("Token is required".to_string());
            return Ok(None);
        }

        let model = self.model.trim();
        if model.is_empty() {
            self.error = Some("Model is required".to_string());
            return Ok(None);
        }

        let url = self.url.trim();
        if url.is_empty() {
            self.error = Some("URL is required".to_string());
            return Ok(None);
        }

        let config = Config::OpenAI(OpenAIConfig {
            request_encrypted_reasoning: None,
            auth: OpenAIAuthConfig::OpenRouter(OpenRouterConfig {
                api_key: api_key.to_string(),
                url: Some(url.to_string()),
            }),
            model: model.to_string(),
            effort: OpenAIEffort::Medium,
        });
        config.save().await?;
        Ok(Some(config))
    }

    fn start_codex_login(&mut self) -> Result<()> {
        let model = self.model.trim();
        if model.is_empty() {
            self.error = Some("Model is required".to_string());
            return Ok(());
        }

        let flow = create_authorization_flow()?;
        let (callback_rx, server_ready) = match spawn_codex_callback_server(flow.state.clone()) {
            Ok(rx) => (Some(rx), true),
            Err(err) => {
                self.status = Some(format!(
                    "Local callback server could not start ({err}). Use manual redirect paste."
                ));
                (None, false)
            }
        };
        let browser_opened = open_browser(&flow.auth_url);
        if browser_opened {
            self.status = Some(
                "Browser login started. Complete auth there or paste the redirect URL here."
                    .to_string(),
            );
        } else if self.status.is_none() {
            self.status = Some(
                "Open the authorization URL manually, then paste the redirect URL here."
                    .to_string(),
            );
        }

        self.error = None;
        self.codex_login = Some(CodexLoginState {
            auth_url: flow.auth_url,
            verifier: flow.verifier,
            expected_state: flow.state,
            redirect_input: String::new(),
            callback_rx,
            browser_opened,
            server_ready,
        });
        self.selected_field = InitField::RedirectInput;
        self.sync_cursor_to_selected_field();
        Ok(())
    }

    async fn submit_codex_manual_redirect(&mut self) -> Result<Option<Config>> {
        let Some(login) = &self.codex_login else {
            self.error = Some("Start the Codex login flow first".to_string());
            return Ok(None);
        };

        let parsed = parse_redirect_input(&login.redirect_input)?;
        if let Some(state) = parsed.state.as_deref() {
            if state != login.expected_state {
                self.error = Some("Redirect state did not match the login session".to_string());
                return Ok(None);
            }
        }

        self.complete_codex_login(parsed.code).await
    }

    async fn complete_codex_login(&mut self, code: String) -> Result<Option<Config>> {
        let login = self
            .codex_login
            .take()
            .context("Codex login was not initialized")?;
        let codex_auth = exchange_authorization_code(&code, &login.verifier).await?;
        let config = self.build_codex_config(codex_auth)?;
        config.save().await?;
        Ok(Some(config))
    }

    fn build_codex_config(&mut self, auth: OpenAICodexConfig) -> Result<Config> {
        let model = self.model.trim();
        if model.is_empty() {
            self.error = Some("Model is required".to_string());
            return Err(anyhow!("Model is required"));
        }

        Ok(Config::OpenAI(OpenAIConfig {
            request_encrypted_reasoning: None,
            auth: OpenAIAuthConfig::Codex(auth),
            model: model.to_string(),
            effort: OpenAIEffort::Medium,
        }))
    }

    fn set_provider(&mut self, provider: Provider) {
        if self.provider == provider {
            return;
        }

        let previous_default_model = default_model(self.provider, self.openai_auth_mode);
        let previous_default_url = default_url(self.provider, self.openai_auth_mode);
        self.provider = provider;
        if self.model.is_empty() || self.model == previous_default_model {
            self.model = default_model(self.provider, self.openai_auth_mode).to_string();
        }
        if self.url.is_empty() || self.url == previous_default_url {
            self.url = default_url(self.provider, self.openai_auth_mode).to_string();
        }
        self.reset_openai_login_if_needed();
        self.error = None;
        self.status = None;
        self.ensure_selected_field_visible();
    }

    fn set_openai_auth_mode(&mut self, auth_mode: OpenAIAuthMode) {
        if self.openai_auth_mode == auth_mode {
            return;
        }

        let previous_default_model = default_model(self.provider, self.openai_auth_mode);
        let previous_default_url = default_url(self.provider, self.openai_auth_mode);
        self.openai_auth_mode = auth_mode;
        if self.model.is_empty() || self.model == previous_default_model {
            self.model = default_model(self.provider, self.openai_auth_mode).to_string();
        }
        if self.url.is_empty() || self.url == previous_default_url {
            self.url = default_url(self.provider, self.openai_auth_mode).to_string();
        }
        self.reset_openai_login_if_needed();
        self.error = None;
        self.status = None;
        self.ensure_selected_field_visible();
    }

    fn reset_openai_login_if_needed(&mut self) {
        if self.provider != Provider::OpenAI || self.openai_auth_mode != OpenAIAuthMode::Codex {
            self.codex_login = None;
        }
    }

    fn select_next_field(&mut self) {
        let fields = self.visible_fields();
        let current_index = fields
            .iter()
            .position(|field| *field == self.selected_field)
            .unwrap_or(0);
        self.selected_field = fields[(current_index + 1) % fields.len()];
        self.sync_cursor_to_selected_field();
    }

    fn select_prev_field(&mut self) {
        let fields = self.visible_fields();
        let current_index = fields
            .iter()
            .position(|field| *field == self.selected_field)
            .unwrap_or(0);
        let next_index = if current_index == 0 {
            fields.len() - 1
        } else {
            current_index - 1
        };
        self.selected_field = fields[next_index];
        self.sync_cursor_to_selected_field();
    }

    fn ensure_selected_field_visible(&mut self) {
        if !self.visible_fields().contains(&self.selected_field) {
            self.selected_field = self.visible_fields()[0];
        }
        self.sync_cursor_to_selected_field();
    }

    fn visible_fields(&self) -> Vec<InitField> {
        let mut fields = vec![InitField::Provider];

        match self.provider {
            Provider::Claude => {
                fields.push(InitField::Credential);
            }
            Provider::OpenAI => {
                fields.push(InitField::OpenAIAuth);
                match self.openai_auth_mode {
                    OpenAIAuthMode::ApiKey => fields.push(InitField::Credential),
                    OpenAIAuthMode::Codex => {
                        if self.codex_login.is_some() {
                            fields.push(InitField::RedirectInput);
                        }
                    }
                }
            }
            Provider::Local | Provider::OpenRouter => {
                fields.push(InitField::Credential);
                fields.push(InitField::Url);
            }
        }

        fields.push(InitField::Model);
        fields.push(InitField::Action);
        fields
    }

    fn sync_cursor_to_selected_field(&mut self) {
        self.character_index = self
            .selected_field
            .current_text(self)
            .map_or(0, |value| value.chars().count());
    }

    fn move_cursor_left(&mut self) {
        let cursor_moved_left = self.character_index.saturating_sub(1);
        self.character_index = self.clamp_cursor(cursor_moved_left);
    }

    fn move_cursor_right(&mut self) {
        let cursor_moved_right = self.character_index.saturating_add(1);
        self.character_index = self.clamp_cursor(cursor_moved_right);
    }

    fn enter_char(&mut self, new_char: char) {
        let character_index = self.character_index;
        if let Some(input) = self.selected_field.current_text_mut(self) {
            let index = Self::byte_index(input, character_index);
            input.insert(index, new_char);
            self.character_index = self.clamp_cursor(self.character_index.saturating_add(1));
            self.error = None;
        }
    }

    fn paste(&mut self, string: &str) {
        let character_index = self.character_index;
        if let Some(input) = self.selected_field.current_text_mut(self) {
            let index = Self::byte_index(input, character_index);
            input.insert_str(index, string);
            self.character_index =
                self.clamp_cursor(self.character_index.saturating_add(string.chars().count()));
            self.error = None;
        }
    }

    fn delete_char(&mut self) {
        if self.character_index == 0 {
            return;
        }

        let current_index = self.character_index;
        if let Some(input) = self.selected_field.current_text_mut(self) {
            let before_char_to_delete = input.chars().take(current_index - 1);
            let after_char_to_delete = input.chars().skip(current_index);
            *input = before_char_to_delete.chain(after_char_to_delete).collect();
            self.character_index = self.character_index.saturating_sub(1);
            self.error = None;
        }
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        self.selected_field
            .current_text(self)
            .map_or(0, |value| new_cursor_pos.clamp(0, value.chars().count()))
    }

    fn byte_index(input: &str, character_index: usize) -> usize {
        input
            .char_indices()
            .map(|(i, _)| i)
            .nth(character_index)
            .unwrap_or(input.len())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let popup = centered_rect(frame.area(), 88, 24);
        frame.render_widget(Clear, popup);

        let block = Block::bordered().title(" turbo-code setup ");
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let [header_area, form_area, footer_area] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(11),
            Constraint::Length(7),
        ])
        .areas(inner);

        let header = Paragraph::new(vec![
            Line::from(Span::styled(
                "No config found. Create one here and the app will reuse it next time.",
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                "Use dedicated tabs for Claude, OpenAI, local-compatible endpoints, and OpenRouter.",
                Style::default().fg(Color::Gray),
            )),
        ]);
        frame.render_widget(header, header_area);

        let mut lines = Vec::new();
        let mut cursor = None;

        lines.push(self.form_line(
            InitField::Provider,
            vec![
                Span::styled("Provider: ", self.label_style()),
                self.provider_chip(Provider::Claude),
                Span::raw(" "),
                self.provider_chip(Provider::OpenAI),
                Span::raw(" "),
                self.provider_chip(Provider::Local),
                Span::raw(" "),
                self.provider_chip(Provider::OpenRouter),
            ],
        ));
        lines.push(Line::default());

        if self.provider == Provider::OpenAI {
            lines.push(self.form_line(
                InitField::OpenAIAuth,
                vec![
                    Span::styled("Auth:     ", self.label_style()),
                    self.openai_auth_chip(OpenAIAuthMode::ApiKey),
                    Span::raw(" "),
                    self.openai_auth_chip(OpenAIAuthMode::Codex),
                ],
            ));
            lines.push(Line::default());
        }

        match self.provider {
            Provider::Claude => {
                let api_key_display = if self.api_key.is_empty() {
                    String::new()
                } else {
                    "*".repeat(self.api_key.chars().count())
                };
                let y = lines.len() as u16;
                lines.push(self.form_line(
                    InitField::Credential,
                    vec![
                        Span::styled("API Key:  ", self.label_style()),
                        Span::styled(api_key_display, self.value_style(InitField::Credential)),
                    ],
                ));
                if self.selected_field == InitField::Credential {
                    cursor = Some((
                        form_area.x + 12 + self.character_index as u16,
                        form_area.y + y,
                    ));
                }
                lines.push(Line::default());
            }
            Provider::OpenAI => match self.openai_auth_mode {
                OpenAIAuthMode::ApiKey => {
                    let api_key_display = if self.api_key.is_empty() {
                        String::new()
                    } else {
                        "*".repeat(self.api_key.chars().count())
                    };
                    let y = lines.len() as u16;
                    lines.push(self.form_line(
                        InitField::Credential,
                        vec![
                            Span::styled("API Key:  ", self.label_style()),
                            Span::styled(api_key_display, self.value_style(InitField::Credential)),
                        ],
                    ));
                    if self.selected_field == InitField::Credential {
                        cursor = Some((
                            form_area.x + 12 + self.character_index as u16,
                            form_area.y + y,
                        ));
                    }
                    lines.push(Line::default());
                }
                OpenAIAuthMode::Codex => {
                    if let Some(login) = &self.codex_login {
                        let auth_url = format!("Auth URL: {}", login.auth_url);
                        lines.push(Line::from(Span::styled(
                            auth_url,
                            Style::default().fg(Color::DarkGray),
                        )));
                        lines.push(Line::from(Span::styled(
                            codex_status_line(login),
                            Style::default().fg(Color::DarkGray),
                        )));
                        lines.push(Line::default());

                        let y = lines.len() as u16;
                        lines.push(self.form_line(
                            InitField::RedirectInput,
                            vec![
                                Span::styled("Redirect: ", self.label_style()),
                                Span::styled(
                                    login.redirect_input.clone(),
                                    self.value_style(InitField::RedirectInput),
                                ),
                            ],
                        ));
                        if self.selected_field == InitField::RedirectInput {
                            cursor = Some((
                                form_area.x + 12 + self.character_index as u16,
                                form_area.y + y,
                            ));
                        }
                        lines.push(Line::default());
                    } else {
                        lines.push(Line::from(Span::styled(
                            format!(
                                "Codex login uses a PKCE browser flow and expects a callback on {}.",
                                REDIRECT_URI
                            ),
                            Style::default().fg(Color::DarkGray),
                        )));
                        lines.push(Line::from(Span::styled(
                            "Start login to open the browser. If the callback cannot reach the app, paste the full redirect URL manually.",
                            Style::default().fg(Color::DarkGray),
                        )));
                        lines.push(Line::default());
                    }
                }
            },
            Provider::Local | Provider::OpenRouter => {
                let token_optional = self.provider == Provider::Local;
                let token_display = if self.api_key.is_empty() {
                    if token_optional {
                        "<optional>".to_string()
                    } else {
                        String::new()
                    }
                } else {
                    "*".repeat(self.api_key.chars().count())
                };
                let token_y = lines.len() as u16;
                lines.push(self.form_line(
                    InitField::Credential,
                    vec![
                        Span::styled("Token:    ", self.label_style()),
                        Span::styled(token_display, self.value_style(InitField::Credential)),
                    ],
                ));
                if self.selected_field == InitField::Credential {
                    cursor = Some((
                        form_area.x + 12 + self.character_index as u16,
                        form_area.y + token_y,
                    ));
                }
                lines.push(Line::default());

                let url_y = lines.len() as u16;
                lines.push(self.form_line(
                    InitField::Url,
                    vec![
                        Span::styled("URL:      ", self.label_style()),
                        Span::styled(self.url.clone(), self.value_style(InitField::Url)),
                    ],
                ));
                if self.selected_field == InitField::Url {
                    cursor = Some((
                        form_area.x + 12 + self.character_index as u16,
                        form_area.y + url_y,
                    ));
                }
                lines.push(Line::default());
            }
        }

        let model_y = lines.len() as u16;
        lines.push(self.form_line(
            InitField::Model,
            vec![
                Span::styled("Model:    ", self.label_style()),
                Span::styled(self.model.clone(), self.value_style(InitField::Model)),
            ],
        ));
        if self.selected_field == InitField::Model {
            cursor = Some((
                form_area.x + 12 + self.character_index as u16,
                form_area.y + model_y,
            ));
        }
        lines.push(Line::default());

        lines.push(self.form_line(InitField::Action, vec![self.action_button()]));

        let form = Paragraph::new(lines);
        frame.render_widget(form, form_area);

        let mut footer_lines = vec![
            Line::from(Span::styled(
                "Tab/Shift+Tab move  •  Left/Right switches provider or auth mode  •  Enter continues",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                format!(
                    "Config path: {}",
                    Config::path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "<unavailable>".to_string())
                ),
                Style::default().fg(Color::DarkGray),
            )),
        ];

        if let Some(status) = &self.status {
            footer_lines.push(Line::default());
            footer_lines.push(Line::from(Span::styled(
                status.clone(),
                Style::default().fg(Color::Cyan),
            )));
        }

        if let Some(error) = &self.error {
            footer_lines.push(Line::default());
            footer_lines.push(Line::from(Span::styled(
                error.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
        }

        let footer = Paragraph::new(footer_lines);
        frame.render_widget(footer, footer_area);

        if let Some((x, y)) = cursor {
            frame.set_cursor_position((x, y));
        }
    }

    fn form_line(&self, field: InitField, spans: Vec<Span<'static>>) -> Line<'static> {
        let prefix = if self.selected_field == field {
            "› "
        } else {
            "  "
        };
        let prefix_style = if self.selected_field == field {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let mut line = vec![Span::styled(prefix, prefix_style)];
        line.extend(spans);
        Line::from(line)
    }

    fn label_style(&self) -> Style {
        Style::default().fg(Color::Gray)
    }

    fn value_style(&self, field: InitField) -> Style {
        if self.selected_field == field {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        }
    }

    fn provider_chip(&self, provider: Provider) -> Span<'static> {
        selectable_chip(
            provider.label(),
            self.provider == provider,
            self.selected_field == InitField::Provider,
        )
    }

    fn openai_auth_chip(&self, auth_mode: OpenAIAuthMode) -> Span<'static> {
        selectable_chip(
            auth_mode.label(),
            self.openai_auth_mode == auth_mode,
            self.selected_field == InitField::OpenAIAuth,
        )
    }

    fn action_button(&self) -> Span<'static> {
        let label = match (
            self.provider,
            self.openai_auth_mode,
            self.codex_login.is_some(),
        ) {
            (Provider::OpenAI, OpenAIAuthMode::Codex, false) => "[ Start Codex login ]",
            (Provider::OpenAI, OpenAIAuthMode::Codex, true) => "[ Finish Codex login ]",
            _ => "[ Save config ]",
        };

        let style = if self.selected_field == InitField::Action {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };

        Span::styled(label, style)
    }
}

impl Provider {
    fn label(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::OpenAI => "OpenAI",
            Provider::Local => "Local",
            Provider::OpenRouter => "OpenRouter",
        }
    }

    fn next(self) -> Self {
        match self {
            Provider::Claude => Provider::OpenAI,
            Provider::OpenAI => Provider::Local,
            Provider::Local => Provider::OpenRouter,
            Provider::OpenRouter => Provider::Claude,
        }
    }

    fn previous(self) -> Self {
        match self {
            Provider::Claude => Provider::OpenRouter,
            Provider::OpenAI => Provider::Claude,
            Provider::Local => Provider::OpenAI,
            Provider::OpenRouter => Provider::Local,
        }
    }
}

impl OpenAIAuthMode {
    fn label(self) -> &'static str {
        match self {
            OpenAIAuthMode::ApiKey => "API key",
            OpenAIAuthMode::Codex => "Codex login",
        }
    }

    fn next(self) -> Self {
        match self {
            OpenAIAuthMode::ApiKey => OpenAIAuthMode::Codex,
            OpenAIAuthMode::Codex => OpenAIAuthMode::ApiKey,
        }
    }

    fn previous(self) -> Self {
        self.next()
    }
}

impl InitField {
    fn is_text_input(self) -> bool {
        matches!(
            self,
            InitField::Credential | InitField::Url | InitField::RedirectInput | InitField::Model
        )
    }

    fn current_text<'a>(self, app: &'a InitApp) -> Option<&'a String> {
        match self {
            InitField::Credential => Some(&app.api_key),
            InitField::Url => Some(&app.url),
            InitField::Model => Some(&app.model),
            InitField::RedirectInput => app.codex_login.as_ref().map(|login| &login.redirect_input),
            InitField::Provider | InitField::OpenAIAuth | InitField::Action => None,
        }
    }

    fn current_text_mut<'a>(self, app: &'a mut InitApp) -> Option<&'a mut String> {
        match self {
            InitField::Credential => Some(&mut app.api_key),
            InitField::Url => Some(&mut app.url),
            InitField::Model => Some(&mut app.model),
            InitField::RedirectInput => app
                .codex_login
                .as_mut()
                .map(|login| &mut login.redirect_input),
            InitField::Provider | InitField::OpenAIAuth | InitField::Action => None,
        }
    }
}

fn default_url(provider: Provider, auth_mode: OpenAIAuthMode) -> &'static str {
    match (provider, auth_mode) {
        (Provider::Claude, _) => "",
        (Provider::Local, _) => "http://localhost:11434/v1",
        (Provider::OpenRouter, _) => "https://openrouter.ai/api/v1",
        (Provider::OpenAI, OpenAIAuthMode::ApiKey) => "",
        (Provider::OpenAI, OpenAIAuthMode::Codex) => "",
    }
}

fn default_model(provider: Provider, auth_mode: OpenAIAuthMode) -> &'static str {
    match (provider, auth_mode) {
        (Provider::Claude, _) => "claude-sonnet-4-20250514",
        (Provider::Local, _) => "qwen2.5-coder:latest",
        (Provider::OpenRouter, _) => "openai/gpt-5",
        (Provider::OpenAI, OpenAIAuthMode::ApiKey) => "gpt-5.4",
        (Provider::OpenAI, OpenAIAuthMode::Codex) => "gpt-5.4",
    }
}

fn selectable_chip(label: &'static str, is_selected: bool, is_active_field: bool) -> Span<'static> {
    let style = if is_selected {
        let mut style = Style::default().fg(Color::Black).bg(Color::Cyan);
        if is_active_field {
            style = style.add_modifier(Modifier::BOLD);
        }
        style
    } else {
        Style::default().fg(Color::DarkGray)
    };

    Span::styled(label, style)
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let [vertical] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [horizontal] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(vertical);
    horizontal
}

fn codex_status_line(login: &CodexLoginState) -> String {
    match (login.browser_opened, login.server_ready) {
        (true, true) => "Browser opened and callback listener is ready.".to_string(),
        (true, false) => {
            "Browser opened. Paste the redirect URL because local callback is unavailable."
                .to_string()
        }
        (false, true) => "Open the auth URL manually. Callback listener is ready.".to_string(),
        (false, false) => {
            "Open the auth URL manually, then paste the redirect URL here.".to_string()
        }
    }
}

fn spawn_codex_callback_server(
    expected_state: String,
) -> Result<mpsc::UnboundedReceiver<CodexCallbackEvent>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:1455")?;
    listener.set_nonblocking(true)?;
    let listener = TcpListener::from_std(listener)?;
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(err) => {
                    let _ = tx.send(CodexCallbackEvent::Error(format!(
                        "Callback server error: {err}"
                    )));
                    break;
                }
            };

            let mut buffer = [0_u8; 4096];
            let read_len = match socket.read(&mut buffer).await {
                Ok(len) => len,
                Err(err) => {
                    let _ = tx.send(CodexCallbackEvent::Error(format!(
                        "Failed to read the callback request: {err}"
                    )));
                    break;
                }
            };

            let request = String::from_utf8_lossy(&buffer[..read_len]);
            let request_line = request.lines().next().unwrap_or_default();
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or("/auth/callback");
            let url = match Url::parse(&format!("http://localhost{path}")) {
                Ok(url) => url,
                Err(err) => {
                    let _ = write_http_response(&mut socket, 400, "Invalid callback URL").await;
                    let _ = tx.send(CodexCallbackEvent::Error(format!(
                        "Failed to parse callback URL: {err}"
                    )));
                    continue;
                }
            };

            let mut code = None;
            let mut state = None;
            for (key, value) in url.query_pairs() {
                match key.as_ref() {
                    "code" => code = Some(value.to_string()),
                    "state" => state = Some(value.to_string()),
                    _ => {}
                }
            }

            match (code, state) {
                (Some(code), Some(state)) if state == expected_state => {
                    let _ = write_http_response(
                        &mut socket,
                        200,
                        "Login complete. You can return to turbo-code.",
                    )
                    .await;
                    let _ = tx.send(CodexCallbackEvent::Code(code));
                    break;
                }
                _ => {
                    let _ = write_http_response(
                        &mut socket,
                        400,
                        "Callback was missing a valid code or state.",
                    )
                    .await;
                    let _ = tx.send(CodexCallbackEvent::Error(
                        "Callback did not contain a valid authorization code".to_string(),
                    ));
                    break;
                }
            }
        }
    });

    Ok(rx)
}

async fn write_http_response(
    socket: &mut tokio::net::TcpStream,
    status: u16,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await
}

fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        return Command::new("open").arg(url).spawn().is_ok();
    }

    #[cfg(target_os = "linux")]
    {
        return Command::new("xdg-open").arg(url).spawn().is_ok();
    }

    #[cfg(target_os = "windows")]
    {
        return Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .is_ok();
    }

    #[allow(unreachable_code)]
    false
}
