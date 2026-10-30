use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

pub trait Storage: Send + Sync + 'static {
    fn put(&self, name: &str, data: &[u8]) -> impl Future<Output = io::Result<()>> + Send;
    fn get(&self, name: &str) -> impl Future<Output = io::Result<Vec<u8>>> + Send;
    fn delete(&self, name: &str) -> impl Future<Output = io::Result<()>> + Send;
}

pub struct LocalStorage {
    root: PathBuf,
}

impl LocalStorage {
    pub fn new(root: &str) -> LocalStorage {
        std::fs::create_dir_all(root).unwrap();
        LocalStorage {
            root: PathBuf::from(root),
        }
    }

    fn path(&self, name: &str) -> io::Result<PathBuf> {
        if name.is_empty() || name.starts_with('.') || name.contains('/') || name.contains('\\') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad file name"));
        }
        Ok(self.root.join(name))
    }
}

impl Storage for LocalStorage {
    async fn put(&self, name: &str, data: &[u8]) -> io::Result<()> {
        tokio::fs::write(self.path(name)?, data).await
    }

    async fn get(&self, name: &str) -> io::Result<Vec<u8>> {
        tokio::fs::read(self.path(name)?).await
    }

    async fn delete(&self, name: &str) -> io::Result<()> {
        tokio::fs::remove_file(self.path(name)?).await
    }
}

#[derive(Default)]
pub struct MemStorage {
    files: Mutex<HashMap<String, Vec<u8>>>,
}

impl Storage for MemStorage {
    async fn put(&self, name: &str, data: &[u8]) -> io::Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(name.to_string(), data.to_vec());
        Ok(())
    }

    async fn get(&self, name: &str) -> io::Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or(io::Error::from(io::ErrorKind::NotFound))
    }

    async fn delete(&self, name: &str) -> io::Result<()> {
        self.files.lock().unwrap().remove(name);
        Ok(())
    }
}
