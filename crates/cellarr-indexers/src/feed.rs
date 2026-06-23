//! Torznab/Newznab search-result (RSS) parsing into [`Release`].
//!
//! Both protocols answer searches with `<rss><channel><item>…` where each item
//! carries an `<enclosure>` pointing at the `.torrent`/`.nzb`/magnet and a run of
//! `<torznab:attr>` / `<newznab:attr>` `name=/value=` pairs (size, seeders,
//! infohash, freeleech factors, …). We normalize each item into a protocol-
//! agnostic [`Release`] so downstream stages stay indexer-agnostic
//! (`docs/06-integrations.md`).

use cellarr_core::{IndexerId, Protocol, Release};
use quick_xml::events::Event;
use quick_xml::Reader;

use crate::error::{IndexerError, Result};

/// An item under construction while streaming the feed.
#[derive(Default)]
struct ItemBuilder {
    title: Option<String>,
    guid: Option<String>,
    link: Option<String>,
    enclosure_url: Option<String>,
    size: Option<u64>,
    seeders: Option<u32>,
    flags: Vec<String>,
}

impl ItemBuilder {
    /// Apply one `<torznab:attr>`/`<newznab:attr>` `name`/`value` pair.
    fn apply_attr(&mut self, name: &str, value: &str) {
        match name {
            "size" => self.size = value.parse().ok(),
            "seeders" => self.seeders = value.parse().ok(),
            // A download volume factor of 0 is total freeleech; <1 is partial.
            // We record the human flag rather than the factor so the decision
            // stage can match on a stable string.
            "downloadvolumefactor" => {
                if let Ok(factor) = value.parse::<f64>() {
                    if factor <= 0.0 {
                        self.flags.push("freeleech".to_string());
                    } else if factor < 1.0 {
                        self.flags.push("partial-freeleech".to_string());
                    }
                }
            }
            _ => {}
        }
    }

    /// Finalize into a [`Release`], if it has the minimum required fields.
    fn build(self, indexer_id: IndexerId, protocol: Protocol) -> Option<Release> {
        let title = self.title?;
        // Prefer the enclosure URL (the actual download); fall back to <link>.
        let download_url = self.enclosure_url.or(self.link)?;
        Some(Release {
            indexer_id,
            title,
            download_url,
            guid: self.guid,
            protocol,
            size: self.size,
            seeders: self.seeders,
            indexer_flags: self.flags,
        })
    }
}

/// Parse a Torznab/Newznab search response into a list of [`Release`].
///
/// `protocol` is supplied by the caller because the same XML shape serves both
/// torrents (Torznab) and Usenet (Newznab); the document does not reliably
/// declare which it is.
pub fn parse_feed(xml: &str, indexer_id: IndexerId, protocol: Protocol) -> Result<Vec<Release>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut releases = Vec::new();
    let mut current: Option<ItemBuilder> = None;
    // The element whose text content we are currently capturing (title/guid/link).
    let mut text_target: Option<String> = None;
    let mut buf = Vec::new();

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| IndexerError::Parse(format!("feed xml: {e}")))?;

        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let local = e.local_name();
                let name = String::from_utf8_lossy(local.as_ref()).to_string();
                match name.as_str() {
                    "item" => current = Some(ItemBuilder::default()),
                    "enclosure" => {
                        if let Some(item) = current.as_mut() {
                            for attr in e.attributes().flatten() {
                                let key = String::from_utf8_lossy(attr.key.local_name().as_ref())
                                    .to_string();
                                if key == "url" {
                                    item.enclosure_url =
                                        Some(attr.unescape_value().unwrap_or_default().to_string());
                                }
                            }
                        }
                    }
                    "attr" => {
                        // `<torznab:attr name= value=>` or the newznab equivalent.
                        if let Some(item) = current.as_mut() {
                            let mut attr_name = String::new();
                            let mut attr_value = String::new();
                            for attr in e.attributes().flatten() {
                                let key = String::from_utf8_lossy(attr.key.local_name().as_ref())
                                    .to_string();
                                let val = attr.unescape_value().unwrap_or_default().to_string();
                                match key.as_str() {
                                    "name" => attr_name = val,
                                    "value" => attr_value = val,
                                    _ => {}
                                }
                            }
                            item.apply_attr(&attr_name, &attr_value);
                        }
                    }
                    "title" | "guid" | "link" if current.is_some() => {
                        text_target = Some(name);
                    }
                    _ => {}
                }
            }
            Event::Text(e) => {
                if let (Some(target), Some(item)) = (text_target.as_deref(), current.as_mut()) {
                    let text = e
                        .unescape()
                        .map_err(|err| IndexerError::Parse(format!("feed text: {err}")))?
                        .to_string();
                    match target {
                        "title" => item.title = Some(text),
                        "guid" => item.guid = Some(text),
                        "link" => item.link = Some(text),
                        _ => {}
                    }
                }
            }
            Event::End(e) => {
                let local = e.local_name();
                let name = String::from_utf8_lossy(local.as_ref()).to_string();
                match name.as_str() {
                    "item" => {
                        if let Some(item) = current.take() {
                            if let Some(release) = item.build(indexer_id, protocol) {
                                releases.push(release);
                            }
                        }
                    }
                    "title" | "guid" | "link" => text_target = None,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(releases)
}
