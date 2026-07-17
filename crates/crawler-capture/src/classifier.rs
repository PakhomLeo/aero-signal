//! ImageClassifier：kept / decorative / too_small / hidden / duplicate / unsupported。

use std::collections::HashSet;

use sha2::{Digest, Sha256};

use crate::types::{ImageCandidate, ImageDecision, ImageDecisionKind, ImageThresholds};

#[derive(Debug, Clone)]
pub struct MeasuredImage {
    pub width: u32,
    pub height: u32,
    pub mime_type: Option<String>,
    pub bytes: Vec<u8>,
}

pub struct ImageClassifier {
    thresholds: ImageThresholds,
}

impl Default for ImageClassifier {
    fn default() -> Self {
        Self::new(ImageThresholds::default())
    }
}

impl ImageClassifier {
    pub fn new(thresholds: ImageThresholds) -> Self {
        Self { thresholds }
    }

    /// 对候选逐条决策；`measured` 按 URL 提供下载后实测尺寸（可选）。
    pub fn classify(
        &self,
        candidates: &[ImageCandidate],
        measured: &[(String, MeasuredImage)],
    ) -> Vec<ImageDecision> {
        let mut seen_hashes: HashSet<String> = HashSet::new();
        let mut seen_urls: HashSet<String> = HashSet::new();
        let mut out = Vec::with_capacity(candidates.len());

        for c in candidates {
            let m = measured.iter().find(|(u, _)| u == &c.url).map(|(_, m)| m);
            out.push(self.decide_one(c, m, &mut seen_hashes, &mut seen_urls));
        }
        out
    }

    fn decide_one(
        &self,
        c: &ImageCandidate,
        measured: Option<&MeasuredImage>,
        seen_hashes: &mut HashSet<String>,
        seen_urls: &mut HashSet<String>,
    ) -> ImageDecision {
        let (w, h) = match measured {
            Some(m) => (Some(m.width), Some(m.height)),
            None => (c.declared_width, c.declared_height),
        };

        if !is_supported_url(&c.url, measured.and_then(|m| m.mime_type.as_deref())) {
            return decision(
                c,
                ImageDecisionKind::Unsupported,
                w,
                h,
                json_reason("unsupported_format", &self.thresholds, w, h),
            );
        }

        if c.hidden {
            return decision(
                c,
                ImageDecisionKind::Hidden,
                w,
                h,
                json_reason("hidden_in_dom", &self.thresholds, w, h),
            );
        }

        if c.decorative_hint || !c.in_body {
            return decision(
                c,
                ImageDecisionKind::Decorative,
                w,
                h,
                json_reason(
                    if !c.in_body {
                        "outside_body_dom"
                    } else {
                        "decorative_hint"
                    },
                    &self.thresholds,
                    w,
                    h,
                ),
            );
        }

        if let (Some(width), Some(height)) = (w, h) {
            let min_side = width.min(height);
            let area = width.saturating_mul(height);
            if min_side < self.thresholds.min_side_px || area < self.thresholds.min_area_px {
                return decision(
                    c,
                    ImageDecisionKind::TooSmall,
                    w,
                    h,
                    serde_json::json!({
                        "reason": "below_threshold",
                        "min_side_px": self.thresholds.min_side_px,
                        "min_area_px": self.thresholds.min_area_px,
                        "measured_min_side": min_side,
                        "measured_area": area,
                    }),
                );
            }
        }

        if !seen_urls.insert(c.url.clone()) {
            return decision(
                c,
                ImageDecisionKind::Duplicate,
                w,
                h,
                json_reason("duplicate_url", &self.thresholds, w, h),
            );
        }

        if let Some(m) = measured {
            let hash = hex_sha256(&m.bytes);
            if !seen_hashes.insert(hash) {
                return decision(
                    c,
                    ImageDecisionKind::Duplicate,
                    w,
                    h,
                    json_reason("duplicate_content_hash", &self.thresholds, w, h),
                );
            }
        }

        decision(
            c,
            ImageDecisionKind::Kept,
            w,
            h,
            json_reason("passed_filters", &self.thresholds, w, h),
        )
    }
}

fn decision(
    c: &ImageCandidate,
    kind: ImageDecisionKind,
    w: Option<u32>,
    h: Option<u32>,
    reason: serde_json::Value,
) -> ImageDecision {
    ImageDecision {
        candidate_url: c.url.clone(),
        decision: kind,
        measured_width: w,
        measured_height: h,
        reason_json: reason,
    }
}

fn json_reason(
    reason: &str,
    t: &ImageThresholds,
    w: Option<u32>,
    h: Option<u32>,
) -> serde_json::Value {
    serde_json::json!({
        "reason": reason,
        "min_side_px": t.min_side_px,
        "min_area_px": t.min_area_px,
        "measured_width": w,
        "measured_height": h,
    })
}

fn is_supported_url(url: &str, mime: Option<&str>) -> bool {
    if let Some(m) = mime {
        let m = m.to_ascii_lowercase();
        return m.starts_with("image/") && !m.contains("svg");
    }
    let lower = url.to_ascii_lowercase();
    if lower.contains(".svg") || lower.starts_with("data:image/svg") {
        return false;
    }
    if lower.ends_with(".gif") && (lower.contains("track") || lower.contains("pixel")) {
        return false;
    }
    true
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(url: &str, in_body: bool, w: u32, h: u32) -> ImageCandidate {
        ImageCandidate {
            url: url.into(),
            alt: None,
            in_body,
            hidden: false,
            lazy: false,
            decorative_hint: false,
            declared_width: Some(w),
            declared_height: Some(h),
        }
    }

    #[test]
    fn filters_too_small_and_decorative() {
        let clf = ImageClassifier::default();
        let candidates = vec![
            cand("https://cdn/body.jpg", true, 400, 300),
            cand("https://cdn/tiny.png", true, 16, 16),
            ImageCandidate {
                url: "https://cdn/logo.png".into(),
                alt: None,
                in_body: false,
                hidden: false,
                lazy: false,
                decorative_hint: true,
                declared_width: Some(200),
                declared_height: Some(200),
            },
            ImageCandidate {
                url: "https://cdn/hide.gif".into(),
                alt: None,
                in_body: true,
                hidden: true,
                lazy: false,
                decorative_hint: false,
                declared_width: Some(100),
                declared_height: Some(100),
            },
        ];
        let decisions = clf.classify(&candidates, &[]);
        assert_eq!(decisions[0].decision, ImageDecisionKind::Kept);
        assert_eq!(decisions[1].decision, ImageDecisionKind::TooSmall);
        assert_eq!(decisions[2].decision, ImageDecisionKind::Decorative);
        assert_eq!(decisions[3].decision, ImageDecisionKind::Hidden);
    }

    #[test]
    fn detects_duplicate_by_hash() {
        let clf = ImageClassifier::default();
        let bytes = vec![1, 2, 3, 4];
        let candidates = vec![
            cand("https://cdn/a.jpg", true, 200, 200),
            cand("https://cdn/b.jpg", true, 200, 200),
        ];
        let measured = vec![
            (
                "https://cdn/a.jpg".into(),
                MeasuredImage {
                    width: 200,
                    height: 200,
                    mime_type: Some("image/jpeg".into()),
                    bytes: bytes.clone(),
                },
            ),
            (
                "https://cdn/b.jpg".into(),
                MeasuredImage {
                    width: 200,
                    height: 200,
                    mime_type: Some("image/jpeg".into()),
                    bytes,
                },
            ),
        ];
        let decisions = clf.classify(&candidates, &measured);
        assert_eq!(decisions[0].decision, ImageDecisionKind::Kept);
        assert_eq!(decisions[1].decision, ImageDecisionKind::Duplicate);
    }

    #[test]
    fn measured_dimensions_override_large_html_declaration() {
        let clf = ImageClassifier::default();
        let candidates = vec![cand("https://cdn/image.jpg", true, 1200, 900)];
        let measured = vec![(
            candidates[0].url.clone(),
            MeasuredImage {
                width: 20,
                height: 20,
                mime_type: Some("image/png".into()),
                bytes: vec![1, 2, 3],
            },
        )];
        let decisions = clf.classify(&candidates, &measured);
        assert_eq!(decisions[0].decision, ImageDecisionKind::TooSmall);
        assert_eq!(decisions[0].measured_width, Some(20));
        assert_eq!(decisions[0].measured_height, Some(20));
    }
}
