use crate::QueueResult;
use sqlx::PgPool;
use uuid::Uuid;

/// A single log chunk ready for insertion.
pub struct RunLogChunk {
    pub run_id: Uuid,
    pub seq: i32,
    pub min_level: i16,
    pub max_level: i16,
    pub line_count: i16,
    pub entries: serde_json::Value,
}

/// A single service log chunk ready for insertion.
pub struct ServiceLogChunk {
    pub instance_id: String,
    pub service: String,
    pub seq: i32,
    pub min_level: i16,
    pub max_level: i16,
    pub line_count: i16,
    pub entries: serde_json::Value,
}

/// Row returned when querying run log chunks.
#[derive(Debug, serde::Serialize)]
pub struct RunLogChunkRow {
    pub id: i64,
    pub run_id: Uuid,
    pub seq: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub min_level: i16,
    pub max_level: i16,
    pub line_count: i16,
    pub entries: serde_json::Value,
}

/// Row returned when querying service log chunks.
#[derive(Debug, serde::Serialize)]
pub struct ServiceLogChunkRow {
    pub id: i64,
    pub instance_id: String,
    pub service: String,
    pub seq: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub min_level: i16,
    pub max_level: i16,
    pub line_count: i16,
    pub entries: serde_json::Value,
}

/// Batch INSERT run log chunks.
#[tracing::instrument(
    name = "queue::append_run_log_chunks",
    skip(db, chunks),
    fields(db_log_skip = true, chunk_count = chunks.len())
)]
pub async fn append_run_log_chunks(db: &PgPool, chunks: &[RunLogChunk]) -> QueueResult<()> {
    if chunks.is_empty() {
        return Ok(());
    }

    // Multi-row INSERT via UNNEST: one DB round-trip for all chunks.
    // ON CONFLICT DO NOTHING handles stale chunks whose run no longer exists
    // (FK violation would be caught by the WHERE EXISTS check below instead).
    let run_ids: Vec<Uuid> = chunks.iter().map(|c| c.run_id).collect();
    let seqs: Vec<i32> = chunks.iter().map(|c| c.seq).collect();
    let min_levels: Vec<i16> = chunks.iter().map(|c| c.min_level).collect();
    let max_levels: Vec<i16> = chunks.iter().map(|c| c.max_level).collect();
    let line_counts: Vec<i16> = chunks.iter().map(|c| c.line_count).collect();
    let entries: Vec<serde_json::Value> = chunks.iter().map(|c| c.entries.clone()).collect();

    let result = sqlx::query!(
        r#"
        INSERT INTO run_log (run_id, seq, min_level, max_level, line_count, entries)
        SELECT t.run_id, t.seq, t.min_level, t.max_level, t.line_count, t.entries
        FROM UNNEST(
            $1::uuid[],
            $2::int4[],
            $3::int2[],
            $4::int2[],
            $5::int2[],
            $6::jsonb[]
        ) WITH ORDINALITY AS t(run_id, seq, min_level, max_level, line_count, entries, ord)
        WHERE EXISTS (SELECT 1 FROM run WHERE id = t.run_id)
        ORDER BY t.ord
        "#,
        &run_ids,
        &seqs,
        &min_levels,
        &max_levels,
        &line_counts,
        &entries as &[serde_json::Value],
    )
    .execute(db)
    .await;

    if let Err(e) = result {
        tracing::warn!(
            chunk_count = chunks.len(),
            error = %e,
            "failed to append run log chunks"
        );
    }

    Ok(())
}

/// Fetch run log chunks with cursor-based pagination.
///
/// `after_id`: only return chunks with `id > after_id` (cursor).
/// `min_level`: only return chunks where `max_level >= min_level` (level filter).
/// `limit`: max number of chunks to return.
#[tracing::instrument(
    name = "queue::get_run_log_chunks",
    skip(db),
    fields(db_log_skip = true, %run_id, after_id, limit)
)]
pub async fn get_run_log_chunks(
    db: &PgPool,
    run_id: Uuid,
    after_id: i64,
    min_level: Option<i16>,
    limit: i64,
) -> QueueResult<Vec<RunLogChunkRow>> {
    let level_filter = min_level.unwrap_or(0);

    let rows = sqlx::query_as!(
        RunLogChunkRow,
        r#"
        SELECT
            id as "id!",
            run_id as "run_id!",
            seq as "seq!",
            created_at as "created_at!",
            min_level as "min_level!",
            max_level as "max_level!",
            line_count as "line_count!",
            entries as "entries!"
        FROM run_log
        WHERE run_id = $1
          AND id > $2
          AND max_level >= $3
        ORDER BY seq ASC
        LIMIT $4
        "#,
        run_id,
        after_id,
        level_filter,
        limit,
    )
    .fetch_all(db)
    .await?;

    Ok(rows)
}

/// Batch INSERT service log chunks.
#[tracing::instrument(
    name = "queue::append_service_log_chunks",
    skip(db, chunks),
    fields(db_log_skip = true, chunk_count = chunks.len())
)]
pub async fn append_service_log_chunks(db: &PgPool, chunks: &[ServiceLogChunk]) -> QueueResult<()> {
    if chunks.is_empty() {
        return Ok(());
    }

    // Multi-row INSERT via UNNEST: one DB round-trip for all chunks.
    let instance_ids: Vec<String> = chunks.iter().map(|c| c.instance_id.clone()).collect();
    let services: Vec<String> = chunks.iter().map(|c| c.service.clone()).collect();
    let seqs: Vec<i32> = chunks.iter().map(|c| c.seq).collect();
    let min_levels: Vec<i16> = chunks.iter().map(|c| c.min_level).collect();
    let max_levels: Vec<i16> = chunks.iter().map(|c| c.max_level).collect();
    let line_counts: Vec<i16> = chunks.iter().map(|c| c.line_count).collect();
    let entries: Vec<serde_json::Value> = chunks.iter().map(|c| c.entries.clone()).collect();

    sqlx::query!(
        r#"
        INSERT INTO service_log (instance_id, service, seq, min_level, max_level, line_count, entries)
        SELECT t.instance_id, t.service, t.seq, t.min_level, t.max_level, t.line_count, t.entries
        FROM UNNEST(
            $1::text[],
            $2::text[],
            $3::int4[],
            $4::int2[],
            $5::int2[],
            $6::int2[],
            $7::jsonb[]
        ) WITH ORDINALITY AS t(instance_id, service, seq, min_level, max_level, line_count, entries, ord)
        ORDER BY t.ord
        "#,
        &instance_ids,
        &services,
        &seqs,
        &min_levels,
        &max_levels,
        &line_counts,
        &entries as &[serde_json::Value],
    )
    .execute(db)
    .await?;

    Ok(())
}

/// Fetch service log chunks starting from a given timestamp (milliseconds since epoch).
/// Used by time-based tail mode: the client computes `Date.now() - windowMs` and passes
/// it as `since_ms`; subsequent polls switch to cursor-based `get_service_log_chunks`.
#[tracing::instrument(
    name = "queue::get_service_log_since",
    skip(db),
    fields(db_log_skip = true, since_ms, limit)
)]
pub async fn get_service_log_since(
    db: &PgPool,
    service: Option<&str>,
    instance_id: Option<&str>,
    min_level: Option<i16>,
    since_ms: i64,
    limit: i64,
) -> QueueResult<Vec<ServiceLogChunkRow>> {
    let level_filter = min_level.unwrap_or(0);
    let since = chrono::DateTime::from_timestamp_millis(since_ms).unwrap_or_default();

    let rows = sqlx::query_as!(
        ServiceLogChunkRow,
        r#"
        SELECT
            id as "id!",
            instance_id as "instance_id!",
            service as "service!",
            seq as "seq!",
            created_at as "created_at!",
            min_level as "min_level!",
            max_level as "max_level!",
            line_count as "line_count!",
            entries as "entries!"
        FROM service_log
        WHERE created_at >= $1
          AND max_level >= $2
          AND ($3::TEXT IS NULL OR service = $3)
          AND ($4::TEXT IS NULL OR instance_id = $4)
        ORDER BY id ASC
        LIMIT $5
        "#,
        since,
        level_filter,
        service,
        instance_id,
        limit,
    )
    .fetch_all(db)
    .await?;

    Ok(rows)
}

/// Fetch service log chunks with cursor-based pagination and filters.
#[tracing::instrument(
    name = "queue::get_service_log_chunks",
    skip(db),
    fields(db_log_skip = true, after_id, limit)
)]
pub async fn get_service_log_chunks(
    db: &PgPool,
    service: Option<&str>,
    instance_id: Option<&str>,
    after_id: i64,
    min_level: Option<i16>,
    limit: i64,
) -> QueueResult<Vec<ServiceLogChunkRow>> {
    let level_filter = min_level.unwrap_or(0);

    let rows = sqlx::query_as!(
        ServiceLogChunkRow,
        r#"
        SELECT
            id as "id!",
            instance_id as "instance_id!",
            service as "service!",
            seq as "seq!",
            created_at as "created_at!",
            min_level as "min_level!",
            max_level as "max_level!",
            line_count as "line_count!",
            entries as "entries!"
        FROM service_log
        WHERE id > $1
          AND max_level >= $2
          AND ($3::TEXT IS NULL OR service = $3)
          AND ($4::TEXT IS NULL OR instance_id = $4)
        ORDER BY id ASC
        LIMIT $5
        "#,
        after_id,
        level_filter,
        service,
        instance_id,
        limit,
    )
    .fetch_all(db)
    .await?;

    Ok(rows)
}
