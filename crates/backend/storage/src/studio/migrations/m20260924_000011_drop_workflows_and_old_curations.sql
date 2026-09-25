-- Workflow is gone: Curation is the one way to choose training data. Saved
-- workflows have no curation form, so they are dropped with their table and
-- its index. Every curation saved before this migration uses the retired
-- encoding 1, which this Studio does not read, so those are deleted too.
DROP TABLE workflows;
DELETE FROM curations;
