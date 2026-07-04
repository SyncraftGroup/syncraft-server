//! Java / Python style logger — single-character levels, coloured output,
//! continuations for multi-line messages.
//!
//! # Format
//!
//! ```text
//! HH:MM:SS [I] : message
//! HH:MM:SS [W] : warning text
//! HH:MM:SS [E] : error text
//! HH:MM:SS [D] : debug text
//! HH:MM:SS [S] : success text
//! ```
//!
//! Multi-line messages align continuations under the first line:
//!
//! ```text
//! HH:MM:SS [I] : first line
//!              : continuation
//!              : last line
//! ```
//!
//! # Usage
//!
//! ```ignore
//! log_info!("Server listening on port {}", port);
//! log_warn!("Rate limit exceeded for {}", ip);
//! log_error!("Failed to bind socket: {}", e);
//! log_debug!("Request took {}ms", ms);
//! ```
//!
//! Level is controlled by the `SYNCRAFT_LOG` env var (`error`, `warn`,
//! `info`, `debug`).  Default is `info`.

use std::fmt::Write;
use std::fs;
use std::io::Write as IoWrite;
use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;

use chrono::Local;
use owo_colors::OwoColorize;

// ── Level ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

static FILTER: OnceLock<Level> = OnceLock::new();

/// Initialise the logger.  Idempotent — the first call wins.
pub fn init() {
    FILTER.get_or_init(|| match std::env::var("SYNCRAFT_LOG").ok().as_deref() {
        Some("error") => Level::Error,
        Some("warn")  => Level::Warn,
        Some("debug") => Level::Debug,
        _             => Level::Info,
    });
}

/// Optional log file for writing colour-preserving output alongside stderr.
///
/// Under systemd, stderr is captured by journald which strips ANSI codes.
/// Setting a log file ensures the raw coloured output is preserved so that
/// `syncraft serve logs` can tail it.  The fallback daemon skips this —
/// stderr is already redirected to the log file via `dup2`.
static LOG_FILE: OnceLock<Mutex<fs::File>> = OnceLock::new();

/// Enable dual-output to a log file.  Creates parent directories as needed.
/// Idempotent — the first call wins, subsequent calls are ignored.
pub fn init_file(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    LOG_FILE.get_or_init(|| {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("failed to open log file");
        Mutex::new(file)
    });
}

pub(crate) fn enabled(lvl: Level) -> bool {
    FILTER.get().map_or(true, |&max| lvl <= max)
}

// ── Emit ──────────────────────────────────────────────────────────────

pub(crate) fn emit(level_char: &str, color: impl owo_colors::DynColor + Copy, msg: &str) {
    let ts = Local::now().format("%H:%M:%S").to_string();
    let ts = ts.bright_black();

    let lines: Vec<&str> = msg.lines().collect();

    // Build the formatted message in memory so we can write it to both
    // stderr and (optionally) the log file.
    let mut formatted = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i == 0 {
            let _ = writeln!(
                formatted,
                "{} [{}] : {}",
                ts,
                level_char.color(color).bold(),
                line
            );
        } else {
            let _ = writeln!(formatted, "             : {}", line);
        }
    }

    // Stderr (always).
    let mut stderr = std::io::stderr().lock();
    let _ = write!(stderr, "{}", formatted);

    // Log file (optional — set via init_file).
    if let Some(mutex) = LOG_FILE.get() {
        let mut file = mutex.lock().unwrap();
        let _ = write!(file, "{}", formatted);
    }
}

// ── Macros ────────────────────────────────────────────────────────────

/// Log at INFO level (blue  `I`).
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {{
        if $crate::log::enabled($crate::log::Level::Info) {
            $crate::log::emit("I", owo_colors::AnsiColors::Blue, &format!($($arg)*));
        }
    }};
}

/// Log at WARN level (yellow  `W`).
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {{
        if $crate::log::enabled($crate::log::Level::Warn) {
            $crate::log::emit("W", owo_colors::AnsiColors::Yellow, &format!($($arg)*));
        }
    }};
}

/// Log at ERROR level (red  `E`).
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {{
        if $crate::log::enabled($crate::log::Level::Error) {
            $crate::log::emit("E", owo_colors::AnsiColors::Red, &format!($($arg)*));
        }
    }};
}

/// Log at DEBUG level (magenta  `D`).
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {{
        if $crate::log::enabled($crate::log::Level::Debug) {
            $crate::log::emit("D", owo_colors::AnsiColors::Magenta, &format!($($arg)*));
        }
    }};
}

// ═══════════════════════════════════════════════════════════════════════
// HTTP middleware
// ═══════════════════════════════════════════════════════════════════════

/// Axum middleware that logs every HTTP request / response in the style of
/// the main logger:
///
/// ```text
/// 2026-7-4 12:00:00 [200]   GET /yggdrasil/authserver/authenticate in 0.01s
/// ```
///
/// # Usage
///
/// ```ignore
/// use axum::middleware;
/// let app = Router::new()
///     .layer(middleware::from_fn(log::http_trace));
/// ```
pub async fn http_trace(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let start = std::time::Instant::now();

    let response = next.run(request).await;

    let latency = start.elapsed();
    let status = response.status();

    let color = match status.as_u16() {
        200..=299 => owo_colors::AnsiColors::Green,
        300..=399 => owo_colors::AnsiColors::Blue,
        400..=499 => owo_colors::AnsiColors::Red,
        _         => owo_colors::AnsiColors::Magenta,
    };

    let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let ts = ts.bright_black();

    let line = format!(
        "{} [{}] {:>5} {} {}\n",
        ts,
        status.as_str().color(color).bold(),
        method.to_string().bold(),
        uri.path(),
        format!("in {:.2}s", latency.as_secs_f32()).bright_black(),
    );

    // Stderr (always).
    eprint!("{}", line);

    // Log file (optional).
    if let Some(mutex) = LOG_FILE.get() {
        let mut file = mutex.lock().unwrap();
        let _ = write!(file, "{}", line);
    }

    response
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_ordering() {
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
        assert!(Level::Info < Level::Debug);
    }

    #[test]
    fn emit_does_not_panic() {
        emit("I", owo_colors::AnsiColors::Blue, "test message");
        emit("E", owo_colors::AnsiColors::Red, "multi\nline\nerror");
    }
}
