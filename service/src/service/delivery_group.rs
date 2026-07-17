//! DeliveryGroupService：分组 CRUD 与 source 绑定（不改抓取规则、不复制文章）。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use server_core::{AppError, Clock};
use uuid::Uuid;

use crate::audit::{ACTOR_SYSTEM_ADMINISTRATOR, AuditWriter};
use crate::domain::delivery::{DeliveryGroup, DeliveryGroupPublic, DeliveryGroupStatus};
use crate::repository::delivery::{
    CreateDeliveryGroup, CreateDeliveryGroupWithSources, DeliveryRepository, PatchDeliveryGroup,
    PatchDeliveryGroupWithSources,
};
use crate::repository::source::SourceRepository;

pub struct DeliveryGroupService {
    delivery: Arc<dyn DeliveryRepository>,
    sources: Arc<dyn SourceRepository>,
    clock: Arc<dyn Clock>,
    audit: Option<AuditWriter>,
}

#[derive(Debug, Clone)]
pub struct CreateGroupCommand {
    pub name: String,
    pub description: Option<String>,
    pub source_account_ids: Vec<Uuid>,
}

#[derive(Debug, Clone)]
pub struct PatchGroupCommand {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub status: Option<DeliveryGroupStatus>,
    pub source_account_ids: Option<Vec<Uuid>>,
}

impl DeliveryGroupService {
    pub fn new(
        delivery: Arc<dyn DeliveryRepository>,
        sources: Arc<dyn SourceRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            delivery,
            sources,
            clock,
            audit: None,
        }
    }

    pub fn with_audit(mut self, audit: AuditWriter) -> Self {
        self.audit = Some(audit);
        self
    }

    pub fn delivery_repo(&self) -> Arc<dyn DeliveryRepository> {
        self.delivery.clone()
    }

    fn now(&self) -> DateTime<Utc> {
        DateTime::<Utc>::from(self.clock.now())
    }

    async fn ensure_sources_exist(&self, ids: &[Uuid]) -> Result<(), AppError> {
        for id in ids {
            let found = self
                .sources
                .get_source(*id)
                .await
                .map_err(AppError::internal)?;
            if found.is_none() {
                return Err(AppError::not_found(
                    "source.not_found",
                    format!("监测公众号不存在: {id}"),
                ));
            }
        }
        Ok(())
    }

    async fn to_public(&self, g: DeliveryGroup) -> Result<DeliveryGroupPublic, AppError> {
        let bindings = self
            .delivery
            .list_group_sources(g.id)
            .await
            .map_err(AppError::internal)?;
        Ok(DeliveryGroupPublic {
            id: g.id,
            name: g.name,
            description: g.description,
            status: g.status,
            source_account_ids: bindings.into_iter().map(|b| b.source_account_id).collect(),
            created_at: g.created_at,
            updated_at: g.updated_at,
        })
    }

    pub async fn create(&self, cmd: CreateGroupCommand) -> Result<DeliveryGroupPublic, AppError> {
        let name = cmd.name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::new("delivery.invalid_name", "分组名称不能为空"));
        }
        self.ensure_sources_exist(&cmd.source_account_ids).await?;
        let now = self.now();
        let g = self
            .delivery
            .create_group_with_sources(CreateDeliveryGroupWithSources {
                group: CreateDeliveryGroup {
                    id: Uuid::new_v4(),
                    name,
                    description: cmd.description,
                    now,
                },
                source_ids: cmd.source_account_ids.clone(),
            })
            .await
            .map_err(|e| {
                if e.starts_with("delivery.group_name_conflict") {
                    AppError::new("delivery.group_name_conflict", "分组名称已存在")
                        .with_status(axum::http::StatusCode::CONFLICT)
                } else {
                    AppError::internal(e)
                }
            })?;
        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "delivery.group_changed",
                    Some("delivery_group"),
                    Some(&g.id.to_string()),
                    json!({
                        "op": "created",
                        "source_count": cmd.source_account_ids.len(),
                        "status": g.status.as_str(),
                    }),
                    now,
                )
                .await;
        }
        self.to_public(g).await
    }

    pub async fn get(&self, id: Uuid) -> Result<DeliveryGroupPublic, AppError> {
        let g = self
            .delivery
            .get_group(id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("delivery.group_not_found", "分组不存在"))?;
        self.to_public(g).await
    }

    pub async fn list(&self) -> Result<Vec<DeliveryGroupPublic>, AppError> {
        let groups = self
            .delivery
            .list_groups()
            .await
            .map_err(AppError::internal)?;
        let mut out = Vec::with_capacity(groups.len());
        for g in groups {
            out.push(self.to_public(g).await?);
        }
        Ok(out)
    }

    pub async fn patch(
        &self,
        id: Uuid,
        cmd: PatchGroupCommand,
    ) -> Result<DeliveryGroupPublic, AppError> {
        if let Some(ref ids) = cmd.source_account_ids {
            self.ensure_sources_exist(ids).await?;
        }
        let now = self.now();
        let g = self
            .delivery
            .patch_group_with_sources(PatchDeliveryGroupWithSources {
                group_id: id,
                group: PatchDeliveryGroup {
                    name: cmd.name.map(|n| n.trim().to_string()),
                    description: cmd.description,
                    status: cmd.status,
                    now,
                },
                source_ids: cmd.source_account_ids.clone(),
            })
            .await
            .map_err(|e| {
                if e.starts_with("delivery.group_not_found") {
                    AppError::not_found("delivery.group_not_found", "分组不存在")
                } else if e.starts_with("delivery.group_name_conflict") {
                    AppError::new("delivery.group_name_conflict", "分组名称已存在")
                        .with_status(axum::http::StatusCode::CONFLICT)
                } else {
                    AppError::internal(e)
                }
            })?;
        if let Some(ids) = cmd.source_account_ids {
            if let Some(ref audit) = self.audit {
                let _ = audit
                    .success(
                        ACTOR_SYSTEM_ADMINISTRATOR,
                        None,
                        "delivery.group_changed",
                        Some("delivery_group"),
                        Some(&id.to_string()),
                        json!({
                            "op": "patched",
                            "source_count": ids.len(),
                            "status": g.status.as_str(),
                        }),
                        now,
                    )
                    .await;
            }
        } else if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "delivery.group_changed",
                    Some("delivery_group"),
                    Some(&id.to_string()),
                    json!({
                        "op": "patched",
                        "status": g.status.as_str(),
                    }),
                    now,
                )
                .await;
        }
        self.to_public(g).await
    }

    pub async fn list_source_ids(&self, group_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        let bindings = self
            .delivery
            .list_group_sources(group_id)
            .await
            .map_err(AppError::internal)?;
        Ok(bindings.into_iter().map(|b| b.source_account_id).collect())
    }
}
