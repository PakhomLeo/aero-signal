//! 对象存储：Fake（默认）+ MinIO/S3 兼容 PUT。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::error::{CrawlerError, CrawlerResult};
use crate::types::StoredObject;

const DEDUP_INDEX_PREFIX: &str = "__dedup/";

#[async_trait]
pub trait ObjectStore: Send + Sync {
    /// 按 content hash 去重写入；已存在则返回原 key 并标记 deduplicated。
    async fn put_bytes(
        &self,
        key_prefix: &str,
        bytes: &[u8],
        mime_type: Option<&str>,
        width: Option<u32>,
        height: Option<u32>,
    ) -> CrawlerResult<StoredObject>;

    async fn get_bytes(&self, storage_key: &str) -> CrawlerResult<Option<Vec<u8>>>;

    async fn exists_by_hash(&self, content_hash: &str) -> CrawlerResult<bool>;
}

fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn key_for(prefix: &str, hash: &str, mime: Option<&str>) -> String {
    let ext = match mime {
        Some(m) if m.contains("png") => "png",
        Some(m) if m.contains("webp") => "webp",
        Some(m) if m.contains("gif") => "gif",
        Some(m) if m.contains("jpeg") || m.contains("jpg") => "jpg",
        Some(m) if m.contains("html") => "html",
        _ => "bin",
    };
    format!("{prefix}/{hash}.{ext}")
}

fn dedup_index_key(hash: &str) -> String {
    format!("{DEDUP_INDEX_PREFIX}{hash}")
}

fn full_storage_key(bucket: &str, relative: &str) -> String {
    format!("s3://{bucket}/{relative}")
}

fn relative_storage_key(bucket: &str, storage_key: &str) -> String {
    storage_key
        .strip_prefix(&format!("s3://{bucket}/"))
        .unwrap_or(storage_key)
        .to_string()
}

/// 内存 Fake 对象存储（测试与本地默认）。
#[derive(Debug, Default, Clone)]
pub struct FakeObjectStore {
    inner: Arc<RwLock<FakeState>>,
}

#[derive(Debug, Default)]
struct FakeState {
    by_key: HashMap<String, Vec<u8>>,
    hash_to_key: HashMap<String, String>,
}

impl FakeObjectStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.by_key.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.by_key.is_empty()
    }
}

#[async_trait]
impl ObjectStore for FakeObjectStore {
    async fn put_bytes(
        &self,
        key_prefix: &str,
        bytes: &[u8],
        mime_type: Option<&str>,
        width: Option<u32>,
        height: Option<u32>,
    ) -> CrawlerResult<StoredObject> {
        let hash = content_hash(bytes);
        let mut guard = self.inner.write().await;
        if let Some(existing) = guard.hash_to_key.get(&hash).cloned() {
            return Ok(StoredObject {
                storage_key: existing,
                content_hash: hash,
                byte_size: bytes.len() as u64,
                mime_type: mime_type.map(str::to_string),
                width,
                height,
                deduplicated: true,
            });
        }
        let key = key_for(key_prefix, &hash, mime_type);
        guard.by_key.insert(key.clone(), bytes.to_vec());
        guard.hash_to_key.insert(hash.clone(), key.clone());
        Ok(StoredObject {
            storage_key: key,
            content_hash: hash,
            byte_size: bytes.len() as u64,
            mime_type: mime_type.map(str::to_string),
            width,
            height,
            deduplicated: false,
        })
    }

    async fn get_bytes(&self, storage_key: &str) -> CrawlerResult<Option<Vec<u8>>> {
        Ok(self.inner.read().await.by_key.get(storage_key).cloned())
    }

    async fn exists_by_hash(&self, content_hash: &str) -> CrawlerResult<bool> {
        Ok(self
            .inner
            .read()
            .await
            .hash_to_key
            .contains_key(content_hash))
    }
}

/// MinIO / S3 兼容对象存储（path-style，按 hash 全局去重）。
#[derive(Clone)]
pub struct MinioObjectStore {
    bucket: Bucket,
    bucket_name: String,
    hash_cache: Arc<RwLock<HashMap<String, String>>>,
}

impl MinioObjectStore {
    pub fn new(
        endpoint: impl Into<String>,
        bucket: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> CrawlerResult<Self> {
        let endpoint = endpoint.into();
        let bucket_name = bucket.into();
        if endpoint.trim().is_empty() || bucket_name.trim().is_empty() {
            return Err(CrawlerError::store_failed("MinIO endpoint/bucket 未配置"));
        }
        let region = Region::Custom {
            region: "us-east-1".into(),
            endpoint: endpoint.trim_end_matches('/').into(),
        };
        let access_key = access_key.into();
        let secret_key = secret_key.into();
        let credentials = Credentials::new(
            Some(access_key.as_str()),
            Some(secret_key.as_str()),
            None,
            None,
            None,
        )
        .map_err(|e| CrawlerError::store_failed(format!("S3 凭证无效: {e}")))?;
        let bucket = Bucket::new(&bucket_name, region, credentials)
            .map_err(|e| CrawlerError::store_failed(format!("初始化 S3 bucket 失败: {e}")))?
            .with_path_style();
        Ok(Self {
            bucket: *bucket,
            bucket_name,
            hash_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn from_env_config(
        endpoint: impl Into<String>,
        bucket: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> CrawlerResult<Self> {
        Self::new(endpoint, bucket, access_key, secret_key)
    }

    #[deprecated(note = "请使用 from_env_config 或 ServiceConfig 字段")]
    pub fn from_env() -> Option<Self> {
        let endpoint = std::env::var("S3_ENDPOINT")
            .or_else(|_| std::env::var("SIGNAL_MINIO_ENDPOINT"))
            .ok()?;
        let bucket = std::env::var("S3_BUCKET")
            .or_else(|_| std::env::var("SIGNAL_MINIO_BUCKET"))
            .unwrap_or_else(|_| "aero-signal-dev".into());
        let access = std::env::var("S3_ACCESS_KEY").unwrap_or_else(|_| "aero".into());
        let secret = std::env::var("S3_SECRET_KEY").unwrap_or_else(|_| "aero_local_dev".into());
        if endpoint.trim().is_empty() {
            return None;
        }
        Self::new(endpoint, bucket, access, secret).ok()
    }

    async fn lookup_hash_key(&self, hash: &str) -> CrawlerResult<Option<String>> {
        if let Some(key) = self.hash_cache.read().await.get(hash).cloned() {
            return Ok(Some(key));
        }
        let index_key = dedup_index_key(hash);
        match self.bucket.head_object(&index_key).await {
            Ok((_head, 200)) => match self.bucket.get_object(&index_key).await {
                Ok(data) if (200..300).contains(&data.status_code()) => {
                    let relative = String::from_utf8_lossy(data.as_slice()).trim().to_string();
                    if !relative.is_empty() {
                        self.hash_cache
                            .write()
                            .await
                            .insert(hash.to_string(), relative.clone());
                        return Ok(Some(relative));
                    }
                }
                _ => {}
            },
            Ok(_) => {}
            Err(e) => {
                tracing::debug!(hash = %hash, error = %e, "S3 dedup index head 失败");
            }
        }
        Ok(None)
    }

    async fn write_dedup_index(&self, hash: &str, relative_key: &str) -> CrawlerResult<()> {
        let index_key = dedup_index_key(hash);
        self.bucket
            .put_object(&index_key, relative_key.as_bytes())
            .await
            .map_err(|e| CrawlerError::store_failed(format!("写入 dedup 索引失败: {e}")))?;
        self.hash_cache
            .write()
            .await
            .insert(hash.to_string(), relative_key.to_string());
        Ok(())
    }
}

#[async_trait]
impl ObjectStore for MinioObjectStore {
    async fn put_bytes(
        &self,
        key_prefix: &str,
        bytes: &[u8],
        mime_type: Option<&str>,
        width: Option<u32>,
        height: Option<u32>,
    ) -> CrawlerResult<StoredObject> {
        let hash = content_hash(bytes);
        if let Some(relative) = self.lookup_hash_key(&hash).await? {
            return Ok(StoredObject {
                storage_key: full_storage_key(&self.bucket_name, &relative),
                content_hash: hash,
                byte_size: bytes.len() as u64,
                mime_type: mime_type.map(str::to_string),
                width,
                height,
                deduplicated: true,
            });
        }

        let relative = key_for(key_prefix, &hash, mime_type);
        let content_type = mime_type.unwrap_or("application/octet-stream");
        self.bucket
            .put_object_with_content_type(&relative, bytes, content_type)
            .await
            .map_err(|e| CrawlerError::store_failed(format!("S3 PUT 失败: {e}")))?;
        self.write_dedup_index(&hash, &relative).await?;

        Ok(StoredObject {
            storage_key: full_storage_key(&self.bucket_name, &relative),
            content_hash: hash,
            byte_size: bytes.len() as u64,
            mime_type: mime_type.map(str::to_string),
            width,
            height,
            deduplicated: false,
        })
    }

    async fn get_bytes(&self, storage_key: &str) -> CrawlerResult<Option<Vec<u8>>> {
        let relative = relative_storage_key(&self.bucket_name, storage_key);
        match self.bucket.get_object(&relative).await {
            Ok(resp) if (200..300).contains(&resp.status_code()) => Ok(Some(resp.to_vec())),
            Ok(resp) if resp.status_code() == 404 => Ok(None),
            Ok(resp) => Err(CrawlerError::store_failed(format!(
                "S3 GET 异常状态: {}",
                resp.status_code()
            ))),
            Err(e) => Err(CrawlerError::store_failed(format!("S3 GET 失败: {e}"))),
        }
    }

    async fn exists_by_hash(&self, content_hash: &str) -> CrawlerResult<bool> {
        Ok(self.lookup_hash_key(content_hash).await?.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_dedups_by_hash() {
        let store = FakeObjectStore::new();
        let a = store
            .put_bytes(
                "articles/x",
                b"hello",
                Some("image/png"),
                Some(10),
                Some(10),
            )
            .await
            .unwrap();
        let b = store
            .put_bytes(
                "articles/x",
                b"hello",
                Some("image/png"),
                Some(10),
                Some(10),
            )
            .await
            .unwrap();
        assert!(!a.deduplicated);
        assert!(b.deduplicated);
        assert_eq!(a.storage_key, b.storage_key);
        assert_eq!(store.len().await, 1);
    }

    #[test]
    fn storage_key_roundtrip() {
        let bucket = "aero-signal-dev";
        let rel = "articles/x/abc.png";
        let full = full_storage_key(bucket, rel);
        assert_eq!(relative_storage_key(bucket, &full), rel);
    }
}
