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

use tokio::sync::{mpsc, oneshot};

use crate::dialect::{DbConnection, DbPool};
use crate::error::{DbError, Result};

/// A unit of write work: given the writer's connection, do the writes and return.
///
/// The closure runs inside an `IMMEDIATE` transaction the actor manages; the
/// closure itself does not commit. Returning `Err` rolls the transaction back.
type WriteJob = Box<
    dyn for<'c> FnOnce(
            &'c mut DbConnection,
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

/// Control side of the writer task, held by [`crate::Database`] (once, behind an
/// `Arc`) so shutdown can stop the actor **explicitly** and wait for it to drop
/// its connection — independent of how many [`WriterHandle`] clones exist.
///
/// This is the crux of clean shutdown: `SqlitePool::close()` waits for every
/// outstanding connection to be returned, and the actor holds one for its whole
/// life. Relying on all handle clones dropping is unworkable (the pool, the
/// scheduler, and per-repo borrows all clone the handle). So we signal the actor
/// directly and `join` it before closing the pool.
pub struct WriterShutdown {
    stop: Option<oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<()>,
}

impl WriterShutdown {
    /// Signal the writer actor to stop and wait for it to finish (dropping its
    /// connection back to the pool). Idempotent-safe: callable once; the
    /// [`crate::Database`] guards against double-invocation.
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // The actor returns promptly once signalled; awaiting it guarantees its
        // pooled connection has been released before the caller closes the pool.
        let _ = self.join.await;
    }
}

impl WriterHandle {
    /// Spawn the writer task on the current runtime.
    ///
    /// The task acquires one connection from `pool` and keeps it for its
    /// lifetime so writes are strictly serialized. `bound` is the channel
    /// capacity (backpressure for write bursts). Returns the cloneable submit
    /// handle plus the single [`WriterShutdown`] control.
    #[must_use]
    pub fn spawn(pool: DbPool, bound: usize) -> (Self, WriterShutdown) {
        let (tx, mut rx) = mpsc::channel::<WriteMessage>(bound);
        let (stop_tx, mut stop_rx) = oneshot::channel::<()>();

        let join = tokio::spawn(async move {
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

            loop {
                tokio::select! {
                    // Bias toward draining queued writes before honoring a stop,
                    // so an in-flight burst isn't dropped at shutdown.
                    biased;
                    msg = rx.recv() => match msg {
                        Some(WriteMessage { job, reply }) => {
                            let outcome = run_in_immediate(&mut conn, job).await;
                            // The receiver may have given up; ignore a closed channel.
                            let _ = reply.send(outcome);
                        }
                        // All submit handles dropped: nothing more can arrive.
                        None => break,
                    },
                    _ = &mut stop_rx => break,
                }
            }
            // `conn` drops here, returning the writer's connection to the pool so
            // a subsequent `pool.close()` can complete.
        });

        (
            Self { tx },
            WriterShutdown {
                stop: Some(stop_tx),
                join,
            },
        )
    }

    /// Submit a write job and await its result.
    ///
    /// # Errors
    /// Returns [`DbError::WriterUnavailable`] if the writer task has stopped, or
    /// whatever error the job produced.
    #[tracing::instrument(name = "db.write", skip_all)]
    pub async fn submit<F>(&self, job: F) -> Result<()>
    where
        F: for<'c> FnOnce(
                &'c mut DbConnection,
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

/// Run a job inside a transaction, committing on success and rolling back on
/// error.
async fn run_in_immediate(conn: &mut DbConnection, job: WriteJob) -> Result<()> {
    // On SQLite `BEGIN IMMEDIATE` takes the write lock up front so a deferred
    // read→write upgrade can never error mid-transaction (docs/08-database.md).
    // Postgres has no such lock-upgrade hazard (MVCC), so a plain `BEGIN` is the
    // correct and only valid spelling there.
    #[cfg(not(feature = "postgres"))]
    let begin = "BEGIN IMMEDIATE";
    #[cfg(feature = "postgres")]
    let begin = "BEGIN";
    sqlx::query(begin).execute(&mut *conn).await?;
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
