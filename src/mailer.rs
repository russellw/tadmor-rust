//! Outbound email over SMTP, through lettre. Without `SMTP_ADDR` configured
//! there is no mailer, and the email endpoints answer 501 (spec/api.md
//! §5.11). Like tadmor's, the connection upgrades to TLS with STARTTLS
//! when the server offers it, and authenticates when a username is set.

use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::error::{Error, Result};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SmtpConfig {
    /// `host:port`; empty disables email.
    pub addr: String,
    pub username: String,
    pub password: String,
    pub from: String,
}

pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer {
    /// The mailer the configuration describes, or None when email is off.
    pub fn new(config: &SmtpConfig) -> std::result::Result<Option<Mailer>, String> {
        if config.addr.is_empty() {
            return Ok(None);
        }
        let (host, port) = config.addr.rsplit_once(':').ok_or("SMTP_ADDR must be host:port")?;
        let port: u16 = port.parse().map_err(|_| "SMTP_ADDR has an invalid port")?;
        let tls = TlsParameters::new(host.to_string()).map_err(|e| format!("SMTP TLS: {e}"))?;
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host).port(port).tls(Tls::Opportunistic(tls));
        if !config.username.is_empty() {
            builder = builder.credentials(Credentials::new(config.username.clone(), config.password.clone()));
        }
        let from = config.from.parse().map_err(|e| format!("MAIL_FROM: {e}"))?;
        Ok(Some(Mailer { transport: builder.build(), from }))
    }

    /// Sends a short note with a PDF attached. An unusable recipient
    /// address is a 422.
    pub async fn send_pdf(&self, to: &[String], subject: &str, body: &str, filename: &str, pdf: Vec<u8>) -> Result<()> {
        let mut message = Message::builder().from(self.from.clone()).subject(subject);
        for address in to {
            let mailbox: Mailbox = address.parse().map_err(|_| Error::unprocessable(format!("{address:?} is not an email address")))?;
            message = message.to(mailbox);
        }
        let attachment = Attachment::new(filename.to_string()).body(pdf, ContentType::parse("application/pdf").expect("a valid type"));
        let message = message
            .multipart(MultiPart::mixed().singlepart(SinglePart::plain(body.to_string())).singlepart(attachment))
            .map_err(Error::internal)?;
        self.transport.send(message).await.map_err(Error::internal)?;
        Ok(())
    }
}
