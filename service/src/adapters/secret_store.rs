//! SecretStore：密钥只存引用，不进业务表普通列。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use uuid::Uuid;

/// 密钥存储抽象。
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// 写入密钥，返回 `secret_ref`（不包含明文）。
    async fn put(&self, value: &str) -> Result<String, String>;

    /// 按引用读取密钥明文（仅内部适配器使用，不得写入 API 响应）。
    async fn get(&self, secret_ref: &str) -> Result<Option<String>, String>;

    /// 删除引用。
    async fn delete(&self, secret_ref: &str) -> Result<(), String>;
}

fn new_ref() -> String {
    format!("sec_{}", Uuid::new_v4().simple())
}

/// 内存实现（测试与无持久化启动路径）。
#[derive(Clone, Default)]
pub struct MemorySecretStore {
    inner: Arc<Mutex<HashMap<String, String>>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }
}

#[async_trait]
impl SecretStore for MemorySecretStore {
    async fn put(&self, value: &str) -> Result<String, String> {
        let r = new_ref();
        self.inner.lock().await.insert(r.clone(), value.to_string());
        Ok(r)
    }

    async fn get(&self, secret_ref: &str) -> Result<Option<String>, String> {
        Ok(self.inner.lock().await.get(secret_ref).cloned())
    }

    async fn delete(&self, secret_ref: &str) -> Result<(), String> {
        self.inner.lock().await.remove(secret_ref);
        Ok(())
    }
}

/// 可选文件实现：每个 secret 一个文件，文件名即 ref。
#[derive(Clone)]
pub struct FileSecretStore {
    root: PathBuf,
}

impl FileSecretStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, String> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        Ok(Self { root })
    }

    fn path_for(&self, secret_ref: &str) -> Result<PathBuf, String> {
        if secret_ref.is_empty()
            || secret_ref.contains('/')
            || secret_ref.contains('\\')
            || secret_ref.contains("..")
        {
            return Err("非法 secret_ref".into());
        }
        Ok(self.root.join(secret_ref))
    }
}

#[async_trait]
impl SecretStore for FileSecretStore {
    async fn put(&self, value: &str) -> Result<String, String> {
        let r = new_ref();
        let path = self.path_for(&r)?;
        tokio::fs::write(&path, value.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        Ok(r)
    }

    async fn get(&self, secret_ref: &str) -> Result<Option<String>, String> {
        let path = self.path_for(secret_ref)?;
        match tokio::fs::read_to_string(&path).await {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn delete(&self, secret_ref: &str) -> Result<(), String> {
        let path = self.path_for(secret_ref)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 便于测试：确认路径存在。
pub fn secret_store_root_exists(path: &Path) -> bool {
    path.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_roundtrip_and_delete() {
        let store = MemorySecretStore::new();
        let r = store.put("proxy://user:pass@host:8080").await.unwrap();
        assert!(r.starts_with("sec_"));
        assert_eq!(
            store.get(&r).await.unwrap().as_deref(),
            Some("proxy://user:pass@host:8080")
        );
        store.delete(&r).await.unwrap();
        assert!(store.get(&r).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn file_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("aero-secret-{}", Uuid::new_v4()));
        let store = FileSecretStore::new(&dir).unwrap();
        let r = store.put("gateway-token-xyz").await.unwrap();
        assert_eq!(
            store.get(&r).await.unwrap().as_deref(),
            Some("gateway-token-xyz")
        );
        store.delete(&r).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
