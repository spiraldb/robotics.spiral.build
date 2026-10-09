-- A finished episode a workspace's robot connector uploaded (October 9, 2026),
-- kept until a signed-in browser stores it in its dataset. Its parts arrive
-- in order, each starting where the bytes before it end; the episode is whole
-- once `received` reaches `size`. A deleted workspace's episodes go with it.
CREATE TABLE staged_episodes (
    endpoint TEXT NOT NULL,
    project_id TEXT NOT NULL,
    dataset TEXT NOT NULL,
    episode TEXT NOT NULL,
    size BIGINT NOT NULL CHECK (size > 0),
    received BIGINT NOT NULL CHECK (received BETWEEN 0 AND size),
    PRIMARY KEY (endpoint, project_id, dataset, episode),
    FOREIGN KEY (endpoint, project_id)
        REFERENCES workspaces (endpoint, project_id) ON DELETE CASCADE
);
CREATE TABLE staged_episode_parts (
    endpoint TEXT NOT NULL,
    project_id TEXT NOT NULL,
    dataset TEXT NOT NULL,
    episode TEXT NOT NULL,
    start BIGINT NOT NULL CHECK (start >= 0),
    bytes BLOB NOT NULL CHECK (length(bytes) > 0),
    PRIMARY KEY (endpoint, project_id, dataset, episode, start),
    FOREIGN KEY (endpoint, project_id, dataset, episode)
        REFERENCES staged_episodes (endpoint, project_id, dataset, episode) ON DELETE CASCADE
);
