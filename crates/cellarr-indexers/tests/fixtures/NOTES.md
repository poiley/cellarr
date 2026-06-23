# Indexer test fixtures (SYNTHETIC)

Every file in this directory is **synthetic** — hand-authored from the *documented
protocol shapes*, not captured from any live indexer or tracker. No live service
was contacted to produce them (see the project's offline rule and
`docs/06-integrations.md`). They exist so the record/replay tests can assert
normalization into `cellarr_core::Release` with zero network.

Shapes are modeled on the public Torznab/Newznab specifications and the Cardigann
definition format:

- `torznab_caps.xml` — a Torznab `t=caps` response: `<caps>` with `<server>`,
  `<limits>`, `<searching>` (search / tv-search / movie-search modes with
  `available` + `supportedParams`), and a nested `<categories>` tree using the
  thousands-based scheme (2000 Movies, 5000 TV, subcats 5040 TV/HD, 5070 TV/Anime).
- `torznab_search.xml` — a Torznab search response: `<rss><channel><item>` rows
  with `<enclosure>` magnet/.torrent links and `<torznab:attr>` pairs (size,
  seeders, peers, infohash, downloadvolumefactor for freeleech).
- `newznab_caps.xml` — a Newznab `t=caps` response (same shape; Usenet retention
  in `<server>`).
- `newznab_search.xml` — a Newznab search response: `<item>` rows with `.nzb`
  `<enclosure>` and `<newznab:attr>` pairs (size, grabs).
- `cardigann_mytracker.yml` — a synthetic Cardigann definition (`id`, `name`,
  `caps` with `categorymappings`/`modes`, `search` with `paths`/`rows`/`fields`).
  **NOT** copied from `Prowlarr/Indexers` (which has no declared license and must
  never be vendored — see `docs/agents/legal-and-licensing.md`); it is an original
  example written to match the documented field structure.
- `cardigann_mytracker.html` — a synthetic search-results page matching the
  definition's CSS `rows`/`fields` selectors.

## Known follow-ups (skeleton scope)

The Cardigann engine here interprets **CSS** selectors only. XPath selectors,
`filters` (regex replace/append/dateparse), templated paths/inputs, and the
login/download flows are explicit follow-ups, called out in `cardigann.rs`.
