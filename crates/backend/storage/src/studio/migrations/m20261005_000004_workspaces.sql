-- A workspace: one Spiral project under the name Studio shows for it. Its
-- records stay keyed by the project, as `["<endpoint>","<project_id>"]`.
CREATE TABLE workspaces (
    endpoint TEXT NOT NULL,
    project_id TEXT NOT NULL CHECK (project_id <> ''),
    name TEXT NOT NULL CHECK (trim(name) <> ''),
    created_ms BIGINT NOT NULL,
    PRIMARY KEY (endpoint, project_id)
);
-- Every project Studio already keeps records for becomes a workspace, named
-- by its project ID until someone renames it.
INSERT INTO workspaces (endpoint, project_id, name, created_ms)
SELECT
    json_extract(project, '$[0]'),
    json_extract(project, '$[1]'),
    json_extract(project, '$[1]'),
    CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
FROM (
    SELECT project FROM studio_settings
    UNION SELECT project FROM curations
    UNION SELECT project FROM curated_views
    UNION SELECT project FROM training_runs
    UNION SELECT project FROM dataset_robots
    UNION SELECT project FROM dataset_joint_roles
    UNION SELECT project FROM dataset_check_reports
)
WHERE json_valid(project) AND json_extract(project, '$[1]') <> '';
