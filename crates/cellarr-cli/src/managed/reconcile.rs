//! The apply step: reconcile a validated [`ManagedConfig`] into the database.
//!
//! Reconciliation walks the declared sections in **dependency order** (tags →
//! quality definitions → custom formats → quality profiles → root folders →
//! libraries → indexers → download clients), so a thing is created before
//! whatever references it. For each *declared* section it:
//!
//! 1. lists the tracking-ledger rows config previously wrote for that kind,
//! 2. computes a stable content hash per declared item,
//! 3. diffs declared-vs-ledger with the pure [`super::plan`] step,
//! 4. applies each [`Action`] through the existing repo upsert/delete methods, and
//! 5. records / refreshes / removes the ledger row to match.
//!
//! A section **absent** from the file is skipped entirely (its `Option` is `None`)
//! — config does not touch a kind it does not declare. The prune in step 4 only
//! ever targets ledger rows (entities config created), so a UI-created entity is
//! never deleted. Re-running the same file is a no-op: every item hashes to its
//! recorded hash, so the plan is all-`Unchanged` with zero prunes.
//!
//! Cross-references are resolved by **name** here against what is live in the DB
//! after the earlier sections were applied (a library's quality profile, the tags
//! an indexer is scoped to). Because validation already proved the references
//! resolve within the file and the dependency order applies the referent first,
//! resolution cannot dangle.

use std::collections::BTreeMap;

use cellarr_core::importlist::{ImportListConfig, ImportListRepository};
use cellarr_core::repo::ProfileRepository;
use cellarr_core::{
    CustomFormat, DelayProfile, DelayProfileId, DownloadClientConfig, IndexerConfig, Library,
    LibraryId, MediaManagement, NamingFormats, NotificationConfig, QualityDefinition,
    QualityProfile, QualityProfileId, QualityRanking, ReleaseProfile, ReleaseProfileId,
    RemotePathMapping, RootFolder,
};
use cellarr_db::{Database, ManagedEntity};

use crate::managed::error::ManagedError;
use crate::managed::plan::{diff_kind, kind, Action, Counts, KindPlan, PlanItem};
use crate::managed::schema::{
    AuthSpec, CustomFormatSpec, DelayProfileSpec, DownloadClientSpec, ImportListSpec, IndexerSpec,
    LibrarySpec, ManagedConfig, MediaManagementSpec, NamingSpec, NotificationSpec,
    QualityDefinitionSpec, QualityProfileSpec, ReleaseProfileSpec, RemotePathMappingSpec,
    RootFolderSpec, TagSpec,
};

/// The summary of a full reconcile (or a dry-run plan): the per-kind plans, in the
/// order they were (or would be) applied.
#[derive(Debug, Clone, Default)]
pub struct ReconcileReport {
    /// The per-kind plans, in dependency order.
    pub kinds: Vec<KindPlan>,
}

impl ReconcileReport {
    /// Whether any kind carries a change (used for the `config validate` drift
    /// exit code).
    #[must_use]
    pub fn has_changes(&self) -> bool {
        self.kinds.iter().any(KindPlan::has_changes)
    }

    /// The total counts across all kinds.
    #[must_use]
    pub fn totals(&self) -> Counts {
        let mut total = Counts::default();
        for k in &self.kinds {
            let c = k.counts();
            total.created += c.created;
            total.updated += c.updated;
            total.unchanged += c.unchanged;
            total.pruned += c.pruned;
        }
        total
    }
}

/// Compute the plan for `config` against the live DB **without applying it**.
///
/// This is what `config validate` prints: the same per-kind diff reconciliation
/// would apply, but read-only. It still resolves the content hashes and reads the
/// ledger, so the create/update/unchanged/prune verdicts are exactly what a real
/// reconcile would do.
///
/// # Errors
/// Returns a [`ManagedError`] if reading the DB fails.
pub async fn plan(db: &Database, config: &ManagedConfig) -> Result<ReconcileReport, ManagedError> {
    reconcile_inner(db, config, false).await
}

/// Apply `config` to the DB, returning the report of what changed.
///
/// # Errors
/// Returns a [`ManagedError`] if reading or writing the DB fails.
pub async fn apply(db: &Database, config: &ManagedConfig) -> Result<ReconcileReport, ManagedError> {
    reconcile_inner(db, config, true).await
}

/// The shared engine for [`plan`] (dry-run) and [`apply`]. When `commit` is false
/// nothing is written; the verdicts are identical either way.
async fn reconcile_inner(
    db: &Database,
    config: &ManagedConfig,
    commit: bool,
) -> Result<ReconcileReport, ManagedError> {
    let mut report = ReconcileReport::default();

    // Dependency order: a referent is reconciled before its referrer.
    if let Some(tags) = &config.tags {
        report.kinds.push(reconcile_tags(db, tags, commit).await?);
    }
    if let Some(defs) = &config.quality_definitions {
        report
            .kinds
            .push(reconcile_quality_definitions(db, defs, commit).await?);
    }
    if let Some(cfs) = &config.custom_formats {
        report
            .kinds
            .push(reconcile_custom_formats(db, cfs, commit).await?);
    }
    if let Some(profiles) = &config.quality_profiles {
        report
            .kinds
            .push(reconcile_quality_profiles(db, profiles, commit).await?);
    }
    if let Some(rfs) = &config.root_folders {
        report
            .kinds
            .push(reconcile_root_folders(db, rfs, commit).await?);
    }
    if let Some(libs) = &config.libraries {
        report
            .kinds
            .push(reconcile_libraries(db, libs, commit).await?);
    }
    if let Some(ixs) = &config.indexers {
        report
            .kinds
            .push(reconcile_indexers(db, ixs, commit).await?);
    }
    if let Some(dcs) = &config.download_clients {
        report
            .kinds
            .push(reconcile_download_clients(db, dcs, commit).await?);
    }
    if let Some(rps) = &config.release_profiles {
        report
            .kinds
            .push(reconcile_release_profiles(db, rps, commit).await?);
    }
    if let Some(dps) = &config.delay_profiles {
        report
            .kinds
            .push(reconcile_delay_profiles(db, dps, commit).await?);
    }
    if let Some(lists) = &config.import_lists {
        report
            .kinds
            .push(reconcile_import_lists(db, lists, commit).await?);
    }
    if let Some(notifs) = &config.notifications {
        report
            .kinds
            .push(reconcile_notifications(db, notifs, commit).await?);
    }
    if let Some(maps) = &config.remote_path_mappings {
        report
            .kinds
            .push(reconcile_remote_path_mappings(db, maps, commit).await?);
    }
    if let Some(naming) = &config.naming {
        report
            .kinds
            .push(reconcile_naming(db, naming, commit).await?);
    }
    if let Some(mm) = &config.media_management {
        report
            .kinds
            .push(reconcile_media_management(db, mm, commit).await?);
    }
    if let Some(auth) = &config.auth {
        report.kinds.push(reconcile_auth(db, auth, commit).await?);
    }

    Ok(report)
}

/// A stable content hash of any serializable value: canonical JSON (serde_json
/// serializes struct fields in declaration order and `BTreeMap` keys sorted) run
/// through a fast non-cryptographic hash. The hash only needs to be stable and
/// collision-resistant enough to detect an edit, not cryptographic.
fn content_hash<T: serde::Serialize>(value: &T) -> String {
    use std::hash::{Hash, Hasher};
    let json = serde_json::to_string(value).unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    json.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Record a ledger row for a created/updated entity (no-op when not committing).
async fn record(
    db: &Database,
    kind: &str,
    name: &str,
    entity_id: String,
    content_hash: String,
    commit: bool,
) -> Result<(), ManagedError> {
    if commit {
        db.managed_config()
            .upsert(&ManagedEntity {
                kind: kind.to_string(),
                name: name.to_string(),
                entity_id,
                content_hash,
            })
            .await?;
    }
    Ok(())
}

/// Drop a ledger row for a pruned entity (no-op when not committing).
async fn forget(db: &Database, kind: &str, name: &str, commit: bool) -> Result<(), ManagedError> {
    if commit {
        db.managed_config().delete(kind, name).await?;
    }
    Ok(())
}

// === Tags ================================================================

async fn reconcile_tags(
    db: &Database,
    specs: &[TagSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::TAG).await?;
    // A tag's only content is its label, which *is* the name — so the hash is the
    // lowercased label. A tag therefore never "updates"; it is create or prune.
    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|t| (t.name.clone(), content_hash(&t.name.to_ascii_lowercase())))
        .collect();
    let plan = diff_kind(kind::TAG, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }

    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                // create() dedups case-insensitively; it returns the (possibly
                // existing) id. The ledger then owns this id as config-managed.
                let id = if commit {
                    db.tags().create(&item.name).await?.id.to_string()
                } else {
                    item.entity_id.clone().unwrap_or_default()
                };
                record(
                    db,
                    kind::TAG,
                    &item.name,
                    id,
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if let Some(id) = item.entity_id.as_ref().and_then(|s| s.parse::<u32>().ok()) {
                    if commit {
                        db.tags().delete(id).await?;
                    }
                }
                forget(db, kind::TAG, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Quality definitions =================================================

async fn reconcile_quality_definitions(
    db: &Database,
    specs: &[QualityDefinitionSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db
        .managed_config()
        .list_kind(kind::QUALITY_DEFINITION)
        .await?;
    let defs: Vec<QualityDefinition> = specs.iter().map(spec_to_quality_definition).collect();
    let declared: Vec<(String, String)> = specs
        .iter()
        .zip(&defs)
        .map(|(s, d)| (s.name.clone(), content_hash(d)))
        .collect();
    let plan = diff_kind(kind::QUALITY_DEFINITION, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }

    let by_name: BTreeMap<&str, &QualityDefinition> = specs
        .iter()
        .map(|s| s.name.as_str())
        .zip(defs.iter())
        .collect();

    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                if commit {
                    if let Some(def) = by_name.get(item.name.as_str()) {
                        db.profiles().upsert_quality_definition(def).await?;
                    }
                }
                // The entity id of a quality definition *is* its canonical name.
                record(
                    db,
                    kind::QUALITY_DEFINITION,
                    &item.name,
                    item.name.clone(),
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if commit {
                    db.profiles().delete_quality_definition(&item.name).await?;
                }
                forget(db, kind::QUALITY_DEFINITION, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Custom formats ======================================================

async fn reconcile_custom_formats(
    db: &Database,
    specs: &[CustomFormatSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::CUSTOM_FORMAT).await?;
    // Hash the declared content (name+score+conditions), *not* the assigned id, so
    // an unchanged definition stays unchanged across the random id minted on first
    // create.
    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|s| (s.name.clone(), content_hash(&hashable_custom_format(s))))
        .collect();
    let plan = diff_kind(kind::CUSTOM_FORMAT, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }
    let by_name: BTreeMap<&str, &CustomFormatSpec> =
        specs.iter().map(|s| (s.name.as_str(), s)).collect();

    for item in &plan.items {
        apply_id_kind(
            db,
            &plan.kind,
            item,
            commit,
            |id_str| {
                // Reuse the existing id on update; mint a new one on create.
                let id = id_str
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(cellarr_core::CustomFormatId::from_uuid)
                    .unwrap_or_default();
                let spec = by_name.get(item.name.as_str()).expect("declared");
                let cf = CustomFormat {
                    id,
                    name: spec.name.clone(),
                    conditions: spec.conditions.clone(),
                    score: spec.score,
                };
                (id.to_string(), cf)
            },
            |db, cf| async move {
                db.profiles()
                    .upsert_custom_format(&cf)
                    .await
                    .map_err(Into::into)
            },
            |db, id_str| async move {
                if let Ok(uuid) = id_str.parse::<uuid::Uuid>() {
                    db.profiles()
                        .delete_custom_format(cellarr_core::CustomFormatId::from_uuid(uuid))
                        .await?;
                }
                Ok(())
            },
        )
        .await?;
    }
    Ok(plan)
}

// === Quality profiles ====================================================

async fn reconcile_quality_profiles(
    db: &Database,
    specs: &[QualityProfileSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::QUALITY_PROFILE).await?;
    // Resolve allowed-quality names → ranks against the *effective* catalogue
    // (default + any declared/edited quality definitions already applied).
    let ranking = db.profiles().quality_ranking().await?;

    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|s| {
            Ok((
                s.name.clone(),
                content_hash(&hashable_profile(s, &ranking)?),
            ))
        })
        .collect::<Result<_, ManagedError>>()?;
    let plan = diff_kind(kind::QUALITY_PROFILE, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }
    let by_name: BTreeMap<&str, &QualityProfileSpec> =
        specs.iter().map(|s| (s.name.as_str(), s)).collect();

    // Note: a profile's `customFormatScores` are *references* validated at load
    // time, not an independent store. cellarr scores a custom format on the
    // `CustomFormat` itself (the v3 model), so the authoritative score lives in the
    // `customFormats` section; the profile map is required to agree with it
    // (enforced by validation) and is not separately written here — which keeps the
    // reconcile idempotent (no cross-section write-back that a later section would
    // see as drift).
    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                let spec = by_name.get(item.name.as_str()).expect("declared");
                let id = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(QualityProfileId::from_uuid)
                    .unwrap_or_default();
                let profile = spec_to_quality_profile(spec, id, &ranking)?;
                if commit {
                    db.profiles().upsert_profile(&profile).await?;
                }
                record(
                    db,
                    kind::QUALITY_PROFILE,
                    &item.name,
                    profile.id.to_string(),
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if let Some(id) = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(QualityProfileId::from_uuid)
                {
                    if commit {
                        db.profiles().delete_profile(id).await?;
                    }
                }
                forget(db, kind::QUALITY_PROFILE, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Root folders ========================================================

async fn reconcile_root_folders(
    db: &Database,
    specs: &[RootFolderSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::ROOT_FOLDER).await?;
    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|s| (s.name.clone(), content_hash(&hashable_root_folder(s))))
        .collect();
    let plan = diff_kind(kind::ROOT_FOLDER, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }
    let by_name: BTreeMap<&str, &RootFolderSpec> =
        specs.iter().map(|s| (s.name.as_str(), s)).collect();

    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                let spec = by_name.get(item.name.as_str()).expect("declared");
                // The root-folder id is a stable string; config keys it by the
                // declared name (preserved on update via the ledger id).
                let id = item.entity_id.clone().unwrap_or_else(|| spec.name.clone());
                let folder = RootFolder {
                    id: id.clone(),
                    path: spec.path.clone(),
                    name: Some(spec.name.clone()),
                    enabled: spec.enabled,
                };
                if commit {
                    db.config().upsert_root_folder(&folder).await?;
                }
                record(
                    db,
                    kind::ROOT_FOLDER,
                    &item.name,
                    id,
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if let Some(id) = &item.entity_id {
                    if commit {
                        db.config().delete_root_folder(id).await?;
                    }
                }
                forget(db, kind::ROOT_FOLDER, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Libraries ===========================================================

async fn reconcile_libraries(
    db: &Database,
    specs: &[LibrarySpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::LIBRARY).await?;
    // Resolve referenced quality profiles + root folders (now live) by name.
    let profiles = db.profiles().list_profiles().await?;
    let root_folders = db.config().list_root_folders().await?;

    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|s| (s.name.clone(), content_hash(&hashable_library(s))))
        .collect();
    let plan = diff_kind(kind::LIBRARY, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }
    let by_name: BTreeMap<&str, &LibrarySpec> =
        specs.iter().map(|s| (s.name.as_str(), s)).collect();

    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                let spec = by_name.get(item.name.as_str()).expect("declared");
                let id = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(LibraryId::from_uuid)
                    .unwrap_or_default();
                let library = resolve_library(spec, id, &profiles, &root_folders)?;
                if commit {
                    db.config().upsert_library(&library).await?;
                }
                record(
                    db,
                    kind::LIBRARY,
                    &item.name,
                    library.id.to_string(),
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if let Some(id) = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(LibraryId::from_uuid)
                {
                    if commit {
                        db.config().delete_library(id).await?;
                    }
                }
                forget(db, kind::LIBRARY, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Indexers ============================================================

async fn reconcile_indexers(
    db: &Database,
    specs: &[IndexerSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::INDEXER).await?;
    let tags = db.tags().list().await?;
    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|s| (s.name.clone(), content_hash(&hashable_indexer(s))))
        .collect();
    let plan = diff_kind(kind::INDEXER, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }
    let by_name: BTreeMap<&str, &IndexerSpec> =
        specs.iter().map(|s| (s.name.as_str(), s)).collect();

    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                let spec = by_name.get(item.name.as_str()).expect("declared");
                let id = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(cellarr_core::IndexerId::from_uuid)
                    .unwrap_or_default();
                let ix = resolve_indexer(spec, id, &tags)?;
                if commit {
                    db.config().upsert_indexer(&ix).await?;
                }
                record(
                    db,
                    kind::INDEXER,
                    &item.name,
                    ix.id.to_string(),
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if let Some(id) = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(cellarr_core::IndexerId::from_uuid)
                {
                    if commit {
                        db.config().delete_indexer(id).await?;
                    }
                }
                forget(db, kind::INDEXER, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Download clients ====================================================

async fn reconcile_download_clients(
    db: &Database,
    specs: &[DownloadClientSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::DOWNLOAD_CLIENT).await?;
    let tags = db.tags().list().await?;
    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|s| (s.name.clone(), content_hash(&hashable_download_client(s))))
        .collect();
    let plan = diff_kind(kind::DOWNLOAD_CLIENT, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }
    let by_name: BTreeMap<&str, &DownloadClientSpec> =
        specs.iter().map(|s| (s.name.as_str(), s)).collect();

    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                let spec = by_name.get(item.name.as_str()).expect("declared");
                let id = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(cellarr_core::DownloadClientId::from_uuid)
                    .unwrap_or_default();
                let dc = resolve_download_client(spec, id, &tags)?;
                if commit {
                    db.config().upsert_download_client(&dc).await?;
                }
                record(
                    db,
                    kind::DOWNLOAD_CLIENT,
                    &item.name,
                    dc.id.to_string(),
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if let Some(id) = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(cellarr_core::DownloadClientId::from_uuid)
                {
                    if commit {
                        db.config().delete_download_client(id).await?;
                    }
                }
                forget(db, kind::DOWNLOAD_CLIENT, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Release profiles ====================================================

async fn reconcile_release_profiles(
    db: &Database,
    specs: &[ReleaseProfileSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::RELEASE_PROFILE).await?;
    let tags = db.tags().list().await?;
    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|s| (s.name.clone(), content_hash(&hashable_release_profile(s))))
        .collect();
    let plan = diff_kind(kind::RELEASE_PROFILE, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }
    let by_name: BTreeMap<&str, &ReleaseProfileSpec> =
        specs.iter().map(|s| (s.name.as_str(), s)).collect();

    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                let spec = by_name.get(item.name.as_str()).expect("declared");
                let id = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(ReleaseProfileId::from_uuid)
                    .unwrap_or_default();
                let rp = resolve_release_profile(spec, id, &tags)?;
                if commit {
                    db.profiles().upsert_release_profile(&rp).await?;
                }
                record(
                    db,
                    kind::RELEASE_PROFILE,
                    &item.name,
                    rp.id.to_string(),
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if let Some(id) = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(ReleaseProfileId::from_uuid)
                {
                    if commit {
                        db.profiles().delete_release_profile(id).await?;
                    }
                }
                forget(db, kind::RELEASE_PROFILE, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Delay profiles ======================================================

async fn reconcile_delay_profiles(
    db: &Database,
    specs: &[DelayProfileSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::DELAY_PROFILE).await?;
    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|s| (s.name.clone(), content_hash(&hashable_delay_profile(s))))
        .collect();
    let plan = diff_kind(kind::DELAY_PROFILE, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }
    let by_name: BTreeMap<&str, &DelayProfileSpec> =
        specs.iter().map(|s| (s.name.as_str(), s)).collect();

    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                let spec = by_name.get(item.name.as_str()).expect("declared");
                let id = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(DelayProfileId::from_uuid)
                    .unwrap_or_default();
                let dp = spec_to_delay_profile(spec, id);
                if commit {
                    db.profiles().upsert_delay_profile(&dp).await?;
                }
                record(
                    db,
                    kind::DELAY_PROFILE,
                    &item.name,
                    dp.id.to_string(),
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if let Some(id) = item
                    .entity_id
                    .as_deref()
                    .and_then(|s| s.parse::<uuid::Uuid>().ok())
                    .map(DelayProfileId::from_uuid)
                {
                    if commit {
                        db.profiles().delete_delay_profile(id).await?;
                    }
                }
                forget(db, kind::DELAY_PROFILE, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Import lists ========================================================

async fn reconcile_import_lists(
    db: &Database,
    specs: &[ImportListSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::IMPORT_LIST).await?;
    // Resolve the referenced quality profiles (now live) by name.
    let profiles = db.profiles().list_profiles().await?;
    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|s| (s.name.clone(), content_hash(&hashable_import_list(s))))
        .collect();
    let plan = diff_kind(kind::IMPORT_LIST, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }
    let by_name: BTreeMap<&str, &ImportListSpec> =
        specs.iter().map(|s| (s.name.as_str(), s)).collect();

    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                let spec = by_name.get(item.name.as_str()).expect("declared");
                // A new list gets a fresh uuid id; an update reuses the ledger id.
                let id = item
                    .entity_id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let list = resolve_import_list(spec, id.clone(), &profiles)?;
                if commit {
                    db.import_lists().upsert(&list).await?;
                }
                record(
                    db,
                    kind::IMPORT_LIST,
                    &item.name,
                    id,
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if let Some(id) = &item.entity_id {
                    if commit {
                        db.import_lists().delete(id).await?;
                    }
                }
                forget(db, kind::IMPORT_LIST, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Notifications =======================================================

async fn reconcile_notifications(
    db: &Database,
    specs: &[NotificationSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::NOTIFICATION).await?;
    let tags = db.tags().list().await?;
    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|s| (s.name.clone(), content_hash(&hashable_notification(s))))
        .collect();
    let plan = diff_kind(kind::NOTIFICATION, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }
    let by_name: BTreeMap<&str, &NotificationSpec> =
        specs.iter().map(|s| (s.name.as_str(), s)).collect();

    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                let spec = by_name.get(item.name.as_str()).expect("declared");
                let id = item
                    .entity_id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let n = resolve_notification(spec, id.clone(), &tags)?;
                if commit {
                    db.config().upsert_notification(&n).await?;
                }
                record(
                    db,
                    kind::NOTIFICATION,
                    &item.name,
                    id,
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if let Some(id) = &item.entity_id {
                    if commit {
                        db.config().delete_notification(id).await?;
                    }
                }
                forget(db, kind::NOTIFICATION, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Remote-path mappings ================================================

async fn reconcile_remote_path_mappings(
    db: &Database,
    specs: &[RemotePathMappingSpec],
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db
        .managed_config()
        .list_kind(kind::REMOTE_PATH_MAPPING)
        .await?;
    let declared: Vec<(String, String)> = specs
        .iter()
        .map(|s| {
            (
                s.name.clone(),
                content_hash(&hashable_remote_path_mapping(s)),
            )
        })
        .collect();
    let plan = diff_kind(kind::REMOTE_PATH_MAPPING, &declared, &ledger);
    if !commit {
        return Ok(plan);
    }
    let by_name: BTreeMap<&str, &RemotePathMappingSpec> =
        specs.iter().map(|s| (s.name.as_str(), s)).collect();

    for item in &plan.items {
        match item.action {
            Action::Create | Action::Update => {
                let spec = by_name.get(item.name.as_str()).expect("declared");
                let id = item
                    .entity_id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let mapping = RemotePathMapping {
                    id: id.clone(),
                    host: spec.host.clone(),
                    remote_path: spec.remote_path.clone(),
                    local_path: spec.local_path.clone(),
                };
                if commit {
                    db.config().upsert_remote_path_mapping(&mapping).await?;
                }
                record(
                    db,
                    kind::REMOTE_PATH_MAPPING,
                    &item.name,
                    id,
                    item.content_hash.clone(),
                    commit,
                )
                .await?;
            }
            Action::Prune => {
                if let Some(id) = &item.entity_id {
                    if commit {
                        db.config().delete_remote_path_mapping(id).await?;
                    }
                }
                forget(db, kind::REMOTE_PATH_MAPPING, &item.name, commit).await?;
            }
            Action::Unchanged => {}
        }
    }
    Ok(plan)
}

// === Singletons (naming / media-management / auth) =======================
//
// A singleton is not a name-keyed list: there is exactly one document. It is
// reconciled as a whole-config *set* under a fixed ledger name. The ledger row
// tracks the content hash so a re-apply of an unchanged singleton is a no-op
// (`Unchanged`), an edit is an `Update`, and *omitting* the section leaves the
// live document untouched (the section's `Option` is `None`, so the reconcile fn
// is never called). A singleton is never pruned by declaring an "empty" — there is
// no empty form; to revert it an operator declares the explicit default. This is
// deliberate: zeroing global naming/auth by omission would be a footgun.

/// The fixed ledger name a singleton is tracked under (one row per singleton kind).
const SINGLETON_NAME: &str = "_singleton";

/// Build a one-item [`KindPlan`] for a singleton.
///
/// The verdict compares the **declared** document against what is **live** in the
/// DB (`live_hash`), not against the ledger: a singleton has exactly one document,
/// so the live document *is* its identity, and a declared singleton that already
/// matches the live state is `Unchanged` even if config has not tracked it before
/// (which is what makes `export → re-import` an empty plan — the exported document
/// equals the live one). When they differ the verdict is `Update` if config already
/// tracked the singleton, else `Create`. There is no prune branch: an omitted
/// section leaves the live document untouched (the reconcile fn is not called).
fn singleton_plan(
    kind: &str,
    declared_hash: &str,
    live_hash: &str,
    ledger: &[ManagedEntity],
) -> KindPlan {
    let existing = ledger.iter().find(|e| e.name == SINGLETON_NAME);
    let action = if declared_hash == live_hash {
        Action::Unchanged
    } else if existing.is_some() {
        Action::Update
    } else {
        Action::Create
    };
    KindPlan {
        kind: kind.to_string(),
        items: vec![PlanItem {
            name: SINGLETON_NAME.to_string(),
            action,
            content_hash: declared_hash.to_string(),
            entity_id: existing.map(|e| e.entity_id.clone()),
        }],
    }
}

async fn reconcile_naming(
    db: &Database,
    spec: &NamingSpec,
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::NAMING).await?;
    // Merge the declared naming onto the live media-management document so naming
    // and media-management can be declared independently without clobbering.
    let current = db.config().get_media_management().await?;
    let live_hash = content_hash(&current.naming);
    let naming = spec_to_naming(spec, &current.naming);
    let plan = singleton_plan(kind::NAMING, &content_hash(&naming), &live_hash, &ledger);
    if !commit {
        return Ok(plan);
    }
    if plan.items[0].action.is_change() {
        let mut mm = current;
        mm.naming = naming;
        db.config().set_media_management(&mm).await?;
        record(
            db,
            kind::NAMING,
            SINGLETON_NAME,
            SINGLETON_NAME.to_string(),
            plan.items[0].content_hash.clone(),
            commit,
        )
        .await?;
    }
    Ok(plan)
}

async fn reconcile_media_management(
    db: &Database,
    spec: &MediaManagementSpec,
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db
        .managed_config()
        .list_kind(kind::MEDIA_MANAGEMENT)
        .await?;
    // The naming sub-document is owned by its own section; preserve whatever is live
    // here so the two singletons compose. Hash only the fields this section owns.
    let current = db.config().get_media_management().await?;
    let live_hash = content_hash(&HashableMediaManagement {
        recycle_bin_path: &current.recycle_bin_path,
        permissions: &current.permissions,
        extra_files: &current.extra_files,
        write_nfo: current.write_nfo,
    });
    let plan = singleton_plan(
        kind::MEDIA_MANAGEMENT,
        &content_hash(&hashable_media_management(spec)),
        &live_hash,
        &ledger,
    );
    if !commit {
        return Ok(plan);
    }
    if plan.items[0].action.is_change() {
        let mm = MediaManagement {
            recycle_bin_path: spec.recycle_bin_path.clone(),
            naming: current.naming,
            permissions: spec.permissions.clone(),
            extra_files: spec.extra_files.clone(),
            write_nfo: spec.write_nfo,
        };
        db.config().set_media_management(&mm).await?;
        record(
            db,
            kind::MEDIA_MANAGEMENT,
            SINGLETON_NAME,
            SINGLETON_NAME.to_string(),
            plan.items[0].content_hash.clone(),
            commit,
        )
        .await?;
    }
    Ok(plan)
}

async fn reconcile_auth(
    db: &Database,
    spec: &AuthSpec,
    commit: bool,
) -> Result<KindPlan, ManagedError> {
    let ledger = db.managed_config().list_kind(kind::AUTH).await?;
    let live = db.auth().get_config().await?;
    let auth = cellarr_core::AuthConfig {
        method: spec.method,
        username: spec.username.clone(),
        password_hash: spec.password_hash.clone(),
    };
    let plan = singleton_plan(
        kind::AUTH,
        &content_hash(&auth),
        &content_hash(&live),
        &ledger,
    );
    if !commit {
        return Ok(plan);
    }
    if plan.items[0].action.is_change() {
        db.auth().set_config(&auth).await?;
        // Changing the credential/method invalidates any live session.
        db.auth().delete_all_sessions().await?;
        record(
            db,
            kind::AUTH,
            SINGLETON_NAME,
            SINGLETON_NAME.to_string(),
            plan.items[0].content_hash.clone(),
            commit,
        )
        .await?;
    }
    Ok(plan)
}

// === Generic id-kind apply helper ========================================

/// A small driver for a uuid-id kind whose create/update upserts an entity and
/// whose prune deletes by id, recording/forgetting the ledger row. Used by
/// `custom_format` (the others inline their resolution because it needs the
/// declared-name lookup before the closure boundary).
#[allow(clippy::too_many_arguments)]
async fn apply_id_kind<E, Build, Up, UpFut, Del, DelFut>(
    db: &Database,
    kind: &str,
    item: &PlanItem,
    commit: bool,
    build: Build,
    upsert: Up,
    delete: Del,
) -> Result<(), ManagedError>
where
    Build: FnOnce(Option<&str>) -> (String, E),
    Up: FnOnce(Database, E) -> UpFut,
    UpFut: std::future::Future<Output = Result<(), ManagedError>>,
    Del: FnOnce(Database, String) -> DelFut,
    DelFut: std::future::Future<Output = Result<(), ManagedError>>,
{
    match item.action {
        Action::Create | Action::Update => {
            let (id, entity) = build(item.entity_id.as_deref());
            if commit {
                upsert(db.clone(), entity).await?;
            }
            record(db, kind, &item.name, id, item.content_hash.clone(), commit).await?;
        }
        Action::Prune => {
            if let Some(id) = &item.entity_id {
                if commit {
                    delete(db.clone(), id.clone()).await?;
                }
            }
            forget(db, kind, &item.name, commit).await?;
        }
        Action::Unchanged => {}
    }
    Ok(())
}

// === Spec → core model mappers ===========================================

fn spec_to_quality_definition(s: &QualityDefinitionSpec) -> QualityDefinition {
    QualityDefinition {
        name: s.name.clone(),
        title: s.title.clone(),
        // The rank is code-owned; the override carries 0 here and the repo merges
        // it onto the catalogue by name (see `quality_ranking`). Keep it out of the
        // hash via `hashable` below so a rank change never appears as drift.
        rank: 0,
        min_size_per_min: s.min_size_per_min,
        max_size_per_min: s.max_size_per_min,
        preferred_size_per_min: s.preferred_size_per_min,
    }
}

/// Resolve a quality name to its rank against the effective catalogue.
fn resolve_quality_rank(name: &str, ranking: &QualityRanking) -> Result<u32, ManagedError> {
    ranking
        .by_name(name)
        .map(|q| q.rank)
        .ok_or_else(|| ManagedError::Validation(format!("unknown quality `{name}`")))
}

fn spec_to_quality_profile(
    s: &QualityProfileSpec,
    id: QualityProfileId,
    ranking: &QualityRanking,
) -> Result<QualityProfile, ManagedError> {
    let allowed_qualities: Vec<u32> = s
        .qualities
        .iter()
        .map(|q| resolve_quality_rank(q, ranking))
        .collect::<Result<_, _>>()?;
    let cutoff_quality = match &s.cutoff {
        Some(c) => resolve_quality_rank(c, ranking)?,
        None => allowed_qualities.iter().copied().max().unwrap_or(0),
    };
    Ok(QualityProfile {
        id,
        name: s.name.clone(),
        allowed_qualities,
        upgrades_allowed: s.upgrades_allowed,
        cutoff_quality,
        min_custom_format_score: s.min_custom_format_score,
        upgrade_until_custom_format_score: s.upgrade_until_custom_format_score,
        required_languages: s.required_languages.clone(),
    })
}

fn resolve_library(
    s: &LibrarySpec,
    id: LibraryId,
    profiles: &[QualityProfile],
    root_folders: &[RootFolder],
) -> Result<Library, ManagedError> {
    let profile = profiles
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(&s.quality_profile))
        .ok_or_else(|| {
            ManagedError::Validation(format!(
                "library `{}` references quality profile `{}`, which is not present",
                s.name, s.quality_profile
            ))
        })?;
    // Resolve root-folder names to their stored ids.
    let folder_ids: Vec<String> = s
        .root_folders
        .iter()
        .map(|rf_name| {
            root_folders
                .iter()
                .find(|rf| {
                    rf.name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(rf_name))
                })
                .map(|rf| rf.id.clone())
                .ok_or_else(|| {
                    ManagedError::Validation(format!(
                        "library `{}` references root folder `{}`, which is not present",
                        s.name, rf_name
                    ))
                })
        })
        .collect::<Result<_, _>>()?;
    Ok(Library {
        id,
        media_type: s.media_type,
        name: s.name.clone(),
        root_folders: folder_ids,
        default_quality_profile: profile.id,
    })
}

/// Resolve tag names to their live integer ids (validation + dependency order
/// guarantee they exist).
fn resolve_tag_ids(names: &[String], tags: &[cellarr_core::Tag]) -> Result<Vec<u32>, ManagedError> {
    names
        .iter()
        .map(|name| {
            tags.iter()
                .find(|t| t.label.eq_ignore_ascii_case(name))
                .map(|t| t.id)
                .ok_or_else(|| {
                    ManagedError::Validation(format!("tag `{name}` is referenced but not present"))
                })
        })
        .collect()
}

fn resolve_indexer(
    s: &IndexerSpec,
    id: cellarr_core::IndexerId,
    tags: &[cellarr_core::Tag],
) -> Result<IndexerConfig, ManagedError> {
    Ok(IndexerConfig {
        id,
        name: s.name.clone(),
        kind: s.kind.clone(),
        protocol: s.protocol,
        enabled: s.enabled,
        priority: s.priority,
        criteria: s.criteria.clone(),
        tags: resolve_tag_ids(&s.tags, tags)?,
        settings: s.settings.clone(),
    })
}

fn resolve_download_client(
    s: &DownloadClientSpec,
    id: cellarr_core::DownloadClientId,
    tags: &[cellarr_core::Tag],
) -> Result<DownloadClientConfig, ManagedError> {
    Ok(DownloadClientConfig {
        id,
        name: s.name.clone(),
        kind: s.kind.clone(),
        protocol: s.protocol,
        enabled: s.enabled,
        priority: s.priority,
        category: s.category.clone(),
        tags: resolve_tag_ids(&s.tags, tags)?,
        settings: s.settings.clone(),
    })
}

// === Hashable projections (id-free, so a minted id never shows as drift) ==

#[derive(serde::Serialize)]
struct HashableProfile<'a> {
    name: &'a str,
    allowed_ranks: Vec<u32>,
    upgrades_allowed: bool,
    cutoff_rank: u32,
    min_cf_score: i32,
    upgrade_until_cf_score: i32,
    required_languages: &'a [String],
    custom_format_scores: &'a BTreeMap<String, i32>,
}

fn hashable_profile<'a>(
    s: &'a QualityProfileSpec,
    ranking: &QualityRanking,
) -> Result<HashableProfile<'a>, ManagedError> {
    let allowed_ranks = s
        .qualities
        .iter()
        .map(|q| resolve_quality_rank(q, ranking))
        .collect::<Result<_, _>>()?;
    let cutoff_rank = match &s.cutoff {
        Some(c) => resolve_quality_rank(c, ranking)?,
        None => 0,
    };
    Ok(HashableProfile {
        name: &s.name,
        allowed_ranks,
        upgrades_allowed: s.upgrades_allowed,
        cutoff_rank,
        min_cf_score: s.min_custom_format_score,
        upgrade_until_cf_score: s.upgrade_until_custom_format_score,
        required_languages: &s.required_languages,
        custom_format_scores: &s.custom_format_scores,
    })
}

#[derive(serde::Serialize)]
struct HashableCustomFormat<'a> {
    name: &'a str,
    score: i32,
    conditions: &'a [cellarr_core::Condition],
}

fn hashable_custom_format(s: &CustomFormatSpec) -> HashableCustomFormat<'_> {
    HashableCustomFormat {
        name: &s.name,
        score: s.score,
        conditions: &s.conditions,
    }
}

#[derive(serde::Serialize)]
struct HashableRootFolder<'a> {
    name: &'a str,
    path: &'a str,
    enabled: bool,
}

fn hashable_root_folder(s: &RootFolderSpec) -> HashableRootFolder<'_> {
    HashableRootFolder {
        name: &s.name,
        path: &s.path,
        enabled: s.enabled,
    }
}

// Cross-referencing kinds (library / indexer / download client) hash their
// declared content by the **names** they reference, NOT by resolved DB ids. This
// is deliberate: a dry-run plan diffs without applying earlier sections, so a
// referent's id may not be live yet — hashing by stable declared names keeps the
// plan computable in dry-run and identical to the committed-apply plan
// (idempotency). The id resolution happens only at apply time.

#[derive(serde::Serialize)]
struct HashableLibrary<'a> {
    name: &'a str,
    media_type: cellarr_core::MediaType,
    root_folder_names: Vec<String>,
    quality_profile_name: &'a str,
}

fn hashable_library(s: &LibrarySpec) -> HashableLibrary<'_> {
    HashableLibrary {
        name: &s.name,
        media_type: s.media_type,
        root_folder_names: s
            .root_folders
            .iter()
            .map(|r| r.to_ascii_lowercase())
            .collect(),
        quality_profile_name: &s.quality_profile,
    }
}

#[derive(serde::Serialize)]
struct HashableIndexer<'a> {
    name: &'a str,
    kind: &'a str,
    protocol: cellarr_core::Protocol,
    enabled: bool,
    priority: i32,
    criteria: &'a cellarr_core::IndexerCriteria,
    tag_names: Vec<String>,
    settings: &'a serde_json::Value,
}

fn hashable_indexer(s: &IndexerSpec) -> HashableIndexer<'_> {
    HashableIndexer {
        name: &s.name,
        kind: &s.kind,
        protocol: s.protocol,
        enabled: s.enabled,
        priority: s.priority,
        criteria: &s.criteria,
        tag_names: s.tags.iter().map(|t| t.to_ascii_lowercase()).collect(),
        settings: &s.settings,
    }
}

#[derive(serde::Serialize)]
struct HashableDownloadClient<'a> {
    name: &'a str,
    kind: &'a str,
    protocol: cellarr_core::Protocol,
    enabled: bool,
    priority: i32,
    category: &'a str,
    tag_names: Vec<String>,
    settings: &'a serde_json::Value,
}

fn hashable_download_client(s: &DownloadClientSpec) -> HashableDownloadClient<'_> {
    HashableDownloadClient {
        name: &s.name,
        kind: &s.kind,
        protocol: s.protocol,
        enabled: s.enabled,
        priority: s.priority,
        category: &s.category,
        tag_names: s.tags.iter().map(|t| t.to_ascii_lowercase()).collect(),
        settings: &s.settings,
    }
}

// === Release / delay profiles =============================================

fn resolve_release_profile(
    s: &ReleaseProfileSpec,
    id: ReleaseProfileId,
    tags: &[cellarr_core::Tag],
) -> Result<ReleaseProfile, ManagedError> {
    Ok(ReleaseProfile {
        id,
        name: s.name.clone(),
        enabled: s.enabled,
        tags: resolve_tag_ids(&s.tags, tags)?,
        required: s.required.clone(),
        ignored: s.ignored.clone(),
        preferred: s.preferred.clone(),
    })
}

#[derive(serde::Serialize)]
struct HashableReleaseProfile<'a> {
    name: &'a str,
    enabled: bool,
    tag_names: Vec<String>,
    required: &'a [String],
    ignored: &'a [String],
    preferred: &'a [cellarr_core::PreferredTerm],
}

fn hashable_release_profile(s: &ReleaseProfileSpec) -> HashableReleaseProfile<'_> {
    HashableReleaseProfile {
        name: &s.name,
        enabled: s.enabled,
        tag_names: s.tags.iter().map(|t| t.to_ascii_lowercase()).collect(),
        required: &s.required,
        ignored: &s.ignored,
        preferred: &s.preferred,
    }
}

fn spec_to_delay_profile(s: &DelayProfileSpec, id: DelayProfileId) -> DelayProfile {
    DelayProfile {
        id,
        enabled: s.enabled,
        preferred_protocol: s.preferred_protocol,
        usenet_delay: s.usenet_delay,
        torrent_delay: s.torrent_delay,
        bypass_if_highest_quality: s.bypass_if_highest_quality,
        // The core model stores delay-profile tags as opaque label strings (no id
        // resolution), so they pass through verbatim.
        tags: s.tags.clone(),
        order: s.order,
    }
}

#[derive(serde::Serialize)]
struct HashableDelayProfile<'a> {
    // The config `name` IS part of identity (it is the ledger key) but the core
    // model has no name, so it is hashed here to detect a rename as a no-op edit —
    // a rename is a prune+create (different ledger key), not an in-place update,
    // which is the same identity model every other name-keyed kind uses.
    name: &'a str,
    enabled: bool,
    preferred_protocol: cellarr_core::PreferredProtocol,
    usenet_delay: u32,
    torrent_delay: u32,
    bypass_if_highest_quality: bool,
    tags: Vec<String>,
    order: i32,
}

fn hashable_delay_profile(s: &DelayProfileSpec) -> HashableDelayProfile<'_> {
    HashableDelayProfile {
        name: &s.name,
        enabled: s.enabled,
        preferred_protocol: s.preferred_protocol,
        usenet_delay: s.usenet_delay,
        torrent_delay: s.torrent_delay,
        bypass_if_highest_quality: s.bypass_if_highest_quality,
        tags: s.tags.iter().map(|t| t.to_ascii_lowercase()).collect(),
        order: s.order,
    }
}

// === Import lists =========================================================

fn resolve_import_list(
    s: &ImportListSpec,
    id: String,
    profiles: &[QualityProfile],
) -> Result<ImportListConfig, ManagedError> {
    let quality_profile_id = match &s.quality_profile {
        Some(name) => Some(
            profiles
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(name))
                .map(|p| p.id.to_string())
                .ok_or_else(|| {
                    ManagedError::Validation(format!(
                        "import list `{}` references quality profile `{}`, which is not present",
                        s.name, name
                    ))
                })?,
        ),
        None => None,
    };
    Ok(ImportListConfig {
        id,
        name: s.name.clone(),
        kind: s.kind.clone(),
        enabled: s.enabled,
        media_type: s.media_type,
        monitored: s.monitored,
        clean_action: s.clean_action,
        quality_profile_id,
        // `last_successful_sync` is operational state stamped by the sync job, never
        // declared in config; a managed list starts (and stays, on re-apply) unset
        // until the runner stamps it. Kept out of the hash below.
        last_successful_sync: None,
        settings: s.settings.clone(),
    })
}

#[derive(serde::Serialize)]
struct HashableImportList<'a> {
    name: &'a str,
    kind: &'a str,
    enabled: bool,
    media_type: cellarr_core::MediaType,
    monitored: bool,
    clean_action: cellarr_core::importlist::CleanAction,
    // Hash by the referenced profile NAME (not the resolved id), like libraries —
    // so the dry-run plan is computable before the profile exists and matches the
    // committed-apply plan.
    quality_profile_name: Option<String>,
    settings: &'a serde_json::Value,
}

fn hashable_import_list(s: &ImportListSpec) -> HashableImportList<'_> {
    HashableImportList {
        name: &s.name,
        kind: &s.kind,
        enabled: s.enabled,
        media_type: s.media_type,
        monitored: s.monitored,
        clean_action: s.clean_action,
        quality_profile_name: s.quality_profile.as_ref().map(|p| p.to_ascii_lowercase()),
        settings: &s.settings,
    }
}

// === Notifications ========================================================

fn resolve_notification(
    s: &NotificationSpec,
    id: String,
    tags: &[cellarr_core::Tag],
) -> Result<NotificationConfig, ManagedError> {
    Ok(NotificationConfig {
        id,
        name: s.name.clone(),
        kind: s.kind.clone(),
        enabled: s.enabled,
        on_events: s.on_events.clone(),
        tags: resolve_tag_ids(&s.tags, tags)?,
        settings: s.settings.clone(),
    })
}

#[derive(serde::Serialize)]
struct HashableNotification<'a> {
    name: &'a str,
    kind: &'a str,
    enabled: bool,
    on_events: &'a [String],
    tag_names: Vec<String>,
    settings: &'a serde_json::Value,
}

fn hashable_notification(s: &NotificationSpec) -> HashableNotification<'_> {
    HashableNotification {
        name: &s.name,
        kind: &s.kind,
        enabled: s.enabled,
        on_events: &s.on_events,
        tag_names: s.tags.iter().map(|t| t.to_ascii_lowercase()).collect(),
        settings: &s.settings,
    }
}

// === Remote-path mappings =================================================

#[derive(serde::Serialize)]
struct HashableRemotePathMapping<'a> {
    name: &'a str,
    host: &'a str,
    remote_path: &'a str,
    local_path: &'a str,
}

fn hashable_remote_path_mapping(s: &RemotePathMappingSpec) -> HashableRemotePathMapping<'_> {
    HashableRemotePathMapping {
        name: &s.name,
        host: &s.host,
        remote_path: &s.remote_path,
        local_path: &s.local_path,
    }
}

// === Singletons ===========================================================

/// Build a [`NamingFormats`] from the declared [`NamingSpec`], falling back to the
/// **live** naming for any field the spec leaves unset (so a partial naming
/// declaration edits only what it names and is idempotent against the live doc).
fn spec_to_naming(s: &NamingSpec, current: &NamingFormats) -> NamingFormats {
    NamingFormats {
        series_folder_format: s
            .series_folder_format
            .clone()
            .unwrap_or_else(|| current.series_folder_format.clone()),
        season_folder_format: s
            .season_folder_format
            .clone()
            .unwrap_or_else(|| current.season_folder_format.clone()),
        episode_file_format: s
            .episode_file_format
            .clone()
            .unwrap_or_else(|| current.episode_file_format.clone()),
        anime_episode_file_format: s
            .anime_episode_file_format
            .clone()
            .unwrap_or_else(|| current.anime_episode_file_format.clone()),
        movie_file_format: s
            .movie_file_format
            .clone()
            .unwrap_or_else(|| current.movie_file_format.clone()),
    }
}

#[derive(serde::Serialize)]
struct HashableMediaManagement<'a> {
    recycle_bin_path: &'a Option<String>,
    permissions: &'a cellarr_core::ImportPermissions,
    extra_files: &'a cellarr_core::ExtraFileImport,
    write_nfo: bool,
}

fn hashable_media_management(s: &MediaManagementSpec) -> HashableMediaManagement<'_> {
    HashableMediaManagement {
        recycle_bin_path: &s.recycle_bin_path,
        permissions: &s.permissions,
        extra_files: &s.extra_files,
        write_nfo: s.write_nfo,
    }
}
