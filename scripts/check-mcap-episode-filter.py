"""Verify returned MCAP IDs survive ADBC parameter binding without writes."""

import argparse
import json
from pathlib import Path

import adbc_driver_manager
import pyarrow as pa
from adbc_driver_manager import dbapi


class EpisodeFilterCheck:
    def __init__(self, driver: str):
        self.driver = driver

    def run(self):
        state = json.loads(
            (
                Path.home()
                / "Library/Application Support/spiral-robotics/backend-state.json"
            ).read_text()
        )
        database = adbc_driver_manager.AdbcDatabase(
            driver=self.driver,
            uri=f"spiraldb://?project={state['project_id']}",
            **{
                "spiral.api_url": state["spiral_endpoint"],
                "spiral.spfs_url": state["spiral_spfs_endpoint"],
                "username": state["client_id"],
                "password": state["client_secret"],
            },
        )
        native = adbc_driver_manager.AdbcConnection(database)
        with dbapi.Connection(database, native, autocommit=False) as connection:
            try:
                with connection.cursor() as cursor:
                    cursor.execute(
                        "SELECT id, message_start_time, source.uri AS source_uri FROM spql.mcap.episodes('dataset_default') "
                        "ORDER BY message_start_time, source_uri LIMIT 2"
                    )
                    episodes = cursor.fetch_arrow_table()
                assert episodes.num_rows == 2, "Need two episodes to verify isolation"
                ids = episodes.column("id")
                assert ids[0].as_py() != ids[1].as_py()
                for index in range(2):
                    parameter = pa.RecordBatch.from_arrays(
                        [ids.slice(index, 1).combine_chunks()],
                        schema=pa.schema([episodes.schema.field("id")]),
                    )
                    with adbc_driver_manager.AdbcStatement(native) as statement:
                        statement.set_sql_query(
                            "SELECT id, value.episode_id AS original_episode "
                            "FROM spql.mcap.read_topic('dataset_default', '/studio/joints/leader') "
                            "WHERE id = $1"
                        )
                        statement.prepare()
                        array = adbc_driver_manager.ArrowArrayHandle()
                        schema = adbc_driver_manager.ArrowSchemaHandle()
                        parameter._export_to_c(array.address, schema.address)
                        statement.bind(array, schema)
                        stream, _ = statement.execute_query()
                        with pa.RecordBatchReader._import_from_c(
                            stream.address
                        ) as reader:
                            messages = reader.read_all()
                    expected = ids[index].as_py()
                    assert messages.num_rows > 0, "Expected joint messages"
                    assert all(
                        value == expected for value in messages.column("id").to_pylist()
                    )
                    print(
                        f"Episode {index + 1}: {messages.num_rows} messages, exact ID membership passed",
                        flush=True,
                    )
            finally:
                connection.rollback()
        print("Read transaction rolled back", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--driver",
        default=str(
            Path(__file__).resolve().parents[1]
            / "target/release/libadbc_driver_spiraldb.dylib"
        ),
    )
    EpisodeFilterCheck(parser.parse_args().driver).run()
