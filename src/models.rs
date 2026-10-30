use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError};

#[derive(Serialize, Deserialize, Clone, sqlx::FromRow, ToSchema)]
pub struct Post {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub slug: String,
    pub author_id: Option<i64>,
    pub created_at: String,
}

#[derive(Deserialize, Validate, ToSchema)]
pub struct NewPost {
    #[validate(
        length(min = 1, max = 200, message = "1 to 200 characters"),
        custom(function = "not_blank")
    )]
    pub title: String,
    #[validate(length(min = 1, max = 20000, message = "must not be empty"))]
    pub body: String,
    #[serde(default)]
    #[validate(length(max = 5, message = "at most 5 tags"))]
    pub tags: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct Comment {
    pub id: i64,
    pub post_id: i64,
    pub body: String,
    pub created_at: String,
}

#[derive(Deserialize, Validate)]
pub struct NewComment {
    #[validate(
        length(min = 1, max = 2000, message = "1 to 2000 characters"),
        custom(function = "not_blank")
    )]
    pub body: String,
}

pub fn slugify(title: &str) -> String {
    let mut slug = String::new();
    for c in title.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        return format!("post-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    }
    slug
}

#[derive(Serialize, ToSchema)]
pub struct PostWithTags {
    #[serde(flatten)]
    pub post: Post,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
pub struct ListParams {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub tag: Option<String>,
    #[serde(default)]
    pub sort: Sort,
}

#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    Title,
}

#[derive(Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub per_page: u32,
    pub total: i64,
}

pub struct ListFilter {
    pub page: u32,
    pub per_page: u32,
    pub tag: Option<String>,
    pub sort: Sort,
}

#[derive(Serialize)]
pub struct Hit {
    pub id: i64,
    pub title: String,
    pub snippet: String,
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: String,
}

pub fn not_blank(value: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        return Err(ValidationError::new("blank").with_message("must not be blank".into()));
    }
    Ok(())
}

#[derive(Deserialize, Validate)]
pub struct NewUser {
    #[validate(email(message = "must be a valid email"))]
    pub email: String,
    #[validate(length(min = 10, max = 200, message = "10 to 200 characters"))]
    pub password: String,
}

#[derive(Serialize, Clone)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub role: String,
    pub created_at: String,
}

pub struct UserWithHash {
    pub id: i64,
    pub email: String,
    pub password_hash: String,
    pub role: String,
}

#[derive(Serialize)]
pub struct Image {
    pub id: i64,
    pub post_id: i64,
    pub url: String,
}

#[derive(Serialize, ToSchema)]
pub struct PostV2 {
    pub id: i64,
    #[schema(example = "Hello")]
    pub title: String,
    /// The post's text in Markdown.
    pub content: String,
    pub slug: String,
    pub author_id: Option<i64>,
    pub created_at: String,
}

impl From<Post> for PostV2 {
    fn from(p: Post) -> PostV2 {
        PostV2 {
            id: p.id,
            title: p.title,
            content: p.body,
            slug: p.slug,
            author_id: p.author_id,
            created_at: p.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_from_title() {
        assert_eq!(slugify("Hello, World"), "hello-world");
    }

    #[test]
    fn slug_never_empty() {
        assert!(!slugify("!!!").is_empty());
    }
}
