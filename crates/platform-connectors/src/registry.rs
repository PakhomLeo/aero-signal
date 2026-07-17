//! 连接器注册表：按名注册，暴露版本化 capability。

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::backend::ConnectorBackend;
use crate::error::{ConnectorError, ConnectorResult, codes};
use crate::types::CapabilitySet;

/// 注册条目的公开摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorDescriptor {
    pub name: String,
    pub version: String,
    pub capabilities: CapabilitySet,
}

/// 按名称注册的连接器后端。
#[derive(Default)]
pub struct ConnectorRegistry {
    backends: BTreeMap<String, Arc<dyn ConnectorBackend>>,
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册后端；同名覆盖。
    pub fn register(&mut self, backend: Arc<dyn ConnectorBackend>) {
        self.backends.insert(backend.name().to_string(), backend);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn ConnectorBackend>> {
        self.backends.get(name).cloned()
    }

    pub fn require(&self, name: &str) -> ConnectorResult<Arc<dyn ConnectorBackend>> {
        self.get(name).ok_or_else(|| {
            ConnectorError::new(codes::UNSUPPORTED, format!("未注册的连接器: {name}"))
        })
    }

    pub fn list(&self) -> Vec<ConnectorDescriptor> {
        self.backends
            .values()
            .map(|b| ConnectorDescriptor {
                name: b.name().to_string(),
                version: b.version().to_string(),
                capabilities: b.capabilities(),
            })
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.backends.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.backends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }
}
