-- Synthetic, SANITIZED Sonarr fixture (no personal data).
--
-- A schema-representative *subset* of the real Sonarr SQLite schema: only the
-- tables/columns cellarr-migrate reads. One invented series with two seasons and
-- three episodes (one episode has a recognized file, one shares a multi-episode
-- file, one is missing), one quality profile, one TRaSH-shaped custom format, and
-- one each of root folder / indexer / download client. All data is fabricated.

PRAGMA foreign_keys = OFF;

CREATE TABLE Series (
    Id        INTEGER PRIMARY KEY,
    Title     TEXT NOT NULL,
    Year      INTEGER,
    TvdbId    INTEGER,
    TmdbId    INTEGER,
    ImdbId    TEXT,
    Monitored INTEGER NOT NULL DEFAULT 1,
    QualityProfileId INTEGER
);

CREATE TABLE Episodes (
    Id                    INTEGER PRIMARY KEY,
    SeriesId              INTEGER NOT NULL,
    SeasonNumber          INTEGER NOT NULL,
    EpisodeNumber         INTEGER NOT NULL,
    AbsoluteEpisodeNumber INTEGER,
    Title                 TEXT,
    Monitored             INTEGER NOT NULL DEFAULT 1,
    EpisodeFileId         INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE EpisodeFiles (
    Id           INTEGER PRIMARY KEY,
    SeriesId     INTEGER NOT NULL,
    SeasonNumber INTEGER NOT NULL,
    Path         TEXT NOT NULL,
    Size         INTEGER NOT NULL DEFAULT 0,
    Quality      TEXT NOT NULL
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

INSERT INTO Series (Id, Title, Year, TvdbId, TmdbId, ImdbId, Monitored, QualityProfileId)
VALUES (1, 'Synthetic Series', 2015, 900001, 800001, 'tt2000001', 1, 1);

-- S01E01 has its own file (id 21); S01E02 and S02E01 share a multi-episode file
-- (id 22) to exercise the multi-content link; S02E02 is missing (file id 0).
INSERT INTO Episodes
    (Id, SeriesId, SeasonNumber, EpisodeNumber, AbsoluteEpisodeNumber, Title, Monitored, EpisodeFileId)
VALUES
    (101, 1, 1, 1, 1, 'Pilot',        1, 21),
    (102, 1, 1, 2, 2, 'Second',       1, 22),
    (103, 1, 2, 1, 3, 'Season Two',   1, 22),
    (104, 1, 2, 2, 4, 'Finale',       1, 0);

INSERT INTO EpisodeFiles (Id, SeriesId, SeasonNumber, Path, Size, Quality)
VALUES
    (21, 1, 1, '/tv/Synthetic Series/Season 01/Synthetic Series - S01E01 - Pilot WEBDL-1080p.mkv',
     2000000000,
     '{"quality":{"id":3,"name":"WEBDL-1080p","source":"web","resolution":1080},"revision":{"version":1,"real":0}}'),
    (22, 1, 1, '/tv/Synthetic Series/Season 01/Synthetic Series - S01E02-E03 Bluray-1080p.mkv',
     4000000000,
     '{"quality":{"id":7,"name":"Bluray-1080p","source":"bluray","resolution":1080},"revision":{"version":1,"real":0}}');

INSERT INTO QualityProfiles (Id, Name, Cutoff, Items, MinFormatScore, CutoffFormatScore, FormatItems, UpgradeAllowed)
VALUES
    (1, 'WEB-1080p', 7,
     '[{"quality":{"id":1,"name":"SDTV"},"allowed":false},
       {"quality":{"id":3,"name":"WEBDL-1080p"},"allowed":true},
       {"quality":{"id":7,"name":"Bluray-1080p"},"allowed":true}]',
     0, 100,
     '[{"format":1,"score":25}]',
     1);

INSERT INTO CustomFormats (Id, Name, Specifications)
VALUES
    (1, 'HDR10',
     '[{"name":"HDR10","implementation":"ReleaseTitleSpecification","negate":false,"required":true,"fields":{"value":"\\bHDR10\\b"}}]');

INSERT INTO RootFolders (Id, Path) VALUES (1, '/tv');

INSERT INTO Indexers (Id, Name, Implementation, Settings, Protocol, Priority, EnableRss)
VALUES
    (1, 'Synthetic Newznab', 'Newznab',
     '{"baseUrl":"http://synthetic.invalid/api","apiKey":"REDACTED","categories":[5000]}',
     'usenet', 25, 1);

INSERT INTO DownloadClients (Id, Name, Implementation, Settings, Protocol, Priority, Enable)
VALUES
    (1, 'Synthetic SABnzbd', 'Sabnzbd',
     '{"host":"127.0.0.1","port":8085,"category":"sonarr"}',
     'usenet', 1, 1);
