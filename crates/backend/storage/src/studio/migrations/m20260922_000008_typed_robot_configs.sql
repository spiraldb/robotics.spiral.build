-- Robot configurations become one typed document. Each motor carries its own
-- measured range, so calibration no longer repeats motor IDs and directions; a
-- measured range whose ID or direction disagreed with its motor was never
-- usable and is dropped. Fields no Studio build reads are removed, and saves
-- compare a revision number instead of the whole document.
ALTER TABLE robots ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;

UPDATE robots SET document = json_patch(
    json_object(
        'robot_id', document ->> '$.robot_id',
        'motors', json((
            SELECT json_group_object(motor.key, json_patch(
                json_object(
                    'id', motor.value ->> '$.id',
                    'direction', coalesce(motor.value ->> '$.direction', 'normal')
                ),
                json_object(
                    'model', motor.value -> '$.model',
                    'calibration', CASE WHEN measured.key IS NULL THEN NULL ELSE json_object(
                        'range_min', measured.value ->> '$.range_min',
                        'center', measured.value ->> '$.center',
                        'range_max', measured.value ->> '$.range_max'
                    ) END
                )
            ))
            FROM json_each(robots.document, '$.motors') AS motor
            LEFT JOIN json_each(robots.document, '$.calibration.motors') AS measured
                ON measured.key = motor.key
                AND measured.value ->> '$.id' = motor.value ->> '$.id'
                AND coalesce(measured.value ->> '$.direction', 'normal')
                    = coalesce(motor.value ->> '$.direction', 'normal')
        ))
    ),
    json_object(
        'robot_type', document -> '$.robot_type',
        'port', CASE WHEN trim(coalesce(document ->> '$.port.path', '')) = '' THEN NULL ELSE json_object(
            'path', document ->> '$.port.path',
            'baud_rate', coalesce(document ->> '$.port.baud_rate', 1000000)
        ) END,
        'cameras', json((
            SELECT json_group_object(camera.key, json_patch(
                json_object('id', camera.value ->> '$.id'),
                json_object('mount', camera.value -> '$.mount')
            ))
            FROM json_each(robots.document, '$.cameras') AS camera
        )),
        'visualization', document -> '$.visualization'
    )
);
