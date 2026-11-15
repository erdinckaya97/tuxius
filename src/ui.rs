use inquire::ui::{Attributes, Color, RenderConfig, StyleSheet, Styled};
use std::io::IsTerminal;
use std::sync::OnceLock;

/// ANSI 256-color index of the single accent color (Tux yellow).
const ACCENT: u8 = 220;

fn color_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal())
}

pub fn paint(text: &str, code: &str) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn bold(t: &str) -> String {
    paint(t, "1")
}
pub fn dim(t: &str) -> String {
    paint(t, "2")
}
pub fn accent(t: &str) -> String {
    paint(t, &format!("38;5;{ACCENT}"))
}

pub fn ok(msg: &str) {
    eprintln!("  {} {msg}", paint("✔", "32"));
}
pub fn warn(msg: &str) {
    eprintln!("  {} {msg}", paint("!", "33"));
}
pub fn error(msg: &str) {
    eprintln!("  {} {msg}", paint("✘", "31"));
}
pub fn step(msg: &str) {
    eprintln!("  {} {}", accent("◌"), dim(msg));
}

const LOGO: [&str; 2] = ["▀█▀ █ █ ▀▄▀ █ █ █ █▀", " █  █▄█ █ █ █ █▄█ ▄█"];

pub fn banner() {
    eprintln!();
    for line in LOGO {
        eprintln!("  {}", paint(line, &format!("1;38;5;{ACCENT}")));
    }
    eprintln!(
        "  {}  {}",
        dim("ssh, without the typing"),
        dim(&format!("v{}", env!("CARGO_PKG_VERSION")))
    );
    eprintln!();
}

pub fn install_theme() {
    let mut cfg = RenderConfig::default_colored();
    if !color_enabled() {
        cfg = RenderConfig::empty();
    } else {
        let accent = Color::AnsiValue(ACCENT);
        cfg.prompt_prefix = Styled::new("›").with_fg(accent);
        cfg.answered_prompt_prefix = Styled::new("✔").with_fg(Color::DarkGreen);
        cfg.highlighted_option_prefix = Styled::new("❯").with_fg(accent);
        cfg.scroll_up_prefix = Styled::new("↑").with_fg(Color::DarkGrey);
        cfg.scroll_down_prefix = Styled::new("↓").with_fg(Color::DarkGrey);
        cfg.selected_option = Some(
            StyleSheet::new()
                .with_fg(accent)
                .with_attr(Attributes::BOLD),
        );
        cfg.help_message = StyleSheet::new().with_fg(Color::DarkGrey);
        cfg.answer = StyleSheet::new().with_fg(accent);
        cfg.placeholder = StyleSheet::new().with_fg(Color::DarkGrey);
        cfg.default_value = StyleSheet::new().with_fg(Color::DarkGrey);
    }
    inquire::set_global_render_config(cfg);
}
