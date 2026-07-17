//! 出口与代理供应商仓储。

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::egress::{EgressProfile, ProxyProvider};

#[async_trait]
pub trait EgressRepository: Send + Sync {
    async fn list_proxy_providers(&self) -> Result<Vec<ProxyProvider>, String>;
    async fn get_proxy_provider(&self, id: Uuid) -> Result<Option<ProxyProvider>, String>;
    async fn insert_proxy_provider(&self, p: &ProxyProvider) -> Result<(), String>;
    async fn update_proxy_provider(&self, p: &ProxyProvider) -> Result<(), String>;

    async fn list_egress_profiles(&self) -> Result<Vec<EgressProfile>, String>;
    async fn get_egress_profile(&self, id: Uuid) -> Result<Option<EgressProfile>, String>;
    async fn get_egress_profile_by_name(&self, name: &str)
    -> Result<Option<EgressProfile>, String>;
    async fn insert_egress_profile(&self, p: &EgressProfile) -> Result<(), String>;
    async fn update_egress_profile(&self, p: &EgressProfile) -> Result<(), String>;
}
