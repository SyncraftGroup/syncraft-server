//! Structured terminal UI with fixed-width labels and Unicode boxes.
//!
//! Multi-line messages are drawn inside a Unicode box; single-line messages
//! use a plain label prefix.
//!
//! ```text
//! [STEP] Initialising workspace...
//! ┌─[INFO]───────────────────────────────────────────┐
//! │   Name: MyServer                                 │
//! │   Minecraft: 1.21.1                              │
//! └──────────────────────────────────────────────────┘
//! [ OK ] Workspace initialised successfully.
//! ```

pub mod message;

use std::io::Write;

use inquire::ui::Attributes;
use owo_colors::OwoColorize;

use crate::error::Error;
use message::{translate, MessageKey};

const L_STEP: &str = "[STEP]";
const L_OK:   &str = "[ OK ]";
const L_INFO: &str = "[INFO]";
const L_WARN: &str = "[WARN]";
const L_FAIL: &str = "[FAIL]";
const L_QEST: &str = "[ ?? ]";
const L_GOT:  &str = "[ -- ]";

/// Visual width of every label (used for border alignment).
const LABEL_W: usize = 6;

enum BoxKind {
    Info,
    Warn,
    Error,
    Plain,
}

/// Stateless UI handle.  Obtain via [`Ui::get()`].
pub struct Ui;

impl Ui {
    pub fn get() -> Self { Ui }

    /// Print `[STEP]` in cyan.
    pub fn step(&self, key: MessageKey) {
        eprintln!("{} {}", L_STEP.bright_cyan().bold(), translate(&key));
    }

    /// Print `[ OK ]` in green.
    pub fn ok(&self, key: MessageKey) {
        eprintln!("{} {}", L_OK.bright_green().bold(), translate(&key));
    }

    /// Neutral information. Multi-line content is drawn in a dimmed box.
    pub fn info(&self, key: MessageKey) {
        let msg = translate(&key);
        if msg.contains('\n') {
            draw_box(&msg, BoxKind::Info);
        } else {
            eprintln!("{} {}", L_INFO.dimmed(), msg);
        }
    }

    /// Non-fatal warning. Multi-line content is drawn in a yellow box.
    pub fn warn(&self, key: MessageKey) {
        let msg = translate(&key);
        if msg.contains('\n') {
            draw_box(&msg, BoxKind::Warn);
        } else {
            eprintln!("{} {}", L_WARN.yellow(), msg);
        }
    }

    /// Error (does not exit). Multi-line content is drawn in a red box.
    pub fn error(&self, key: MessageKey) {
        let msg = translate(&key);
        if msg.contains('\n') {
            draw_box(&msg, BoxKind::Error);
        } else {
            eprintln!("{} {}", L_FAIL.red(), msg);
        }
    }

    /// Plain text. Multi-line content gets a plain dimmed box (no label).
    pub fn plain(&self, key: MessageKey) {
        let msg = translate(&key);
        if msg.contains('\n') {
            draw_box(&msg, BoxKind::Plain);
        } else {
            eprintln!("{}", msg);
        }
    }

    /// Prompt for text input.
    pub fn input(&self, key: MessageKey) -> Result<String, Error> {
        message::prompt_input(&translate(&key))
    }

    /// Prompt for y/n confirmation.
    pub fn confirm(&self, key: MessageKey) -> Result<bool, Error> {
        message::prompt_confirm(&translate(&key))
    }

    pub fn echo(&self, text: &str) {
        eprintln!("{}", text);
    }
}

/// Pick a box width wide enough for content but bounded by the terminal.
fn box_width(content: &str) -> usize {
    let max_visual = content
        .lines()
        .map(|l| console::measure_text_width(l))
        .max()
        .unwrap_or(0);

    let term_w = console::Term::stderr().size().1 as usize;

    // Inner width = text + 2 spaces padding.  Outer adds 2 border chars.
    let inner_needed = max_visual + 2;
    let outer_needed = inner_needed + 2;

    outer_needed.clamp(44, term_w.saturating_sub(2).max(44))
}

/// Draw a labelled or plain Unicode box around `content`.
fn draw_box(content: &str, kind: BoxKind) {
    let w = box_width(content);
    let inner = w - 2;
    let text_area = inner - 2;

    let (top, bot, vert) = match kind {
        BoxKind::Plain => {
            let t = format!("┌{}┐", "─".repeat(w - 2));
            let b = format!("└{}┘", "─".repeat(w - 2));
            (t.dimmed().to_string(), b.dimmed().to_string(), "│".dimmed().to_string())
        }
        BoxKind::Info => {
            let t = format!("┌─{}─{}┐", L_INFO, "─".repeat(w.saturating_sub(LABEL_W + 4)));
            let b = format!("└{}┘", "─".repeat(w - 2));
            (t.blue().to_string(), b.blue().to_string(), "│".blue().to_string())
        }
        BoxKind::Warn => {
            let t = format!("┌─{}─{}┐", L_WARN, "─".repeat(w.saturating_sub(LABEL_W + 4)));
            let b = format!("└{}┘", "─".repeat(w - 2));
            (t.yellow().to_string(), b.yellow().to_string(), "│".yellow().to_string())
        }
        BoxKind::Error => {
            let t = format!("┌─{}─{}┐", L_FAIL, "─".repeat(w.saturating_sub(LABEL_W + 4)));
            let b = format!("└{}┘", "─".repeat(w - 2));
            (t.red().to_string(), b.red().to_string(), "│".red().to_string())
        }
    };

    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{top}");
    for line in content.lines() {
        let vis = console::measure_text_width(line);
        let rhs_pad = text_area.saturating_sub(vis);
        let _ = writeln!(stderr, "{vert} {line}{} {vert}", " ".repeat(rhs_pad));
    }
    let _ = writeln!(stderr, "{bot}");
}

/// Configure inquire's prompt colours to match the Syncraft theme.
/// Call once at startup.
pub fn configure_inquire() {
    use inquire::ui::{Color, RenderConfig, Styled};

    let config = RenderConfig::default()
        .with_prompt_prefix(Styled::new(L_QEST).with_fg(Color::LightBlue).with_attr(Attributes::BOLD))
        .with_answered_prompt_prefix(Styled::new(L_GOT).with_fg(Color::LightBlue).with_attr(Attributes::BOLD));

    inquire::set_global_render_config(config);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_get_is_singleton() {
        let _ui = Ui::get();
    }

    #[test]
    fn box_width_is_reasonable() {
        let w = box_width("short line");
        assert!(w >= 44, "box too narrow: {w}");
        assert!(w <= 120, "box suspiciously wide: {w}");
    }

    #[test]
    fn box_width_grows_with_content() {
        let short = box_width("a");
        let long = box_width(&"x".repeat(80));
        assert!(long > short, "box_width should grow: {short} vs {long}");
    }
}
