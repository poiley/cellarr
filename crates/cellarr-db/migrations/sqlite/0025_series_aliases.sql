-- Alternate titles a series is also known by (TheTVDB aliases / romanizations /
-- English names), stored as a JSON array of strings. Lets an English-named library
-- file match a series whose canonical title is non-English (e.g. a "Naruto" file
-- adopting onto "NARUTO－ナルト－"): the content matcher checks the parsed title
-- against these aliases, not just the exact indexed title.
ALTER TABLE series_meta ADD COLUMN aliases TEXT;
