use std::sync::OnceLock;

pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub jwt_secret: String,
    pub storage: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub mail_from: String,
    pub cleanup_secs: u64,
    pub redis_url: String,
}

impl Config {
    fn from_env() -> Config {
        dotenvy::dotenv().ok();
        let host = std::env::var("HOST").unwrap_or("0.0.0.0".to_string());
        let port = std::env::var("PORT").unwrap_or("3000".to_string());
        let port = port
            .parse()
            .unwrap_or_else(|e| panic!("PORT {port:?} is not a port number: {e}"));
        let database_url = std::env::var("DATABASE_URL").unwrap_or("sqlite://blog.db".to_string());
        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
        let storage = std::env::var("STORAGE").unwrap_or("local".to_string());
        let smtp_host = std::env::var("SMTP_HOST").unwrap_or("localhost".to_string());
        let smtp_port = std::env::var("SMTP_PORT")
            .unwrap_or("1025".to_string())
            .parse()
            .expect("SMTP_PORT is a port number");
        let mail_from =
            std::env::var("MAIL_FROM").unwrap_or("LCE Blog <blog@example.com>".to_string());
        let cleanup_secs = std::env::var("CLEANUP_SECS")
            .unwrap_or("3600".to_string())
            .parse()
            .expect("CLEANUP_SECS is a number");
        let redis_url = std::env::var("REDIS_URL").unwrap_or("redis://127.0.0.1:6379".to_string());
        Config {
            host,
            port,
            database_url,
            jwt_secret,
            storage,
            smtp_host,
            smtp_port,
            mail_from,
            cleanup_secs,
            redis_url,
        }
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn config() -> &'static Config {
    CONFIG.get_or_init(Config::from_env)
}
