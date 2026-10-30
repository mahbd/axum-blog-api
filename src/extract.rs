use axum::Json;
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::{header, request::Parts};
use serde::de::DeserializeOwned;
use std::str::FromStr;
use validator::Validate;

use crate::auth;
use crate::config::config;
use crate::error::ApiError;

pub struct ValidJson<T>(pub T);

impl<T, S> FromRequest<S> for ValidJson<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state).await?;
        value.validate()?;
        Ok(ValidJson(value))
    }
}

pub struct AuthUser {
    pub id: i64,
    pub role: Role,
}

impl<S: Send + Sync> FromRequestParts<S> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthorized)?;
        let claims = auth::read_token(token, &config().jwt_secret)?;
        Ok(AuthUser {
            id: claims.sub,
            role: claims.role.parse()?,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    User,
    Admin,
}

impl FromStr for Role {
    type Err = ApiError;

    fn from_str(s: &str) -> Result<Role, ApiError> {
        match s {
            "user" => Ok(Role::User),
            "admin" => Ok(Role::Admin),
            _ => Err(ApiError::Unauthorized),
        }
    }
}

pub struct AdminUser(pub AuthUser);

impl<S: Send + Sync> FromRequestParts<S> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        if user.role != Role::Admin {
            return Err(ApiError::Forbidden);
        }
        Ok(AdminUser(user))
    }
}
