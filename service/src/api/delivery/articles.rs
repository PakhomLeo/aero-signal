//! `GET /api/articles`、`/api/articles/{id}`、`/api/articles/since`

use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use server_core::AppError;
use uuid::Uuid;

use super::auth::{DeliveryState, RequireArticlesRead};
use crate::domain::delivery::ApiScope;
use crate::service::article_cursor::{
    DeliveryArticleDetail, DeliveryArticleSummary, SinceResponse,
};

pub fn articles_routes() -> Router<DeliveryState> {
    Router::new()
        .route("/articles", get(list_articles))
        .route("/articles/since", get(articles_since))
        .route("/articles/{id}", get(get_article))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    50
}

#[derive(Debug, Deserialize)]
pub struct SinceQuery {
    /// 上次消费到的 change_sequence（不含）；缺省从 0 起。
    #[serde(default)]
    pub after: i64,
    #[serde(default = "default_since_limit")]
    pub limit: usize,
}

fn default_since_limit() -> usize {
    100
}

async fn list_articles(
    State(state): State<DeliveryState>,
    RequireArticlesRead(auth): RequireArticlesRead,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<DeliveryArticleSummary>>, AppError> {
    let started = Instant::now();
    let result = state
        .articles
        .list_articles(&auth.auth.token, q.limit)
        .await;
    let status = match &result {
        Ok(_) => 200,
        Err(e) => e.status.as_u16() as i32,
    };
    let _ = state
        .tokens
        .log_call(
            &auth.auth,
            "GET",
            "/api/articles",
            Some(ApiScope::ArticlesRead.as_str()),
            status,
            started.elapsed().as_millis() as i32,
            0,
            auth.client_ip.as_deref(),
        )
        .await;
    Ok(Json(result?))
}

async fn get_article(
    State(state): State<DeliveryState>,
    RequireArticlesRead(auth): RequireArticlesRead,
    Path(id): Path<Uuid>,
) -> Result<Json<DeliveryArticleDetail>, AppError> {
    let started = Instant::now();
    let result = state.articles.get_article(&auth.auth.token, id).await;
    let status = match &result {
        Ok(_) => 200,
        Err(e) => e.status.as_u16() as i32,
    };
    let _ = state
        .tokens
        .log_call(
            &auth.auth,
            "GET",
            &format!("/api/articles/{id}"),
            Some(ApiScope::ArticlesRead.as_str()),
            status,
            started.elapsed().as_millis() as i32,
            0,
            auth.client_ip.as_deref(),
        )
        .await;
    Ok(Json(result?))
}

async fn articles_since(
    State(state): State<DeliveryState>,
    RequireArticlesRead(auth): RequireArticlesRead,
    Query(q): Query<SinceQuery>,
) -> Result<Json<SinceResponse>, AppError> {
    let started = Instant::now();
    let result = state
        .articles
        .since(&auth.auth.token, q.after, q.limit)
        .await;
    let status = match &result {
        Ok(_) => 200,
        Err(e) => e.status.as_u16() as i32,
    };
    let _ = state
        .tokens
        .log_call(
            &auth.auth,
            "GET",
            "/api/articles/since",
            Some(ApiScope::ArticlesRead.as_str()),
            status,
            started.elapsed().as_millis() as i32,
            0,
            auth.client_ip.as_deref(),
        )
        .await;
    Ok(Json(result?))
}
