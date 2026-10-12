-- Encoding 6 (October 11, 2026) records each published view by the digest of
-- the statement it was created with, so a view goes out of date only when its
-- node compiles to another statement; Studio opens no curation saved before
-- it. Drop those curations, and first every view their stored payload lists
-- as published, so no view outlives the recipe it read.
DELETE FROM curated_views
WHERE EXISTS (
    SELECT 1
    FROM curations c, json_each(CASE WHEN json_valid(c.payload) THEN c.payload ELSE '{}' END, '$.published') p
    WHERE c.encoding < 6
        AND c.project = curated_views.project
        AND curated_views.name = json_extract(p.value, '$.view')
);
DELETE FROM curations WHERE encoding < 6;
