"""Migrate the audited legacy Studio catalog using native resource identities.

Run prepare, publish, then verify with the same --audit directory. Preparation
adds the shared source table and root views. Publication atomically moves names,
advances Dataset definitions, and removes the copied legacy source bindings.
Never replay publication after an uncertain network response: inspect the saved
request and journal first. The audit contains exact immutable versions and row
fingerprints, but no credentials.
"""

import argparse
import copy
import hashlib
import json
import uuid
from pathlib import Path

import adbc_driver_manager as adbc

from flat_catalog import FlatCatalogAudit


class RootSourceMigration(FlatCatalogAudit):
    def save(self, name, value):
        path = self.arguments.audit / (name + '.json')
        with path.open('x') as output:
            json.dump(value, output, indent=2)
            output.write('\n')

    def read(self, name):
        return json.loads((self.arguments.audit / (name + '.json')).read_text())

    @staticmethod
    def execute(connection, sql):
        with adbc.AdbcStatement(connection.adbc_connection) as statement:
            statement.set_sql_query(sql)
            statement.execute_update()

    @staticmethod
    def literal(value):
        return "'" + value.replace("'", "''") + "'"

    @classmethod
    def fingerprint(cls, rows):
        def encode(value):
            if isinstance(value, bytes):
                return {'bytes': len(value), 'sha256': hashlib.sha256(value).hexdigest()}
            return str(value)

        encoded = sorted(json.dumps(row, sort_keys=True, default=encode) for row in rows)
        return {'rows': len(rows), 'sha256': hashlib.sha256('\n'.join(encoded).encode()).hexdigest()}

    def capture(self, connection, plan):
        tables = {}
        for move in plan['moves']:
            old = '.'.join(move['from'])
            if move['resource']['descriptor']['kind'] == 'table':
                tables[old] = self.fingerprint(self.query(connection, 'SELECT * FROM ' + self.quoted(move['from'])))
        datasets = {}
        for name in plan['datasets']:
            datasets[name] = self.fingerprint(self.query(connection, f'SELECT * FROM spql.mcap.episodes({self.literal(name)})'))
        return {'tables': tables, 'episodes': datasets}

    def prepare(self):
        self.arguments.audit.mkdir(parents=True, exist_ok=False)
        catalog = self.snapshot()
        entries = self.entries(catalog)
        plan = self.plan(entries)
        sources = {}
        for name, version in plan['datasets'].items():
            payload = json.loads(version['definition']['canonicalPayload'])
            if version['definition']['producerId'] != 'spql.mcap.dataset':
                raise ValueError('This migration supports only MCAP datasets')
            source = tuple(payload['sourceRelation'])
            if source not in entries or entries[source]['resource']['descriptor']['kind'] != 'table':
                raise ValueError('Expected a concrete legacy source table')
            if (plan['source_views'][name],) in entries:
                raise ValueError('Root source view already exists')
            sources[name] = list(source)
        if not sources:
            raise ValueError('No legacy datasets to migrate')
        handled = {tuple(move['from']) for move in plan['moves']} | {tuple(source) for source in sources.values()}
        handled |= {('spiral_robotics',), ('spiral_dataset',)}
        if any(path[0] in ('spiral_robotics', 'spiral_dataset') and path not in handled for path in entries):
            raise ValueError('Unplanned resources remain inside legacy containers')
        self.save('before', {'project': self.settings['project_id'], 'catalog': catalog, 'plan': plan, 'sources': sources})
        with self.connect() as connection:
            evidence = self.capture(connection, plan)
            source_rows = {name: self.fingerprint(self.query(connection, 'SELECT recording, source FROM ' + self.quoted(path))) for name, path in sources.items()}
            evidence['sources'] = source_rows
            self.save('data-before', evidence)
            if self.snapshot() != catalog:
                raise ValueError('Catalog changed before preparation')
            selects = [f'SELECT {self.literal(name.split(".")[-1])} AS dataset_name, recording, source FROM {self.quoted(path)}' for name, path in sources.items()]
            self.execute(connection, 'CREATE TABLE dataset_source AS ' + ' UNION ALL '.join(selects))
            for name, view in plan['source_views'].items():
                self.execute(connection, f'CREATE VIEW {self.quoted([view])} AS SELECT recording, source FROM dataset_source WHERE dataset_name = {self.literal(name.split(".")[-1])}')
                copied = self.fingerprint(self.query(connection, 'SELECT recording, source FROM ' + self.quoted([view])))
                if copied != source_rows[name]:
                    raise ValueError('Source copy differs from original')
            self.save('source-commit-started', {'base': catalog['branch']['headCommitId']})
            connection.commit()
        self.save('sources-prepared', self.snapshot())
        print('Prepared shared source table and root views; all source contents match.', flush=True)

    def dataset_successors(self, plan):
        prepared = {}
        for name, old in plan['datasets'].items():
            definition = copy.deepcopy(old['definition'])
            payload = json.loads(definition['canonicalPayload'])
            payload['sourceRelation'] = [plan['source_views'][name]]
            definition['canonicalPayload'] = json.dumps(payload, sort_keys=True, separators=(',', ':'))
            prefix = f"/datasets/{old['datasetId']}"
            successor = self.request(prefix + '/versions', {'baseVersionId': old['versionId'], 'definition': definition})['version']
            self.save('definition-' + name, successor)
            revision = self.request(prefix + '/revisions', {
                'baseVersionId': successor['versionId'],
                'data': {'mode': 'new', 'data': old['dataRevision']['data']},
                'layout': old['layoutRevision']['layout'],
            })['version']
            if revision['dataRevision']['data'] != old['dataRevision']['data'] or revision['layoutRevision']['layout'] != old['layoutRevision']['layout']:
                raise ValueError('Dataset successor changed member data or layout')
            prepared[name] = revision
        self.save('dataset-successors', prepared)
        return prepared

    def publish(self):
        if (self.arguments.audit / 'publish-request.json').exists():
            raise ValueError('Publication was already attempted; inspect journal instead of replaying')
        before = self.read('before')
        prepared_catalog = self.read('sources-prepared')
        if self.snapshot() != prepared_catalog:
            raise ValueError('Project changed after source preparation')
        old_entries = self.entries(before['catalog'])
        current_entries = self.entries(prepared_catalog)
        if any(current_entries.get(path) != entry for path, entry in old_entries.items()):
            raise ValueError('Legacy resources changed during preparation')
        successors = self.dataset_successors(before['plan'])
        request = {
            'branchId': prepared_catalog['branch']['branchId'],
            'baseCommitId': prepared_catalog['branch']['headCommitId'],
            'expectedHeadCommitId': prepared_catalog['branch']['headCommitId'],
            'transactionId': str(uuid.uuid7()), 'idempotencyKey': str(uuid.uuid4()),
            'message': 'Flatten Studio tables and datasets; derive datasets from root views of dataset_source',
            'resources': [], 'namespace': [],
        }
        for move in before['plan']['moves']:
            entry = old_entries[tuple(move['from'])]
            resource = entry['resource']
            resource_id = resource['descriptor']['resourceId']
            successor = successors.get('.'.join(move['from']))
            if successor:
                request['resources'].append({'resourceId': resource_id, 'expected': resource['versionId'], 'replacement': successor['versionId']})
            request['namespace'].extend([
                {'parent': entry['parent'], 'name': entry['name'], 'expected': resource_id},
                {'parent': {'kind': 'projectRoot'}, 'name': move['to'][0], 'replacement': resource_id},
            ])
        removed = {tuple(source) for source in before['sources'].values()} | {('spiral_robotics',), ('spiral_dataset',)}
        for path in sorted(removed):
            entry = old_entries[path]
            resource = entry['resource']
            resource_id = resource['descriptor']['resourceId']
            request['resources'].append({'resourceId': resource_id, 'expected': resource['versionId']})
            request['namespace'].append({'parent': entry['parent'], 'name': entry['name'], 'expected': resource_id})
        self.save('publish-request', request)
        result = self.request(f"/projects/{self.settings['project_id']}/journal/commits/publish", request)
        self.save('published', result)
        if result['kind'] != 'published':
            raise ValueError('Publication did not succeed; inspect response')
        print('Published flat catalog atomically, preserving Dataset and Studio table identities.', flush=True)

    def verify(self):
        before = self.read('before')
        evidence = self.read('data-before')
        catalog = self.snapshot()
        entries = self.entries(catalog)
        checks = {}
        with self.connect() as connection:
            for move in before['plan']['moves']:
                name = '.'.join(move['from'])
                resource = entries[tuple(move['to'])]['resource']
                if resource['descriptor']['resourceId'] != move['resource']['descriptor']['resourceId']:
                    raise ValueError('Resource identity changed: ' + name)
                if resource['descriptor']['kind'] == 'table':
                    if resource['versionId'] != move['resource']['versionId']:
                        raise ValueError('Studio table version changed: ' + name)
                    actual = self.fingerprint(self.query(connection, 'SELECT * FROM ' + self.quoted(move['to'])))
                    expected = evidence['tables'][name]
                else:
                    actual = self.fingerprint(self.query(connection, f'SELECT * FROM spql.mcap.episodes({self.literal(move["to"][0])})'))
                    expected = evidence['episodes'][name]
                    version = self.request(f"/datasets/{resource['descriptor']['resourceId']}/versions/{resource['versionId']}")['version']
                    if version != self.read('dataset-successors')[name]:
                        raise ValueError('Published dataset version differs from prepared successor')
                if actual != expected:
                    raise ValueError('Data changed: ' + name)
                checks[move['to'][0]] = actual
            for name, view in before['plan']['source_views'].items():
                actual = self.fingerprint(self.query(connection, 'SELECT recording, source FROM ' + self.quoted([view])))
                if actual != evidence['sources'][name]:
                    raise ValueError('Source view differs from original: ' + name)
                checks[view] = actual
            source_count = self.query(connection, 'SELECT COUNT(*) AS rows FROM dataset_source')[0]['rows']
            if source_count != sum(value['rows'] for value in evidence['sources'].values()):
                raise ValueError('Shared source table row count differs')
        if any(path[0] in ('spiral_robotics', 'spiral_dataset') for path in entries):
            raise ValueError('Legacy containers remain')
        self.save('verified', {'catalog': catalog, 'checks': checks, 'source_rows': source_count})
        print(json.dumps({'verified': checks, 'source_rows': source_count}, indent=2), flush=True)

    @classmethod
    def main(cls):
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument('phase', choices=['prepare', 'publish', 'verify'])
        parser.add_argument('--settings', type=Path, default=Path.home() / 'Library/Application Support/spiral-robotics/backend-state.json')
        parser.add_argument('--driver', type=Path, default=Path(__file__).resolve().parents[1] / 'target/release/libadbc_driver_spiraldb.dylib')
        parser.add_argument('--audit', type=Path, required=True)
        arguments = parser.parse_args()
        migration = cls(arguments)
        getattr(migration, arguments.phase)()


if __name__ == '__main__':
    RootSourceMigration.main()
