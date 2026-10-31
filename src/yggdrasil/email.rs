use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header},
};
use serde::{Deserialize, Serialize};

use crate::error::Error;

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
    /// Send a verification email via SMTP.
    pub async fn send_email(&self, to_email: &str, msg: String) -> Result<(), Error> {
        let from: Mailbox = format!("{} <{}>", self.from_name, self.username)
            .parse()
            .map_err(|e| Error::Server(format!("Invalid from address: {e}")))?;

        let to: Mailbox = to_email
            .parse()
            .map_err(|e| Error::Server(format!("Invalid to address: {e}")))?;

        let email = Message::builder()
            .from(from)
            .to(to)
            .subject("Verify your Syncraft account")
            .header(header::ContentType::TEXT_HTML)
            .body(msg)
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

/// Minimal HTML escaping for values interpolated into the mail body.
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Build a simple HTML verification email body containing the given link.
pub fn verify_email_string(verify_link: String) -> String {
    let verify_link = escape_html(&verify_link);
    format!(
        r#"<h1>Syncraft Verify</h1>
<p>If you don't know what this is, just ignore it.</p>
<p>Please click the url below:</p>
<a href="{verify_link}">{verify_link}</a>"#
    )
}
