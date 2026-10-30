use axum::extract::{Extension, Multipart, Path, Query, State, rejection::QueryRejection};
use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::auth;
use crate::cache;
use crate::config::config;
use crate::error::{ApiError, ErrorBody};
use crate::extract::{AdminUser, AuthUser, Role, ValidJson};
use crate::jobs::Job;
use crate::models::{
    Comment, Hit, Image, ListFilter, ListParams, NewComment, NewPost, NewUser, Page, Post, PostV2,
    PostWithTags, SearchParams, User,
};
use crate::repo::Repo;
use crate::storage::Storage;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use metrics_exporter_prometheus::PrometheusHandle;
use redis::aio::ConnectionManager;
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::io;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt, wrappers::BroadcastStream};
use uuid::Uuid;

#[utoipa::path(
    post,
    path = "/posts",
    request_body = NewPost,
    responses(
        (status = 201, description = "Created", body = PostWithTags),
        (status = 401, description = "Missing or bad token", body = ErrorBody),
        (status = 422, description = "Validation failed", body = ErrorBody),
    ),
    security(("bearer" = [])),
    tag = "posts"
)]
#[tracing::instrument(skip_all)]
pub async fn create_post<R: Repo>(
    State(repo): State<R>,
    user: AuthUser,
    ValidJson(new): ValidJson<NewPost>,
) -> Result<(StatusCode, Json<PostWithTags>), ApiError> {
    let post = repo
        .create(user.id, new)
        .await
        .map_err(|err| match err.as_database_error() {
            Some(e) if e.is_unique_violation() => {
                ApiError::Conflict("a post with this title already exists".into())
            }
            _ => ApiError::Database(err),
        })?;
    tracing::debug!(id = post.post.id, "created");
    metrics::counter!("posts_created_total").increment(1);
    Ok((StatusCode::CREATED, Json(post)))
}

pub async fn get_post<R: Repo>(
    State(repo): State<R>,
    Extension(redis): Extension<ConnectionManager>,
    Path(id): Path<i64>,
) -> Result<Json<Post>, ApiError> {
    if let Some(post) = cache::get(&redis, id).await {
        tracing::debug!(id, "cache hit");
        return Ok(Json(post));
    }
    tracing::debug!(id, "cache miss");
    let post = repo.get(id).await?.ok_or(ApiError::NotFound(id))?;
    cache::put(&redis, &post).await;
    Ok(Json(post))
}

pub async fn list_posts<R: Repo>(
    State(repo): State<R>,
    params: Result<Query<ListParams>, QueryRejection>,
) -> Result<Json<Page<Post>>, ApiError> {
    let Query(p) = params?;
    let page = p.page.unwrap_or(1).max(1);
    let per_page = p.per_page.unwrap_or(10).clamp(1, 100);
    let (items, total) = repo
        .list(ListFilter {
            page,
            per_page,
            tag: p.tag,
            sort: p.sort,
        })
        .await?;
    Ok(Json(Page {
        items,
        page,
        per_page,
        total,
    }))
}

pub async fn update_post<R: Repo>(
    State(repo): State<R>,
    Extension(redis): Extension<ConnectionManager>,
    Path(id): Path<i64>,
    user: AuthUser,
    ValidJson(new): ValidJson<NewPost>,
) -> Result<Json<Post>, ApiError> {
    let post = repo.get(id).await?.ok_or(ApiError::NotFound(id))?;
    if !can_edit(&user, &post) {
        return Err(ApiError::Forbidden);
    }
    let post = repo.update(id, new).await?.ok_or(ApiError::NotFound(id))?;
    cache::del(&redis, id).await;
    Ok(Json(post))
}

pub async fn delete_post<R: Repo>(
    State(repo): State<R>,
    Extension(redis): Extension<ConnectionManager>,
    Path(id): Path<i64>,
    user: AuthUser,
) -> Result<StatusCode, ApiError> {
    let post = repo.get(id).await?.ok_or(ApiError::NotFound(id))?;
    if !can_edit(&user, &post) {
        return Err(ApiError::Forbidden);
    }
    repo.delete(id).await?;
    cache::del(&redis, id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_comments<R: Repo>(
    State(repo): State<R>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Comment>>, ApiError> {
    repo.get(id).await?.ok_or(ApiError::NotFound(id))?;
    Ok(Json(repo.list_comments(id).await?))
}

pub async fn add_comment<R: Repo>(
    State(repo): State<R>,
    Path(id): Path<i64>,
    Extension(live): Extension<broadcast::Sender<Comment>>,
    _user: AuthUser,
    ValidJson(new): ValidJson<NewComment>,
) -> Result<(StatusCode, Json<Comment>), ApiError> {
    let comment = repo
        .add_comment(id, new)
        .await
        .map_err(|err| match err.as_database_error() {
            Some(db_err) if db_err.is_foreign_key_violation() => ApiError::NotFound(id),
            _ => ApiError::Database(err),
        })?;
    let _ = live.send(comment.clone());
    Ok((StatusCode::CREATED, Json(comment)))
}

pub async fn search_posts<R: Repo>(
    State(repo): State<R>,
    Query(p): Query<SearchParams>,
) -> Result<Json<Vec<Hit>>, ApiError> {
    Ok(Json(repo.search(&p.q).await?))
}

pub async fn restore_post<R: Repo>(
    State(repo): State<R>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if !repo.restore(id).await? {
        return Err(ApiError::NotFound(id));
    }
    Ok(StatusCode::OK)
}

pub async fn signup<R: Repo>(
    State(repo): State<R>,
    Extension(jobs): Extension<mpsc::Sender<Job>>,
    ValidJson(new): ValidJson<NewUser>,
) -> Result<(StatusCode, Json<User>), ApiError> {
    let hash = crate::auth::hash_password(new.password).await?;
    let user =
        repo.create_user(&new.email, &hash)
            .await
            .map_err(|err| match err.as_database_error() {
                Some(e) if e.is_unique_violation() => {
                    ApiError::Conflict("email already registered".into())
                }
                _ => ApiError::Database(err),
            })?;
    if jobs
        .send(Job::Welcome {
            email: user.email.clone(),
        })
        .await
        .is_err()
    {
        tracing::error!("job queue closed");
    }
    Ok((StatusCode::CREATED, Json(user)))
}

#[derive(Deserialize)]
pub struct Login {
    pub email: String,
    pub password: String,
}

fn refresh_cookie(token: String) -> Cookie<'static> {
    Cookie::build(("refresh", token))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .path("/auth")
        .max_age(time::Duration::days(30))
        .build()
}

pub async fn login<R: Repo>(
    State(repo): State<R>,
    jar: CookieJar,
    Json(body): Json<Login>,
) -> Result<(CookieJar, Json<Value>), ApiError> {
    let user = repo
        .find_user_by_email(&body.email)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    if !auth::verify_password(body.password, user.password_hash).await? {
        return Err(ApiError::Unauthorized);
    }
    let access = auth::make_token(user.id, &user.role, &config().jwt_secret)?;
    let refresh = auth::new_refresh_token();
    repo.store_refresh(
        &auth::hash_token(&refresh),
        user.id,
        auth::now() + auth::REFRESH_SECS,
    )
    .await?;
    Ok((
        jar.add(refresh_cookie(refresh)),
        Json(json!({ "token": access })),
    ))
}

pub async fn me<R: Repo>(State(repo): State<R>, user: AuthUser) -> Result<Json<User>, ApiError> {
    let user = repo
        .get_user(user.id)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    Ok(Json(user))
}

pub async fn refresh<R: Repo>(
    State(repo): State<R>,
    jar: CookieJar,
) -> Result<(CookieJar, Json<Value>), ApiError> {
    let old = jar
        .get("refresh")
        .ok_or(ApiError::Unauthorized)?
        .value()
        .to_string();
    let user_id = repo
        .take_refresh(&auth::hash_token(&old), auth::now())
        .await?
        .ok_or(ApiError::Unauthorized)?;
    let user = repo
        .get_user(user_id)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    let access = auth::make_token(user.id, &user.role, &config().jwt_secret)?;
    let new = auth::new_refresh_token();
    repo.store_refresh(
        &auth::hash_token(&new),
        user.id,
        auth::now() + auth::REFRESH_SECS,
    )
    .await?;
    Ok((
        jar.add(refresh_cookie(new)),
        Json(json!({ "token": access })),
    ))
}

pub async fn logout<R: Repo>(
    State(repo): State<R>,
    jar: CookieJar,
) -> Result<(CookieJar, StatusCode), ApiError> {
    if let Some(cookie) = jar.get("refresh") {
        repo.delete_refresh(&auth::hash_token(cookie.value()))
            .await?;
    }
    Ok((
        jar.remove(Cookie::build("refresh").path("/auth")),
        StatusCode::NO_CONTENT,
    ))
}

pub fn can_edit(user: &AuthUser, post: &Post) -> bool {
    user.role == Role::Admin || post.author_id == Some(user.id)
}

pub async fn list_users<R: Repo>(
    State(repo): State<R>,
    _admin: AdminUser,
) -> Result<Json<Vec<User>>, ApiError> {
    Ok(Json(repo.list_users().await?))
}

pub async fn upload_image<R: Repo, S: Storage>(
    State(repo): State<R>,
    user: AuthUser,
    Path(id): Path<i64>,
    Extension(store): Extension<Arc<S>>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<Image>), ApiError> {
    let post = repo.get(id).await?.ok_or(ApiError::NotFound(id))?;
    if !can_edit(&user, &post) {
        return Err(ApiError::Forbidden);
    }
    while let Some(field) = multipart.next_field().await? {
        if field.name() != Some("file") {
            continue;
        }
        let bytes = field.bytes().await?;
        let ext = image_kind(&bytes).ok_or(ApiError::UnsupportedMedia)?;
        let name = format!("{}.{ext}", Uuid::new_v4());
        store.put(&name, &bytes).await?;
        let image = repo.add_image(id, &format!("/files/{name}")).await?;
        return Ok((StatusCode::CREATED, Json(image)));
    }
    Err(ApiError::MissingFile)
}

fn image_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else {
        None
    }
}

pub async fn get_file<S: Storage>(
    Extension(store): Extension<Arc<S>>,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    let bytes = store.get(&name).await.map_err(|err| match err.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput => ApiError::FileNotFound,
        _ => ApiError::Io(err),
    })?;
    let content_type = if name.ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    };
    Ok(([(header::CONTENT_TYPE, content_type)], bytes).into_response())
}

pub async fn live(
    Extension(live): Extension<broadcast::Sender<Comment>>,
    Path(post_id): Path<i64>,
    ws: WebSocketUpgrade,
) -> Response {
    let rx = live.subscribe();
    ws.on_upgrade(move |socket| live_socket(socket, rx, post_id))
}

async fn live_socket(mut socket: WebSocket, mut rx: broadcast::Receiver<Comment>, post_id: i64) {
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok(c) => {
                    if c.post_id != post_id {
                        continue;
                    }
                    let text = serde_json::to_string(&c).unwrap();
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    let _ = socket.send(Message::Text(format!("{{\"missed\":{n}}}").into())).await;
                }
                Err(RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                None | Some(Ok(Message::Close(_))) | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    tracing::info!(post_id, "live socket closed");
}

pub async fn events(
    Extension(live): Extension<broadcast::Sender<Comment>>,
    Path(post_id): Path<i64>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(live.subscribe()).filter_map(move |msg| match msg {
        Ok(c) if c.post_id == post_id => {
            Some(Ok(Event::default().event("comment").json_data(&c).unwrap()))
        }
        _ => None,
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[utoipa::path(
    get,
    path = "/api/v2/posts/{id}",
    params(("id" = i64, Path, description = "Post id")),
    responses(
        (status = 200, description = "The post", body = PostV2),
        (status = 404, description = "No such post", body = ErrorBody),
    ),
    tag = "posts"
)]
pub async fn get_post_v2<R: Repo>(
    State(repo): State<R>,
    Path(id): Path<i64>,
) -> Result<Json<PostV2>, ApiError> {
    let post = repo.get(id).await?.ok_or(ApiError::NotFound(id))?;
    Ok(Json(post.into()))
}

#[derive(Deserialize)]
pub struct SlowParams {
    secs: Option<u64>,
}

pub async fn slow(Query(p): Query<SlowParams>) -> &'static str {
    tokio::time::sleep(std::time::Duration::from_secs(p.secs.unwrap_or(30))).await;
    "finally done"
}

pub async fn health_live() -> &'static str {
    "ok"
}

pub async fn health_ready<R: Repo>(
    State(repo): State<R>,
    Extension(redis): Extension<ConnectionManager>,
) -> (StatusCode, Json<Value>) {
    let db = repo.ping().await.is_ok();
    let redis_ok = redis::cmd("PING")
        .query_async::<String>(&mut redis.clone())
        .await
        .is_ok();
    let status = if db && redis_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(
            json!({ "database": if db { "up" } else { "down" }, "redis": if redis_ok { "up" } else { "down" } }),
        ),
    )
}

pub async fn metrics_text(Extension(handle): Extension<PrometheusHandle>) -> String {
    handle.render()
}
