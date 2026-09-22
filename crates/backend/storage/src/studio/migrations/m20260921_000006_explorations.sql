-- The Workflow view these documents belong to is named Explore, so the table
-- takes the document's noun. Saved work lives in these rows, so the table is
-- renamed in place rather than recreated. SQLite carries an index across a
-- table rename under its old name, so the recency index is renamed with it.
ALTER TABLE build_documents RENAME TO explorations;
DROP INDEX IF EXISTS build_documents_recency;
CREATE INDEX explorations_recency ON explorations(project, updated_ms DESC, id);
