-- Episodes are numbered within their dataset (October 15, 2026) instead of
-- carrying a UUID, and every dataset's tables were written again with the
-- numbers.
--
-- A curation saved before encoding 8 names episodes by UUID, so Studio opens
-- none of them: drop them, as each earlier encoding did.
DELETE FROM curations WHERE encoding < 8;

-- What Studio kept about a dataset (its robot, its joint roles) was filed
-- under the version of its `episodes` table, which writing the table again
-- replaced: drop it, so the browser resolves robots and joint roles again.
DELETE FROM dataset_robots;
DELETE FROM dataset_joint_roles;
