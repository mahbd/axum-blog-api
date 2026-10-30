-- Add migration script here
ALTER TABLE posts ADD COLUMN slug TEXT NOT NULL DEFAULT '';
UPDATE posts SET slug = 'post-' || id;
CREATE UNIQUE INDEX posts_slug ON posts(slug);
