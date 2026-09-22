-- Every rollout session is kept under its own identity, not only the latest
-- one per project. SQLite cannot change a primary key in place, so the table
-- is rebuilt; the latest session carries over under its session identity.
CREATE TABLE rollout_sessions (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    document TEXT NOT NULL CHECK (json_valid(document)),
    PRIMARY KEY (project, id)
);
INSERT INTO rollout_sessions (project, id, document)
SELECT project, json_extract(document, '$.value.session_id'), document
FROM rollout_runs
WHERE json_extract(document, '$.value.session_id') IS NOT NULL;
DROP TABLE rollout_runs;
ALTER TABLE rollout_sessions RENAME TO rollout_runs;
