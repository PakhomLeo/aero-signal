//! 调度与内容阈值系统设置。

use serde::{Deserialize, Serialize};

/// 工作时间窗口（本地时区墙钟）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingWindow {
    pub start: String,
    pub end: String,
}

/// `system_settings` 中的调度相关字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleSettings {
    pub timezone: String,
    /// 每日 preflight，默认 07:30。
    pub preflight_time: String,
    /// 每日计划起始，默认 08:00。
    pub plan_start_time: String,
    pub working_windows: Vec<WorkingWindow>,
    /// 正文图片最小边长（px），默认 80。
    pub image_min_side_px: i32,
    /// 正文图片最小面积（px²），默认 14400。
    pub image_min_area_px: i32,
    /// OCR 最低置信度，默认 0.7。
    pub ocr_min_confidence: f64,
}

impl Default for ScheduleSettings {
    fn default() -> Self {
        Self {
            timezone: "Asia/Shanghai".into(),
            preflight_time: "07:30".into(),
            plan_start_time: "08:00".into(),
            working_windows: vec![WorkingWindow {
                start: "09:00".into(),
                end: "18:00".into(),
            }],
            image_min_side_px: 80,
            image_min_area_px: 14400,
            ocr_min_confidence: 0.7,
        }
    }
}

impl ScheduleSettings {
    /// 校验 HH:MM 与阈值范围。
    pub fn validate(&self) -> Result<(), String> {
        validate_hhmm("preflight_time", &self.preflight_time)?;
        validate_hhmm("plan_start_time", &self.plan_start_time)?;
        if self.timezone.trim().is_empty() {
            return Err("timezone 不能为空".into());
        }
        if self.working_windows.is_empty() {
            return Err("working_windows 至少需要一个窗口".into());
        }
        for (i, w) in self.working_windows.iter().enumerate() {
            validate_hhmm(&format!("working_windows[{i}].start"), &w.start)?;
            validate_hhmm(&format!("working_windows[{i}].end"), &w.end)?;
        }
        if self.image_min_side_px < 1 {
            return Err("image_min_side_px 必须 >= 1".into());
        }
        if self.image_min_area_px < 1 {
            return Err("image_min_area_px 必须 >= 1".into());
        }
        if !(0.0..=1.0).contains(&self.ocr_min_confidence) {
            return Err("ocr_min_confidence 必须在 0..=1".into());
        }
        Ok(())
    }
}

fn validate_hhmm(field: &str, value: &str) -> Result<(), String> {
    let parts: Vec<_> = value.split(':').collect();
    if parts.len() != 2 {
        return Err(format!("{field} 必须为 HH:MM"));
    }
    let h: u32 = parts[0].parse().map_err(|_| format!("{field} 小时无效"))?;
    let m: u32 = parts[1].parse().map_err(|_| format!("{field} 分钟无效"))?;
    if h > 23 || m > 59 {
        return Err(format!("{field} 超出有效时间范围"));
    }
    if parts[0].len() != 2 || parts[1].len() != 2 {
        return Err(format!("{field} 必须为两位 HH:MM"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_valid() {
        ScheduleSettings::default().validate().unwrap();
    }

    #[test]
    fn rejects_bad_time() {
        let s = ScheduleSettings {
            preflight_time: "7:30".into(),
            ..ScheduleSettings::default()
        };
        assert!(s.validate().is_err());
    }
}
