-- Series alternate titles as a JSON array (see the SQLite 0025 twin). Additive
-- nullable column; the checksum-tracked 0001 schema is untouched.
ALTER TABLE series_meta ADD COLUMN aliases text;
