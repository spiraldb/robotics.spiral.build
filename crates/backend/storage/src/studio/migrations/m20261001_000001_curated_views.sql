-- A published curation output: the statement Studio reads it through, by the
-- name Data lists it under. Spiral keeps no hosted views, so Studio does. A
-- name is a SQL identifier, so two that differ only in case are one view.
CREATE TABLE curated_views (
    project TEXT NOT NULL,
    name TEXT NOT NULL COLLATE NOCASE,
    statement TEXT NOT NULL,
    PRIMARY KEY (project, name)
);
