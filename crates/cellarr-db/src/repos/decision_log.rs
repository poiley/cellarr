//! The append-only `decision_log` repository.

use async_trait::async_trait;
use cellarr_core::decision::Decision;
use cellarr_core::history::DecisionLogRecord;
use cellarr_core::pipeline::Transition;
use cellarr_core::repo::DecisionLogRepository;
use cellarr_core::PipelineRunId;
use crate::dialect::{pq, DbPool};
use sqlx::Row;
use uuid::Uuid;

use crate::convert::{format_time, parse_time, parse_uuid};
use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// Append-only writes and queries for the decision log.
#[derive(Clone)]
pub struct DecisionLogRepo {
    pool: DbPool,
    writer: WriterHandle,
}

impl DecisionLogRepo {
    pub(crate) fn new(pool: DbPool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// All decision-log records for a pipeline run, oldest first.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn for_run(&self, run_id: PipelineRunId) -> Result<Vec<DecisionLogRecord>> {
        let rows = sqlx::query(&pq(
            "SELECT at, run_id, transition, decision, note
             FROM decision_log WHERE run_id = ?1 ORDER BY at ASC, id ASC"),
        )
        .bind(run_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let at: String = row.try_get("at")?;
                let run_id: String = row.try_get("run_id")?;
                let transition: String = row.try_get("transition")?;
                let decision: Option<String> = row.try_get("decision")?;
                let note: Option<String> = row.try_get("note")?;
                let transition: Transition = serde_json::from_str(&transition)?;
                let decision: Option<Decision> =
                    decision.map(|d| serde_json::from_str(&d)).transpose()?;
                Ok(DecisionLogRecord {
                    at: parse_time("at", &at)?,
                    run_id: PipelineRunId::from_uuid(parse_uuid("run_id", &run_id)?),
                    transition,
                    decision,
                    note,
                })
            })
            .collect()
    }
}

#[async_trait]
impl DecisionLogRepository for DecisionLogRepo {
    type Error = DbError;

    async fn append(&self, record: &DecisionLogRecord) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        let at = format_time(record.at)?;
        let run_id = record.run_id.to_string();
        let transition = serde_json::to_string(&record.transition)?;
        let decision = record
            .decision
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let note = record.note.clone();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(
                        "INSERT INTO decision_log (id, at, run_id, transition, decision, note)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)"),
                    )
                    .bind(id)
                    .bind(at)
                    .bind(run_id)
                    .bind(transition)
                    .bind(decision)
                    .bind(note)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }
}
