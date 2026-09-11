use super::*;
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn setup_shares_ferris_branding_without_displacing_provider_fields() {
    for provider in [
        Provider::Claude,
        Provider::OpenAI,
        Provider::Local,
        Provider::OpenRouter,
    ] {
        let mut app = InitApp::default();
        app.set_provider(provider);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        for expected in [
            branding::TITLE,
            branding::TAGLINE,
            branding::CAPTION,
            "Connect your preferred provider",
            "Provider:",
            "Model:",
            "[ Save config ]",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        let title = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == "V")
            .unwrap();
        assert_eq!(title.fg, theme::ACCENT);
        assert_eq!(title.bg, theme::SURFACE);
        assert_eq!(buffer[(0, 0)].bg, theme::BACKGROUND);
    }
}
