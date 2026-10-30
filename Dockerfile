FROM rust:1.99-slim AS build
WORKDIR /app
ENV SQLX_OFFLINE=true
RUN apt-get update && apt-get install -y curl
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src
COPY . .
RUN touch src/main.rs && cargo build --release

FROM debian:trixie-slim
RUN useradd -r app && mkdir /data && chown app /data
WORKDIR /data
COPY --from=build /app/target/release/blog_api /usr/local/bin/blog_api
USER app
ENV HOST=0.0.0.0 PORT=8080 DATABASE_URL=sqlite:///data/blog.db RUST_LOG=info
VOLUME /data
EXPOSE 8080
CMD ["blog_api"]
