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
