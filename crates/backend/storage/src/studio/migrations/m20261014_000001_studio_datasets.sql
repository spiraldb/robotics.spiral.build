-- Datasets became Studio's own tables under `datasets.<id>` (October 14,
-- 2026): a dataset is named by its id, `default` for what was
-- `dataset_default`, and has a format instead of a Spiral producer.
--
-- Published views were compiled against Spiral's dataset functions, and what
-- Studio kept about a dataset (its robot, its joint roles) was filed under the
-- retired Dataset resource's identity, so neither can be read any more: drop
-- them, so the browser resolves robots and joint roles again for the new
-- tables.
DELETE FROM curated_views;
DELETE FROM dataset_robots;
DELETE FROM dataset_joint_roles;

-- Every saved curation read one of the retired Datasets by its old name, so
-- Studio opens none saved before encoding 7 (which names a dataset as
-- `{"id", "format"}`): drop them, as each earlier encoding did.
DELETE FROM curations WHERE encoding < 7;

-- A training run is kept: its source names its dataset by id from now on.
UPDATE training_runs
SET document = json_set(
        document,
        '$.config.source.dataset',
        json_object(
            'id',
            CASE
                WHEN json_extract(document, '$.config.source.dataset.path[0]') LIKE 'dataset\_%' ESCAPE '\'
                    THEN substr(json_extract(document, '$.config.source.dataset.path[0]'), 9)
                ELSE json_extract(document, '$.config.source.dataset.path[0]')
            END
        )
    )
WHERE json_type(document, '$.config.source.dataset.path') = 'array';
