use crate::mail::Mailer;
use tokio::sync::mpsc;
use crate::auth;
use crate::repo::Repo;
use tokio::time::{interval, Duration, MissedTickBehavior};

pub enum Job {
    Welcome { email: String },
}

pub async fn worker(mut rx: mpsc::Receiver<Job>, mailer: Mailer) {
    while let Some(job) = rx.recv().await {
        match job {
            Job::Welcome { email } => match mailer.welcome(&email).await {
                Ok(()) => tracing::info!(%email, "welcome mail sent"),
                Err(err) => tracing::warn!(?err, %email, "welcome mail failed"),
            },
        }
    }
}

pub async fn cleanup<R: Repo>(repo: R, every: Duration) {
    let mut ticker = interval(every);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match repo.delete_expired_refresh(auth::now()).await {
            Ok(n) => tracing::info!(removed = n, "cleanup"),
            Err(err) => tracing::warn!(?err, "cleanup failed"),
        }
    }
}
