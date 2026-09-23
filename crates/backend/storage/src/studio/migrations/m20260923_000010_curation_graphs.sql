-- Curation absorbed Explore: a curation is now the graph Explore authored.
-- The old recipe rows have no graph form, so they are dropped; the graphs keep
-- their rows and take the activity's name. SQLite carries an index across a
-- table rename under its old name, so the recency index is renamed with it.
DROP INDEX IF EXISTS curations_recency;
DROP TABLE curations;
ALTER TABLE explorations RENAME TO curations;
DROP INDEX IF EXISTS explorations_recency;
CREATE INDEX curations_recency ON curations(project, updated_ms DESC, id);
