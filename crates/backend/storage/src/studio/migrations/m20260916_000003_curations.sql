CREATE TABLE curations (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    document TEXT NOT NULL CHECK (json_valid(document)),
    updated_ms BIGINT NOT NULL,
    PRIMARY KEY (project, id)
);
CREATE INDEX curations_recency ON curations(project, updated_ms DESC, id);
