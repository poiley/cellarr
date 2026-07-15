//! The per-media-type [`MediaModule`] implementations (Movie and TV for v1).
//!
//! Each module is the one place type-specific behavior lives; the pipeline never
//! branches on [`MediaType`], it calls the module (see
//! `docs/specs/cellarr-media.md`). A module is constructed with the two seams it
//! needs — a [`ContentLookup`] (to answer "which node?") and a [`MetadataLookup`]
//! (to get titles/aliases/ids) — so it stays pure and is trivially mockable.
//!
//! Confidence policy (shared by both modules, tested at the boundary):
//! - an **exact** normalized title match → [`Confidence::CERTAIN`];
//! - a match via an **alias** (or a year-disambiguated movie) → high but not
//!   certain;
//! - **two or more** candidate nodes a parse could equally satisfy → each is
//!   returned at a deliberately **low** confidence, so the caller surfaces the
//!   ambiguity for manual resolution instead of force-fitting one. This is the
//!   library-safety rule made concrete (never auto-grab onto the wrong node).

use async_trait::async_trait;

use cellarr_core::{
    Confidence, ContentMatch, ContentRef, Coordinates, MediaModule, MediaType, NamingTokens,
    ParsedRelease, SearchTerms,
};

use crate::content::ContentLookup;
use crate::error::MediaError;
use crate::meta::MetadataLookup;

/// Confidence assigned to a single, exact, unambiguous title match.
const CONF_EXACT: f32 = 1.0;
/// Confidence for a sole match via an alias (high, not certain).
const CONF_ALIAS: f32 = 0.85;
/// Confidence at or below which the caller must treat the match as ambiguous and
/// route it to manual resolution. Every member of an N>1 candidate set is
/// emitted at this level.
pub const AMBIGUOUS_CONFIDENCE: f32 = 0.4;

/// Module-level error: a logic failure, or one of the injected seams' errors.
///
/// Keeping the seam errors as distinct variants (rather than flattening to a
/// string) lets the pipeline tell a transient lookup failure from a genuine
/// "could not place this release" outcome.
#[derive(Debug, thiserror::Error)]
pub enum ModuleError<L, M>
where
    L: std::error::Error + Send + Sync + 'static,
    M: std::error::Error + Send + Sync + 'static,
{
    /// A media-logic failure (wrong type, unresolved identity, …).
    #[error(transparent)]
    Media(#[from] MediaError),
    /// The content lookup seam failed.
    #[error("content lookup failed: {0}")]
    Content(#[source] L),
    /// The metadata lookup seam failed.
    #[error("metadata lookup failed: {0}")]
    Metadata(#[source] M),
}

/// Normalize a title for comparison: lowercase, strip punctuation to spaces,
/// collapse runs of whitespace. This makes "The Show: A.K.A." and
/// "the show aka" compare equal without a fuzzy-match dependency, which is all
/// the modules need on top of the parser's already-cleaned title.
fn normalize_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut last_was_space = true; // trims leading space
    for ch in title.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// The parse's best title for matching: the cleaned title the parser produced,
/// falling back to the raw title when it left none.
fn parse_title(parsed: &ParsedRelease) -> &str {
    parsed
        .clean_title
        .as_deref()
        .unwrap_or(parsed.raw_title.as_str())
}

/// Build naming tokens common to every media type from a title.
fn base_tokens(title_label: &str, title: &str) -> Vec<(String, String)> {
    vec![(title_label.to_string(), title.to_string())]
}

/// The movie [`MediaModule`].
///
/// Search terms are the title (+ aliases) plus any external ids; matching is by
/// normalized title with year as a disambiguator; naming exposes the movie title
/// and year.
pub struct MovieModule<L, M> {
    content: L,
    meta: M,
}

impl<L, M> MovieModule<L, M> {
    /// Construct the module from its two seams.
    pub fn new(content: L, meta: M) -> Self {
        Self { content, meta }
    }
}

#[async_trait]
impl<L, M> MediaModule for MovieModule<L, M>
where
    L: ContentLookup,
    M: MetadataLookup,
{
    type Error = ModuleError<L::Error, M::Error>;

    fn media_type(&self) -> MediaType {
        MediaType::Movie
    }

    async fn search_terms(&self, content: &ContentRef) -> Result<SearchTerms, Self::Error> {
        ensure_media_type(content, MediaType::Movie)?;
        let meta = self
            .meta
            .movie_meta(content.id, None)
            .await
            .map_err(ModuleError::Metadata)?
            .ok_or_else(|| MediaError::UnresolvedIdentity(content.id.to_string()))?;

        // Most specific query first: title, then aliases. A year, when known,
        // is appended to the primary title so the indexer narrows same-named
        // films, but a bare title is kept too (some indexers reject the year).
        let mut queries = Vec::new();
        match meta.year {
            Some(year) => {
                queries.push(format!("{} {year}", meta.title));
                queries.push(meta.title.clone());
            }
            None => queries.push(meta.title.clone()),
        }
        queries.extend(meta.aliases.iter().cloned());

        Ok(SearchTerms {
            queries,
            ids: meta.external_ids,
            numbering: Vec::new(),
            // Movies live under the Torznab 2000 range.
            categories: vec![2000],
        })
    }

    async fn match_release(
        &self,
        parsed: &ParsedRelease,
    ) -> Result<Vec<ContentMatch>, Self::Error> {
        let query = parse_title(parsed);
        let candidates = self
            .content
            .candidates_for_title(MediaType::Movie, query)
            .await
            .map_err(ModuleError::Content)?;

        let norm_query = normalize_title(query);
        // A movie parse addresses exactly one unit; coordinates are `Movie`.
        let mut matches = Vec::new();
        for cand in &candidates {
            let conf = title_confidence(&norm_query, &cand.title, &cand.aliases);
            if let Some(conf) = conf {
                matches.push((cand.content_ref.clone(), conf));
            }
        }

        Ok(finalize_matches(matches))
    }

    async fn naming_tokens(&self, content: &ContentRef) -> Result<NamingTokens, Self::Error> {
        ensure_media_type(content, MediaType::Movie)?;
        let meta = self
            .meta
            .movie_meta(content.id, None)
            .await
            .map_err(ModuleError::Metadata)?
            .ok_or_else(|| MediaError::UnresolvedIdentity(content.id.to_string()))?;

        let mut tokens = base_tokens("Movie Title", &meta.title);
        if let Some(year) = meta.year {
            tokens.push(("Release Year".to_string(), year.to_string()));
        }
        Ok(NamingTokens { tokens })
    }
}

/// The TV [`MediaModule`].
///
/// Search terms add season/episode numbering; matching keys on series title and
/// the parse's episode coordinates (a multi-episode parse fans out to several
/// matches); naming exposes series/season/episode tokens, zero-padded.
pub struct TvModule<L, M> {
    content: L,
    meta: M,
}

impl<L, M> TvModule<L, M> {
    /// Construct the module from its two seams.
    pub fn new(content: L, meta: M) -> Self {
        Self { content, meta }
    }
}

#[async_trait]
impl<L, M> MediaModule for TvModule<L, M>
where
    L: ContentLookup,
    M: MetadataLookup,
{
    type Error = ModuleError<L::Error, M::Error>;

    fn media_type(&self) -> MediaType {
        MediaType::Tv
    }

    async fn search_terms(&self, content: &ContentRef) -> Result<SearchTerms, Self::Error> {
        ensure_media_type(content, MediaType::Tv)?;
        let meta = self
            .meta
            .series_meta(content.id, None)
            .await
            .map_err(ModuleError::Metadata)?
            .ok_or_else(|| MediaError::UnresolvedIdentity(content.id.to_string()))?;

        let mut queries = vec![meta.title.clone()];
        queries.extend(meta.aliases.iter().cloned());

        // Numbering parameters mirror the Torznab/Newznab season/ep query keys
        // so an indexer adapter can pass them through directly.
        let numbering = tv_numbering(&content.coords);

        Ok(SearchTerms {
            queries,
            ids: meta.external_ids,
            numbering,
            // TV lives under the Torznab 5000 range.
            categories: vec![5000],
        })
    }

    async fn match_release(
        &self,
        parsed: &ParsedRelease,
    ) -> Result<Vec<ContentMatch>, Self::Error> {
        let query = parse_title(parsed);
        let candidates = self
            .content
            .candidates_for_title(MediaType::Tv, query)
            .await
            .map_err(ModuleError::Content)?;

        let norm_query = normalize_title(query);

        // The episode coordinates the parse advertises. A multi-episode parse
        // carries several; each must find its own node. `Episode` is matched on
        // season+episode; `Absolute` is matched on the episode node's absolute
        // number (the on-disk adopt path has no Identify step to normalize anime
        // absolute numbering to Episode first, so an "[Group] Naruto - 001" file
        // arrives here still absolute-addressed — it must land on the node
        // `expand_series` stamped with that absolute number, not go unmatched).
        // Daily/SeasonPack still address no canonical single node here and are
        // dropped (surfaced upstream / handled by Identify on the grab path).
        let wanted: Vec<&Coordinates> = parsed
            .coordinates
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    Coordinates::Episode { .. } | Coordinates::Absolute { .. }
                )
            })
            .collect();

        let mut matches = Vec::new();
        for cand in &candidates {
            let Some(title_conf) = title_confidence(&norm_query, &cand.title, &cand.aliases) else {
                continue;
            };
            // The candidate's own coordinates must be one the parse wants.
            let coord_hit = wanted.is_empty()
                || wanted
                    .iter()
                    .any(|w| episode_coords_match(w, &cand.content_ref.coords));
            if coord_hit {
                matches.push((cand.content_ref.clone(), title_conf));
            }
        }

        Ok(finalize_matches(matches))
    }

    async fn naming_tokens(&self, content: &ContentRef) -> Result<NamingTokens, Self::Error> {
        ensure_media_type(content, MediaType::Tv)?;
        let meta = self
            .meta
            .series_meta(content.id, None)
            .await
            .map_err(ModuleError::Metadata)?
            .ok_or_else(|| MediaError::UnresolvedIdentity(content.id.to_string()))?;

        let mut tokens = base_tokens("Series Title", &meta.title);
        if let Coordinates::Episode {
            season,
            episode,
            absolute,
        } = &content.coords
        {
            tokens.push(("Season".to_string(), format!("{season:02}")));
            tokens.push(("Episode".to_string(), format!("{episode:02}")));
            if let Some(abs) = absolute {
                tokens.push(("Absolute Episode".to_string(), format!("{abs:03}")));
            }
        }
        Ok(NamingTokens { tokens })
    }
}

/// Confidence of a single candidate title against the (normalized) parse title,
/// or `None` when it is not a plausible match at all.
///
/// Exact normalized match → certain; alias match → high. The N>1 demotion to
/// ambiguous happens in [`finalize_matches`], not here, so a single confident
/// match stays confident.
fn title_confidence(norm_query: &str, title: &str, aliases: &[String]) -> Option<f32> {
    if normalize_title(title) == norm_query {
        return Some(CONF_EXACT);
    }
    if aliases.iter().any(|a| normalize_title(a) == norm_query) {
        return Some(CONF_ALIAS);
    }
    None
}

/// Apply the ambiguity rule and wrap raw `(ref, confidence)` pairs into
/// [`ContentMatch`]es.
///
/// When more than one *distinct* content node matched, no single one can be
/// trusted, so every match is demoted to [`AMBIGUOUS_CONFIDENCE`] and the set is
/// surfaced for manual resolution. A single match keeps its earned confidence.
/// (A multi-episode release legitimately produces several matches for *different*
/// episode nodes; those are distinct nodes the release jointly satisfies, not
/// rival interpretations — they are not demoted. We distinguish the two by
/// whether the matched coordinates differ.)
fn finalize_matches(raw: Vec<(ContentRef, f32)>) -> Vec<ContentMatch> {
    let mut seen: Vec<&Coordinates> = Vec::new();
    for (r, _) in &raw {
        if !seen.contains(&&r.coords) {
            seen.push(&r.coords);
        }
    }
    let distinct_coords = seen.len();
    // Rival interpretations = several matches that resolve to the *same* unit
    // (same coordinates) via different nodes/titles. If they all address the
    // same coordinate but came from >1 node, the title was ambiguous.
    let ambiguous = raw.len() > 1 && distinct_coords <= 1;

    raw.into_iter()
        .map(|(content_ref, conf)| {
            let conf = if ambiguous {
                AMBIGUOUS_CONFIDENCE.min(conf)
            } else {
                conf
            };
            ContentMatch {
                content_ref,
                confidence: Confidence::new(conf),
            }
        })
        .collect()
}

/// Whether two TV coordinates address the same episode (ignoring whether the
/// absolute number is filled in — a parse may carry it, the node may not).
fn episode_coords_match(a: &Coordinates, b: &Coordinates) -> bool {
    match (a, b) {
        (
            Coordinates::Episode {
                season: sa,
                episode: ea,
                ..
            },
            Coordinates::Episode {
                season: sb,
                episode: eb,
                ..
            },
        ) => sa == sb && ea == eb,
        // An absolute-numbered file (anime, `[Group] Show - 001`) matches the
        // episode node carrying that absolute number — how `expand_series` records
        // TVDB's `absoluteNumber`. This is the adopt-path counterpart to the grab
        // path's Identify-time absolute→Episode normalization. `a` is always the
        // parse (file) and `b` the candidate node.
        (
            Coordinates::Absolute { number },
            Coordinates::Episode {
                absolute: Some(abs),
                ..
            },
        ) => number == abs,
        _ => false,
    }
}

/// Torznab/Newznab-style numbering query params for a TV node's coordinates.
fn tv_numbering(coords: &Coordinates) -> Vec<(String, String)> {
    match coords {
        Coordinates::Episode {
            season, episode, ..
        } => vec![
            ("season".to_string(), season.to_string()),
            ("ep".to_string(), episode.to_string()),
        ],
        Coordinates::SeasonPack { season } => vec![("season".to_string(), season.to_string())],
        // A node addressed by air date queries by that date; Absolute has no
        // native indexer numbering key (it is searched by title), so neither
        // contributes a numbering param.
        Coordinates::Daily { .. } | Coordinates::Absolute { .. } => Vec::new(),
        _ => Vec::new(),
    }
}

/// Guard that a content node belongs to the module's media type.
fn ensure_media_type(content: &ContentRef, expected: MediaType) -> Result<(), MediaError> {
    if content.media_type == expected {
        Ok(())
    } else {
        Err(MediaError::WrongMediaType {
            expected,
            coords: format!("{:?}", content.coords),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_punctuation_and_case() {
        assert_eq!(normalize_title("The Show: A.K.A."), "the show a k a");
        assert_eq!(normalize_title("  Spaced   Out  "), "spaced out");
        assert_eq!(
            normalize_title("Already-clean_title"),
            "already clean title"
        );
    }

    #[test]
    fn parse_title_falls_back_to_raw_when_unclean() {
        let p = ParsedRelease::new("Raw.Title.1080p");
        assert_eq!(parse_title(&p), "Raw.Title.1080p");
        let mut p2 = ParsedRelease::new("Raw.Title.1080p");
        p2.clean_title = Some("Raw Title".to_string());
        assert_eq!(parse_title(&p2), "Raw Title");
    }

    #[test]
    fn title_confidence_grades_exact_above_alias() {
        let exact = title_confidence("the matrix", "The Matrix", &[]);
        let alias = title_confidence("matrix", "The Matrix", &["Matrix".to_string()]);
        assert_eq!(exact, Some(CONF_EXACT));
        assert_eq!(alias, Some(CONF_ALIAS));
        assert!(exact.unwrap() > alias.unwrap());
        assert_eq!(title_confidence("nope", "The Matrix", &[]), None);
    }

    #[test]
    fn episode_coords_match_ignores_absolute() {
        let a = Coordinates::Episode {
            season: 1,
            episode: 2,
            absolute: Some(14),
        };
        let b = Coordinates::Episode {
            season: 1,
            episode: 2,
            absolute: None,
        };
        assert!(episode_coords_match(&a, &b));
        let c = Coordinates::Episode {
            season: 1,
            episode: 3,
            absolute: None,
        };
        assert!(!episode_coords_match(&a, &c));
    }

    #[test]
    fn absolute_file_matches_episode_node_by_absolute_number() {
        // An anime "[Group] Show - 014" file parses to Absolute{14}; on adopt it
        // must land on the episode node expand_series stamped with absolute 14.
        let file = Coordinates::Absolute { number: 14 };
        let node = Coordinates::Episode {
            season: 1,
            episode: 2,
            absolute: Some(14),
        };
        assert!(episode_coords_match(&file, &node));
        // A different absolute number does not match.
        let other = Coordinates::Episode {
            season: 1,
            episode: 3,
            absolute: Some(15),
        };
        assert!(!episode_coords_match(&file, &other));
        // A node with no absolute number is never matched by an absolute file.
        let no_abs = Coordinates::Episode {
            season: 1,
            episode: 2,
            absolute: None,
        };
        assert!(!episode_coords_match(&file, &no_abs));
    }

    #[test]
    fn tv_numbering_for_episode_and_seasonpack() {
        let ep = tv_numbering(&Coordinates::Episode {
            season: 3,
            episode: 7,
            absolute: None,
        });
        assert!(ep.contains(&("season".to_string(), "3".to_string())));
        assert!(ep.contains(&("ep".to_string(), "7".to_string())));

        let pack = tv_numbering(&Coordinates::SeasonPack { season: 4 });
        assert_eq!(pack, vec![("season".to_string(), "4".to_string())]);
    }
}
