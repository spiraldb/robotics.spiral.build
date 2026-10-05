-- One browser's Spiral sign-in. The browser holds `id` in a cookie; the tokens
-- never leave the backend. A session remembers the project its browser works on.
CREATE TABLE sessions (
    id TEXT PRIMARY KEY NOT NULL,
    access_token TEXT NOT NULL,
    refresh_token TEXT NOT NULL,
    project_id TEXT NOT NULL
);
