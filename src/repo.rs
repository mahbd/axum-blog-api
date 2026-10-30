use std::future::Future;

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::models::{slugify, Comment, Hit, Image, ListFilter, NewComment, NewPost, Post, PostWithTags, Sort, User, UserWithHash};

pub trait Repo: Clone + Send + Sync + 'static {
    fn list(&self, f: ListFilter) -> impl Future<Output = sqlx::Result<(Vec<Post>, i64)>> + Send;
    fn get(&self, id: i64) -> impl Future<Output = sqlx::Result<Option<Post>>> + Send;
    fn create(&self, author_id: i64, new: NewPost) -> impl Future<Output = sqlx::Result<PostWithTags>> + Send;
    fn update(&self, id: i64, new: NewPost) -> impl Future<Output = sqlx::Result<Option<Post>>> + Send;
    fn delete(&self, id: i64) -> impl Future<Output = sqlx::Result<bool>> + Send;
    fn restore(&self, id: i64) -> impl Future<Output = sqlx::Result<bool>> + Send;
    fn list_comments(&self, post_id: i64) -> impl Future<Output = sqlx::Result<Vec<Comment>>> + Send;
    fn add_comment(&self, post_id: i64, new: NewComment) -> impl Future<Output = sqlx::Result<Comment>> + Send;
    fn search(&self, q: &str) -> impl Future<Output = sqlx::Result<Vec<Hit>>> + Send;
    fn create_user(&self, email: &str, password_hash: &str) -> impl Future<Output = sqlx::Result<User>> + Send;
    fn find_user_by_email(&self, email: &str) -> impl Future<Output = sqlx::Result<Option<UserWithHash>>> + Send;
    fn get_user(&self, id: i64) -> impl Future<Output = sqlx::Result<Option<User>>> + Send;
    fn store_refresh(&self, hash: &str, user_id: i64, expires_at: i64) -> impl Future<Output = sqlx::Result<()>> + Send;
    fn take_refresh(&self, hash: &str, now: i64) -> impl Future<Output = sqlx::Result<Option<i64>>> + Send;
    fn delete_refresh(&self, hash: &str) -> impl Future<Output = sqlx::Result<()>> + Send;
    fn delete_expired_refresh(&self, now: i64) -> impl Future<Output = sqlx::Result<u64>> + Send;
    fn list_users(&self) -> impl Future<Output = sqlx::Result<Vec<User>>> + Send;
    fn ping(&self) -> impl Future<Output = sqlx::Result<()>> + Send;
    fn add_image(&self, post_id: i64, url: &str) -> impl Future<Output = sqlx::Result<Image>> + Send;
}

#[derive(Clone)]
pub struct SqliteRepo {
    pub pool: SqlitePool,
}

impl Repo for SqliteRepo {
    async fn list(&self, f: ListFilter) -> sqlx::Result<(Vec<Post>, i64)> {
        let mut q = QueryBuilder::<Sqlite>::new("SELECT p.id, p.title, p.body, p.slug, p.created_at, p.author_id FROM posts p WHERE p.deleted_at IS NULL");
        if let Some(tag) = &f.tag {
            q.push(" AND p.id IN (SELECT pt.post_id FROM post_tags pt JOIN tags t ON t.id = pt.tag_id WHERE t.name = ")
                .push_bind(tag.clone())
                .push(")");
        }
        q.push(match f.sort {
            Sort::Newest => " ORDER BY p.id DESC",
            Sort::Oldest => " ORDER BY p.id ASC",
            Sort::Title => " ORDER BY p.title ASC",
        });
        q.push(" LIMIT ").push_bind(f.per_page as i64);
        q.push(" OFFSET ").push_bind(((f.page - 1) * f.per_page) as i64);
        let posts = q.build_query_as::<Post>().fetch_all(&self.pool).await?;

        let mut c = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM posts p WHERE p.deleted_at IS NULL");
        if let Some(tag) = &f.tag {
            c.push(" AND p.id IN (SELECT pt.post_id FROM post_tags pt JOIN tags t ON t.id = pt.tag_id WHERE t.name = ")
                .push_bind(tag.clone())
                .push(")");
        }
        let total: i64 = c.build_query_scalar().fetch_one(&self.pool).await?;
        Ok((posts, total))
    }

    async fn search(&self, q: &str) -> sqlx::Result<Vec<Hit>> {
        let q = fts_query(q);
        sqlx::query_as!(
            Hit,
            r#"SELECT p.id as "id!", p.title, snippet(posts_fts, 1, '[', ']', '...', 8) as "snippet!: String"
               FROM posts_fts JOIN posts p ON p.id = posts_fts.rowid
               WHERE posts_fts MATCH ? AND p.deleted_at IS NULL
               ORDER BY bm25(posts_fts) LIMIT 20"#,
            q
        )
        .fetch_all(&self.pool)
        .await
    }

    async fn get(&self, id: i64) -> sqlx::Result<Option<Post>> {
        sqlx::query_as!(Post, "SELECT id, title, body, slug, created_at, author_id FROM posts WHERE id = ? AND deleted_at IS NULL", id)
            .fetch_optional(&self.pool)
            .await
    }

    async fn create(&self, author_id: i64, new: NewPost) -> sqlx::Result<PostWithTags> {
        let slug = slugify(&new.title);
        let mut tx = self.pool.begin().await?;
        let post = sqlx::query_as!(
            Post,
            r#"INSERT INTO posts (title, body, slug, author_id) VALUES (?, ?, ?, ?) RETURNING id, title, body, slug, created_at, author_id"#,
            new.title,
            new.body,
            slug,
            author_id
        )
        .fetch_one(&mut *tx)
        .await?;
        for tag in &new.tags {
            sqlx::query!("INSERT INTO tags (name) VALUES (?) ON CONFLICT (name) DO NOTHING", tag)
                .execute(&mut *tx)
                .await?;
            sqlx::query!(
                "INSERT INTO post_tags (post_id, tag_id) SELECT ?, id FROM tags WHERE name = ?",
                post.id,
                tag
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(PostWithTags { post, tags: new.tags })
    }

    async fn update(&self, id: i64, new: NewPost) -> sqlx::Result<Option<Post>> {
        sqlx::query_as!(
            Post,
            r#"UPDATE posts SET title = ?, body = ? WHERE id = ? AND deleted_at IS NULL RETURNING id, title, body, slug, created_at, author_id"#,
            new.title,
            new.body,
            id
        )
        .fetch_optional(&self.pool)
        .await
    }

    async fn delete(&self, id: i64) -> sqlx::Result<bool> {
        let result = sqlx::query!("UPDATE posts SET deleted_at = CURRENT_TIMESTAMP WHERE id = ? AND deleted_at IS NULL", id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn restore(&self, id: i64) -> sqlx::Result<bool> {
        let result = sqlx::query!("UPDATE posts SET deleted_at = NULL WHERE id = ? AND deleted_at IS NOT NULL", id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn create_user(&self, email: &str, password_hash: &str) -> sqlx::Result<User> {
        sqlx::query_as!(
            User,
            r#"INSERT INTO users (email, password_hash) VALUES (?, ?) RETURNING id, email, role, created_at"#,
            email,
            password_hash
        )
        .fetch_one(&self.pool)
        .await
    }

    async fn find_user_by_email(&self, email: &str) -> sqlx::Result<Option<UserWithHash>> {
        sqlx::query_as!(
            UserWithHash,
            "SELECT id, email, password_hash, role FROM users WHERE email = ?",
            email
        )
        .fetch_optional(&self.pool)
        .await
    }

    async fn get_user(&self, id: i64) -> sqlx::Result<Option<User>> {
        sqlx::query_as!(User, "SELECT id, email, role, created_at FROM users WHERE id = ?", id)
            .fetch_optional(&self.pool)
            .await
    }

    async fn store_refresh(&self, hash: &str, user_id: i64, expires_at: i64) -> sqlx::Result<()> {
        sqlx::query!(
            "INSERT INTO refresh_tokens (token_hash, user_id, expires_at) VALUES (?, ?, ?)",
            hash,
            user_id,
            expires_at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn take_refresh(&self, hash: &str, now: i64) -> sqlx::Result<Option<i64>> {
        sqlx::query_scalar!(
            r#"DELETE FROM refresh_tokens WHERE token_hash = ? AND expires_at > ? RETURNING user_id as "user_id!""#,
            hash,
            now
        )
        .fetch_optional(&self.pool)
        .await
    }

    async fn delete_refresh(&self, hash: &str) -> sqlx::Result<()> {
        sqlx::query!("DELETE FROM refresh_tokens WHERE token_hash = ?", hash).execute(&self.pool).await?;
        Ok(())
    }

    async fn delete_expired_refresh(&self, now: i64) -> sqlx::Result<u64> {
        let done = sqlx::query!("DELETE FROM refresh_tokens WHERE expires_at < ?", now).execute(&self.pool).await?;
        Ok(done.rows_affected())
    }

    async fn list_comments(&self, post_id: i64) -> sqlx::Result<Vec<Comment>> {
        sqlx::query_as!(
            Comment,
            "SELECT id, post_id, body, created_at FROM comments WHERE post_id = ? ORDER BY id",
            post_id
        )
        .fetch_all(&self.pool)
        .await
    }

    async fn add_comment(&self, post_id: i64, new: NewComment) -> sqlx::Result<Comment> {
        sqlx::query_as!(
            Comment,
            r#"INSERT INTO comments (post_id, body) VALUES (?, ?) RETURNING id as "id!", post_id, body, created_at"#,
            post_id,
            new.body
        )
        .fetch_one(&self.pool)
        .await
    }

    async fn list_users(&self) -> sqlx::Result<Vec<User>> {
        sqlx::query_as!(User, "SELECT id, email, role, created_at FROM users ORDER BY id")
            .fetch_all(&self.pool)
            .await
    }

    async fn ping(&self) -> sqlx::Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    async fn add_image(&self, post_id: i64, url: &str) -> sqlx::Result<Image> {
        sqlx::query_as!(
            Image,
            r#"INSERT INTO images (post_id, url) VALUES (?, ?) RETURNING id as "id!", post_id, url"#,
            post_id,
            url
        )
        .fetch_one(&self.pool)
        .await
    }
}

fn fts_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}
