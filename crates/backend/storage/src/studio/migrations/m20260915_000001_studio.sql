CREATE TABLE backend_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    document TEXT NOT NULL CHECK (json_valid(document))
);
CREATE TABLE robots (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    document TEXT NOT NULL CHECK (json_valid(document)),
    updated_ms INTEGER NOT NULL,
    PRIMARY KEY (project, id)
);
CREATE TABLE workflows (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    document TEXT NOT NULL CHECK (json_valid(document)),
    updated_ms BIGINT NOT NULL,
    PRIMARY KEY (project, id)
);
CREATE INDEX workflows_recency ON workflows(project, updated_ms DESC, id);
CREATE TABLE training_runs (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    ordinal BIGINT NOT NULL CHECK (ordinal >= 0),
    document TEXT NOT NULL CHECK (json_valid(document)),
    PRIMARY KEY (project, id)
);
CREATE TABLE studio_settings (
    project TEXT PRIMARY KEY NOT NULL,
    document TEXT NOT NULL CHECK (json_valid(document))
);
CREATE TABLE rollout_runs (
    project TEXT PRIMARY KEY NOT NULL,
    document TEXT NOT NULL CHECK (json_valid(document))
);
-- Retained for portable transfer of legacy feature definitions. Execution uses
-- native Spiral SQL; this relation has no active inference consumer.
CREATE TABLE feature_registry (
    project TEXT PRIMARY KEY NOT NULL,
    document TEXT NOT NULL CHECK (json_valid(document))
);
