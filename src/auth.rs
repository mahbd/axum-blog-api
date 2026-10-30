use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use argon2::{password_hash::PasswordHasher, Argon2};
use argon2::password_hash::{phc::PasswordHash, PasswordVerifier};
use sha2::{Digest, Sha256};

use crate::error::ApiError;

pub async fn hash_password(password: String) -> Result<String, ApiError> {
    let hash = tokio::task::spawn_blocking(move || {
        Argon2::default().hash_password(password.as_bytes()).map(|h| h.to_string())
    })
    .await
    .expect("hash task panicked")?;
    Ok(hash)
}

#[derive(Serialize, Deserialize)]
pub struct Claims {
    pub sub: i64,
    pub role: String,
    pub exp: u64,
}

pub fn make_token(user_id: i64, role: &str, secret: &str) -> Result<String, ApiError> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let claims = Claims { sub: user_id, role: role.to_string(), exp: now + 900 };
    Ok(encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))?)
}

pub async fn verify_password(password: String, hash: String) -> Result<bool, ApiError> {
    let ok = tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&hash)?;
        Ok::<_, argon2::password_hash::Error>(Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
    })
    .await
    .expect("verify task panicked")?;
    Ok(ok)
}

pub fn read_token(token: &str, secret: &str) -> Result<Claims, ApiError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    let data = decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation)
        .map_err(|err| {
            tracing::info!(?err, "bad token");
            ApiError::Unauthorized
        })?;
    Ok(data.claims)
}

pub const REFRESH_SECS: i64 = 30 * 24 * 3600;

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

pub fn new_refresh_token() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    hex::encode(bytes)
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}
