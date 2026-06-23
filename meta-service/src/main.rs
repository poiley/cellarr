//! cellarr-meta — standalone metadata service binary (stub).
//!
//! A thin wrapper that runs `cellarr-meta` as its own HTTP service. Not yet
//! implemented; this stub starts and exits cleanly so the binary builds and the
//! workspace resolves. Real work lands per `docs/07-metadata-service.md`.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cellarr_meta::serve_standalone()
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(())
}
