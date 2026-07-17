//! 进度辅助。

use crate::handler::{ProgressSender, ProgressUpdate};

/// 发送进度更新。
pub fn send_progress(tx: &ProgressSender, percent: f64, message: &str, stage: &str) {
    let _ = tx.send(ProgressUpdate::Progress {
        percent,
        message: message.to_string(),
        stage: stage.to_string(),
    });
}

/// 发送结果。
pub fn send_result(tx: &ProgressSender, result: serde_json::Value) {
    let _ = tx.send(ProgressUpdate::Result(result));
}
