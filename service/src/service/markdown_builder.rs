//! MarkdownBuilder：正文 block AST → Markdown；受控图片引用 + OCR anchor。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 正文块 AST（最小集）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Paragraph {
        text: String,
    },
    Heading {
        level: u8,
        text: String,
    },
    Image {
        asset_id: Uuid,
        storage_key: String,
        alt: Option<String>,
        /// 稳定 OCR 回填锚点，如 `img:asset:<uuid>`。
        anchor: String,
    },
    /// OCR 回填块（低置信度须标明）。
    OcrText {
        anchor: String,
        text: String,
        confidence: Option<String>,
        low_confidence: bool,
    },
}

pub struct MarkdownBuilder;

impl MarkdownBuilder {
    pub fn new() -> Self {
        Self
    }

    /// 从段落文本与保留图片资产构建 AST。
    /// `body_paragraphs` 为纯文本段；图片按给定顺序插入（通常在段落后）。
    pub fn build_blocks(
        &self,
        body_paragraphs: &[String],
        images: &[(Uuid, String)],
    ) -> Vec<ContentBlock> {
        let mut blocks = Vec::new();
        for p in body_paragraphs {
            let t = p.trim();
            if !t.is_empty() {
                blocks.push(ContentBlock::Paragraph {
                    text: t.to_string(),
                });
            }
        }
        for (asset_id, storage_key) in images {
            let anchor = Self::image_anchor(*asset_id);
            blocks.push(ContentBlock::Image {
                asset_id: *asset_id,
                storage_key: storage_key.clone(),
                alt: None,
                anchor,
            });
        }
        blocks
    }

    pub fn image_anchor(asset_id: Uuid) -> String {
        format!("img:asset:{asset_id}")
    }

    /// 将 OCR 结果按 anchor 插入到对应图片块之后。
    pub fn insert_ocr(
        &self,
        blocks: &mut Vec<ContentBlock>,
        anchor: &str,
        text: &str,
        confidence: Option<f64>,
        low_confidence_threshold: f64,
    ) {
        // 先移除同 anchor 旧 OCR 块，保证重跑幂等。
        blocks.retain(|b| {
            !matches!(
                b,
                ContentBlock::OcrText {
                    anchor: a,
                    ..
                } if a == anchor
            )
        });

        let low = confidence
            .map(|c| c < low_confidence_threshold)
            .unwrap_or(false);
        let conf_str = confidence.map(|c| format!("{c:.4}"));
        let ocr = ContentBlock::OcrText {
            anchor: anchor.to_string(),
            text: text.to_string(),
            confidence: conf_str,
            low_confidence: low,
        };

        if let Some(idx) = blocks.iter().position(|b| {
            matches!(
                b,
                ContentBlock::Image {
                    anchor: a,
                    ..
                } if a == anchor
            )
        }) {
            blocks.insert(idx + 1, ocr);
        } else {
            blocks.push(ocr);
        }
    }

    /// 渲染为对外唯一正文 Markdown。
    pub fn render(&self, blocks: &[ContentBlock]) -> String {
        let mut out = String::new();
        for b in blocks {
            match b {
                ContentBlock::Paragraph { text } => {
                    out.push_str(text);
                    out.push_str("\n\n");
                }
                ContentBlock::Heading { level, text } => {
                    let hashes = "#".repeat((*level).clamp(1, 6) as usize);
                    out.push_str(&format!("{hashes} {text}\n\n"));
                }
                ContentBlock::Image {
                    storage_key,
                    alt,
                    anchor,
                    ..
                } => {
                    let alt_t = alt.as_deref().unwrap_or("");
                    // 受控本地引用：signal-asset:// + storage_key；anchor 以 HTML 注释保留。
                    out.push_str(&format!(
                        "<!-- ocr-anchor:{anchor} -->\n![{alt_t}](signal-asset://{storage_key})\n\n"
                    ));
                }
                ContentBlock::OcrText {
                    text,
                    low_confidence,
                    confidence,
                    anchor,
                } => {
                    out.push_str(&format!("<!-- ocr-block:{anchor} -->\n"));
                    if *low_confidence {
                        let c = confidence.as_deref().unwrap_or("unknown");
                        out.push_str(&format!(
                            "> **[图片文字·低置信度 {c}]**\n>\n> {}\n\n",
                            text.replace('\n', "\n> ")
                        ));
                    } else {
                        out.push_str(&format!(
                            "> **[图片文字]**\n>\n> {}\n\n",
                            text.replace('\n', "\n> ")
                        ));
                    }
                }
            }
        }
        out.trim().to_string()
    }

    /// 从已有 Markdown（抓取产物）与资产列表重建带图片块的 AST。
    /// 将远程/原始 `![](url)` 替换为受控 asset 引用；无匹配则保留段落。
    pub fn from_captured_markdown(
        &self,
        content_md: &str,
        assets: &[(Uuid, String, Option<String>)],
    ) -> Vec<ContentBlock> {
        // assets: (id, storage_key, original_url)
        let mut url_to_asset: std::collections::HashMap<String, (Uuid, String)> =
            std::collections::HashMap::new();
        for (id, key, url) in assets {
            if let Some(u) = url {
                url_to_asset.insert(u.clone(), (*id, key.clone()));
            }
        }

        let mut blocks = Vec::new();
        let mut used_assets = std::collections::HashSet::new();

        for line in content_md.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(caps) = extract_md_image(trimmed) {
                let (alt, url) = caps;
                if let Some((id, key)) = url_to_asset.get(&url) {
                    used_assets.insert(*id);
                    blocks.push(ContentBlock::Image {
                        asset_id: *id,
                        storage_key: key.clone(),
                        alt: if alt.is_empty() { None } else { Some(alt) },
                        anchor: Self::image_anchor(*id),
                    });
                    continue;
                }
            }
            // 合并连续非空行为段落（简化）
            if let Some(ContentBlock::Paragraph { text }) = blocks.last_mut() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(trimmed);
            } else {
                blocks.push(ContentBlock::Paragraph {
                    text: trimmed.to_string(),
                });
            }
        }

        // 正文中未出现的保留图追加到末尾（保证 OCR planner 有锚点）。
        for (id, key, _) in assets {
            if !used_assets.contains(id) {
                blocks.push(ContentBlock::Image {
                    asset_id: *id,
                    storage_key: key.clone(),
                    alt: None,
                    anchor: Self::image_anchor(*id),
                });
            }
        }

        blocks
    }
}

impl Default for MarkdownBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_md_image(line: &str) -> Option<(String, String)> {
    // ![alt](url)
    let start = line.find("![")?;
    let mid = line[start..].find("](")?;
    let alt_end = start + mid;
    let alt = line[start + 2..alt_end].to_string();
    let url_start = alt_end + 2;
    let url_end = line[url_start..].find(')')? + url_start;
    let url = line[url_start..url_end].to_string();
    Some((alt, url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_anchor_inserts_after_image() {
        let b = MarkdownBuilder::new();
        let asset = Uuid::new_v4();
        let mut blocks = b.build_blocks(
            &["正文第一段".into()],
            &[(asset, "articles/a/img.jpg".into())],
        );
        let anchor = MarkdownBuilder::image_anchor(asset);
        b.insert_ocr(&mut blocks, &anchor, "图上文字", Some(0.95), 0.7);
        let md = b.render(&blocks);
        assert!(md.contains("signal-asset://articles/a/img.jpg"));
        assert!(md.contains("ocr-anchor:"));
        assert!(md.contains("[图片文字]"));
        assert!(md.contains("图上文字"));
        assert!(!md.contains("[图片文字·低置信度"));
    }

    #[test]
    fn low_confidence_marked() {
        let b = MarkdownBuilder::new();
        let asset = Uuid::new_v4();
        let mut blocks = b.build_blocks(&["p".into()], &[(asset, "k".into())]);
        let anchor = MarkdownBuilder::image_anchor(asset);
        b.insert_ocr(&mut blocks, &anchor, "模糊字", Some(0.4), 0.7);
        let md = b.render(&blocks);
        assert!(md.contains("低置信度"));
    }
}
