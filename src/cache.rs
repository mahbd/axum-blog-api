use redis::{AsyncCommands, aio::ConnectionManager};

use crate::models::Post;

pub async fn get(redis: &ConnectionManager, id: i64) -> Option<Post> {
    let mut conn = redis.clone();
    match conn.get::<_, Option<String>>(format!("post:{id}")).await {
        Ok(Some(json)) => serde_json::from_str(&json).ok(),
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(?err, "redis get failed");
            None
        }
    }
}

pub async fn put(redis: &ConnectionManager, post: &Post) {
    let mut conn = redis.clone();
    let json = serde_json::to_string(post).unwrap();
    if let Err(err) = conn
        .set_ex::<_, _, ()>(format!("post:{}", post.id), json, 60)
        .await
    {
        tracing::warn!(?err, "redis set failed");
    }
}

pub async fn del(redis: &ConnectionManager, id: i64) {
    let mut conn = redis.clone();
    if let Err(err) = conn.del::<_, ()>(format!("post:{id}")).await {
        tracing::warn!(?err, "redis del failed");
    }
}
