use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message as Mail, Tokio1Executor};

use super::{Channel, SendError, Sent, http::TIMEOUT};
use crate::config::settings::{EmailSettings, SmtpSecurity};
use crate::server::notify::Letter;

pub(super) struct Email {
    mail: EmailSettings,
    smtp: AsyncSmtpTransport<Tokio1Executor>,
}

impl Email {
    pub(super) fn new(m: &EmailSettings) -> Result<Email, SendError> {
        let tls =
            || TlsParameters::new(m.host.clone()).map_err(|e| SendError::Permanent(e.to_string()));
        let security = match m.security {
            SmtpSecurity::Tls => Tls::Wrapper(tls()?),
            SmtpSecurity::StartTls => Tls::Required(tls()?),
            SmtpSecurity::StartTlsIfOffered => Tls::Opportunistic(tls()?),
        };
        let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(m.host.as_str())
            .port(m.port)
            .tls(security)
            .timeout(Some(TIMEOUT));
        if let (Some(user), Some(password)) = (&m.user, &m.password) {
            b = b.credentials(Credentials::new(
                user.clone(),
                password.expose().to_string(),
            ));
        }
        Ok(Email {
            mail: m.clone(),
            smtp: b.build(),
        })
    }
}

fn mail(m: &EmailSettings, letter: &Letter) -> Result<Mail, SendError> {
    let address = |a: &str| {
        a.parse::<lettre::message::Mailbox>()
            .map_err(|e| SendError::Permanent(format!("{a}: {e}")))
    };
    Mail::builder()
        .from(address(&m.from)?)
        .to(address(&m.to)?)
        .subject(letter.title())
        .header(ContentType::TEXT_PLAIN)
        .body(letter.message.text.clone())
        .map_err(|e| SendError::Permanent(e.to_string()))
}

pub(super) fn verdict(e: &lettre::transport::smtp::Error) -> SendError {
    match e.is_permanent() || e.is_client() {
        true => SendError::Permanent(e.to_string()),
        false => SendError::Retry(e.to_string()),
    }
}

impl Channel for Email {
    fn kind(&self) -> &'static str {
        "email"
    }

    fn send<'a>(&'a self, letter: &'a Letter) -> Sent<'a> {
        Box::pin(async move {
            let m = mail(&self.mail, letter)?;
            self.smtp.send(m).await.map(|_| ()).map_err(|e| verdict(&e))
        })
    }
}

#[cfg(test)]
mod tests;
