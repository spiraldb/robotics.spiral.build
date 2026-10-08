-- Published progress models (October 8, 2026): a progress run's checkpoint
-- published under a name, which Score progress lists and scores with. `model`
-- is the selector the scorer reads the weights by; the weights stay on the
-- training service. Two names that differ only in case are one name.
CREATE TABLE published_models (
    project TEXT NOT NULL,
    name TEXT NOT NULL COLLATE NOCASE,
    model TEXT NOT NULL,
    camera TEXT NOT NULL,
    run_id TEXT NOT NULL,
    step INTEGER NOT NULL,
    created_ms INTEGER NOT NULL,
    PRIMARY KEY (project, name)
);
