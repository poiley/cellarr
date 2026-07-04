//! The append-only `history` repository.

use async_trait::async_trait;
use cellarr_core::history::{HistoryEvent, HistoryRecord};
use cellarr_core::repo::HistoryRepository;
use cellarr_core::{ContentId, PipelineRunId};
use crate::dialect::{pq, DbPool};
use sqlx::Row;
use uuid::Uuid;

use crate::convert::{format_time, parse_time, parse_uuid};
use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// Append-only writes and queries for the history stream.
#[derive(Clone)]
pub struct HistoryRepo {
    pool: DbPool,
    writer: WriterHandle,
}

impl HistoryRepo {
    pub(crate) fn new(pool: DbPool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }
}

#[async_trait]
impl HistoryRepository for HistoryRepo {
    type Error = DbError;

    async fn append(&self, record: &HistoryRecord) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        let at = format_time(record.at)?;
        let content_id = record.content_id.to_string();
        let run_id = record.run_id.to_string();
        let event = serde_json::to_string(&record.event)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(
                        "INSERT INTO history (id, at, content_id, run_id, event)
                         VALUES (?1, ?2, ?3, ?4, ?5)"),
                    )
                    .bind(id)
                    .bind(at)
                    .bind(content_id)
                    .bind(run_id)
                    .bind(event)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn for_content(&self, id: ContentId) -> Result<Vec<HistoryRecord>> {
        let rows = sqlx::query(&pq(
            "SELECT at, content_id, run_id, event
             FROM history WHERE content_id = ?1 ORDER BY at ASC, id ASC"),
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let at: String = row.try_get("at")?;
                let content_id: String = row.try_get("content_id")?;
                let run_id: String = row.try_get("run_id")?;
                let event: String = row.try_get("event")?;
                let event: HistoryEvent = serde_json::from_str(&event)?;
                Ok(HistoryRecord {
                    at: parse_time("at", &at)?,
                    content_id: ContentId::from_uuid(parse_uuid("content_id", &content_id)?),
                    run_id: PipelineRunId::from_uuid(parse_uuid("run_id", &run_id)?),
                    event,
                })
            })
            .collect()
    }

    async fn recent(&self, limit: u32, offset: u32) -> Result<Vec<HistoryRecord>> {
        // Global feed, newest first. `at` is a zero-padded rfc3339 UTC string, so a
        // lexical sort is chronological; `id` breaks ties within the same instant.
        let rows = sqlx::query(&pq(
            "SELECT at, content_id, run_id, event
             FROM history ORDER BY at DESC, id DESC LIMIT ?1 OFFSET ?2"),
        )
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let at: String = row.try_get("at")?;
                let content_id: String = row.try_get("content_id")?;
                let run_id: String = row.try_get("run_id")?;
                let event: String = row.try_get("event")?;
                let event: HistoryEvent = serde_json::from_str(&event)?;
                Ok(HistoryRecord {
                    at: parse_time("at", &at)?,
                    content_id: ContentId::from_uuid(parse_uuid("content_id", &content_id)?),
                    run_id: PipelineRunId::from_uuid(parse_uuid("run_id", &run_id)?),
                    event,
                })
            })
            .collect()
    }
}
