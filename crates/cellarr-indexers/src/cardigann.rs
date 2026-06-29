//! The Cardigann YAML engine.
//!
//! Hundreds of trackers are described declaratively as **Cardigann YAML**
//! definitions — links, search paths, and row/field selectors with filter chains
//! and templating. These are *data, not code*; cellarr ships a generic engine that
//! interprets a user-supplied definition at runtime and exposes the tracker as a
//! normal [`Indexer`] (`docs/06-integrations.md`).
//!
//! **Licensing:** the community definitions repo has no declared license, so
//! definitions are **never vendored** into this repo. Users point cellarr at a
//! definitions source they choose; this engine only interprets whatever YAML it is
//! handed (`docs/agents/legal-and-licensing.md`).
//!
//! **Supported subset.** This engine interprets the parts of the format that cover
//! the great majority of *public* trackers:
//!
//! - `links` → the site base, used to resolve relative download/details URLs.
//! - `search.paths[].path` + `inputs` → the request, with templating for
//!   `{{ .Keywords }}` / `{{ .Query.Keywords }}` / `{{ .Config.<key> }}` and quoted
//!   literals.
//! - `search.rows.selector` + `search.fields` → **CSS** row/field extraction, each
//!   field reading element text or an `attribute`, then running a **filter chain**
//!   (`regexp`, `re_replace`, `replace`, `split`, `querystring`, `append`,
//!   `prepend`, `trim`, `tolower`, `toupper`).
//! - `caps.categorymappings` → tracker-category ↔ Torznab-category mapping.
//!
//! Unsupported constructs (XPath selectors, `range`/`if` template control flow, an
//! unknown filter) are rejected as a [`IndexerError::Definition`] at extract time
//! rather than silently producing wrong data — the integration layer must never be
//! quietly incorrect.
//!
//! **Record/replay.** The HTTP seam is the [`Fetcher`] trait, so [`Indexer::search`]
//! is exercised against recorded responses; a live tracker is never a test
//! dependency.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::{Indexer, IndexerId, Protocol, Release, SearchTerms};
use regex::Regex;
use scraper::{Html, Selector};
use serde::Deserialize;
use url::Url;

use crate::error::{IndexerError, Result};
use crate::http::{Fetcher, ReqwestFetcher};
use crate::ratelimit::HostRateLimiter;

/// A parsed Cardigann definition (the subset this engine interprets).
#[derive(Debug, Clone, Deserialize)]
pub struct Definition {
    /// Stable tracker id (e.g. `mytracker`).
    pub id: String,
    /// Human-facing name.
    pub name: String,
    /// Site links; the first is used as the base for relative-URL resolution and
    /// for building the search request.
    #[serde(default)]
    pub links: Vec<String>,
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
    pub modes: BTreeMap<String, Vec<String>>,
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
    /// Request paths the search hits.
    #[serde(default)]
    pub paths: Vec<SearchPath>,
    /// The selector that yields one element per result row.
    pub rows: RowsBlock,
    /// Per-field selectors keyed by field name (`title`, `download`, `size`, …).
    pub fields: BTreeMap<String, Field>,
}

/// One configured search path.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchPath {
    /// The path template (e.g. `/torrents.php`), rendered against the query.
    pub path: String,
    /// Optional method (`get`/`post`); defaults to GET semantics.
    #[serde(default)]
    pub method: Option<String>,
    /// Query inputs (`name → template`), e.g. `q: "{{ .Keywords }}"`. Each value is
    /// rendered against the query and appended to the request URL.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
}

/// The `rows` block: the selector that delimits result rows.
#[derive(Debug, Clone, Deserialize)]
pub struct RowsBlock {
    /// CSS selector matching each result row.
    pub selector: String,
}

/// A field extraction rule.
///
/// A field reads an element's text (or an `attribute`), or resolves to a literal
/// `text`, then runs its `filters` chain in order. Selectors are CSS, relative to
/// the row.
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
    /// Filter chain applied, in order, to the extracted value.
    #[serde(default)]
    pub filters: Vec<Filter>,
}

/// One filter in a field's chain. `args` is a string, a list, or null depending on
/// the filter (interpreted in [`CompiledFilter`]).
#[derive(Debug, Clone, Deserialize)]
pub struct Filter {
    /// The filter name (e.g. `regexp`, `replace`, `querystring`).
    pub name: String,
    /// The filter's arguments, shape depending on the filter.
    #[serde(default)]
    pub args: serde_yaml::Value,
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

    /// Map a tracker category id to its Torznab category, per the definition's
    /// `categorymappings`. Returns `None` when the id is unmapped.
    #[must_use]
    pub fn torznab_category(&self, tracker_cat: &str) -> Option<&str> {
        self.caps
            .categorymappings
            .iter()
            .find(|m| m.id == tracker_cat)
            .map(|m| m.cat.as_str())
    }
}

// ---------------------------------------------------------------------------
// Templating: the `{{ .Keywords }}` / `{{ .Config.<key> }}` subset.
// ---------------------------------------------------------------------------

/// The values a template can reference.
struct TemplateContext<'a> {
    keywords: &'a str,
    config: &'a BTreeMap<String, String>,
}

/// Render a Cardigann template string, substituting `{{ … }}` expressions.
///
/// Supports `{{ .Keywords }}`, `{{ .Query.Keywords }}`, `{{ .Config.<key> }}` and
/// quoted literals. Any other expression is an unsupported-construct error, so a
/// definition that needs template control flow fails loudly instead of silently
/// rendering wrong.
fn render_template(tmpl: &str, ctx: &TemplateContext) -> Result<String> {
    let mut out = String::with_capacity(tmpl.len());
    let mut rest = tmpl;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| {
            IndexerError::Definition(format!("unterminated template expression in '{tmpl}'"))
        })?;
        let expr = after[..end].trim();
        out.push_str(&resolve_expr(expr, ctx)?);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Resolve a single trimmed `{{ … }}` expression.
fn resolve_expr(expr: &str, ctx: &TemplateContext) -> Result<String> {
    match expr {
        ".Keywords" | ".Query.Keywords" | ".Query.q" => Ok(ctx.keywords.to_string()),
        e if e.starts_with(".Config.") => Ok(ctx
            .config
            .get(&e[".Config.".len()..])
            .cloned()
            .unwrap_or_default()),
        // A quoted literal: `{{ "x" }}`.
        e if e.len() >= 2 && e.starts_with('"') && e.ends_with('"') => {
            Ok(e[1..e.len() - 1].to_string())
        }
        other => Err(IndexerError::Definition(format!(
            "unsupported template expression: {{{{ {other} }}}}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Filters: the post-extraction transform chain.
// ---------------------------------------------------------------------------

/// A filter compiled from a [`Filter`] (args validated, any regex pre-compiled),
/// so applying it per-row is infallible.
enum CompiledFilter {
    /// Extract capture group 1 (or the whole match) of a regex; empty if no match.
    Regexp(Regex),
    /// Regex search-and-replace.
    ReReplace(Regex, String),
    /// Literal `from → to` replacement.
    Replace(String, String),
    /// Split on a separator and take the element at an index.
    Split(String, usize),
    /// Read a query-string parameter out of a URL/query value.
    Querystring(String),
    /// Append a literal.
    Append(String),
    /// Prepend a literal.
    Prepend(String),
    /// Trim whitespace (or a given cutset).
    Trim(Option<String>),
    /// Lowercase.
    ToLower,
    /// Uppercase.
    ToUpper,
}

impl CompiledFilter {
    /// Compile one filter, validating its arguments up front.
    fn compile(filter: &Filter) -> Result<Self> {
        let name = filter.name.as_str();
        match name {
            "regexp" => Ok(Self::Regexp(compile_regex(&arg_one(filter)?)?)),
            "re_replace" => {
                let (pat, rep) = arg_two(filter)?;
                Ok(Self::ReReplace(compile_regex(&pat)?, rep))
            }
            "replace" => {
                let (from, to) = arg_two(filter)?;
                Ok(Self::Replace(from, to))
            }
            "split" => {
                let (sep, idx) = arg_two(filter)?;
                let idx: usize = idx.trim().parse().map_err(|_| {
                    IndexerError::Definition(format!("split index not a number: '{idx}'"))
                })?;
                Ok(Self::Split(sep, idx))
            }
            "querystring" => Ok(Self::Querystring(arg_one(filter)?)),
            "append" => Ok(Self::Append(arg_one(filter)?)),
            "prepend" => Ok(Self::Prepend(arg_one(filter)?)),
            "trim" => Ok(Self::Trim(arg_one(filter).ok())),
            "tolower" => Ok(Self::ToLower),
            "toupper" => Ok(Self::ToUpper),
            other => Err(IndexerError::Definition(format!(
                "unsupported field filter: '{other}'"
            ))),
        }
    }

    /// Apply the filter to a value.
    fn apply(&self, value: String) -> String {
        match self {
            Self::Regexp(re) => re
                .captures(&value)
                .and_then(|c| c.get(1).or_else(|| c.get(0)))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            Self::ReReplace(re, rep) => re.replace_all(&value, rep.as_str()).into_owned(),
            Self::Replace(from, to) => value.replace(from, to),
            Self::Split(sep, idx) => value
                .split(sep.as_str())
                .nth(*idx)
                .unwrap_or("")
                .to_string(),
            Self::Querystring(key) => querystring(&value, key),
            Self::Append(s) => value + s,
            Self::Prepend(s) => format!("{s}{value}"),
            Self::Trim(cutset) => match cutset {
                Some(c) => value.trim_matches(|ch: char| c.contains(ch)).to_string(),
                None => value.trim().to_string(),
            },
            Self::ToLower => value.to_lowercase(),
            Self::ToUpper => value.to_uppercase(),
        }
    }
}

/// Compile a regex, mapping the error to a definition error.
fn compile_regex(pattern: &str) -> Result<Regex> {
    Regex::new(pattern)
        .map_err(|e| IndexerError::Definition(format!("invalid filter regex '{pattern}': {e}")))
}

/// A filter's single string argument (`args: "x"` or `args: ["x"]`).
fn arg_one(filter: &Filter) -> Result<String> {
    match &filter.args {
        serde_yaml::Value::String(s) => Ok(s.clone()),
        serde_yaml::Value::Number(n) => Ok(n.to_string()),
        serde_yaml::Value::Sequence(seq) if seq.len() == 1 => yaml_string(&seq[0]),
        _ => Err(IndexerError::Definition(format!(
            "filter '{}' expects one argument",
            filter.name
        ))),
    }
}

/// A filter's two string arguments (`args: ["a", "b"]`).
fn arg_two(filter: &Filter) -> Result<(String, String)> {
    match &filter.args {
        serde_yaml::Value::Sequence(seq) if seq.len() == 2 => {
            Ok((yaml_string(&seq[0])?, yaml_string(&seq[1])?))
        }
        _ => Err(IndexerError::Definition(format!(
            "filter '{}' expects two arguments",
            filter.name
        ))),
    }
}

/// Coerce a scalar YAML value to a string.
fn yaml_string(v: &serde_yaml::Value) -> Result<String> {
    match v {
        serde_yaml::Value::String(s) => Ok(s.clone()),
        serde_yaml::Value::Number(n) => Ok(n.to_string()),
        serde_yaml::Value::Bool(b) => Ok(b.to_string()),
        _ => Err(IndexerError::Definition(
            "filter argument is not a scalar".into(),
        )),
    }
}

/// Extract a query-string parameter from a URL or bare query value.
fn querystring(value: &str, key: &str) -> String {
    let query = value.split_once('?').map_or(value, |(_, q)| q);
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The engine.
// ---------------------------------------------------------------------------

/// A runtime engine bound to one parsed definition.
pub struct CardigannIndexer {
    indexer_id: IndexerId,
    definition: Definition,
    /// Site base, parsed from `definition.links[0]`, used to resolve relative URLs
    /// and build search requests. `None` when no usable link is configured.
    base: Option<Url>,
    /// User-supplied settings for `{{ .Config.<key> }}` (e.g. a passkey).
    config: BTreeMap<String, String>,
    fetcher: Arc<dyn Fetcher>,
    rate_limiter: Arc<HostRateLimiter>,
}

impl CardigannIndexer {
    /// Build an engine from a parsed definition, using the default `reqwest`
    /// fetcher and a conservative shared rate limiter.
    #[must_use]
    pub fn new(indexer_id: IndexerId, definition: Definition) -> Self {
        Self::with_deps(
            indexer_id,
            definition,
            BTreeMap::new(),
            Arc::new(ReqwestFetcher::default()),
            Arc::new(HostRateLimiter::conservative_default()),
        )
    }

    /// Build with explicit config + dependencies (used by tests for record/replay
    /// and by the integration layer to inject a DB-backed fetcher).
    #[must_use]
    pub fn with_deps(
        indexer_id: IndexerId,
        definition: Definition,
        config: BTreeMap<String, String>,
        fetcher: Arc<dyn Fetcher>,
        rate_limiter: Arc<HostRateLimiter>,
    ) -> Self {
        let base = definition.links.first().and_then(|l| Url::parse(l).ok());
        Self {
            indexer_id,
            definition,
            base,
            config,
            fetcher,
            rate_limiter,
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

    /// Build the search request URLs for the given keywords, by rendering each
    /// configured path + its inputs against the query.
    fn search_urls(&self, keywords: &str) -> Result<Vec<Url>> {
        let base = self
            .base
            .as_ref()
            .ok_or_else(|| IndexerError::Definition("definition has no usable site link".into()))?;
        let ctx = TemplateContext {
            keywords,
            config: &self.config,
        };
        if self.definition.search.paths.is_empty() {
            return Err(IndexerError::Definition("search has no paths".into()));
        }
        let mut urls = Vec::with_capacity(self.definition.search.paths.len());
        for path in &self.definition.search.paths {
            let rendered = render_template(&path.path, &ctx)?;
            let mut url = base.join(&rendered).map_err(|e| {
                IndexerError::Definition(format!("bad search path '{rendered}': {e}"))
            })?;
            {
                let mut qp = url.query_pairs_mut();
                for (name, template) in &path.inputs {
                    qp.append_pair(name, &render_template(template, &ctx)?);
                }
            }
            urls.push(url);
        }
        Ok(urls)
    }

    /// Resolve a possibly-relative URL against the site base. An absolute URL (a
    /// magnet link, or a full `http(s)` link) is returned unchanged.
    fn resolve_url(&self, value: &str) -> String {
        match &self.base {
            Some(base) => base
                .join(value)
                .map_or_else(|_| value.to_string(), |u| u.to_string()),
            None => value.to_string(),
        }
    }

    /// Extract releases from an already-fetched HTML document by running the
    /// definition's `rows`/`fields` selectors + filter chains over it, resolving
    /// download/details URLs against the site base.
    ///
    /// This is the heart of the engine and is deliberately I/O-free so it can be
    /// exercised against recorded HTML in the record/replay tests.
    pub fn extract(&self, html: &str) -> Result<Vec<Release>> {
        let document = Html::parse_document(html);
        let search = &self.definition.search;

        let row_selector = Selector::parse(&search.rows.selector).map_err(|e| {
            IndexerError::Definition(format!(
                "invalid row selector '{}': {e}",
                search.rows.selector
            ))
        })?;

        // Pre-compile field selectors and filter chains once.
        let mut fields = Vec::with_capacity(search.fields.len());
        for (name, field) in &search.fields {
            let selector = match &field.selector {
                Some(sel) => Some(Selector::parse(sel).map_err(|e| {
                    IndexerError::Definition(format!("invalid field selector '{sel}': {e}"))
                })?),
                None => None,
            };
            let filters = field
                .filters
                .iter()
                .map(CompiledFilter::compile)
                .collect::<Result<Vec<_>>>()?;
            fields.push((name.clone(), field.clone(), selector, filters));
        }

        let protocol: Protocol = self.definition.protocol.into();
        let mut releases = Vec::new();

        for row in document.select(&row_selector) {
            let mut title = None;
            let mut download_url = None;
            let mut guid = None;
            let mut size = None;
            let mut seeders = None;

            for (name, field, selector, filters) in &fields {
                let Some(raw) = extract_field(&row, field, selector.as_ref()) else {
                    continue;
                };
                let value = filters.iter().fold(raw, |acc, f| f.apply(acc));
                match name.as_str() {
                    "title" => title = Some(value),
                    // `download` is the magnet/.torrent/.nzb link; `details` is the
                    // human page used as a stable guid when present.
                    "download" => download_url = Some(self.resolve_url(&value)),
                    "details" | "guid" => guid = Some(self.resolve_url(&value)),
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

    /// Fetch one search URL (rate-limited) and extract its releases.
    async fn fetch_and_extract(&self, url: &Url) -> Result<Vec<Release>> {
        if let Some(host) = url.host_str() {
            self.rate_limiter.until_ready(host).await;
        }
        let body = self.fetcher.get(url.as_str()).await?;
        self.extract(&body)
    }
}

#[async_trait]
impl Indexer for CardigannIndexer {
    type Error = IndexerError;

    fn name(&self) -> &str {
        &self.definition.name
    }

    async fn search(&self, terms: &SearchTerms) -> Result<Vec<Release>> {
        let keywords = terms.queries.first().map(String::as_str).unwrap_or("");
        let mut releases = Vec::new();
        for url in self.search_urls(keywords)? {
            releases.extend(self.fetch_and_extract(&url).await?);
        }
        Ok(releases)
    }

    async fn latest(&self) -> Result<Vec<Release>> {
        // RSS-style discovery: an empty-keyword search returns the newest rows on
        // the trackers that support it.
        let mut releases = Vec::new();
        for url in self.search_urls("")? {
            releases.extend(self.fetch_and_extract(&url).await?);
        }
        Ok(releases)
    }
}

/// Extract one field's raw string value from a row element (before filters).
fn extract_field(
    row: &scraper::ElementRef,
    field: &Field,
    selector: Option<&Selector>,
) -> Option<String> {
    // A literal `text:` field with no selector resolves to its literal value.
    let Some(selector) = selector else {
        return field.text.clone();
    };
    // Cardigann selectors are relative to the row; the row element itself can also
    // match, so try the row first then its descendants.
    let element = if matches_self(row, selector) {
        Some(*row)
    } else {
        row.select(selector).next()
    }?;

    match &field.attribute {
        Some(attr) => element.value().attr(attr).map(str::to_string),
        None => Some(element.text().collect::<String>().trim().to_string()),
    }
}

/// Whether the row element itself matches the selector (so selectors anchored on
/// the row, not its children, still resolve).
fn matches_self(row: &scraper::ElementRef, selector: &Selector) -> bool {
    row.value().name() != "html" && selector.matches(row)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(keywords: &'a str, config: &'a BTreeMap<String, String>) -> TemplateContext<'a> {
        TemplateContext { keywords, config }
    }

    #[test]
    fn template_substitutes_keywords_and_config() {
        let mut cfg = BTreeMap::new();
        cfg.insert("passkey".to_string(), "SECRET".to_string());
        let c = ctx("the matrix", &cfg);
        assert_eq!(
            render_template("/search?q={{ .Keywords }}&pk={{ .Config.passkey }}", &c).unwrap(),
            "/search?q=the matrix&pk=SECRET"
        );
        // Tolerant of no inner whitespace and the .Query alias.
        assert_eq!(
            render_template("{{.Query.Keywords}}", &c).unwrap(),
            "the matrix"
        );
        // A quoted literal.
        assert_eq!(render_template(r#"{{ "x" }}"#, &c).unwrap(), "x");
        // An unknown/unsupported expression fails loudly.
        assert!(render_template("{{ range .Categories }}", &c).is_err());
        // A missing config key renders empty, not an error.
        assert_eq!(render_template("{{ .Config.missing }}", &c).unwrap(), "");
    }

    fn filter(name: &str, args: serde_yaml::Value) -> Filter {
        Filter {
            name: name.to_string(),
            args,
        }
    }

    fn seq(items: &[&str]) -> serde_yaml::Value {
        serde_yaml::Value::Sequence(
            items
                .iter()
                .map(|s| serde_yaml::Value::String((*s).to_string()))
                .collect(),
        )
    }

    fn apply_chain(value: &str, filters: &[Filter]) -> String {
        let compiled: Vec<_> = filters
            .iter()
            .map(|f| CompiledFilter::compile(f).unwrap())
            .collect();
        compiled
            .iter()
            .fold(value.to_string(), |acc, f| f.apply(acc))
    }

    #[test]
    fn filters_transform_values() {
        // regexp -> capture group 1.
        assert_eq!(
            apply_chain(
                "Seeders: 88 ",
                &[filter("regexp", serde_yaml::Value::String(r"(\d+)".into()))]
            ),
            "88"
        );
        // replace + append + prepend + trim + case.
        assert_eq!(
            apply_chain("a-b", &[filter("replace", seq(&["-", "_"]))]),
            "a_b"
        );
        assert_eq!(
            apply_chain(
                "x",
                &[filter(
                    "append",
                    serde_yaml::Value::String(".torrent".into())
                )]
            ),
            "x.torrent"
        );
        assert_eq!(
            apply_chain(
                "id",
                &[filter("prepend", serde_yaml::Value::String("/dl/".into()))]
            ),
            "/dl/id"
        );
        assert_eq!(
            apply_chain("  hi  ", &[filter("trim", serde_yaml::Value::Null)]),
            "hi"
        );
        assert_eq!(
            apply_chain("AbC", &[filter("tolower", serde_yaml::Value::Null)]),
            "abc"
        );
        // split -> nth.
        assert_eq!(
            apply_chain("a|b|c", &[filter("split", seq(&["|", "1"]))]),
            "b"
        );
        // querystring -> param out of a URL.
        assert_eq!(
            apply_chain(
                "/download.php?id=501&authkey=xyz",
                &[filter(
                    "querystring",
                    serde_yaml::Value::String("id".into())
                )]
            ),
            "501"
        );
        // re_replace -> regex substitution. The replacement uses `$1`/`$2`; a
        // non-identifier char must separate them (regex reads `$1x` as the group
        // named "1x", per Go/Rust `$name` semantics — same as real definitions).
        assert_eq!(
            apply_chain(
                "S01E02",
                &[filter("re_replace", seq(&[r"S(\d+)E(\d+)", "$1-$2"]))]
            ),
            "01-02"
        );
    }

    #[test]
    fn unknown_filter_is_a_definition_error() {
        let err = CompiledFilter::compile(&filter("teleport", serde_yaml::Value::Null));
        assert!(matches!(err, Err(IndexerError::Definition(_))));
    }

    #[test]
    fn resolve_url_makes_relative_absolute_but_leaves_magnets() {
        let def = Definition {
            id: "t".into(),
            name: "T".into(),
            links: vec!["https://tracker.example/".into()],
            caps: DefinitionCaps::default(),
            search: SearchBlock {
                paths: vec![],
                rows: RowsBlock {
                    selector: "tr".into(),
                },
                fields: BTreeMap::new(),
            },
            protocol: ProtocolHint::Torrent,
        };
        let eng = CardigannIndexer::new(IndexerId::new(), def);
        assert_eq!(
            eng.resolve_url("/download.php?id=1"),
            "https://tracker.example/download.php?id=1"
        );
        // A magnet (absolute, opaque) is untouched.
        let magnet = "magnet:?xt=urn:btih:abc";
        assert_eq!(eng.resolve_url(magnet), magnet);
    }

    #[test]
    fn parse_size_units() {
        assert_eq!(
            parse_size("1.5 GB"),
            Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64)
        );
        assert_eq!(parse_size("700 MB"), Some(700 * 1024 * 1024));
        assert_eq!(parse_size("1,234"), Some(1234));
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("garbage"), None);
    }
}
