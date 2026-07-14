-- Remembered directory modification times, so a library rescan only re-reads the
-- directories that changed since the last scan (mtime-based incremental walk).
--
-- A directory's mtime bumps whenever a direct child is added, removed, or renamed
-- — so an unchanged mtime means no new files landed directly in it. The rescan
-- stats every directory (cheap, ~0ms each) but only `read_dir`s the ones whose
-- mtime differs from the value recorded here; unchanged subtrees are reconstructed
-- from this map without any directory reads. A large library on NFS did ~14k
-- read_dir round-trips per rescan even with zero import candidates; this cuts a
-- steady-state rescan to a stat sweep plus a handful of changed-directory reads.
--
-- `mtime` is Unix nanoseconds (see cellarr-fs `dir_mtime_nanos`): fine enough that
-- a file added in the same wall second as the previous scan still registers.
CREATE TABLE IF NOT EXISTS scan_dir (
    path  TEXT PRIMARY KEY NOT NULL,
    mtime INTEGER NOT NULL
);
