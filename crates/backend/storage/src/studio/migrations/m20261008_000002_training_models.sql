-- Training models (October 8, 2026): a run's config names the model it
-- trains, ACT policy or progress model, under `model`, which holds what only
-- that family is set by. Every run recorded before trained ACT; move its
-- `policy`, `observation`, `act`, and `optimization.chunk_size` into
-- `model`, so it still lists.
UPDATE training_runs
SET document = json_set(
    json_remove(
        document,
        '$.config.policy',
        '$.config.observation',
        '$.config.act',
        '$.config.optimization.chunk_size'
    ),
    '$.config.model',
    json_object(
        'kind', 'act',
        'observation', json(json_extract(document, '$.config.observation')),
        'chunk_size', json_extract(document, '$.config.optimization.chunk_size'),
        'architecture', json(json_extract(document, '$.config.act'))
    )
)
WHERE json_extract(document, '$.config.policy') = 'act';
