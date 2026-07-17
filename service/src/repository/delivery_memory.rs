//! 内存 delivery 仓储。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::domain::delivery::{
    ApiCallLog, ApiToken, DeliveryGroup, DeliveryGroupSource, DeliveryGroupStatus,
};
use crate::repository::delivery::{
    CreateDeliveryGroup, CreateDeliveryGroupWithSources, DeliveryRepository, PatchDeliveryGroup,
    PatchDeliveryGroupWithSources,
};

#[derive(Default)]
struct State {
    groups: HashMap<Uuid, DeliveryGroup>,
    /// group_id -> bindings
    sources: HashMap<Uuid, Vec<DeliveryGroupSource>>,
    tokens: HashMap<Uuid, ApiToken>,
    by_hash: HashMap<String, Uuid>,
    call_logs: Vec<ApiCallLog>,
    /// Test-only fault injection used by the repository atomicity contract
    /// tests. The flag is consumed by the next aggregate write.
    fail_next_atomic_group_write: bool,
}

#[derive(Default, Clone)]
pub struct MemoryDeliveryStore {
    state: Arc<RwLock<State>>,
}

impl MemoryDeliveryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Causes the next `*_group_with_sources` command to fail after its
    /// metadata mutation has been staged. This lets integration tests prove
    /// that no partial group or binding survives an aborted command.
    pub async fn fail_next_atomic_group_write(&self) {
        self.state.write().await.fail_next_atomic_group_write = true;
    }
}

#[async_trait]
impl DeliveryRepository for MemoryDeliveryStore {
    async fn create_group(&self, cmd: CreateDeliveryGroup) -> Result<DeliveryGroup, String> {
        let mut s = self.state.write().await;
        if s.groups.values().any(|g| g.name == cmd.name) {
            return Err("delivery.group_name_conflict: 分组名称已存在".into());
        }
        let g = DeliveryGroup {
            id: cmd.id,
            name: cmd.name,
            description: cmd.description,
            status: DeliveryGroupStatus::Active,
            created_at: cmd.now,
            updated_at: cmd.now,
        };
        s.groups.insert(g.id, g.clone());
        s.sources.insert(g.id, Vec::new());
        Ok(g)
    }

    async fn create_group_with_sources(
        &self,
        cmd: CreateDeliveryGroupWithSources,
    ) -> Result<DeliveryGroup, String> {
        let mut s = self.state.write().await;
        if s.groups.values().any(|g| g.name == cmd.group.name) {
            return Err("delivery.group_name_conflict: group name already exists".into());
        }

        let g = DeliveryGroup {
            id: cmd.group.id,
            name: cmd.group.name,
            description: cmd.group.description,
            status: DeliveryGroupStatus::Active,
            created_at: cmd.group.now,
            updated_at: cmd.group.now,
        };
        s.groups.insert(g.id, g.clone());
        s.sources.insert(g.id, Vec::new());

        if s.fail_next_atomic_group_write {
            s.fail_next_atomic_group_write = false;
            // Roll back both sides of the aggregate before exposing the
            // error, matching the transaction boundary in PostgreSQL.
            s.groups.remove(&g.id);
            s.sources.remove(&g.id);
            return Err("delivery.group_sources_commit_failed: injected failure".into());
        }

        let mut seen = HashSet::new();
        let bindings = s.sources.get_mut(&g.id).expect("group sources inserted");
        for source_id in cmd.source_ids {
            if seen.insert(source_id) {
                bindings.push(DeliveryGroupSource {
                    delivery_group_id: g.id,
                    source_account_id: source_id,
                    bound_at: cmd.group.now,
                });
            }
        }
        Ok(g)
    }

    async fn get_group(&self, id: Uuid) -> Result<Option<DeliveryGroup>, String> {
        Ok(self.state.read().await.groups.get(&id).cloned())
    }

    async fn list_groups(&self) -> Result<Vec<DeliveryGroup>, String> {
        let mut rows: Vec<_> = self.state.read().await.groups.values().cloned().collect();
        rows.sort_by_key(|a| a.created_at);
        Ok(rows)
    }

    async fn patch_group(
        &self,
        id: Uuid,
        cmd: PatchDeliveryGroup,
    ) -> Result<DeliveryGroup, String> {
        let mut s = self.state.write().await;
        if !s.groups.contains_key(&id) {
            return Err("delivery.group_not_found: 分组不存在".into());
        }
        if let Some(ref name) = cmd.name
            && s.groups
                .values()
                .any(|other| other.id != id && other.name == *name)
        {
            return Err("delivery.group_name_conflict: 分组名称已存在".into());
        }
        let g = s.groups.get_mut(&id).unwrap();
        if let Some(name) = cmd.name {
            g.name = name;
        }
        if let Some(desc) = cmd.description {
            g.description = desc;
        }
        if let Some(status) = cmd.status {
            g.status = status;
        }
        g.updated_at = cmd.now;
        Ok(g.clone())
    }

    async fn patch_group_with_sources(
        &self,
        cmd: PatchDeliveryGroupWithSources,
    ) -> Result<DeliveryGroup, String> {
        let mut s = self.state.write().await;
        let existing = s
            .groups
            .get(&cmd.group_id)
            .cloned()
            .ok_or_else(|| "delivery.group_not_found: group not found".to_string())?;
        if let Some(ref name) = cmd.group.name
            && s.groups
                .values()
                .any(|other| other.id != cmd.group_id && other.name == *name)
        {
            return Err("delivery.group_name_conflict: group name already exists".into());
        }

        let previous_sources = s.sources.get(&cmd.group_id).cloned().unwrap_or_default();
        let g = s
            .groups
            .get_mut(&cmd.group_id)
            .expect("group checked above");
        if let Some(name) = cmd.group.name {
            g.name = name;
        }
        if let Some(description) = cmd.group.description {
            g.description = description;
        }
        if let Some(status) = cmd.group.status {
            g.status = status;
        }
        g.updated_at = cmd.group.now;
        let updated = g.clone();

        if s.fail_next_atomic_group_write {
            s.fail_next_atomic_group_write = false;
            s.groups.insert(cmd.group_id, existing);
            s.sources.insert(cmd.group_id, previous_sources);
            return Err("delivery.group_sources_commit_failed: injected failure".into());
        }

        if let Some(source_ids) = cmd.source_ids {
            let mut seen = HashSet::new();
            let bindings = source_ids
                .into_iter()
                .filter(|id| seen.insert(*id))
                .map(|source_account_id| DeliveryGroupSource {
                    delivery_group_id: cmd.group_id,
                    source_account_id,
                    bound_at: cmd.group.now,
                })
                .collect();
            s.sources.insert(cmd.group_id, bindings);
        }
        Ok(updated)
    }

    async fn bind_sources(
        &self,
        group_id: Uuid,
        source_ids: &[Uuid],
        now: DateTime<Utc>,
    ) -> Result<Vec<DeliveryGroupSource>, String> {
        let mut s = self.state.write().await;
        if !s.groups.contains_key(&group_id) {
            return Err("delivery.group_not_found: 分组不存在".into());
        }
        let list = s.sources.entry(group_id).or_default();
        let existing: HashSet<Uuid> = list.iter().map(|b| b.source_account_id).collect();
        for sid in source_ids {
            if existing.contains(sid) {
                continue;
            }
            list.push(DeliveryGroupSource {
                delivery_group_id: group_id,
                source_account_id: *sid,
                bound_at: now,
            });
        }
        Ok(list.clone())
    }

    async fn set_group_sources(
        &self,
        group_id: Uuid,
        source_ids: &[Uuid],
        now: DateTime<Utc>,
    ) -> Result<Vec<DeliveryGroupSource>, String> {
        let mut s = self.state.write().await;
        if !s.groups.contains_key(&group_id) {
            return Err("delivery.group_not_found: 分组不存在".into());
        }
        let mut seen = HashSet::new();
        let mut list = Vec::new();
        for sid in source_ids {
            if !seen.insert(*sid) {
                continue;
            }
            list.push(DeliveryGroupSource {
                delivery_group_id: group_id,
                source_account_id: *sid,
                bound_at: now,
            });
        }
        s.sources.insert(group_id, list.clone());
        Ok(list)
    }

    async fn list_group_sources(&self, group_id: Uuid) -> Result<Vec<DeliveryGroupSource>, String> {
        Ok(self
            .state
            .read()
            .await
            .sources
            .get(&group_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn insert_token(&self, token: ApiToken) -> Result<ApiToken, String> {
        let mut s = self.state.write().await;
        if !s.groups.contains_key(&token.delivery_group_id) {
            return Err("delivery.group_not_found: 分组不存在".into());
        }
        if s.by_hash.contains_key(&token.token_hash) {
            return Err("token.hash_conflict: token hash 冲突".into());
        }
        s.by_hash.insert(token.token_hash.clone(), token.id);
        s.tokens.insert(token.id, token.clone());
        Ok(token)
    }

    async fn get_token(&self, id: Uuid) -> Result<Option<ApiToken>, String> {
        Ok(self.state.read().await.tokens.get(&id).cloned())
    }

    async fn find_token_by_hash(&self, token_hash: &str) -> Result<Option<ApiToken>, String> {
        let s = self.state.read().await;
        Ok(s.by_hash
            .get(token_hash)
            .and_then(|id| s.tokens.get(id))
            .cloned())
    }

    async fn list_tokens_for_group(&self, group_id: Uuid) -> Result<Vec<ApiToken>, String> {
        let mut rows: Vec<_> = self
            .state
            .read()
            .await
            .tokens
            .values()
            .filter(|t| t.delivery_group_id == group_id)
            .cloned()
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.created_at));
        Ok(rows)
    }

    async fn revoke_token(&self, id: Uuid, now: DateTime<Utc>) -> Result<ApiToken, String> {
        let mut s = self.state.write().await;
        let t = s
            .tokens
            .get_mut(&id)
            .ok_or_else(|| "token.not_found: Token 不存在".to_string())?;
        if t.revoked_at.is_none() {
            t.revoked_at = Some(now);
        }
        Ok(t.clone())
    }

    async fn touch_token_used(&self, id: Uuid, now: DateTime<Utc>) -> Result<(), String> {
        let mut s = self.state.write().await;
        if let Some(t) = s.tokens.get_mut(&id) {
            t.last_used_at = Some(now);
        }
        Ok(())
    }

    async fn insert_call_log(&self, log: ApiCallLog) -> Result<ApiCallLog, String> {
        self.state.write().await.call_logs.push(log.clone());
        Ok(log)
    }

    async fn list_call_logs(
        &self,
        token_id: Uuid,
        limit: usize,
    ) -> Result<Vec<ApiCallLog>, String> {
        let s = self.state.read().await;
        let mut rows: Vec<_> = s
            .call_logs
            .iter()
            .filter(|l| l.api_token_id == token_id)
            .cloned()
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.created_at));
        rows.truncate(limit);
        Ok(rows)
    }
}
