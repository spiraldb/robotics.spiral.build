-- A training run's metrics, logs, and status now live with its worker, and its
-- checkpoints in Spiral; Studio records only what it asked the worker to do.
-- Rollout keeps only its current session, in memory.
DROP TABLE training_runs;
CREATE TABLE training_runs (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    document TEXT NOT NULL CHECK (json_valid(document)),
    PRIMARY KEY (project, id)
);
DROP TABLE rollout_runs;
