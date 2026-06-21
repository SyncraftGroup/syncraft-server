use lettre::{AsyncTransport, Message, message::{Mailbox, header}};
use serde::Deserialize;

use crate::error::Error;

#[derive(Debug, Deserialize, Clone)]
pub struct Mailer {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    pub from_name: String,
}

impl Mailer {
    pub async fn send_email(&self, to_email: &str, msg: String) -> Result<(), Error> {
        let from: Mailbox = format!("{} <{}>", self.from_name, self.username)
            .parse()
            .map_err(|e| Error::UI(format!("Invalid from address: {e}")))?;

        let to: Mailbox = to_email
            .parse()
            .map_err(|e| Error::UI(format!("Invalid to address: {e}")))?;

        let email = Message::builder()
            .from(from)
            .to(to)
            .subject("Verify your Syncraft account")
            .header(header::ContentType::TEXT_HTML)
            .body(msg)
            .map_err(|e| Error::UI(format!("Failed to build email: {e}")))?;

        let creds = lettre::transport::smtp::authentication::Credentials::new(
            self.username.clone(),
            self.password.clone(),
        );

        let mailer = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(&self.smtp_host)
            .map_err(|e| Error::UI(format!("Invalid SMTP host: {e}")))?;

        mailer
            .port(self.smtp_port)
            .credentials(creds)
            .build()
            .send(email)
            .await
            .map_err(|e| Error::UI(format!("Failed to send email: {e}")))?;

        Ok(())
    }
}

pub fn verify_email_string(verify_link: String) -> String {
    format!(
r#"<h1>Syncraft Verify</h1>
<p>If you don't know what this is, just ignore it.</p>
<p>Please click the url below:</p>
<a href="{verify_link}">{verify_link}<\a>"#
    )
}