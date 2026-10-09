-- The token a workspace's robot connector links to Studio with (October 9,
-- 2026), kept only as its SHA-256: Studio shows a token once, when it is
-- issued. Issuing another replaces it, and a deleted workspace's goes with it.
CREATE TABLE robot_connector_tokens (
    endpoint TEXT NOT NULL,
    project_id TEXT NOT NULL,
    token_sha256 BLOB NOT NULL UNIQUE CHECK (length(token_sha256) = 32),
    PRIMARY KEY (endpoint, project_id),
    FOREIGN KEY (endpoint, project_id)
        REFERENCES workspaces (endpoint, project_id) ON DELETE CASCADE
);
