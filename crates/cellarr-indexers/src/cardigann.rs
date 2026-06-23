//! The Cardigann YAML engine (skeleton).
//!
//! Hundreds of trackers are described declaratively as **Cardigann YAML**
//! definitions — login, search paths, and row/field selectors with regex filters
//! and templating. These are *data, not code*; cellarr ships a generic engine
//! that interprets a user-supplied definition at runtime and exposes the tracker
//! as a normal indexer (`docs/06-integrations.md`).
//!
//! **Licensing:** the community definitions repo has no declared license, so
//! definitions are **never vendored** into this repo. Users point cellarr at a
//! definitions source they choose; this engine only interprets whatever YAML it
//! is handed (`docs/agents/legal-and-licensing.md`).
//!
//! **Scope of this skeleton:** parse a definition's `id`/`name`/`caps`/`search`
//! (`rows`, `fields`), and run **CSS** selector extraction over a fetched
//! document into [`Release`] values. XPath selectors and the login/download flows
//! are explicit follow-ups (see the notes in `tests/fixtures`).

use cellarr_core::{IndexerId, Protocol, Release};
use scraper::{Html, Selector};
use serde::Deserialize;

use crate::error::{IndexerError, Result};

/// A parsed Cardigann definition (the subset this skeleton interprets).
#[derive(Debug, Clone, Deserialize)]
pub struct Definition {
    /// Stable tracker id (e.g. `mytracker`).
    pub id: String,
    /// Human-facing name.
    pub name: String,
    /// Declared capabilities (category mappings + modes).
    #[serde(default)]
    pub caps: DefinitionCaps,
    /// Search configuration (paths, row selector, field selectors).
    pub search: SearchBlock,
    /// Result protocol; Cardigann's `type` is `private`/`public`/`semi-private`
    /// (an access level), so the download protocol is configured separately and
    /// defaults to torrent, which is what the vast majority of these trackers are.
    #[serde(default)]
    pub protocol: ProtocolHint,
}

/// Protocol hint for a Cardigann definition's results.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolHint {
    /// BitTorrent (the default; almost all Cardigann trackers are torrent).
    #[default]
    Torrent,
    /// Usenet.
    Usenet,
}

impl From<ProtocolHint> for Protocol {
    fn from(hint: ProtocolHint) -> Self {
        match hint {
            ProtocolHint::Torrent => Protocol::Torrent,
            ProtocolHint::Usenet => Protocol::Usenet,
        }
    }
}

/// The `caps` block: category mappings and supported modes.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DefinitionCaps {
    /// Tracker-category → Torznab-category mappings.
    #[serde(default)]
    pub categorymappings: Vec<CategoryMapping>,
    /// Supported modes, mapping a `t=` value to the params it accepts.
    #[serde(default)]
    pub modes: std::collections::BTreeMap<String, Vec<String>>,
}

/// One tracker→Torznab category mapping.
#[derive(Debug, Clone, Deserialize)]
pub struct CategoryMapping {
    /// The tracker's own category id, as a string (definitions quote it).
    pub id: String,
    /// The Torznab category to map it to (thousands-based scheme).
    pub cat: String,
    /// Optional human description.
    #[serde(default)]
    pub desc: Option<String>,
}

/// The `search` block.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchBlock {
    /// Request paths the search hits (only the shape is parsed in this skeleton).
    #[serde(default)]
    pub paths: Vec<SearchPath>,
    /// The selector that yields one element per result row.
    pub rows: RowsBlock,
    /// Per-field selectors keyed by field name (`title`, `download`, `size`, …).
    pub fields: std::collections::BTreeMap<String, Field>,
}

/// One configured search path.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchPath {
    /// The path template (e.g. `/torrents.php`).
    pub path: String,
    /// Optional method (`get`/`post`); defaults to GET semantics.
    #[serde(default)]
    pub method: Option<String>,
}

/// The `rows` block: the selector that delimits result rows.
#[derive(Debug, Clone, Deserialize)]
pub struct RowsBlock {
    /// CSS selector matching each result row.
    pub selector: String,
}

/// A field extraction rule.
///
/// In real Cardigann a field may use a `selector`, read an `attribute`, apply
/// `filters` (regex replace, etc.), or be a `text:`/template literal. This
/// skeleton supports selector + attribute + an optional literal `text`.
#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    /// CSS selector relative to the row (omitted for a literal `text` field).
    #[serde(default)]
    pub selector: Option<String>,
    /// Attribute to read instead of the element's text (e.g. `href`, `title`).
    #[serde(default)]
    pub attribute: Option<String>,
    /// A literal value (Cardigann's `text:` form); used when there is no selector.
    #[serde(default)]
    pub text: Option<String>,
}

impl Definition {
    /// Parse a Cardigann definition from YAML.
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).map_err(|e| IndexerError::Definition(format!("yaml parse: {e}")))
    }

    /// Whether the definition advertises the given `t=` mode.
    #[must_use]
    pub fn has_mode(&self, mode: &str) -> bool {
        self.caps.modes.contains_key(mode)
    }
}

/// A runtime engine bound to one parsed definition.
pub struct CardigannIndexer {
    indexer_id: IndexerId,
    definition: Definition,
}

impl CardigannIndexer {
    /// Build an engine from a parsed definition.
    #[must_use]
    pub fn new(indexer_id: IndexerId, definition: Definition) -> Self {
        Self {
            indexer_id,
            definition,
        }
    }

    /// The tracker's human-facing name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.definition.name
    }

    /// The parsed definition.
    #[must_use]
    pub fn definition(&self) -> &Definition {
        &self.definition
    }

    /// Extract releases from an already-fetched HTML document by running the
    /// definition's `rows`/`fields` selectors over it.
    ///
    /// This is the heart of the engine: it is deliberately I/O-free so it can be
    /// exercised against recorded HTML in the record/replay tests. The fetch +
    /// login + URL-templating layers wrap this and are the documented follow-ups.
    pub fn extract(&self, html: &str) -> Result<Vec<Release>> {
        let document = Html::parse_document(html);
        let search = &self.definition.search;

        let row_selector = Selector::parse(&search.rows.selector).map_err(|e| {
            IndexerError::Definition(format!(
                "invalid row selector '{}': {e}",
                search.rows.selector
            ))
        })?;

        // Pre-compile field selectors once.
        let mut field_selectors = Vec::new();
        for (name, field) in &search.fields {
            let selector = match &field.selector {
                Some(sel) => Some(Selector::parse(sel).map_err(|e| {
                    IndexerError::Definition(format!("invalid field selector '{sel}': {e}"))
                })?),
                None => None,
            };
            field_selectors.push((name.clone(), field.clone(), selector));
        }

        let protocol: Protocol = self.definition.protocol.into();
        let mut releases = Vec::new();

        for row in document.select(&row_selector) {
            let mut title = None;
            let mut download_url = None;
            let mut guid = None;
            let mut size = None;
            let mut seeders = None;

            for (name, field, selector) in &field_selectors {
                let value = extract_field(&row, field, selector.as_ref());
                let Some(value) = value else { continue };
                match name.as_str() {
                    "title" => title = Some(value),
                    // `download` is the magnet/.torrent/.nzb link; `details` is
                    // the human page used as a stable guid when present.
                    "download" => download_url = Some(value),
                    "details" | "guid" => guid = Some(value),
                    "size" => size = parse_size(&value),
                    "seeders" => seeders = value.replace(',', "").trim().parse().ok(),
                    _ => {}
                }
            }

            if let (Some(title), Some(download_url)) = (title, download_url) {
                releases.push(Release {
                    indexer_id: self.indexer_id,
                    title,
                    download_url,
                    guid,
                    protocol,
                    size,
                    seeders,
                    indexer_flags: Vec::new(),
                });
            }
        }

        Ok(releases)
    }
}

/// Extract one field's string value from a row element.
fn extract_field(
    row: &scraper::ElementRef,
    field: &Field,
    selector: Option<&Selector>,
) -> Option<String> {
    // A literal `text:` field with no selector resolves to its literal value.
    if selector.is_none() {
        return field.text.clone();
    }
    let selector = selector?;
    // Cardigann selectors are relative to the row; the row element itself can
    // also match, so try the row first then its descendants.
    let element = if row.value().name() != "html" && matches_self(row, selector) {
        Some(*row)
    } else {
        row.select(selector).next()
    };
    let element = element?;

    match &field.attribute {
        Some(attr) => element.value().attr(attr).map(str::to_string),
        None => {
            let text: String = element.text().collect::<String>().trim().to_string();
            Some(text)
        }
    }
}

/// Whether the row element itself matches the selector (so selectors anchored on
/// the row, not its children, still resolve).
fn matches_self(row: &scraper::ElementRef, selector: &Selector) -> bool {
    selector.matches(row)
}

/// Parse a human size string (`"1.5 GB"`, `"700 MB"`, `"1234567"`) into bytes.
fn parse_size(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // Plain integer byte counts are common in JSON-ish feeds.
    if let Ok(bytes) = raw.replace(',', "").parse::<u64>() {
        return Some(bytes);
    }

    let lower = raw.to_ascii_lowercase();
    let (num_part, unit) = lower
        .find(|c: char| c.is_ascii_alphabetic())
        .map(|idx| (lower[..idx].trim(), lower[idx..].trim()))?;
    let value: f64 = num_part.replace(',', "").parse().ok()?;
    let multiplier: f64 = match unit {
        "b" => 1.0,
        "kb" | "kib" => 1024.0,
        "mb" | "mib" => 1024.0 * 1024.0,
        "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((value * multiplier) as u64)
}
