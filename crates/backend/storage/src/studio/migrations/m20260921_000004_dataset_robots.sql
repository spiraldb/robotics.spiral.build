CREATE TABLE dataset_robots (
    project TEXT NOT NULL,
    dataset TEXT NOT NULL,
    version TEXT NOT NULL,
    document JSON NOT NULL,
    PRIMARY KEY (project, dataset)
);
