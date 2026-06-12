use std::convert::Infallible;

use axum::Extension;
use axum::Json;
use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use sqlx::PgPool;

use crate::auth::AuthedUser;
use crate::common::parse_level;
use crate::error::ApiError;

#[derive(serde::Deserialize)]
pub struct ServiceLogsQuery {
    pub service: Option<String>,
    pub instance: Option<String>,
    pub level: Option<String>,
    pub after_chunk: Option<i64>,
    pub limit: Option<i64>,
    /// When set, return chunks with created_at >= this value (milliseconds since
    /// Unix epoch) instead of using `after_chunk`. Used for time-based tail on
    /// first load; subsequent polls switch to cursor-based `after_chunk`.
    pub since_ms: Option<i64>,
}

#[derive(serde::Serialize)]
pub struct ServiceLogsResponse {
    pub chunks: Vec<coveflow_queue::ServiceLogChunkRow>,
    pub next_cursor: Option<i64>,
}

#[tracing::instrument(name = "api::get_service_logs", skip(db, user, query))]
pub async fn get_service_logs(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Query(query): Query<ServiceLogsQuery>,
) -> Result<Json<ServiceLogsResponse>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden(
            "only workspace admins can access service logs".into(),
        ));
    }
    let limit = query.limit.unwrap_or(50).min(200);
    let min_level = query.level.as_deref().and_then(parse_level);

    let chunks = if let Some(since_ms) = query.since_ms {
        coveflow_queue::get_service_log_since(
            &db,
            query.service.as_deref(),
            query.instance.as_deref(),
            min_level,
            since_ms,
            limit,
        )
        .await?
    } else {
        let after_id = query.after_chunk.unwrap_or(0);
        coveflow_queue::get_service_log_chunks(
            &db,
            query.service.as_deref(),
            query.instance.as_deref(),
            after_id,
            min_level,
            limit,
        )
        .await?
    };

    let next_cursor = chunks.last().map(|c| c.id);

    Ok(Json(ServiceLogsResponse {
        chunks,
        next_cursor,
    }))
}

#[tracing::instrument(name = "api::stream_service_logs", skip(db, user, query))]
pub async fn stream_service_logs(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Query(query): Query<ServiceLogsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden(
            "only workspace admins can access service logs".into(),
        ));
    }
    let service_filter = query.service.clone();
    let instance_filter = query.instance.clone();
    let min_level = query.level.as_deref().and_then(parse_level);
    let mut last_chunk_id = query.after_chunk.unwrap_or(0);
    let poll_interval = tokio::time::Duration::from_millis(500);

    let stream = async_stream::stream! {
        loop {
            match coveflow_queue::get_service_log_chunks(
                &db,
                service_filter.as_deref(),
                instance_filter.as_deref(),
                last_chunk_id,
                min_level,
                50,
            )
            .await
            {
                Ok(chunks) => {
                    for chunk in &chunks {
                        last_chunk_id = chunk.id;
                        let data = serde_json::json!({
                            "chunk_id": chunk.id,
                            "seq": chunk.seq,
                            "instance_id": chunk.instance_id,
                            "service": chunk.service,
                            "entries": chunk.entries,
                        });
                        let event = Event::default()
                            .event("log")
                            .data(data.to_string());
                        yield Ok(event);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "SSE: failed to fetch service log chunks");
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
