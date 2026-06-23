//! The single writer-actor.
//!
//! SQLite allows exactly one writer at a time. Rather than scatter
//! `SQLITE_BUSY` handling, every write funnels through one task behind a bounded
//! `mpsc` channel; reads use the pool directly. The task owns a single pooled
//! connection and runs each job inside a `BEGIN IMMEDIATE` transaction so a
//! deferred read→write upgrade can never surprise us mid-flight.
//!
//! Jobs are boxed async closures over a connection. This keeps the actor generic
//! over what callers want to write while still serializing every mutation.

use std::future::Future;
use std::pin::Pin;

use sqlx::sqlite::SqlitePool;
use sqlx::SqliteConnection;
use tokio::sync::{mpsc, oneshot};

use crate::error::{DbError, Result};

/// A unit of write work: given the writer's connection, do the writes and return.
///
/// The closure runs inside an `IMMEDIATE` transaction the actor manages; the
/// closure itself does not commit. Returning `Err` rolls the transaction back.
type WriteJob = Box<
    dyn for<'c> FnOnce(
            &'c mut SqliteConnection,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'c>>
        + Send,
>;

/// One message to the writer task: the work plus a reply channel.
struct WriteMessage {
    job: WriteJob,
    reply: oneshot::Sender<Result<()>>,
}

/// A cheap, cloneable handle used to submit writes to the single writer task.
#[derive(Clone)]
pub struct WriterHandle {
    tx: mpsc::Sender<WriteMessage>,
}

impl WriterHandle {
    /// Spawn the writer task on the current runtime and return its handle.
    ///
    /// The task acquires one connection from `pool` and keeps it for its
    /// lifetime so writes are strictly serialized. `bound` is the channel
    /// capacity (backpressure for write bursts).
    #[must_use]
    pub fn spawn(pool: SqlitePool, bound: usize) -> Self {
        let (tx, mut rx) = mpsc::channel::<WriteMessage>(bound);

        tokio::spawn(async move {
            // Hold a dedicated connection for the actor's life; this is the one
            // and only writer, which is exactly the SQLite single-writer model.
            let mut conn = match pool.acquire().await {
                Ok(c) => c,
                Err(e) => {
                    // Drain pending senders with the failure so callers don't hang.
                    while let Some(msg) = rx.recv().await {
                        let _ = msg.reply.send(Err(DbError::Sqlx(e_clone(&e))));
                    }
                    return;
                }
            };

            while let Some(WriteMessage { job, reply }) = rx.recv().await {
                let outcome = run_in_immediate(&mut conn, job).await;
                // The receiver may have given up; ignore a closed reply channel.
                let _ = reply.send(outcome);
            }
        });

        Self { tx }
    }

    /// Submit a write job and await its result.
    ///
    /// # Errors
    /// Returns [`DbError::WriterUnavailable`] if the writer task has stopped, or
    /// whatever error the job produced.
    pub async fn submit<F>(&self, job: F) -> Result<()>
    where
        F: for<'c> FnOnce(
                &'c mut SqliteConnection,
            ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'c>>
            + Send
            + 'static,
    {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(WriteMessage {
                job: Box::new(job),
                reply,
            })
            .await
            .map_err(|_| DbError::WriterUnavailable("channel closed".into()))?;
        rx.await
            .map_err(|_| DbError::WriterUnavailable("reply dropped".into()))?
    }
}

/// Run a job inside a `BEGIN IMMEDIATE` transaction, committing on success and
/// rolling back on error.
async fn run_in_immediate(conn: &mut SqliteConnection, job: WriteJob) -> Result<()> {
    // BEGIN IMMEDIATE takes the write lock up front so a deferred upgrade can
    // never error mid-transaction (docs/08-database.md).
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
    match job(conn).await {
        Ok(()) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            Ok(())
        }
        Err(e) => {
            // Best-effort rollback; surface the original error.
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            Err(e)
        }
    }
}

/// `sqlx::Error` is not `Clone`; reconstruct a coarse equivalent so a failed
/// connection acquisition can be reported to every waiting caller.
fn e_clone(e: &sqlx::Error) -> sqlx::Error {
    sqlx::Error::Protocol(format!("writer connection unavailable: {e}"))
}
