use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tower::ServiceExt;

use super::*;

fn test_app(pool: SqlitePool) -> Router {
    let (jobs, _rx) = mpsc::channel(10);
    let (live, _) = broadcast::channel(10);
    let client = redis::Client::open("redis://127.0.0.1:1").unwrap();
    let config = ConnectionManagerConfig::new().set_number_of_retries(0).set_connection_timeout(Some(Duration::from_millis(100)));
    let cache = ConnectionManager::new_lazy_with_config(client, config).unwrap();
    let handle = PrometheusBuilder::new().build_recorder().handle();
    app(SqliteRepo { pool }, MemStorage::default(), jobs, cache, live, handle)
}

async fn call(app: &Router, method: Method, uri: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let mut req = req.body(Body::from(body.to_string())).unwrap();
    req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4000))));
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn login(app: &Router, email: &str) -> String {
    let creds = json!({ "email": email, "password": "hunter2hunter2" });
    call(app, Method::POST, "/signup", None, creds.clone()).await;
    let (status, body) = call(app, Method::POST, "/login", None, creds).await;
    assert_eq!(status, StatusCode::OK);
    body["token"].as_str().unwrap().to_string()
}

#[sqlx::test]
async fn create_and_get(pool: SqlitePool) {
    let app = test_app(pool);
    let token = login(&app, "ana@example.com").await;
    let new = json!({ "title": "Hello", "body": "First post" });
    let (status, created) = call(&app, Method::POST, "/posts", Some(&token), new).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, post) = call(&app, Method::GET, &format!("/posts/{}", created["id"]), None, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(post["title"], "Hello");
}

#[sqlx::test]
async fn ben_cannot_delete_ana_post(pool: SqlitePool) {
    let app = test_app(pool);
    let ana = login(&app, "ana@example.com").await;
    let ben = login(&app, "ben@example.com").await;
    let new = json!({ "title": "Mine", "body": "Hands off" });
    let (_, post) = call(&app, Method::POST, "/posts", Some(&ana), new).await;
    let uri = format!("/posts/{}", post["id"]);
    let (status, _) = call(&app, Method::DELETE, &uri, Some(&ben), Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, Method::DELETE, &uri, Some(&ana), Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[sqlx::test]
async fn database_down_gives_500(pool: SqlitePool) {
    let app = test_app(pool.clone());
    pool.close().await;
    let (status, body) = call(&app, Method::GET, "/posts/1", None, Value::Null).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "database error");
}
