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
//! - `search.paths[].path` + `inputs` → the request, with a Go-template subset:
//!   value expressions (`{{ .Keywords }}`, `{{ .Query.<Name> }}` typed fields like
//!   `Season`/`Episode`/`IMDBID`, `{{ .Config.<key> }}`, `{{ . }}`, quoted literals,
//!   `{{ join .Categories "," }}`), `{{ if … }}…{{ else }}…{{ end }}`, and
//!   `{{ range .Categories }}…{{ end }}`. Empty-rendering inputs are dropped. A
//!   `method: post` path carries its inputs as a urlencoded form body.
//! - `search.rows.selector` + `search.fields` → **CSS** row/field extraction, each
//!   field reading element text or an `attribute`, then running a **filter chain**
//!   (`regexp`, `re_replace`, `replace`, `split`, `querystring`, `append`,
//!   `prepend`, `trim`, `tolower`, `toupper`, `urldecode`, `htmldecode`,
//!   `validfilename`). Recognized fields: `title`, `download`/`magnet`/`infohash`
//!   (a bare infohash becomes a magnet), `details`/`guid`, `size`, `seeders`,
//!   `downloadvolumefactor` (`0` → a `freeleech` flag).
//! - `search.response.type: json` → parse a JSON body instead: `rows.selector` and
//!   each field `selector` are dotted paths (`data.torrents[0].name`); filter chains
//!   and field semantics are shared with the HTML path.
//! - `encoding` → decode non-UTF-8 response bytes (e.g. `windows-1251`) before
//!   parsing, for trackers that send no/incorrect charset header.
//! - `caps.categorymappings` → translates the search's requested Torznab categories
//!   into this tracker's own category ids (parent categories expand to their range).
//!
//! Unsupported constructs (XPath selectors, template control flow beyond `if`/`range`,
//! an unknown filter) are rejected as a [`IndexerError::Definition`] rather than
//! silently producing wrong data — the integration layer must never be quietly
//! incorrect.
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
use crate::resolve::DownloadResolver;

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
    /// The response character encoding (e.g. `UTF-8`, `windows-1251`). Used to
    /// decode raw response bytes when the server sends no/incorrect charset header;
    /// defaults to UTF-8.
    #[serde(default)]
    pub encoding: Option<String>,
    /// Minimum seconds the tracker asks callers to leave between requests.
    ///
    /// Honored as a floor on this indexer's own request spacing, on top of the
    /// shared per-host limiter. A tracker that publishes one is telling you the
    /// rate at which it stops answering; ignoring it is how an indexer gets banned.
    #[serde(default, rename = "requestDelay")]
    pub request_delay: Option<f64>,
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
    /// Query inputs shared by every path (`name → template`). Definitions commonly
    /// declare the whole query here and use `paths` only to vary the page, so these
    /// must be merged into each path's own inputs — a path-level entry of the same
    /// name wins.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// How to parse the response body (`html`, the default, or `json`).
    #[serde(default)]
    pub response: ResponseBlock,
    /// For HTML, the CSS selector yielding one element per result row; for JSON, a
    /// dotted path to the array of rows (e.g. `data.torrents`).
    pub rows: RowsBlock,
    /// Per-field rules keyed by field name (`title`, `download`, `size`, …). For
    /// JSON the field `selector` is a dotted path into the row object.
    pub fields: BTreeMap<String, Field>,
}

/// The `response` block: how the search response is parsed.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResponseBlock {
    /// The response body type.
    #[serde(rename = "type", default)]
    pub kind: ResponseType,
}

/// A search response's body format.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ResponseType {
    /// An HTML document parsed with CSS selectors (the default).
    #[default]
    Html,
    /// A JSON document navigated with dotted paths.
    Json,
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
// Templating: the Cardigann template subset (Go text/template-style).
// ---------------------------------------------------------------------------

/// The values a template can reference.
#[derive(Clone, Copy)]
struct TemplateContext<'a> {
    /// `.Keywords` / `.Query.Keywords` — the free-text query.
    keywords: &'a str,
    /// `.Query.<Name>` lookups (`Season`, `Episode`, `IMDBID`, …) derived from the
    /// search's ids/numbering.
    query: &'a BTreeMap<String, String>,
    /// `.Config.<key>` lookups (user settings: passkey, sitelink override, …).
    config: &'a BTreeMap<String, String>,
    /// `.Categories` — the tracker category ids selected for this search.
    categories: &'a [String],
    /// The current item inside a `range` body (`.`), if any.
    dot: Option<&'a str>,
}

/// One parsed template node.
enum TmplNode {
    /// Literal text.
    Text(String),
    /// A value expression (`.Keywords`, `.Query.X`, `.Config.X`, `.`, `join …`, "lit").
    Expr(String),
    /// `{{ if COND }}then{{ else }}els{{ end }}`.
    If {
        /// The truthiness expression.
        cond: String,
        /// Rendered when `cond` is non-empty/true.
        then: Vec<TmplNode>,
        /// Rendered otherwise (empty when there is no `else`).
        els: Vec<TmplNode>,
    },
    /// `{{ range EXPR }}body{{ end }}` — repeats `body` per item, binding `.`.
    Range {
        /// The list expression (only `.Categories` is supported).
        expr: String,
        /// The body rendered once per item.
        body: Vec<TmplNode>,
    },
}

/// Render a Cardigann template string against `ctx`.
///
/// Supports value expressions (`{{ .Keywords }}`, `{{ .Query.<Name> }}`,
/// `{{ .Config.<key> }}`, `{{ . }}`, `{{ join .Categories "," }}`, quoted literals),
/// `{{ if … }}…{{ else }}…{{ end }}`, and `{{ range .Categories }}…{{ end }}`. An
/// unrecognized expression or unbalanced block is a hard [`IndexerError::Definition`]
/// — a definition is never silently mis-rendered.
fn render_template(tmpl: &str, ctx: &TemplateContext) -> Result<String> {
    let tokens = tokenize_template(tmpl)?;
    let mut pos = 0;
    let (nodes, terminator) = parse_nodes(&tokens, &mut pos)?;
    if let Some(t) = terminator {
        return Err(IndexerError::Definition(format!(
            "template has a stray '{{{{ {t} }}}}'"
        )));
    }
    eval_nodes(&nodes, ctx)
}

/// A lexical token: literal text or the trimmed body of a `{{ … }}` action.
enum Tok {
    Text(String),
    Action(String),
}

/// Split a template into text/action tokens.
fn tokenize_template(tmpl: &str) -> Result<Vec<Tok>> {
    let mut toks = Vec::new();
    let mut rest = tmpl;
    while let Some(start) = rest.find("{{") {
        if start > 0 {
            toks.push(Tok::Text(rest[..start].to_string()));
        }
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| {
            IndexerError::Definition(format!("unterminated template action in '{tmpl}'"))
        })?;
        toks.push(Tok::Action(after[..end].trim().to_string()));
        rest = &after[end + 2..];
    }
    if !rest.is_empty() {
        toks.push(Tok::Text(rest.to_string()));
    }
    Ok(toks)
}

/// Parse a node sequence until a block terminator (`end`/`else`), which is returned
/// so the caller (an `if`/`range`) can branch. `None` means end-of-input.
fn parse_nodes(toks: &[Tok], pos: &mut usize) -> Result<(Vec<TmplNode>, Option<String>)> {
    let mut nodes = Vec::new();
    while *pos < toks.len() {
        match &toks[*pos] {
            Tok::Text(t) => {
                nodes.push(TmplNode::Text(t.clone()));
                *pos += 1;
            }
            Tok::Action(a) => {
                let a = a.clone();
                *pos += 1;
                if a == "end" || a == "else" {
                    return Ok((nodes, Some(a)));
                } else if let Some(cond) = a.strip_prefix("if ") {
                    let (then, term) = parse_nodes(toks, pos)?;
                    let els = if term.as_deref() == Some("else") {
                        let (els, term2) = parse_nodes(toks, pos)?;
                        expect_end(term2)?;
                        els
                    } else {
                        expect_end(term)?;
                        Vec::new()
                    };
                    nodes.push(TmplNode::If {
                        cond: cond.trim().to_string(),
                        then,
                        els,
                    });
                } else if let Some(expr) = a.strip_prefix("range ") {
                    let (body, term) = parse_nodes(toks, pos)?;
                    expect_end(term)?;
                    nodes.push(TmplNode::Range {
                        expr: expr.trim().to_string(),
                        body,
                    });
                } else {
                    nodes.push(TmplNode::Expr(a));
                }
            }
        }
    }
    Ok((nodes, None))
}

/// Require that a block closed with `{{ end }}`.
fn expect_end(terminator: Option<String>) -> Result<()> {
    match terminator.as_deref() {
        Some("end") => Ok(()),
        _ => Err(IndexerError::Definition(
            "template block is missing its '{{ end }}'".into(),
        )),
    }
}

/// Evaluate a parsed node sequence.
fn eval_nodes(nodes: &[TmplNode], ctx: &TemplateContext) -> Result<String> {
    let mut out = String::new();
    for node in nodes {
        match node {
            TmplNode::Text(t) => out.push_str(t),
            TmplNode::Expr(e) => out.push_str(&eval_expr(e, ctx)?),
            TmplNode::If { cond, then, els } => {
                if eval_truthy(cond, ctx)? {
                    out.push_str(&eval_nodes(then, ctx)?);
                } else {
                    out.push_str(&eval_nodes(els, ctx)?);
                }
            }
            TmplNode::Range { expr, body } => {
                let items = eval_list(expr, ctx)?;
                for item in items {
                    let inner = TemplateContext {
                        dot: Some(&item),
                        ..*ctx
                    };
                    out.push_str(&eval_nodes(body, &inner)?);
                }
            }
        }
    }
    Ok(out)
}

/// Evaluate a value expression to a string.
fn eval_expr(expr: &str, ctx: &TemplateContext) -> Result<String> {
    if let Some(rest) = expr.strip_prefix("join ") {
        // `join .Categories "sep"` → the list joined by the literal separator.
        let mut parts = rest.splitn(2, char::is_whitespace);
        let list_expr = parts.next().unwrap_or("").trim();
        let sep_lit = parts.next().unwrap_or("").trim();
        let sep = strip_quotes(sep_lit).ok_or_else(|| {
            IndexerError::Definition(format!(
                "join separator must be a quoted literal: {sep_lit}"
            ))
        })?;
        return Ok(eval_list(list_expr, ctx)?.join(sep));
    }
    // `or`/`and` over value expressions, with Go's template semantics: `or` yields
    // the first non-empty argument, `and` the last (or empty if any is empty). Both
    // are common in definitions that vary a query field on whether a keyword or an
    // id was supplied, and both read as booleans in an `if`.
    if let Some(rest) = expr.strip_prefix("or ") {
        for arg in split_args(rest) {
            let value = eval_expr(&arg, ctx)?;
            if !value.is_empty() {
                return Ok(value);
            }
        }
        return Ok(String::new());
    }
    if let Some(rest) = expr.strip_prefix("and ") {
        let mut last = String::new();
        for arg in split_args(rest) {
            last = eval_expr(&arg, ctx)?;
            if last.is_empty() {
                return Ok(String::new());
            }
        }
        return Ok(last);
    }
    match expr {
        "." => Ok(ctx.dot.unwrap_or_default().to_string()),
        ".Keywords" | ".Query.Keywords" | ".Query.q" => Ok(ctx.keywords.to_string()),
        ".Categories" => Ok(ctx.categories.join(",")),
        e if e.starts_with(".Query.") => Ok(ctx
            .query
            .get(&e[".Query.".len()..])
            .cloned()
            .unwrap_or_default()),
        e if e.starts_with(".Config.") => Ok(ctx
            .config
            .get(&e[".Config.".len()..])
            .cloned()
            .unwrap_or_default()),
        e if strip_quotes(e).is_some() => Ok(strip_quotes(e).unwrap().to_string()),
        other => Err(IndexerError::Definition(format!(
            "unsupported template expression: {{{{ {other} }}}}"
        ))),
    }
}

/// Split an argument list on whitespace, keeping a quoted literal (which may itself
/// contain spaces) as one argument.
fn split_args(rest: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for ch in rest.chars() {
        match quote {
            Some(q) => {
                current.push(ch);
                if ch == q {
                    quote = None;
                }
            }
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                current.push(ch);
            }
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Evaluate a truthiness condition (non-empty value / non-empty list = true).
fn eval_truthy(cond: &str, ctx: &TemplateContext) -> Result<bool> {
    if cond == ".Categories" {
        return Ok(!ctx.categories.is_empty());
    }
    Ok(!eval_expr(cond, ctx)?.is_empty())
}

/// Evaluate a list expression (only `.Categories` yields a list).
fn eval_list(expr: &str, ctx: &TemplateContext) -> Result<Vec<String>> {
    match expr {
        ".Categories" => Ok(ctx.categories.to_vec()),
        other => Err(IndexerError::Definition(format!(
            "unsupported list expression: {{{{ range {other} }}}} (only .Categories)"
        ))),
    }
}

/// If `s` is a `"…"`-quoted literal, return its contents.
fn strip_quotes(s: &str) -> Option<&str> {
    (s.len() >= 2 && s.starts_with('"') && s.ends_with('"')).then(|| &s[1..s.len() - 1])
}

/// Build the `.Query.<Name>` map from a search's ids and numbering, using the
/// Cardigann-conventional capitalized keys (`Season`, `Episode`, `IMDBID`, …).
fn query_fields(terms: &SearchTerms) -> BTreeMap<String, String> {
    let mut q = BTreeMap::new();
    for (k, v) in &terms.numbering {
        match k.as_str() {
            "season" => {
                q.insert("Season".to_string(), v.clone());
            }
            "ep" | "episode" => {
                q.insert("Episode".to_string(), v.clone());
                q.insert("Ep".to_string(), v.clone());
            }
            other => {
                q.insert(capitalize(other), v.clone());
            }
        }
    }
    for (k, v) in &terms.ids {
        let key = match k.as_str() {
            "imdbid" => "IMDBID".to_string(),
            "tmdbid" => "TMDBID".to_string(),
            "tvdbid" => "TVDBID".to_string(),
            "tvmazeid" => "TVMazeID".to_string(),
            "rid" => "RageID".to_string(),
            other => other.to_uppercase(),
        };
        q.insert(key, v.clone());
    }
    q
}

/// Capitalize the first character (ASCII).
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
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
    /// Percent-decode (`%20` → space).
    UrlDecode,
    /// Decode common HTML entities (`&amp;` → `&`).
    HtmlDecode,
    /// Replace filesystem-invalid characters with `_`.
    ValidFilename,
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
            "urldecode" => Ok(Self::UrlDecode),
            "htmldecode" | "unescape" => Ok(Self::HtmlDecode),
            "validfilename" => Ok(Self::ValidFilename),
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
            Self::UrlDecode => url_decode(&value),
            Self::HtmlDecode => html_decode(&value),
            Self::ValidFilename => value
                .chars()
                .map(|c| {
                    if "<>:\"/\\|?*".contains(c) || c.is_control() {
                        '_'
                    } else {
                        c
                    }
                })
                .collect(),
        }
    }
}

/// Percent-decode a string (`%20` → space, `+` left as-is; lossy on bad UTF-8).
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decode the common HTML entities (named + numeric) that appear in scraped values.
fn html_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        if let Some(semi) = after.find(';').filter(|&p| p <= 10) {
            let entity = &after[1..semi];
            let decoded = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                num if num.starts_with('#') => {
                    num[1..].parse::<u32>().ok().and_then(char::from_u32)
                }
                _ => None,
            };
            if let Some(c) = decoded {
                out.push(c);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
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
    /// Fills in links for rows that arrive without one (see [`crate::resolve`]).
    /// `None` drops such rows, which is the behavior of a definition-only engine.
    resolver: Option<Arc<dyn DownloadResolver>>,
    /// When this engine last issued a request, for honoring the definition's
    /// `requestDelay`. Per-engine rather than per-host: the delay is this
    /// tracker's own published courtesy interval.
    last_request: tokio::sync::Mutex<Option<std::time::Instant>>,
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
            resolver: None,
            last_request: tokio::sync::Mutex::new(None),
        }
    }

    /// Attach a [`DownloadResolver`] for rows whose link the definition can't
    /// express (a tracker that serves magnets from a signed endpoint).
    #[must_use]
    pub fn with_resolver(mut self, resolver: Arc<dyn DownloadResolver>) -> Self {
        self.resolver = Some(resolver);
        self
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

    /// Resolve the requested Torznab categories to this tracker's own category ids
    /// via `categorymappings`. A requested parent category (e.g. `2000` Movies)
    /// selects every mapping under it (`20xx`); a specific one (`5040`) matches
    /// exactly. Empty when nothing was requested.
    fn tracker_categories(&self, requested: &[u32]) -> Vec<String> {
        if requested.is_empty() {
            return Vec::new();
        }
        self.definition
            .caps
            .categorymappings
            .iter()
            .filter(|m| {
                m.cat
                    .parse::<u32>()
                    .is_ok_and(|c| requested.contains(&c) || requested.contains(&(c / 1000 * 1000)))
            })
            .map(|m| m.id.clone())
            .collect()
    }

    /// Build the search requests from `terms`, rendering each configured path + its
    /// inputs against the query (keywords, `.Query.*` fields, `.Categories`). Inputs
    /// that render empty are omitted, so a movie search never sends a stray `season=`.
    /// A `method: post` path carries its inputs as a form body; otherwise they are
    /// query parameters.
    fn search_requests(&self, terms: &SearchTerms) -> Result<Vec<SearchRequest>> {
        let base = self
            .base
            .as_ref()
            .ok_or_else(|| IndexerError::Definition("definition has no usable site link".into()))?;
        if self.definition.search.paths.is_empty() {
            return Err(IndexerError::Definition("search has no paths".into()));
        }
        let base_keywords = terms.queries.first().map(String::as_str).unwrap_or("");
        let folded = self.keywords_with_numbering(terms, base_keywords);
        let keywords = folded.as_str();
        let query = query_fields(terms);
        let categories = self.tracker_categories(&terms.categories);
        let ctx = TemplateContext {
            keywords,
            query: &query,
            config: &self.config,
            categories: &categories,
            dot: None,
        };
        let mut requests = Vec::with_capacity(self.definition.search.paths.len());
        for path in &self.definition.search.paths {
            let rendered = render_template(&path.path, &ctx)?;
            let mut url = base.join(&rendered).map_err(|e| {
                IndexerError::Definition(format!("bad search path '{rendered}': {e}"))
            })?;
            // Render the non-empty inputs once, taking the search-level block first
            // so a path that repeats a name overrides it.
            let mut merged = self.definition.search.inputs.clone();
            merged.extend(path.inputs.clone());
            let mut inputs = Vec::new();
            for (name, template) in &merged {
                let value = render_template(template, &ctx)?;
                if !value.is_empty() {
                    inputs.push((name.clone(), value));
                }
            }
            let is_post = path
                .method
                .as_deref()
                .is_some_and(|m| m.eq_ignore_ascii_case("post"));
            let body = if is_post {
                // POST: inputs go in a form body, not the query string.
                Some(
                    url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(&inputs)
                        .finish(),
                )
            } else {
                url.query_pairs_mut().extend_pairs(&inputs);
                None
            };
            requests.push(SearchRequest { url, body });
        }
        Ok(requests)
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

    /// Extract releases from an already-fetched response body, dispatching on the
    /// definition's declared response type (HTML by default, or JSON).
    ///
    /// Deliberately I/O-free so it can be exercised against recorded bodies in the
    /// record/replay tests.
    pub fn extract(&self, body: &str) -> Result<Vec<Release>> {
        match self.definition.search.response.kind {
            ResponseType::Html => self.extract_html(body),
            ResponseType::Json => self.extract_json(body),
        }
    }

    /// Extract releases from an HTML document via the `rows`/`fields` CSS selectors.
    fn extract_html(&self, html: &str) -> Result<Vec<Release>> {
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
            let mut pairs = Vec::with_capacity(fields.len());
            for (name, field, selector, filters) in &fields {
                let Some(raw) = extract_field(&row, field, selector.as_ref()) else {
                    continue;
                };
                let value = filters.iter().fold(raw, |acc, f| f.apply(acc));
                pairs.push((name.clone(), value));
            }
            if let Some(rel) = self.assemble_release(&pairs, protocol) {
                releases.push(rel);
            }
        }
        Ok(releases)
    }

    /// Extract releases from a JSON document: `rows.selector` is a dotted path to the
    /// array of rows; each field's `selector` is a dotted path into the row object.
    fn extract_json(&self, body: &str) -> Result<Vec<Release>> {
        let root: serde_json::Value = serde_json::from_str(body)
            .map_err(|e| IndexerError::Parse(format!("json response: {e}")))?;
        let search = &self.definition.search;

        // Pre-compile each field's filter chain once; the JSON path is the selector.
        let mut fields = Vec::with_capacity(search.fields.len());
        for (name, field) in &search.fields {
            let filters = field
                .filters
                .iter()
                .map(CompiledFilter::compile)
                .collect::<Result<Vec<_>>>()?;
            fields.push((name.clone(), field.clone(), filters));
        }

        let rows = match json_pointer(&root, &search.rows.selector) {
            Some(serde_json::Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let protocol: Protocol = self.definition.protocol.into();
        let mut releases = Vec::new();
        for row in &rows {
            let mut pairs = Vec::with_capacity(fields.len());
            for (name, field, filters) in &fields {
                let raw = match &field.selector {
                    Some(path) => json_pointer(row, path).and_then(json_scalar),
                    None => field.text.clone(),
                };
                if let Some(raw) = raw {
                    let value = filters.iter().fold(raw, |acc, f| f.apply(acc));
                    pairs.push((name.clone(), value));
                }
            }
            if let Some(rel) = self.assemble_release(&pairs, protocol) {
                releases.push(rel);
            }
        }
        Ok(releases)
    }

    /// Assemble a [`Release`] from one row's resolved `(field-name, value)` pairs,
    /// applying field semantics: `download`/`magnet`/`infohash` resolve the link (a
    /// bare infohash becomes a magnet), `downloadvolumefactor: 0` flags freeleech.
    /// Returns `None` when the row lacks a title or any usable link.
    fn assemble_release(&self, pairs: &[(String, String)], protocol: Protocol) -> Option<Release> {
        let mut title = None;
        let mut download_url = None;
        let mut magnet = None;
        let mut infohash = None;
        let mut guid = None;
        let mut size = None;
        let mut seeders = None;
        let mut flags = Vec::new();

        for (name, value) in pairs {
            match name.as_str() {
                "title" => title = Some(value.clone()),
                "download" => download_url = Some(self.resolve_url(value)),
                "magnet" => magnet = Some(value.clone()),
                "infohash" => infohash = Some(value.clone()),
                "details" | "guid" => guid = Some(self.resolve_url(value)),
                "size" => size = parse_size(value),
                "seeders" => seeders = parse_count(value),
                "downloadvolumefactor" if value.trim().parse::<f64>().is_ok_and(|f| f == 0.0) => {
                    flags.push("freeleech".to_string());
                }
                _ => {}
            }
        }

        // An explicit magnet wins; otherwise a bare infohash (from `download` or
        // `infohash`) becomes a magnet.
        let link = if let Some(m) = magnet {
            Some(m)
        } else if let Some(dl) = download_url {
            Some(infohash_to_magnet(&dl).unwrap_or(dl))
        } else {
            infohash.as_deref().and_then(infohash_to_magnet)
        };

        let title = title?;
        let link = match link {
            Some(link) => finalize_magnet(link, &title),
            // No link in the row. If a resolver claims the details URL it can fetch
            // one; leave the field empty as the marker for that deferred pass and
            // let `resolve_pending_links` fill it or drop the release. Without a
            // resolver the row is unusable and drops here, as it always has.
            None => {
                let details = guid.as_deref()?;
                if !self.resolver.as_ref().is_some_and(|r| r.handles(details)) {
                    return None;
                }
                PENDING_LINK.to_string()
            }
        };
        Some(Release {
            indexer_id: self.indexer_id,
            title,
            download_url: link,
            guid,
            protocol,
            size,
            seeders,
            indexer_flags: flags,
        })
    }

    /// Issue one search request (rate-limited; GET or form-POST) and extract its
    /// releases.
    async fn fetch_request(&self, req: &SearchRequest) -> Result<Vec<Release>> {
        if let Some(host) = req.url.host_str() {
            self.rate_limiter.until_ready(host).await;
        }
        self.pace().await;
        // Fetch raw bytes and decode with the definition's declared encoding, so a
        // non-UTF-8 tracker (e.g. windows-1251) that sends no/incorrect charset
        // header is read correctly rather than mojibake.
        let bytes = match &req.body {
            Some(form) => {
                self.fetcher
                    .post_bytes(req.url.as_str(), form, "application/x-www-form-urlencoded")
                    .await?
            }
            None => self.fetcher.get_bytes(req.url.as_str()).await?,
        };
        let body = decode_body(&bytes, self.definition.encoding.as_deref());
        let mut releases = self.extract(&body)?;
        self.mark_deferred_links(&mut releases);
        Ok(releases)
    }

    /// The keyword string a search sends, with season/episode folded in when the
    /// definition has no input that could otherwise carry them.
    ///
    /// A scrape-style tracker often has no season/episode URL parameter at all —
    /// its definition declares `tv-search` in `caps` but templates only `q`. The
    /// numbering is computed either way, so without this it is silently dropped and
    /// every episode search degrades to a bare series-title search: thousands of
    /// rows spanning every season, with the wanted episode nowhere near the top.
    /// Folding it into the keyword the way the site's own users would type it
    /// (`Series S06E03`, or `Series S06` for a whole season) is what makes such a
    /// tracker usable for TV at all.
    ///
    /// A definition that *does* template the numbering keeps the keyword untouched,
    /// so nothing is sent twice.
    fn keywords_with_numbering(&self, terms: &SearchTerms, base: &str) -> String {
        if base.is_empty() || self.definition_templates_numbering() {
            return base.to_string();
        }
        let value = |key: &str| {
            terms
                .numbering
                .iter()
                .find(|(k, _)| k == key)
                .and_then(|(_, v)| v.parse::<u32>().ok())
        };
        let Some(season) = value("season") else {
            return base.to_string();
        };
        match value("ep").or_else(|| value("episode")) {
            Some(episode) => format!("{base} S{season:02}E{episode:02}"),
            None => format!("{base} S{season:02}"),
        }
    }

    /// Whether any configured input renders the season/episode query fields, in
    /// which case the definition carries the numbering itself.
    fn definition_templates_numbering(&self) -> bool {
        let mentions = |template: &String| {
            [".Query.Season", ".Query.Episode", ".Query.Ep"]
                .iter()
                .any(|field| template.contains(field))
        };
        self.definition.search.inputs.values().any(mentions)
            || self
                .definition
                .search
                .paths
                .iter()
                .any(|path| path.inputs.values().any(mentions) || mentions(&path.path))
    }

    /// Wait out the definition's `requestDelay` since this engine's last request.
    ///
    /// The shared limiter is keyed on host and tuned for ordinary trackers; a
    /// tracker that publishes a courtesy interval usually wants something much
    /// slower, and one that answers through a headless browser cannot be driven
    /// at the shared rate at all.
    async fn pace(&self) {
        let Some(delay) = self
            .definition
            .request_delay
            .filter(|d| *d > 0.0)
            .map(std::time::Duration::from_secs_f64)
        else {
            return;
        };
        let mut last = self.last_request.lock().await;
        if let Some(previous) = *last {
            let elapsed = previous.elapsed();
            if elapsed < delay {
                tokio::time::sleep(delay - elapsed).await;
            }
        }
        *last = Some(std::time::Instant::now());
    }

    /// How many link-less rows one search may resolve.
    ///
    /// Resolution costs two extra requests per release against a tracker that is
    /// already the slow one, so an unbounded sweep of a hundred-row page would
    /// spend minutes and starve every other indexer. The cap keeps the best
    /// candidates — which is what a decision engine picks from anyway — and the
    /// number of rows it dropped is logged rather than swallowed.
    fn resolve_limit(&self) -> usize {
        self.config
            .get("resolveLimit")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_RESOLVE_LIMIT)
    }

    /// Flag rows that arrived without a link as DEFERRED when a resolver could
    /// fetch one, and leave the rest empty so Discover drops them.
    ///
    /// Nothing is fetched here. Resolving at search time means resolving
    /// speculatively for every candidate, which is expensive enough on a tracker
    /// that signs its magnets that it has to be capped — and the cap threw away 88%
    /// of one tracker's candidates before the decision engine ever saw them. A
    /// deferred release is a complete candidate for deciding: title, size, seeders
    /// and flags all come from the listing. Only the winner needs a link.
    fn mark_deferred_links(&self, releases: &mut [Release]) {
        if self.resolver.is_none() {
            return;
        }
        let mut deferred = 0;
        for release in releases.iter_mut() {
            // A row with no link but a details url is one the resolver can fetch.
            if release.download_url.trim().is_empty()
                && release.guid.as_deref().is_some_and(|g| !g.trim().is_empty())
            {
                release.download_url = cellarr_core::DEFERRED_LINK.to_string();
                deferred += 1;
            }
        }
        if deferred > 0 {
            tracing::debug!(
                indexer = %self.definition.name,
                deferred,
                "links deferred until a release is chosen"
            );
        }
    }

    /// Fetch the real link for one deferred release.
    ///
    /// This is the whole point of deferring: exactly one resolve per acquisition
    /// instead of one per speculatively-kept candidate. A failure is returned rather
    /// than swallowed so the caller can fall through to the next candidate — the
    /// grab-next loop already does exactly that when a grab fails.
    async fn resolve_deferred(&self, mut release: Release) -> Result<Release> {
        let Some(resolver) = self.resolver.clone() else {
            return Ok(release);
        };
        let details = release
            .guid
            .clone()
            .filter(|g| !g.trim().is_empty())
            .ok_or_else(|| {
                IndexerError::Parse("release has a deferred link but no details url".to_string())
            })?;
        if let Some(host) = Url::parse(&details)
            .ok()
            .and_then(|u| u.host_str().map(ToString::to_string))
        {
            self.rate_limiter.until_ready(&host).await;
        }
        self.pace().await;
        let link = resolver.resolve(&details, self.fetcher.as_ref()).await?;
        release.download_url = finalize_magnet(link, &release.title);
        Ok(release)
    }
}

/// Rows one search resolves when the indexer sets no `resolveLimit`.
const DEFAULT_RESOLVE_LIMIT: usize = 15;

/// Placeholder written into `download_url` by the extractor when a row has no link
/// but a [`DownloadResolver`] may be able to fetch one. Never escapes the engine:
/// [`CardigannIndexer::resolve_pending_links`] either replaces it or drops the
/// release. Empty rather than a sentinel string so a leak is inert downstream.
const PENDING_LINK: &str = "";

/// Decode response bytes using the named encoding (Cardigann's `encoding` field),
/// defaulting to UTF-8. An unknown label falls back to UTF-8; decoding is lossy
/// rather than failing, since a stray bad byte must not lose a whole page.
fn decode_body(bytes: &[u8], encoding: Option<&str>) -> String {
    let enc = encoding
        .filter(|e| !e.is_empty())
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = enc.decode(bytes);
    text.into_owned()
}

/// One built search request: a URL plus, for a `method: post` path, the form body.
struct SearchRequest {
    url: Url,
    /// `Some(form)` issues a POST with this body; `None` is a GET.
    body: Option<String>,
}

#[async_trait]
impl Indexer for CardigannIndexer {
    type Error = IndexerError;

    fn name(&self) -> &str {
        &self.definition.name
    }

    #[tracing::instrument(name = "indexer.resolve", skip_all, fields(indexer = %self.definition.name))]
    async fn resolve(&self, release: Release) -> Result<Release> {
        if release.link_is_deferred() {
            return self.resolve_deferred(release).await;
        }
        Ok(release)
    }

    #[tracing::instrument(name = "indexer.search", skip_all, fields(indexer = %self.definition.name))]
    async fn search(&self, terms: &SearchTerms) -> Result<Vec<Release>> {
        let mut releases = Vec::new();
        for req in self.search_requests(terms)? {
            releases.extend(self.fetch_request(&req).await?);
        }
        Ok(releases)
    }

    async fn latest(&self) -> Result<Vec<Release>> {
        // RSS-style discovery: an empty search (no keywords/ids/categories) returns
        // the newest rows on the trackers that support it.
        let mut releases = Vec::new();
        for req in self.search_requests(&SearchTerms::default())? {
            releases.extend(self.fetch_request(&req).await?);
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

/// Navigate a JSON value by a dotted path: object keys and `[index]` array access,
/// with an optional leading `$`/`$.`. E.g. `data.torrents[0].name`. Returns `None`
/// if any segment is absent.
fn json_pointer<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let path = path.trim().trim_start_matches('$').trim_start_matches('.');
    if path.is_empty() {
        return Some(value);
    }
    let mut cur = value;
    for seg in path.split('.') {
        let (key, index) = parse_path_segment(seg);
        if !key.is_empty() {
            cur = cur.get(key)?;
        }
        if let Some(i) = index {
            cur = cur.get(i)?;
        }
    }
    Some(cur)
}

/// Split a path segment into its object key and optional trailing `[index]`.
fn parse_path_segment(seg: &str) -> (&str, Option<usize>) {
    match seg.split_once('[') {
        Some((key, rest)) => {
            let index = rest.trim_end_matches(']').parse::<usize>().ok();
            (key, index)
        }
        None => (seg, None),
    }
}

/// Read a JSON scalar (string/number/bool) as a string; `None` for objects/arrays/null.
fn json_scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// If `s` is a bare 40-character hex infohash, return a magnet URI for it.
fn infohash_to_magnet(s: &str) -> Option<String> {
    let h = s.trim();
    (h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("magnet:?xt=urn:btih:{}", h.to_ascii_lowercase()))
}

/// Ensure a magnet link carries a `dn=` display name; non-magnet links are returned
/// unchanged.
fn finalize_magnet(link: String, title: &str) -> String {
    if link.starts_with("magnet:") && !link.contains("dn=") {
        let dn: String = url::form_urlencoded::byte_serialize(title.as_bytes()).collect();
        format!("{link}&dn={dn}")
    } else {
        link
    }
}

/// Parse a human size string (`"1.5 GB"`, `"700 MB"`, `"1234567"`) into bytes.
fn parse_size(raw: &str) -> Option<u64> {
    let (value, rest) = split_leading_number(raw)?;
    let unit: String = rest
        .trim_start()
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_ascii_lowercase();
    // A bare count with no unit is a byte count, as JSON-ish feeds report it.
    if unit.is_empty() {
        return (value >= 0.0).then_some(value as u64);
    }
    let multiplier: f64 = match unit.as_str() {
        "b" => 1.0,
        "kb" | "kib" => 1024.0,
        "mb" | "mib" => 1024.0 * 1024.0,
        "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((value * multiplier) as u64)
}

/// Parse a count that may be rendered with a label (`Seeds 137`).
fn parse_count(raw: &str) -> Option<u32> {
    let (value, _) = split_leading_number(raw)?;
    (value >= 0.0 && value <= f64::from(u32::MAX)).then_some(value as u32)
}

/// Split the first numeric token out of `raw`, returning its value and the text
/// that follows it.
///
/// Cells commonly carry a label alongside the value (`Size 4.2 GB`, `Seeds 137`) —
/// trackers render one for narrow viewports and hide it with CSS, so the label is
/// part of the element's text even though a reader never sees it. Anchoring on the
/// first digit rather than on the start of the string reads those cells the way the
/// definition's author intended, without every definition needing a strip filter.
fn split_leading_number(raw: &str) -> Option<(f64, &str)> {
    let start = raw.find(|c: char| c.is_ascii_digit())?;
    let rest = &raw[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == ',' || c == '.'))
        .unwrap_or(rest.len());
    // A trailing separator belongs to the prose after the number ("4 GB, seeded"),
    // not to the number itself.
    let digits = rest[..end].trim_end_matches([',', '.']);
    let value: f64 = digits.replace(',', "").parse().ok()?;
    Some((value, &rest[digits.len()..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(
        keywords: &'a str,
        query: &'a BTreeMap<String, String>,
        config: &'a BTreeMap<String, String>,
        categories: &'a [String],
    ) -> TemplateContext<'a> {
        TemplateContext {
            keywords,
            query,
            config,
            categories,
            dot: None,
        }
    }

    #[test]
    fn template_substitutes_keywords_and_config() {
        let mut cfg = BTreeMap::new();
        cfg.insert("passkey".to_string(), "SECRET".to_string());
        let empty = BTreeMap::new();
        let c = ctx("the matrix", &empty, &cfg, &[]);
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
        // A missing config key renders empty, not an error.
        assert_eq!(render_template("{{ .Config.missing }}", &c).unwrap(), "");
        // An unterminated block fails loudly.
        assert!(render_template("{{ range .Categories }}", &c).is_err());
        assert!(render_template("{{ if .Keywords }}x", &c).is_err());
    }

    #[test]
    fn template_typed_query_fields_and_control_flow() {
        let mut query = BTreeMap::new();
        query.insert("Season".to_string(), "2".to_string());
        query.insert("IMDBID".to_string(), "tt0133093".to_string());
        let cfg = BTreeMap::new();
        let cats = ["100".to_string(), "101".to_string()];
        let c = ctx("the matrix", &query, &cfg, &cats);

        // .Query.<Name> typed fields.
        assert_eq!(
            render_template("&imdb={{ .Query.IMDBID }}", &c).unwrap(),
            "&imdb=tt0133093"
        );
        // if/else on a present vs absent field.
        assert_eq!(
            render_template(
                "{{ if .Query.Season }}s={{ .Query.Season }}{{ else }}none{{ end }}",
                &c
            )
            .unwrap(),
            "s=2"
        );
        assert_eq!(
            render_template("{{ if .Query.Episode }}e{{ else }}no-ep{{ end }}", &c).unwrap(),
            "no-ep"
        );
        // range over categories binds `.`.
        assert_eq!(
            render_template("{{ range .Categories }}&cat={{ . }}{{ end }}", &c).unwrap(),
            "&cat=100&cat=101"
        );
        // join builtin.
        assert_eq!(
            render_template(r#"cats={{ join .Categories "," }}"#, &c).unwrap(),
            "cats=100,101"
        );
        // Nested: if guarding a range.
        assert_eq!(
            render_template(
                "{{ if .Categories }}{{ range .Categories }}{{ . }};{{ end }}{{ end }}",
                &c
            )
            .unwrap(),
            "100;101;"
        );
    }

    /// `or`/`and` follow Go's template semantics — they yield a *value*, not a
    /// boolean — which is what lets a definition write
    /// `{{ if or .Keywords .Query.IMDBID }}` and also `{{ or .A .B }}` inline.
    #[test]
    fn template_or_and_yield_go_values_and_read_as_conditions() {
        let mut query = BTreeMap::new();
        query.insert("IMDBID".to_string(), "tt13991232".to_string());
        let c = TemplateContext {
            keywords: "",
            query: &query,
            config: &BTreeMap::new(),
            categories: &[],
            dot: None,
        };

        // `or` returns the first non-empty argument, skipping the empty keywords.
        assert_eq!(
            render_template("{{ or .Keywords .Query.IMDBID }}", &c).unwrap(),
            "tt13991232"
        );
        // As a condition, a non-empty result takes the `if` branch.
        assert_eq!(
            render_template(
                "{{ if or .Keywords .Query.IMDBID }}have{{ else }}none{{ end }}",
                &c
            )
            .unwrap(),
            "have"
        );
        // `and` is empty when any argument is empty, so it takes the `else` branch.
        assert_eq!(
            render_template(
                "{{ if and .Keywords .Query.IMDBID }}both{{ else }}not-both{{ end }}",
                &c
            )
            .unwrap(),
            "not-both"
        );
        // With neither present the `or` is empty and the `else` branch wins — the
        // keyword-less sweep a listing definition relies on.
        let empty = TemplateContext {
            keywords: "",
            query: &BTreeMap::new(),
            config: &BTreeMap::new(),
            categories: &[],
            dot: None,
        };
        assert_eq!(
            render_template(
                "{{ if or .Keywords .Query.IMDBID }}{{ else }}4{{ end }}",
                &empty
            )
            .unwrap(),
            "4"
        );
    }

    /// A quoted literal containing spaces stays one argument.
    #[test]
    fn split_args_keeps_quoted_literals_whole() {
        assert_eq!(
            split_args(r#".Keywords "a b" .Query.X"#),
            vec![".Keywords", "\"a b\"", ".Query.X"]
        );
    }

    #[test]
    fn query_fields_maps_ids_and_numbering() {
        let terms = SearchTerms {
            queries: vec!["x".into()],
            ids: vec![
                ("imdbid".into(), "tt1".into()),
                ("tvdbid".into(), "42".into()),
            ],
            numbering: vec![("season".into(), "3".into()), ("ep".into(), "7".into())],
            categories: vec![5000],
        };
        let q = query_fields(&terms);
        assert_eq!(q.get("IMDBID").map(String::as_str), Some("tt1"));
        assert_eq!(q.get("TVDBID").map(String::as_str), Some("42"));
        assert_eq!(q.get("Season").map(String::as_str), Some("3"));
        assert_eq!(q.get("Episode").map(String::as_str), Some("7"));
        assert_eq!(q.get("Ep").map(String::as_str), Some("7"));
    }

    #[test]
    fn tracker_categories_maps_requested_torznab_to_tracker_ids() {
        const DEF: &str = r#"
id: t
name: T
links: [https://t.example/]
caps:
  categorymappings:
    - { id: "10", cat: "5040" }
    - { id: "11", cat: "5070" }
    - { id: "20", cat: "2040" }
search:
  paths: [{ path: /s }]
  rows: { selector: tr }
  fields: { title: { selector: a }, download: { selector: a, attribute: href } }
"#;
        let eng = CardigannIndexer::new(IndexerId::new(), Definition::from_yaml(DEF).unwrap());
        // The TV parent (5000) selects every 5xxx mapping, not the movie one.
        let mut tv = eng.tracker_categories(&[5000]);
        tv.sort();
        assert_eq!(tv, vec!["10".to_string(), "11".to_string()]);
        // A specific subcategory selects exactly its mapping.
        assert_eq!(eng.tracker_categories(&[2040]), vec!["20".to_string()]);
        // Nothing requested -> nothing mapped.
        assert!(eng.tracker_categories(&[]).is_empty());
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
        // urldecode / htmldecode / validfilename.
        let null = serde_yaml::Value::Null;
        assert_eq!(
            apply_chain("The%20Matrix%201999", &[filter("urldecode", null.clone())]),
            "The Matrix 1999"
        );
        assert_eq!(
            apply_chain(
                "Tom &amp; Jerry &#39;48",
                &[filter("htmldecode", null.clone())]
            ),
            "Tom & Jerry '48"
        );
        assert_eq!(
            apply_chain("a/b:c?d", &[filter("validfilename", null)]),
            "a_b_c_d"
        );
    }

    #[test]
    fn enrichment_builds_magnet_from_infohash_and_flags_freeleech() {
        const DEF: &str = r#"
id: t
name: T
links: [https://t.example/]
protocol: torrent
search:
  paths: [{ path: /s }]
  rows: { selector: tr.r }
  fields:
    title: { selector: td.t }
    infohash: { selector: td.h }
    downloadvolumefactor: { selector: td.dvf }
"#;
        const HTML: &str = r#"<table><tr class="r">
            <td class="t">Cool.Release.1080p</td>
            <td class="h">CAFEBABEDEADBEEF0000111122223333AAAABBBB</td>
            <td class="dvf">0</td>
        </tr></table>"#;
        let eng = CardigannIndexer::new(IndexerId::new(), Definition::from_yaml(DEF).unwrap());
        let rel = &eng.extract(HTML).unwrap()[0];
        // A bare infohash field becomes a magnet carrying the title as dn.
        assert!(rel
            .download_url
            .starts_with("magnet:?xt=urn:btih:cafebabedeadbeef0000111122223333aaaabbbb&dn="));
        assert!(rel.download_url.contains("Cool.Release.1080p"));
        // downloadvolumefactor=0 marks freeleech.
        assert_eq!(rel.indexer_flags, vec!["freeleech".to_string()]);
    }

    #[test]
    fn json_pointer_navigates_keys_and_indices() {
        let v = serde_json::json!({"a": {"b": [{"c": "x"}, {"c": "y"}]}});
        assert_eq!(
            json_pointer(&v, "a.b[1].c").and_then(json_scalar),
            Some("y".to_string())
        );
        assert_eq!(
            json_pointer(&v, "$.a.b[0].c").and_then(json_scalar),
            Some("x".to_string())
        );
        assert!(json_pointer(&v, "a.missing").is_none());
    }

    #[test]
    fn extract_json_navigates_rows_and_fields() {
        const DEF: &str = r#"
id: t
name: T
links: [https://t.example/]
search:
  response: { type: json }
  paths: [{ path: /api }]
  rows: { selector: "data.results" }
  fields:
    title: { selector: name }
    infohash: { selector: hash }
    size: { selector: sizeBytes }
    seeders: { selector: "peers.seed" }
"#;
        const JSON: &str = r#"{ "data": { "results": [
            { "name": "Movie.2024.1080p", "hash": "AAAA1111BBBB2222CCCC3333DDDD4444EEEE5555",
              "sizeBytes": 1572864000, "peers": { "seed": 42 } },
            { "name": "Other.2024.720p", "hash": "0000111122223333444455556666777788889999",
              "sizeBytes": 734003200, "peers": { "seed": 5 } }
        ] } }"#;
        let eng = CardigannIndexer::new(IndexerId::new(), Definition::from_yaml(DEF).unwrap());
        let rels = eng.extract(JSON).unwrap();
        assert_eq!(rels.len(), 2);
        assert_eq!(rels[0].title, "Movie.2024.1080p");
        assert_eq!(rels[0].size, Some(1_572_864_000));
        assert_eq!(rels[0].seeders, Some(42));
        // The numeric infohash field is turned into a magnet.
        assert!(rels[0]
            .download_url
            .starts_with("magnet:?xt=urn:btih:aaaa1111bbbb2222cccc3333dddd4444eeee5555&dn="));
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
            encoding: None,
            request_delay: None,
            caps: DefinitionCaps::default(),
            search: SearchBlock {
                paths: vec![],
                inputs: BTreeMap::new(),
                response: ResponseBlock::default(),
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

    /// Cells often carry a CSS-hidden label next to the value; the label is part of
    /// the element's text, so anchoring on the start of the string loses the number.
    #[test]
    fn parse_size_reads_a_labelled_cell() {
        assert_eq!(
            parse_size("Size 4.2 GB"),
            Some((4.2 * 1024.0 * 1024.0 * 1024.0) as u64)
        );
        assert_eq!(parse_size("  Size\n 700 MB "), Some(700 * 1024 * 1024));
        assert_eq!(parse_size("1.5GiB"), Some((1.5 * 1024.0f64.powi(3)) as u64));
        // A number in a unitless label is still a byte count.
        assert_eq!(parse_size("Size 1234"), Some(1234));
        // A unit that isn't a size unit is not a size.
        assert_eq!(parse_size("Age 4 years ago"), None);
    }

    #[test]
    fn parse_count_reads_a_labelled_cell() {
        assert_eq!(parse_count("Seeds 137"), Some(137));
        assert_eq!(parse_count("88"), Some(88));
        assert_eq!(parse_count("1,204"), Some(1204));
        assert_eq!(parse_count("Seeds"), None);
        assert_eq!(parse_count(""), None);
    }

    #[test]
    fn split_leading_number_stops_at_trailing_separators() {
        assert_eq!(
            split_leading_number("4 GB, seeded"),
            Some((4.0, " GB, seeded"))
        );
        assert_eq!(split_leading_number("no digits here"), None);
    }

    #[test]
    fn decode_body_honors_declared_encoding() {
        // 0xC0,0xC1 are 'А','Б' (U+0410/U+0411) in windows-1251.
        assert_eq!(decode_body(&[0xC0, 0xC1], Some("windows-1251")), "АБ");
        // UTF-8 is the default and the fallback for an unknown/empty label.
        assert_eq!(decode_body("héllo".as_bytes(), None), "héllo");
        assert_eq!(decode_body("x".as_bytes(), Some("bogus-enc")), "x");
        assert_eq!(decode_body("y".as_bytes(), Some("")), "y");
    }
}
