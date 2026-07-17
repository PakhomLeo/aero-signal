//! 系统设置仓储。

use async_trait::async_trait;

use crate::domain::ScheduleSettings;

#[async_trait]
pub trait SettingsRepository: Send + Sync {
    async fn get_schedule(&self) -> Result<ScheduleSettings, String>;

    async fn update_schedule(&self, settings: &ScheduleSettings) -> Result<(), String>;
}
