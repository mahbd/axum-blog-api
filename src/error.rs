use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::multipart::MultipartError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;
use std::collections::BTreeMap;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("post {0} not found")]
    NotFound(i64),
    #[error(transparent)]
    BadJson(#[from] JsonRejection),
    #[error(transparent)]
    BadQuery(#[from] QueryRejection),
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("{0}")]
    Conflict(String),
    #[error("validation failed")]
    Validation(#[from] validator::ValidationErrors),
    #[error("internal error")]
    Hash(#[from] argon2::password_hash::Error),
    #[error("internal error")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("invalid email or password")]
    Unauthorized,
    #[error("you are not allowed to do this")]
    Forbidden,
    #[error("file not found")]
    FileNotFound,
    #[error("{0}")]
    Multipart(#[from] MultipartError),
    #[error("send a file in the form field file")]
    MissingFile,
    #[error("internal error")]
    Io(#[from] std::io::Error),
    #[error("only PNG and JPEG images")]
    UnsupportedMedia,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::warn!(error = %self, "request failed");
        let status = match &self {
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden => StatusCode::FORBIDDEN,
            ApiError::FileNotFound => StatusCode::NOT_FOUND,
            ApiError::UnsupportedMedia => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ApiError::MissingFile => StatusCode::BAD_REQUEST,
            ApiError::Multipart(err) => err.status(),
            ApiError::BadJson(rejection) => rejection.status(),
            ApiError::BadQuery(rejection) => rejection.status(),
            ApiError::Validation(errors) => {
                let fields: BTreeMap<_, Vec<String>> = errors
                    .field_errors()
                    .iter()
                    .map(|(field, errs)| {
                        let msgs = errs.iter().map(|e| e.message.clone().unwrap_or(e.code.clone()).to_string()).collect();
                        (field.to_string(), msgs)
                    })
                    .collect();
                return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({ "error": "validation failed", "fields": fields })))
                    .into_response();
            }
            ApiError::Hash(err) => {
                tracing::error!(?err, "password hashing failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }
            ApiError::Jwt(err) => {
                tracing::error!(?err, "token error");
                StatusCode::INTERNAL_SERVER_ERROR
            }
            ApiError::Database(err) => {
                tracing::error!(?err, "database error");
                StatusCode::INTERNAL_SERVER_ERROR
            }
            ApiError::Io(err) => {
                tracing::error!(?err, "io error");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let message = match &self {
            ApiError::BadJson(rejection) => rejection.body_text(),
            ApiError::BadQuery(rejection) => rejection.body_text(),
            ApiError::Multipart(err) => err.body_text(),
            other => other.to_string(),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: String,
}