//! 内存监测公众号仓储（测试与无 PG 场景）。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::domain::source::{
    AccountFetchRules, SourceAccount, SourceAccountId, SourceHeartbeatEvent, SourceIdType,
    SourceStatus, SourceValidationRun, ValidationRunStatus,
};
use crate::repository::source::{
    AppliedHeartbeatOutcome, ApplyHeartbeatOutcome, ApplyHeartbeatRules, CompleteValidationRun,
    CreateSourceAccount, CreateSourceAccountId, CreateValidationRun, FieldPatch,
    PatchSourceAggregate, PatchedSourceAggregate, SaveValidationProgress, SourceRepository,
    UpsertFetchRules,
};

#[derive(Default)]
struct State {
    sources: HashMap<Uuid, SourceAccount>,
    ids: HashMap<Uuid, SourceAccountId>,
    /// (id_type, id_value) → source_account_id 记录 id
    by_strong: HashMap<(String, String), Uuid>,
    runs: HashMap<Uuid, SourceValidationRun>,
    runs_by_hash: HashMap<String, Uuid>,
    rules: HashMap<Uuid, AccountFetchRules>,
    heartbeat_events: Vec<SourceHeartbeatEvent>,
}

#[derive(Default, Clone)]
pub struct MemorySourceStore {
    state: Arc<RwLock<State>>,
}

impl MemorySourceStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SourceRepository for MemorySourceStore {
    async fn insert_source(&self, cmd: CreateSourceAccount) -> Result<SourceAccount, String> {
        let mut s = self.state.write().await;
        if s.sources.contains_key(&cmd.id) {
            return Err("source.conflict: 监测公众号已存在".into());
        }
        let account = SourceAccount {
            id: cmd.id,
            display_name: cmd.display_name,
            status: cmd.status,
            preferred_connector: cmd.preferred_connector,
            added_at: cmd.now,
            last_fetch_at: None,
            created_at: cmd.now,
            updated_at: cmd.now,
            version: 1,
        };
        s.sources.insert(account.id, account.clone());
        Ok(account)
    }

    async fn get_source(&self, id: Uuid) -> Result<Option<SourceAccount>, String> {
        let s = self.state.read().await;
        Ok(s.sources.get(&id).cloned())
    }

    async fn update_source_status(
        &self,
        id: Uuid,
        status: SourceStatus,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<SourceAccount, String> {
        let mut s = self.state.write().await;
        let account = s
            .sources
            .get_mut(&id)
            .ok_or_else(|| "source.not_found: 监测公众号不存在".to_string())?;
        let next = account.status.transition(status)?;
        account.status = next;
        account.updated_at = now;
        account.version += 1;
        Ok(account.clone())
    }

    async fn patch_source(
        &self,
        id: Uuid,
        display_name: Option<String>,
        preferred_connector: Option<Option<String>>,
        status: Option<SourceStatus>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<SourceAccount, String> {
        let mut s = self.state.write().await;
        let account = s
            .sources
            .get_mut(&id)
            .ok_or_else(|| "source.not_found: 监测公众号不存在".to_string())?;
        if let Some(name) = display_name {
            account.display_name = name;
        }
        if let Some(conn) = preferred_connector {
            account.preferred_connector = conn;
        }
        if let Some(st) = status {
            account.status = account.status.transition(st)?;
        }
        account.updated_at = now;
        account.version += 1;
        Ok(account.clone())
    }

    async fn patch_source_aggregate(
        &self,
        cmd: PatchSourceAggregate,
    ) -> Result<PatchedSourceAggregate, String> {
        let mut s = self.state.write().await;
        let mut account = s
            .sources
            .get(&cmd.source_account_id)
            .cloned()
            .ok_or_else(|| "source.not_found: source account not found".to_string())?;
        if account.version != cmd.expected_version {
            return Err("source.version_conflict: source account was modified".into());
        }
        let mut rules = s
            .rules
            .get(&cmd.source_account_id)
            .cloned()
            .ok_or_else(|| "source.fetch_rules_not_found: fetch rules not found".to_string())?;
        let previous_account = account.clone();
        let previous_rules = rules.clone();
        if let Some(value) = cmd.display_name {
            account.display_name = value;
        }
        match cmd.preferred_connector {
            FieldPatch::Keep => {}
            FieldPatch::Set(value) => account.preferred_connector = Some(value),
            FieldPatch::Clear => account.preferred_connector = None,
        }
        if let Some(value) = cmd.status {
            account.status = account.status.transition(value)?;
        }
        if let Some(value) = cmd.base_level {
            rules.base_level = value;
        }
        if let Some(value) = cmd.working_windows {
            rules.working_windows = value;
        }
        match cmd.connector_override {
            FieldPatch::Keep => {}
            FieldPatch::Set(value) => rules.connector_override = Some(value),
            FieldPatch::Clear => rules.connector_override = None,
        }
        match cmd.interval_override_hours {
            FieldPatch::Keep => {}
            FieldPatch::Set(value) => rules.interval_override_hours = Some(value),
            FieldPatch::Clear => rules.interval_override_hours = None,
        }
        if let Some(value) = cmd.enabled {
            rules.enabled = value;
        }
        match cmd.manual_next_heartbeat_at {
            FieldPatch::Keep => {
                if let Some(value) = cmd.next_heartbeat_at {
                    rules.next_heartbeat_at = value;
                    rules.manual_next_heartbeat_at = Some(value);
                }
            }
            FieldPatch::Set(value) => {
                rules.next_heartbeat_at = value;
                rules.manual_next_heartbeat_at = Some(value);
            }
            FieldPatch::Clear => {
                rules.manual_next_heartbeat_at = None;
            }
        }
        if let Some(value) = cmd.auto_promotion_enabled {
            rules.auto_promotion_enabled = value;
        }
        account.version += 1;
        account.updated_at = cmd.now;
        rules.updated_at = cmd.now;
        s.sources.insert(account.id, account.clone());
        s.rules.insert(account.id, rules.clone());
        Ok(PatchedSourceAggregate {
            previous_account,
            previous_rules,
            account,
            rules,
        })
    }

    async fn list_sources(&self) -> Result<Vec<SourceAccount>, String> {
        let s = self.state.read().await;
        let mut list: Vec<_> = s.sources.values().cloned().collect();
        list.sort_by_key(|source| std::cmp::Reverse(source.added_at));
        Ok(list)
    }

    async fn find_by_strong_id(
        &self,
        id_type: SourceIdType,
        id_value: &str,
    ) -> Result<Option<(SourceAccount, SourceAccountId)>, String> {
        let s = self.state.read().await;
        let key = (id_type.as_str().to_string(), id_value.to_string());
        let Some(rec_id) = s.by_strong.get(&key).copied() else {
            return Ok(None);
        };
        let Some(sid) = s.ids.get(&rec_id).cloned() else {
            return Ok(None);
        };
        let Some(account) = s.sources.get(&sid.source_account_id).cloned() else {
            return Ok(None);
        };
        Ok(Some((account, sid)))
    }

    async fn insert_source_id(
        &self,
        cmd: CreateSourceAccountId,
    ) -> Result<SourceAccountId, String> {
        let mut s = self.state.write().await;
        if !s.sources.contains_key(&cmd.source_account_id) {
            return Err("source.not_found: 监测公众号不存在".into());
        }
        let key = (cmd.id_type.as_str().to_string(), cmd.id_value.clone());
        if s.by_strong.contains_key(&key) {
            return Err("source.strong_id_conflict: 强标识已被占用".into());
        }
        let rec = SourceAccountId {
            id: cmd.id,
            source_account_id: cmd.source_account_id,
            id_type: cmd.id_type,
            id_value: cmd.id_value,
            confidence: cmd.confidence,
        };
        s.by_strong.insert(key, rec.id);
        s.ids.insert(rec.id, rec.clone());
        Ok(rec)
    }

    async fn list_source_ids(
        &self,
        source_account_id: Uuid,
    ) -> Result<Vec<SourceAccountId>, String> {
        let s = self.state.read().await;
        Ok(s.ids
            .values()
            .filter(|i| i.source_account_id == source_account_id)
            .cloned()
            .collect())
    }

    async fn insert_validation_run(
        &self,
        cmd: CreateValidationRun,
    ) -> Result<SourceValidationRun, String> {
        let mut s = self.state.write().await;
        if let Some(existing) = s.runs_by_hash.get(&cmd.input_hash).copied()
            && let Some(run) = s.runs.get(&existing)
        {
            return Ok(run.clone());
        }
        let run = SourceValidationRun {
            id: cmd.id,
            input_url: cmd.input_url,
            input_hash: cmd.input_hash.clone(),
            parsed_source_json: cmd.parsed_source_json,
            connector_account_id: cmd.connector_account_id,
            status: ValidationRunStatus::Running,
            candidate_count: 0,
            successful_count: 0,
            available_count: 0,
            missing_reason: None,
            candidates: vec![],
            source_account_id: None,
            expires_at: cmd.expires_at,
            created_at: cmd.now,
            completed_at: None,
        };
        s.runs_by_hash.insert(cmd.input_hash, run.id);
        s.runs.insert(run.id, run.clone());
        Ok(run)
    }

    async fn get_validation_run(&self, id: Uuid) -> Result<Option<SourceValidationRun>, String> {
        let s = self.state.read().await;
        Ok(s.runs.get(&id).cloned())
    }

    async fn get_validation_run_by_hash(
        &self,
        input_hash: &str,
    ) -> Result<Option<SourceValidationRun>, String> {
        let s = self.state.read().await;
        Ok(s.runs_by_hash
            .get(input_hash)
            .and_then(|id| s.runs.get(id).cloned()))
    }

    async fn complete_validation_run(
        &self,
        cmd: CompleteValidationRun,
    ) -> Result<SourceValidationRun, String> {
        let mut s = self.state.write().await;
        let run = s
            .runs
            .get_mut(&cmd.run_id)
            .ok_or_else(|| "source.validation_not_found: 验证 run 不存在".to_string())?;
        run.status = cmd.status;
        run.candidate_count = cmd.candidate_count;
        run.successful_count = cmd.successful_count;
        run.available_count = cmd.available_count;
        run.missing_reason = cmd.missing_reason;
        run.candidates = cmd.candidates;
        run.source_account_id = cmd.source_account_id;
        run.completed_at = Some(cmd.completed_at);
        Ok(run.clone())
    }

    async fn save_validation_progress(
        &self,
        cmd: SaveValidationProgress,
    ) -> Result<SourceValidationRun, String> {
        let mut s = self.state.write().await;
        let run = s
            .runs
            .get_mut(&cmd.run_id)
            .ok_or_else(|| "source.validation_not_found: 验证 run 不存在".to_string())?;
        if run.status != ValidationRunStatus::Running {
            return Ok(run.clone());
        }
        run.candidate_count = cmd.candidate_count;
        run.available_count = cmd.available_count;
        run.missing_reason = cmd.missing_reason;
        run.candidates = cmd.candidates;
        run.source_account_id = Some(cmd.source_account_id);
        Ok(run.clone())
    }

    async fn upsert_fetch_rules(&self, cmd: UpsertFetchRules) -> Result<AccountFetchRules, String> {
        let mut s = self.state.write().await;
        if !s.sources.contains_key(&cmd.source_account_id) {
            return Err("source.not_found: 监测公众号不存在".into());
        }
        let rules = AccountFetchRules {
            source_account_id: cmd.source_account_id,
            base_level: cmd.base_level,
            connector_override: cmd.connector_override,
            enabled: true,
            first_heartbeat_at: cmd.first_heartbeat_at,
            next_heartbeat_at: cmd.next_heartbeat_at,
            working_windows: cmd.working_windows,
            interval_override_hours: None,
            consecutive_empty_fetches: 0,
            consecutive_technical_failures: 0,
            low_activity: false,
            promotion_until: None,
            manual_next_heartbeat_at: None,
            auto_promotion_enabled: true,
            updated_at: cmd.now,
        };
        s.rules.insert(cmd.source_account_id, rules.clone());
        Ok(rules)
    }

    async fn get_fetch_rules(
        &self,
        source_account_id: Uuid,
    ) -> Result<Option<AccountFetchRules>, String> {
        let s = self.state.read().await;
        Ok(s.rules.get(&source_account_id).cloned())
    }

    async fn apply_heartbeat_rules(
        &self,
        patch: ApplyHeartbeatRules,
    ) -> Result<AccountFetchRules, String> {
        let mut s = self.state.write().await;
        let rules = s
            .rules
            .get_mut(&patch.source_account_id)
            .ok_or_else(|| "source.fetch_rules_not_found: 抓取规则不存在".to_string())?;
        // 禁止改写 first_heartbeat_at
        rules.base_level = patch.base_level;
        rules.next_heartbeat_at = patch.next_heartbeat_at;
        rules.consecutive_empty_fetches = patch.consecutive_empty_fetches;
        rules.consecutive_technical_failures = patch.consecutive_technical_failures;
        rules.low_activity = patch.low_activity;
        rules.promotion_until = patch.promotion_until;
        rules.manual_next_heartbeat_at = patch.manual_next_heartbeat_at;
        rules.updated_at = patch.now;
        let result = rules.clone();
        let source = s
            .sources
            .get_mut(&patch.source_account_id)
            .ok_or_else(|| "source.not_found: source account not found".to_string())?;
        source.version += 1;
        source.updated_at = patch.now;
        Ok(result)
    }

    async fn apply_heartbeat_outcome(
        &self,
        cmd: ApplyHeartbeatOutcome,
    ) -> Result<AppliedHeartbeatOutcome, String> {
        let mut s = self.state.write().await;

        // The event key is scoped to one source.  Replays return the original
        // event without touching counters or source version.
        if let Some(existing) = s
            .heartbeat_events
            .iter()
            .find(|event| {
                event.source_account_id == cmd.source_account_id
                    && event.event_key == cmd.event.event_key
            })
            .cloned()
        {
            if existing.outcome != cmd.event.outcome {
                return Err("source.heartbeat_event_conflict: event key already used".into());
            }
            let rules = s
                .rules
                .get(&cmd.source_account_id)
                .cloned()
                .ok_or_else(|| "source.fetch_rules_not_found: fetch rules not found".to_string())?;
            return Ok(AppliedHeartbeatOutcome {
                rules,
                event: existing,
            });
        }

        let mut source = s
            .sources
            .get(&cmd.source_account_id)
            .cloned()
            .ok_or_else(|| "source.not_found: source account not found".to_string())?;
        if source.version != cmd.expected_version {
            return Err("source.version_conflict: source account was modified".into());
        }
        let mut rules = s
            .rules
            .get(&cmd.source_account_id)
            .cloned()
            .ok_or_else(|| "source.fetch_rules_not_found: fetch rules not found".to_string())?;

        // Validate the status transition before mutating either aggregate so
        // a rejected transition cannot leave a partially applied heartbeat.
        let next_status = match cmd.source_status {
            Some(status) => Some(source.status.transition(status)?),
            None => None,
        };
        rules.base_level = cmd.rules.base_level;
        rules.next_heartbeat_at = cmd.rules.next_heartbeat_at;
        rules.consecutive_empty_fetches = cmd.rules.consecutive_empty_fetches;
        rules.consecutive_technical_failures = cmd.rules.consecutive_technical_failures;
        rules.low_activity = cmd.rules.low_activity;
        rules.promotion_until = cmd.rules.promotion_until;
        rules.manual_next_heartbeat_at = cmd.rules.manual_next_heartbeat_at;
        rules.updated_at = cmd.rules.now;
        if let Some(status) = next_status {
            source.status = status;
        }
        source.version += 1;
        source.updated_at = cmd.rules.now;
        s.sources.insert(source.id, source);
        s.rules.insert(rules.source_account_id, rules.clone());
        s.heartbeat_events.push(cmd.event.clone());
        Ok(AppliedHeartbeatOutcome {
            rules,
            event: cmd.event,
        })
    }

    async fn insert_heartbeat_event(
        &self,
        event: SourceHeartbeatEvent,
    ) -> Result<SourceHeartbeatEvent, String> {
        let mut s = self.state.write().await;
        if !s.sources.contains_key(&event.source_account_id) {
            return Err("source.not_found: 监测公众号不存在".into());
        }
        s.heartbeat_events.push(event.clone());
        Ok(event)
    }

    async fn list_heartbeat_events(
        &self,
        source_account_id: Uuid,
        limit: usize,
    ) -> Result<Vec<SourceHeartbeatEvent>, String> {
        let s = self.state.read().await;
        let mut events: Vec<_> = s
            .heartbeat_events
            .iter()
            .filter(|e| e.source_account_id == source_account_id)
            .cloned()
            .collect();
        events.sort_by_key(|event| std::cmp::Reverse(event.created_at));
        events.truncate(limit);
        Ok(events)
    }
}
