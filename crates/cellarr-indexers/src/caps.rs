//! Capabilities (`t=caps`) parsing.
//!
//! Both Torznab and Newznab expose a `t=caps` document describing which search
//! modes exist, which params each accepts, and the server's category tree. Per
//! `docs/06-integrations.md` we **read** this and never hardcode categories or
//! assume a param is supported — so the rest of the adapter consults [`Caps`]
//! before building a query.

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::error::{IndexerError, Result};

/// One advertised search mode (`search`, `tv-search`, `movie-search`, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMode {
    /// The Torznab/Newznab `t=` value this mode maps to (e.g. `search`,
    /// `tvsearch`, `movie`).
    pub mode: String,
    /// Whether the server advertises the mode as available.
    pub available: bool,
    /// The params the server says it accepts for this mode (e.g. `q`, `season`,
    /// `ep`, `tvdbid`, `imdbid`). We only send params present here.
    pub supported_params: Vec<String>,
}

impl SearchMode {
    /// Whether this mode accepts the given param according to caps.
    #[must_use]
    pub fn supports_param(&self, param: &str) -> bool {
        self.supported_params.iter().any(|p| p == param)
    }
}

/// A category from the server's category tree (with any subcategories).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    /// Numeric category id (thousands-based scheme: 2000 movies, 5000 TV, …).
    pub id: u32,
    /// Human-facing category name.
    pub name: String,
    /// Subcategories (e.g. 5040 TV/HD under 5000 TV).
    pub subcategories: Vec<Category>,
}

/// Parsed capabilities for one indexer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caps {
    /// Advertised search modes, keyed by their `t=` value.
    pub modes: Vec<SearchMode>,
    /// The advertised category tree (top-level categories with subcats).
    pub categories: Vec<Category>,
    /// Server-advertised maximum results per query, when present.
    pub limits_max: Option<u32>,
    /// Server-advertised default results per query, when present.
    pub limits_default: Option<u32>,
}

impl Caps {
    /// Look up an advertised mode by its `t=` value.
    #[must_use]
    pub fn mode(&self, mode: &str) -> Option<&SearchMode> {
        self.modes.iter().find(|m| m.mode == mode && m.available)
    }

    /// Whether the named mode is advertised and available.
    #[must_use]
    pub fn has_mode(&self, mode: &str) -> bool {
        self.mode(mode).is_some()
    }

    /// Flatten the category tree into `(id, name)` pairs, subcategories included.
    #[must_use]
    pub fn flat_categories(&self) -> Vec<(u32, String)> {
        fn walk(cat: &Category, out: &mut Vec<(u32, String)>) {
            out.push((cat.id, cat.name.clone()));
            for sub in &cat.subcategories {
                walk(sub, out);
            }
        }
        let mut out = Vec::new();
        for cat in &self.categories {
            walk(cat, &mut out);
        }
        out
    }
}

/// Map the XML element name of a search node to its `t=` value.
///
/// Newznab uses `<search>` / `<tv-search>` / `<movie-search>` / `<audio-search>`
/// / `<book-search>`; the corresponding `t=` values are `search` / `tvsearch` /
/// `movie` / `music` / `book`.
fn mode_for_element(element: &str) -> Option<&'static str> {
    match element {
        "search" => Some("search"),
        "tv-search" => Some("tvsearch"),
        "movie-search" => Some("movie"),
        "audio-search" | "music-search" => Some("music"),
        "book-search" => Some("book"),
        _ => None,
    }
}

/// Parse a `t=caps` XML document into [`Caps`].
///
/// Tolerant by design: unknown elements/attributes are ignored so a server
/// adding fields never breaks parsing (the integration layer must survive
/// schema drift — see `docs/06-integrations.md`).
pub fn parse_caps(xml: &str) -> Result<Caps> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut caps = Caps::default();
    // Stack of categories under construction, so nested <subcat> attaches to the
    // currently-open <category>.
    let mut cat_stack: Vec<Category> = Vec::new();
    let mut buf = Vec::new();

    // Close the category currently on top of the stack, attaching it to its
    // parent (or to the top-level list if there is none).
    fn close_category(cat_stack: &mut Vec<Category>, caps: &mut Caps) {
        if let Some(done) = cat_stack.pop() {
            match cat_stack.last_mut() {
                Some(parent) => parent.subcategories.push(done),
                None => caps.categories.push(done),
            }
        }
    }

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| IndexerError::Parse(format!("caps xml: {e}")))?;

        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                // An empty element (`<subcat .../>`) has no matching `End`, so a
                // category it opens must be closed before the next event.
                let self_closing = matches!(event, Event::Empty(_));
                let local = e.local_name();
                let name = String::from_utf8_lossy(local.as_ref()).to_string();

                match name.as_str() {
                    "limits" => {
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.local_name().as_ref()).to_string();
                            let val = attr.unescape_value().unwrap_or_default().to_string();
                            match key.as_str() {
                                "max" => caps.limits_max = val.parse().ok(),
                                "default" => caps.limits_default = val.parse().ok(),
                                _ => {}
                            }
                        }
                    }
                    other if mode_for_element(other).is_some() => {
                        let mode = mode_for_element(other).unwrap_or(other);
                        let mut sm = SearchMode {
                            mode: mode.to_string(),
                            available: false,
                            supported_params: Vec::new(),
                        };
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.local_name().as_ref()).to_string();
                            let val = attr.unescape_value().unwrap_or_default().to_string();
                            match key.as_str() {
                                "available" => sm.available = val.eq_ignore_ascii_case("yes"),
                                "supportedParams" => {
                                    sm.supported_params = val
                                        .split(',')
                                        .map(str::trim)
                                        .filter(|s| !s.is_empty())
                                        .map(ToString::to_string)
                                        .collect();
                                }
                                _ => {}
                            }
                        }
                        caps.modes.push(sm);
                    }
                    "category" | "subcat" => {
                        let mut id = 0u32;
                        let mut cname = String::new();
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.local_name().as_ref()).to_string();
                            let val = attr.unescape_value().unwrap_or_default().to_string();
                            match key.as_str() {
                                "id" => id = val.parse().unwrap_or(0),
                                "name" => cname = val,
                                _ => {}
                            }
                        }
                        cat_stack.push(Category {
                            id,
                            name: cname,
                            subcategories: Vec::new(),
                        });
                        if self_closing {
                            close_category(&mut cat_stack, &mut caps);
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => {
                let local = e.local_name();
                let name = String::from_utf8_lossy(local.as_ref()).to_string();
                if name == "category" || name == "subcat" {
                    close_category(&mut cat_stack, &mut caps);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(caps)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPS: &str = r#"<?xml version="1.0"?>
<caps>
  <limits max="100" default="50" />
  <searching>
    <search available="yes" supportedParams="q" />
    <tv-search available="yes" supportedParams="q,season,ep,tvdbid" />
    <movie-search available="no" supportedParams="q,imdbid" />
  </searching>
  <categories>
    <category id="5000" name="TV">
      <subcat id="5040" name="TV/HD" />
      <subcat id="5070" name="TV/Anime" />
    </category>
  </categories>
</caps>"#;

    #[test]
    fn parses_modes_and_supported_params() {
        let caps = parse_caps(CAPS).expect("parse");
        assert!(caps.has_mode("search"));
        assert!(caps.has_mode("tvsearch"));
        // available="no" is not reported as an available mode.
        assert!(!caps.has_mode("movie"));

        let tv = caps.mode("tvsearch").expect("tvsearch present");
        assert!(tv.supports_param("season"));
        assert!(tv.supports_param("tvdbid"));
        assert!(!tv.supports_param("imdbid"));
    }

    #[test]
    fn parses_nested_category_tree_without_hardcoding() {
        let caps = parse_caps(CAPS).expect("parse");
        assert_eq!(caps.categories.len(), 1);
        let tv = &caps.categories[0];
        assert_eq!(tv.id, 5000);
        assert_eq!(tv.subcategories.len(), 2);
        let flat = caps.flat_categories();
        assert!(flat.contains(&(5040, "TV/HD".to_string())));
        assert!(flat.contains(&(5070, "TV/Anime".to_string())));
    }

    #[test]
    fn parses_limits() {
        let caps = parse_caps(CAPS).expect("parse");
        assert_eq!(caps.limits_max, Some(100));
        assert_eq!(caps.limits_default, Some(50));
    }
}
