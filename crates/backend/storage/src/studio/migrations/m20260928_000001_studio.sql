CREATE TABLE backend_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    document TEXT NOT NULL CHECK (json_valid(document))
);
CREATE TABLE robots (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    document TEXT NOT NULL CHECK (json_valid(document)),
    revision INTEGER NOT NULL,
    PRIMARY KEY (project, id)
);
CREATE TABLE studio_settings (
    project TEXT PRIMARY KEY NOT NULL,
    document TEXT NOT NULL CHECK (json_valid(document))
);
CREATE TABLE training_runs (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    document TEXT NOT NULL CHECK (json_valid(document)),
    PRIMARY KEY (project, id)
);
-- The payload stays TEXT without a json_valid check: only the frontend reads
-- it, and the wire contract bounds its size without promising it parses as JSON.
CREATE TABLE curations (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    encoding INTEGER NOT NULL CHECK (encoding > 0),
    payload TEXT NOT NULL,
    updated_ms BIGINT NOT NULL,
    PRIMARY KEY (project, id)
);
CREATE INDEX curations_recency ON curations(project, updated_ms DESC, id);
CREATE TABLE dataset_robots (
    project TEXT NOT NULL,
    dataset TEXT NOT NULL,
    version TEXT NOT NULL,
    document JSON NOT NULL,
    PRIMARY KEY (project, dataset)
);
CREATE TABLE dataset_joint_roles (
    project TEXT NOT NULL,
    dataset TEXT NOT NULL,
    version TEXT NOT NULL,
    document JSON NOT NULL,
    PRIMARY KEY (project, dataset)
);
CREATE TABLE dataset_check_reports (
    project TEXT NOT NULL,
    dataset TEXT NOT NULL,
    version TEXT NOT NULL,
    definition INTEGER NOT NULL,
    document JSON NOT NULL,
    PRIMARY KEY (project, dataset)
);
