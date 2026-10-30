#[cfg(test)]
mod api_tests;
mod auth;
mod cache;
mod config;
mod error;
mod extract;
mod handlers;
mod jobs;
mod mail;
mod models;
mod repo;
mod storage;

use axum::extract::DefaultBodyLimit;
use axum::extract::{MatchedPath, Request};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use jobs::Job;
use mail::Mailer;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use models::Comment;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use repo::{Repo, SqliteRepo};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use storage::{LocalStorage, MemStorage, Storage};
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder};
use tower_http::cors::CorsLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_swagger_ui::SwaggerUi;

pub fn app<R: Repo, S: Storage>(
    repo: R,
    store: S,
    jobs: mpsc::Sender<Job>,
    cache: ConnectionManager,
    live: broadcast::Sender<Comment>,
    handle: PrometheusHandle,
) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(HeaderValue::from_static("http://localhost:5500"))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .allow_credentials(true);
    let governor = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(2)
            .burst_size(5)
            .finish()
            .unwrap(),
    );
    let auth_routes = Router::new()
        .route("/signup", post(handlers::signup::<R>))
        .route("/login", post(handlers::login::<R>))
        .route("/auth/refresh", post(handlers::refresh::<R>))
        .layer(GovernorLayer::new(governor));
    let streaming = Router::new()
        .route("/posts/{id}/live", get(handlers::live))
        .route("/posts/{id}/events", get(handlers::events));
    Router::new()
        .nest("/api/v1", posts_v1())
        .nest("/api/v2", posts_v2())
        .merge(posts_v1())
        .route("/posts/{id}/restore", post(handlers::restore_post::<R>))
        .route("/me", get(handlers::me::<R>))
        .route("/admin/users", get(handlers::list_users::<R>))
        .route("/auth/logout", post(handlers::logout::<R>))
        .route(
            "/posts/{id}/images",
            post(handlers::upload_image::<R, S>).layer(DefaultBodyLimit::max(10 * 1024 * 1024)),
        )
        .route("/files/{name}", get(handlers::get_file::<S>))
        .route("/debug/slow", get(handlers::slow))
        .route("/health/live", get(handlers::health_live))
        .route("/health/ready", get(handlers::health_ready::<R>))
        .route("/metrics", get(handlers::metrics_text))
        .merge(auth_routes)
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ))
        .merge(streaming)
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .route_layer(middleware::from_fn(track))
        .with_state(repo)
        .layer(Extension(Arc::new(store)))
        .layer(Extension(jobs))
        .layer(Extension(cache))
        .layer(Extension(live))
        .layer(Extension(handle))
        .layer(cors)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(TraceLayer::new_for_http())
}

#[tokio::main]
async fn main() {
    let cfg = config::config();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let options = SqliteConnectOptions::from_str(&cfg.database_url)
        .unwrap()
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let repo = SqliteRepo { pool };
    let (tx, rx) = mpsc::channel::<Job>(100);
    let mailer = Mailer::new(cfg);
    tokio::spawn(jobs::worker(rx, mailer));
    tokio::spawn(jobs::cleanup(
        repo.clone(),
        Duration::from_secs(cfg.cleanup_secs),
    ));
    let client = redis::Client::open(cfg.redis_url.as_str()).unwrap();
    let redis_config = ConnectionManagerConfig::new()
        .set_number_of_retries(1)
        .set_connection_timeout(Some(Duration::from_millis(300)))
        .set_response_timeout(Some(Duration::from_millis(300)));
    let cache = ConnectionManager::new_with_config(client, redis_config)
        .await
        .unwrap();
    let (live, _) = broadcast::channel::<Comment>(100);
    let handle = PrometheusBuilder::new()
        .set_buckets(&[0.005, 0.01, 0.05, 0.1, 0.5, 1.0])
        .unwrap()
        .install_recorder()
        .unwrap();
    match cfg.storage.as_str() {
        "memory" => {
            serve(
                app(repo, MemStorage::default(), tx, cache, live, handle),
                &addr,
            )
            .await
        }
        _ => {
            serve(
                app(repo, LocalStorage::new("uploads"), tx, cache, live, handle),
                &addr,
            )
            .await
        }
    }
}

async fn serve(app: Router, addr: &str) {
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    tracing::info!(%addr, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .unwrap();
}

fn posts_common<R: Repo>() -> Router<R> {
    Router::new()
        .route(
            "/posts",
            get(handlers::list_posts::<R>).post(handlers::create_post::<R>),
        )
        .route(
            "/posts/{id}",
            put(handlers::update_post::<R>).delete(handlers::delete_post::<R>),
        )
        .route(
            "/posts/{id}/comments",
            get(handlers::list_comments::<R>).post(handlers::add_comment::<R>),
        )
        .route("/search", get(handlers::search_posts::<R>))
}

fn posts_v1<R: Repo>() -> Router<R> {
    posts_common()
        .route("/posts/{id}", get(handlers::get_post::<R>))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("deprecation"),
            HeaderValue::from_static("true"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("sunset"),
            HeaderValue::from_static("Wed, 30 Jun 2027 00:00:00 GMT"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::LINK,
            HeaderValue::from_static(r#"</api/v2>; rel="successor-version""#),
        ))
}

fn posts_v2<R: Repo>() -> Router<R> {
    posts_common().route("/posts/{id}", get(handlers::get_post_v2::<R>))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("Ctrl+C handler");
    };
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}

async fn track(req: Request, next: Next) -> Response {
    let start = std::time::Instant::now();
    let method = req.method().to_string();
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());
    let response = next.run(req).await;
    let status = response.status().as_u16().to_string();
    let labels = [("method", method), ("path", path), ("status", status)];
    metrics::counter!("http_requests_total", &labels).increment(1);
    metrics::histogram!("http_request_duration_seconds", &labels)
        .record(start.elapsed().as_secs_f64());
    response
}

#[derive(OpenApi)]
#[openapi(
    paths(handlers::get_post_v2, handlers::create_post),
    components(schemas(models::PostV2, models::NewPost, models::PostWithTags, models::Post, error::ErrorBody)),
    modifiers(&SecurityAddon)
)]
struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        );
    }
}
