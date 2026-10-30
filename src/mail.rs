use lettre::message::header::ContentType;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::config::Config;

#[derive(Clone)]
pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: String,
}

impl Mailer {
    pub fn new(cfg: &Config) -> Mailer {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.smtp_host)
            .port(cfg.smtp_port)
            .build();
        Mailer { transport, from: cfg.mail_from.clone() }
    }

    pub async fn welcome(&self, to: &str) -> anyhow::Result<()> {
        let message = Message::builder()
            .from(self.from.parse()?)
            .to(to.parse()?)
            .subject("Welcome to the LCE blog")
            .header(ContentType::TEXT_PLAIN)
            .body(format!("Hi {to},\n\nthanks for signing up. Happy writing!\n"))?;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await; // pretend the provider is slow
        self.transport.send(message).await?;
        Ok(())
    }
}
