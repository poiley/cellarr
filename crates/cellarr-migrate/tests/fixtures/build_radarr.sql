-- Synthetic, SANITIZED Radarr fixture (no personal data).
--
-- A schema-representative *subset* of the real Radarr SQLite schema: only the
-- tables and columns cellarr-migrate reads, with two invented movies, one
-- quality profile, one custom format (TRaSH-shaped specifications), and one each
-- of root folder / indexer / download client. All ids, paths, and titles are
-- fabricated for testing.

PRAGMA foreign_keys = OFF;

CREATE TABLE Movies (
    Id            INTEGER PRIMARY KEY,
    Title         TEXT NOT NULL,
    Year          INTEGER,
    TmdbId        INTEGER,
    ImdbId        TEXT,
    Monitored     INTEGER NOT NULL DEFAULT 1,
    QualityProfileId INTEGER,
    MovieFileId   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE MovieFiles (
    Id        INTEGER PRIMARY KEY,
    MovieId   INTEGER NOT NULL,
    Path      TEXT NOT NULL,
    Size      INTEGER NOT NULL DEFAULT 0,
    Quality   TEXT NOT NULL,
    Languages TEXT
);

CREATE TABLE QualityProfiles (
    Id                INTEGER PRIMARY KEY,
    Name              TEXT NOT NULL,
    Cutoff            INTEGER,
    Items             TEXT NOT NULL,
    MinFormatScore    INTEGER NOT NULL DEFAULT 0,
    CutoffFormatScore INTEGER NOT NULL DEFAULT 0,
    FormatItems       TEXT,
    UpgradeAllowed    INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE CustomFormats (
    Id             INTEGER PRIMARY KEY,
    Name           TEXT NOT NULL,
    Specifications TEXT NOT NULL
);

CREATE TABLE RootFolders (
    Id   INTEGER PRIMARY KEY,
    Path TEXT NOT NULL
);

CREATE TABLE Indexers (
    Id             INTEGER PRIMARY KEY,
    Name           TEXT NOT NULL,
    Implementation TEXT,
    Settings       TEXT,
    Protocol       TEXT,
    Priority       INTEGER NOT NULL DEFAULT 25,
    EnableRss      INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE DownloadClients (
    Id             INTEGER PRIMARY KEY,
    Name           TEXT NOT NULL,
    Implementation TEXT,
    Settings       TEXT,
    Protocol       TEXT,
    Priority       INTEGER NOT NULL DEFAULT 1,
    Enable         INTEGER NOT NULL DEFAULT 1
);

-- Two movies: one with a file recognized in place, one missing (no file).
INSERT INTO Movies (Id, Title, Year, TmdbId, ImdbId, Monitored, QualityProfileId, MovieFileId)
VALUES
    (1, 'Synthetic Movie One', 1999, 100001, 'tt1000001', 1, 1, 11),
    (2, 'Synthetic Movie Two', 2010, 100002, 'tt1000002', 1, 1, 0);

INSERT INTO MovieFiles (Id, MovieId, Path, Size, Quality, Languages)
VALUES
    (11, 1, '/movies/Synthetic Movie One (1999)/Synthetic Movie One (1999) Bluray-1080p.mkv',
     8000000000,
     '{"quality":{"id":7,"name":"Bluray-1080p","source":"bluray","resolution":1080},"revision":{"version":1,"real":0}}',
     '[{"id":1,"name":"English"}]');

-- One profile: allow WEBDL-1080p and Bluray-1080p, cutoff Bluray-1080p (id 7),
-- assign +50 to the one custom format, require min CF score 0.
INSERT INTO QualityProfiles (Id, Name, Cutoff, Items, MinFormatScore, CutoffFormatScore, FormatItems, UpgradeAllowed)
VALUES
    (1, 'HD-1080p', 7,
     '[{"quality":{"id":1,"name":"SDTV"},"allowed":false},
       {"quality":{"id":3,"name":"WEBDL-1080p"},"allowed":true},
       {"quality":{"id":7,"name":"Bluray-1080p"},"allowed":true},
       {"quality":{"id":30,"name":"Bluray-2160p"},"allowed":false}]',
     0, 100,
     '[{"format":1,"score":50}]',
     1);

-- One custom format: matches HDR10 releases, TRaSH-shaped specification.
INSERT INTO CustomFormats (Id, Name, Specifications)
VALUES
    (1, 'HDR10',
     '[{"name":"HDR10","implementation":"ReleaseTitleSpecification","negate":false,"required":true,"fields":{"value":"\\bHDR10\\b"}}]');

INSERT INTO RootFolders (Id, Path) VALUES (1, '/movies');

INSERT INTO Indexers (Id, Name, Implementation, Settings, Protocol, Priority, EnableRss)
VALUES
    (1, 'Synthetic Torznab', 'Torznab',
     '{"baseUrl":"http://synthetic.invalid/api","apiKey":"REDACTED","categories":[2000]}',
     'torrent', 25, 1);

INSERT INTO DownloadClients (Id, Name, Implementation, Settings, Protocol, Priority, Enable)
VALUES
    (1, 'Synthetic qBittorrent', 'QBittorrent',
     '{"host":"127.0.0.1","port":8080,"category":"radarr"}',
     'torrent', 1, 1);
