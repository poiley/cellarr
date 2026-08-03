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

    /// Delete decision-log rows older than `cutoff`, returning how many went.
    ///
    /// The log is append-only diagnostic data with a short useful life, and nothing
    /// trimmed it — it had reached 178,606 rows / 315 MB on the reference deployment
    /// and was growing ~9k rows a day. Deleting by age keeps recent runs fully
    /// explainable (which is what `missing_reason` reads) while bounding the table.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query failure.
    pub async fn prune_before(&self, cutoff: time::OffsetDateTime) -> Result<u64> {
        let cutoff = format_time(cutoff)?;
        // Counted before the delete: the writer channel yields no row count, and the
        // number is wanted only to report what the sweep did.
        let doomed: i64 = sqlx::query(&pq("SELECT count(*) AS n FROM decision_log WHERE at < ?1"))
            .bind(&cutoff)
            .fetch_one(&self.pool)
            .await?
            .try_get("n")?;
        if doomed == 0 {
            return Ok(0);
        }
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq("DELETE FROM decision_log WHERE at < ?1"))
                        .bind(&cutoff)
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .await?;
        Ok(u64::try_from(doomed).unwrap_or(0))
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
        // Denormalized out of the `decision` blob so the log is queryable by content
        // in portable SQL — the JSON accessors differ between SQLite and Postgres, so
        // "why is this item still missing?" could not otherwise be asked at all.
        let content_id = record
            .decision
            .as_ref()
            .map(|d| d.content_ref.id.to_string());
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(
                        "INSERT INTO decision_log (id, at, run_id, transition, decision, note, content_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
                    )
                    .bind(id)
                    .bind(at)
                    .bind(run_id)
                    .bind(transition)
                    .bind(decision)
                    .bind(note)
                    .bind(content_id)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }
}
