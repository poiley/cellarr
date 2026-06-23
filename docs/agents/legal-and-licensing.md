# Legal & licensing — clean-room rules

**This is not legal advice.** It is the project's working policy for reusing upstream material
safely. When something here is load-bearing for a real release, a human with counsel decides — flag
it, don't guess.

## The facts that drive the policy

- **Sonarr, Radarr, Lidarr, Readarr, Prowlarr are all GPLv3.** Translating their *code* into Rust
  (transcribing `Parser.cs`, porting regex tables line by line, copying logic structure) creates a
  **derivative work** — cellarr would then have to be GPLv3.
- **SRCL (`srcl` / www-sacred) is MIT.** Safe to use as our UI component layer.
- **The `Prowlarr/Indexers` Cardigann definitions repo has *no declared license*.** Do **not**
  vendor those YAML files into cellarr.
- **Test-fixture *data*** — individual release strings paired with their correct parse — are close
  to **uncopyrightable facts** (there is one correct parse; merger doctrine). The **selection and
  arrangement** of a whole fixture file may carry **thin** compilation copyright, and the EU adds a
  separate *sui generis* database right.

## The license decision (must be made before first code)

The project's license is **load-bearing** and interacts with how much we reuse:

- If we are comfortable being **GPLv3**, we have maximum freedom to learn from the originals (still
  reimplement, don't transcribe — but the license risk of accidental derivation is moot).
- If we want a **permissive or proprietary/SaaS** option, we must be **strict clean-room**: no
  copied code, and careful about corpus provenance.

Until a human decides, **assume GPLv3 and follow the clean-room rules anyway** (they're good
practice regardless and keep options open).

## Clean-room rules (follow these always)

1. **Learn behavior, not code.** You may read the upstream source to understand *what* it does and
   *why* an edge case exists. You may **not** copy it, paraphrase it line-by-line, or reproduce its
   regex tables or structure. Implement from your understanding + the corpus.
2. **Reuse facts/data, re-curated.** Extract individual input→expected **vectors** into `/corpus`.
   **Re-curate them** — re-order, regroup, merge sources, add your own — rather than copying a whole
   upstream fixture file verbatim (which would copy its selection/arrangement).
3. **Record provenance.** Every corpus vector records its `source`. This documents that we took the
   *fact*, and lets a human audit later.
4. **Don't vendor unlicensed data.** The Cardigann definitions are consumed via our own engine from
   a source the *user* points at — they are not committed to this repo. Same caution for any
   data set without a clear compatible license.
5. **Community data with clear licenses is fine to use per its terms:** MusicBrainz/OpenLibrary
   (CC0), and TRaSH-Guides / TheXEM / anime-lists per their stated terms — check and follow each.
6. **Metadata sources:** never proxy through the originals' Skyhook/RadarrAPI (ToS). Use our own
   keys. Note TMDb's no-commercial-use clause and TheTVDB's paid licensing if cellarr is ever
   distributed commercially — flag to a human.
7. **`reference/` is read-only and never shipped.** It's git-ignored. It exists so you can study; it
   is not a source to copy from.

## When in doubt

Reusing anything beyond individual factual vectors, or anything from a source without a clear
compatible license, is **not** an autonomous decision. Flag it, cite the source and its license, and
let a human decide. A wrong call here is expensive in a way a wrong call in internal code is not.
