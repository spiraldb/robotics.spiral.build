-- Encoding 5 (October 8, 2026) keeps one row per episode, with model nodes
-- that only add columns and Selects that do all the keeping; Studio opens no
-- curation saved before it. Drop those curations, and first every view their
-- stored payload lists as published, so no view outlives the recipe it read.
DELETE FROM curated_views
WHERE EXISTS (
    SELECT 1
    FROM curations c, json_each(CASE WHEN json_valid(c.payload) THEN c.payload ELSE '{}' END, '$.published') p
    WHERE c.encoding < 5
        AND c.project = curated_views.project
        AND curated_views.name = json_extract(p.value, '$.view')
);
DELETE FROM curations WHERE encoding < 5;
