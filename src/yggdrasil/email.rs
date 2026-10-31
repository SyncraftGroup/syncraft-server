use std::path::Path;

use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header},
};
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::{log_debug, log_warn};

/// Built-in verification email body.
///
/// Embedded at compile time via `include_str!`, so the resulting binary never
/// needs the source file at runtime.
const DEFAULT_TEMPLATE: &str = include_str!("templates/verify_email.html");
/// Subject used when the template has no `<title>`.
const DEFAULT_SUBJECT: &str = "Verify your Syncraft account";
/// File name looked up next to `syncraft.toml`.
pub const TEMPLATE_FILE: &str = "verify-email.html";

/// SMTP mailer configuration for sending verification emails.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Mailer {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    pub from_name: String,
}

impl Mailer {
    /// Send an HTML email via SMTP.
    pub async fn send_email(
        &self,
        to_email: &str,
        subject: &str,
        body: String,
    ) -> Result<(), Error> {
        let from: Mailbox = format!("{} <{}>", self.from_name, self.username)
            .parse()
            .map_err(|e| Error::Server(format!("Invalid from address: {e}")))?;

        let to: Mailbox = to_email
            .parse()
            .map_err(|e| Error::Server(format!("Invalid to address: {e}")))?;

        let email = Message::builder()
            .from(from)
            .to(to)
            .subject(subject)
            .header(header::ContentType::TEXT_HTML)
            .body(body)
            .map_err(|e| Error::Server(format!("Failed to build email: {e}")))?;

        let creds = lettre::transport::smtp::authentication::Credentials::new(
            self.username.clone(),
            self.password.clone(),
        );

        // Port 465 speaks implicit TLS; every other commonly used submission
        // port (587, 25) starts in plaintext and upgrades via STARTTLS.
        let builder = if self.smtp_port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&self.smtp_host)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.smtp_host)
        }
        .map_err(|e| Error::Server(format!("Invalid SMTP host: {e}")))?;

        builder
            .port(self.smtp_port)
            .credentials(creds)
            .build()
            .send(email)
            .await
            .map_err(|e| Error::Server(format!("Failed to send email: {e}")))?;

        Ok(())
    }
}

/// Values substituted into a verification email template.
pub struct VerifyEmail<'a> {
    /// Verification URL.
    pub link: &'a str,
    /// Recipient email address.
    pub email: &'a str,
    /// In-game character name.
    pub character_name: &'a str,
    /// Display name of the server.
    pub server_name: &'a str,
}

/// A verification email template, either the built-in one or an HTML file
/// placed next to `syncraft.toml`.
#[derive(Debug, Clone)]
pub struct EmailTemplate {
    html: String,
    subject: String,
}

impl EmailTemplate {
    /// The built-in English template.
    pub fn builtin() -> Self {
        Self::from_html(DEFAULT_TEMPLATE)
    }

    /// Load a template from `path`, falling back to the built-in one when the
    /// file is missing, unreadable or empty.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(html) if !html.trim().is_empty() => {
                log_debug!("Email template loaded from {}", path.display());
                Self::from_html(&html)
            }
            Ok(_) => {
                log_warn!(
                    "Email template at {} is empty; using the built-in template",
                    path.display()
                );
                Self::builtin()
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::builtin(),
            Err(err) => {
                log_warn!(
                    "Failed to read email template at {}: {err}; using the built-in template",
                    path.display()
                );
                Self::builtin()
            }
        }
    }

    fn from_html(html: &str) -> Self {
        let subject = extract_title(html).unwrap_or_else(|| DEFAULT_SUBJECT.to_string());
        Self {
            html: html.to_string(),
            subject,
        }
    }

    /// Render `(subject, body)` with the given values.
    pub fn render(&self, values: &VerifyEmail<'_>) -> (String, String) {
        (
            substitute(&self.subject, values),
            substitute(&self.html, values),
        )
    }
}

/// Minimal HTML escaping for values interpolated into the mail body.
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn substitute(template: &str, values: &VerifyEmail<'_>) -> String {
    template
        .replace("{{link}}", &escape_html(values.link))
        .replace("{{email}}", &escape_html(values.email))
        .replace("{{character_name}}", &escape_html(values.character_name))
        .replace("{{server_name}}", &escape_html(values.server_name))
}

/// Read the first `<title>` element, which becomes the mail subject.
fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title>")? + "<title>".len();
    let end = lower[start..].find("</title>")? + start;
    let title = html[start..end].trim();
    (!title.is_empty()).then(|| title.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values<'a>() -> VerifyEmail<'a> {
        VerifyEmail {
            link: "https://mc.example.com/yggdrasil/verify?code=a&b",
            email: "player@example.com",
            character_name: "Steve",
            server_name: "My Server",
        }
    }

    #[test]
    fn builtin_template_is_embedded() {
        // Guards the `include_str!`: the default body must be part of the binary.
        assert!(DEFAULT_TEMPLATE.contains("<html"));
        assert!(DEFAULT_TEMPLATE.contains("{{link}}"));
    }

    #[test]
    fn builtin_template_contains_link_and_subject() {
        let template = EmailTemplate::builtin();
        let (subject, body) = template.render(&values());
        assert_eq!(subject, "Verify your Syncraft account");
        assert!(body.contains("https://mc.example.com/yggdrasil/verify?code=a&amp;b"));
        assert!(body.contains("Steve"));
    }

    #[test]
    fn title_becomes_subject() {
        let template =
            EmailTemplate::from_html("<html><title>Hello {{character_name}}</title></html>");
        let (subject, _) = template.render(&values());
        assert_eq!(subject, "Hello Steve");
    }

    #[test]
    fn values_are_html_escaped() {
        let template = EmailTemplate::from_html("<a href=\"{{link}}\">{{email}}</a>");
        let (_, body) = template.render(&VerifyEmail {
            link: "https://x/?a=1&b=2",
            email: "<hi@example.com>",
            character_name: "A",
            server_name: "B",
        });
        assert!(body.contains("a=1&amp;b=2"));
        assert!(body.contains("&lt;hi@example.com&gt;"));
    }

    #[test]
    fn missing_file_uses_builtin() {
        let template = EmailTemplate::load(Path::new("/definitely/not/here/verify-email.html"));
        let (subject, _) = template.render(&values());
        assert_eq!(subject, DEFAULT_SUBJECT);
    }
}
