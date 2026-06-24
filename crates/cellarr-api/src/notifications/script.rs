//! The Custom Script notification provider.
//!
//! Runs a local executable (`settings.path`) on each subscribed event, passing
//! the event detail in the environment — the Sonarr/Radarr "Custom Script"
//! connector. The variable names mirror the originals (`cellarr_eventtype`,
//! `cellarr_series_title`/`cellarr_movie_title`, `cellarr_release_title`, …) so a
//! user's existing post-processing scripts keep working with minimal changes.
//!
//! The script is spawned off the async reactor (a blocking `std::process` call on
//! a `spawn_blocking` thread) with a bounded wait, and its exit status decides
//! success — a non-zero exit or a spawn failure is an `Err(detail)` the
//! dispatcher logs. A misbehaving script never blocks or breaks the pipeline.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::notification::kind;
use cellarr_core::{MediaType, NotificationConfig, NotificationMessage, NotificationSender};

use super::providers_support::required_str;

/// The seam the script is run through, so the run is asserted by a test (which
/// records the path + env, or runs a real temp script) with no real subprocess
/// on the unit path.
#[async_trait]
pub trait ScriptRunner: Send + Sync {
    /// Run `path` with `env` set, returning `Ok(())` on a zero exit or
    /// `Err(detail)` on a non-zero exit or a spawn failure.
    async fn run(&self, path: &str, env: BTreeMap<String, String>) -> Result<(), String>;
}

/// Runs the script as a real subprocess via `std::process::Command` on a blocking
/// thread, with a bounded wait.
pub struct ProcessScriptRunner {
    /// How long to wait for the script before treating it as failed.
    timeout: std::time::Duration,
}

impl ProcessScriptRunner {
    /// Build with the default bounded timeout.
    #[must_use]
    pub fn new() -> Self {
        Self {
            timeout: std::time::Duration::from_secs(30),
        }
    }
}

impl Default for ProcessScriptRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ScriptRunner for ProcessScriptRunner {
    async fn run(&self, path: &str, env: BTreeMap<String, String>) -> Result<(), String> {
        let path = path.to_string();
        let timeout = self.timeout;
        let join = tokio::task::spawn_blocking(move || run_blocking(&path, &env, timeout));
        join.await
            .map_err(|e| format!("custom script task failed: {e}"))?
    }
}

/// Spawn `path`, wait up to `timeout`, and map the exit status to a result. Kept
/// separate so it is unit-testable against a real temp script.
fn run_blocking(
    path: &str,
    env: &BTreeMap<String, String>,
    timeout: std::time::Duration,
) -> Result<(), String> {
    use std::process::Command;
    use std::time::Instant;

    if !Path::new(path).exists() {
        return Err(format!("custom script not found: {path}"));
    }
    let mut child = Command::new(path)
        .envs(env)
        .spawn()
        .map_err(|e| format!("spawn custom script {path}: {e}"))?;

    // Bounded wait without an extra dep: poll `try_wait` until the deadline.
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("custom script {path} exited with status {status}"))
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return Err(format!("custom script {path} timed out"));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(format!("wait on custom script {path}: {e}")),
        }
    }
}

/// The Custom Script provider.
pub struct CustomScriptSender {
    runner: Arc<dyn ScriptRunner>,
}

impl CustomScriptSender {
    /// Build over a [`ScriptRunner`].
    #[must_use]
    pub fn new(runner: Arc<dyn ScriptRunner>) -> Self {
        Self { runner }
    }

    /// Build the environment a script run receives from a message. Public so the
    /// env mapping is asserted directly in tests.
    #[must_use]
    pub fn build_env(message: &NotificationMessage) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert(
            "cellarr_eventtype".to_string(),
            message.event.key().to_string(),
        );
        env.insert(
            "cellarr_instancename".to_string(),
            message.instance_name.clone(),
        );
        if let Some(s) = &message.subject {
            let title_key = match s.media_type {
                Some(MediaType::Tv) => "cellarr_series_title",
                _ => "cellarr_movie_title",
            };
            env.insert(title_key.to_string(), s.title.clone());
            env.insert("cellarr_subject_id".to_string(), s.id.clone());
            if let Some(year) = s.year {
                env.insert("cellarr_year".to_string(), year.to_string());
            }
        }
        if let Some(r) = &message.release {
            env.insert("cellarr_release_title".to_string(), r.release_title.clone());
            if let Some(q) = &r.quality {
                env.insert("cellarr_release_quality".to_string(), q.clone());
            }
        }
        if !message.files.is_empty() {
            // Paths joined by `|` (a separator a path never contains), matching
            // how the originals pass multi-file imports to a script.
            env.insert(
                "cellarr_imported_files".to_string(),
                message.files.join("|"),
            );
        }
        if let Some(h) = &message.health {
            env.insert("cellarr_health_level".to_string(), h.level.clone());
            env.insert("cellarr_health_message".to_string(), h.message.clone());
            env.insert("cellarr_health_source".to_string(), h.source.clone());
        }
        env
    }
}

#[async_trait]
impl NotificationSender for CustomScriptSender {
    fn kind(&self) -> &'static str {
        kind::CUSTOM_SCRIPT
    }

    async fn send(
        &self,
        config: &NotificationConfig,
        message: &NotificationMessage,
    ) -> Result<(), String> {
        let path = required_str(config, "path")?;
        let env = Self::build_env(message);
        self.runner.run(path, env).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::{NotificationEvent, NotificationSubject};

    #[test]
    fn build_env_maps_event_and_subject_and_files() {
        let msg = NotificationMessage::new(NotificationEvent::Import, "cellarr")
            .with_subject(NotificationSubject {
                id: "abc".into(),
                title: "The Matrix".into(),
                year: Some(1999),
                media_type: Some(MediaType::Movie),
            })
            .with_files(vec!["/m/a.mkv".into(), "/m/b.mkv".into()]);
        let env = CustomScriptSender::build_env(&msg);
        assert_eq!(env.get("cellarr_eventtype").unwrap(), "download");
        assert_eq!(env.get("cellarr_movie_title").unwrap(), "The Matrix");
        assert_eq!(env.get("cellarr_year").unwrap(), "1999");
        assert_eq!(
            env.get("cellarr_imported_files").unwrap(),
            "/m/a.mkv|/m/b.mkv"
        );
        assert!(!env.contains_key("cellarr_series_title"));
    }

    #[test]
    fn build_env_uses_series_title_for_tv() {
        let msg = NotificationMessage::new(NotificationEvent::Grab, "cellarr").with_subject(
            NotificationSubject {
                id: "1".into(),
                title: "Show".into(),
                year: None,
                media_type: Some(MediaType::Tv),
            },
        );
        let env = CustomScriptSender::build_env(&msg);
        assert_eq!(env.get("cellarr_series_title").unwrap(), "Show");
    }
}
