-- One opaque, versioned Build graph document per row. The payload stays TEXT
-- without a json_valid check: only the frontend reads it, and the wire contract
-- bounds its size without promising it parses as JSON.
CREATE TABLE build_documents (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    encoding INTEGER NOT NULL CHECK (encoding > 0),
    payload TEXT NOT NULL,
    updated_ms BIGINT NOT NULL,
    PRIMARY KEY (project, id)
);
CREATE INDEX build_documents_recency ON build_documents(project, updated_ms DESC, id);
