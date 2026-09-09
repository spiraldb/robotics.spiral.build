"""Audit the native catalog before migrating Studio to flat resource names.

Read-only: writes a local audit, never publishes catalog or table changes.
Root source views are used because child views require keyed parent metadata.
"""

import argparse
import json
from pathlib import Path
from urllib.request import Request, urlopen

import adbc_driver_manager as adbc
from adbc_driver_manager import dbapi


class FlatCatalogAudit:
    def __init__(self, arguments):
        self.arguments = arguments
        self.settings = json.loads(arguments.settings.read_text())
        self.endpoint = self.settings['spiral_endpoint'].rstrip('/')
        token = self.request('/v1/oauth2/token', {
            'grant_type': 'client_credentials',
            'client_id': self.settings['client_id'],
            'client_secret': self.settings['client_secret'],
        })
        self.authorization = 'Bearer ' + token['access_token']

    def request(self, path, body=None):
        headers = {'Content-Type': 'application/json'}
        if hasattr(self, 'authorization'):
            headers['Authorization'] = self.authorization
        request = Request(self.endpoint + path, headers=headers,
                          data=None if body is None else json.dumps(body).encode())
        with urlopen(request, timeout=30) as response:
            return json.load(response)

    def snapshot(self):
        prefix = f"/projects/{self.settings['project_id']}/journal"
        branch = self.request(prefix + '/branches/main')
        namespace = []
        page_token = None
        for _ in range(100):
            page = self.request(prefix + '/state/read', {
                'commitId': branch['headCommitId'], 'pageSize': 1000,
                'pageToken': page_token,
            })
            namespace.extend(page['namespace'])
            page_token = page.get('nextPageToken')
            if not page_token:
                return {'branch': branch, 'namespace': namespace}
        raise ValueError('Catalog exceeds audit page limit')

    @staticmethod
    def entries(snapshot):
        remaining = list(snapshot['namespace'])
        paths_by_id = {}
        entries = {}
        while remaining:
            resolved = []
            for entry in remaining:
                parent = entry['parent']
                if parent['kind'] == 'projectRoot':
                    prefix = ()
                elif parent['resourceId'] in paths_by_id:
                    prefix = paths_by_id[parent['resourceId']]
                else:
                    continue
                path = (*prefix, entry['name'])
                paths_by_id[entry['resource']['descriptor']['resourceId']] = path
                entries[path] = entry
                resolved.append(entry)
            if not resolved:
                raise ValueError('Catalog has unresolved parents')
            remaining = [entry for entry in remaining if entry not in resolved]
        return entries

    def connect(self):
        settings = self.settings
        database = adbc.AdbcDatabase(
            driver=str(self.arguments.driver),
            uri=f"spiraldb://?project={settings['project_id']}",
            **{'spiral.api_url': settings['spiral_endpoint'],
               'spiral.spfs_url': settings['spiral_spfs_endpoint'],
               'username': settings['client_id'], 'password': settings['client_secret']})
        return dbapi.Connection(database, adbc.AdbcConnection(database), autocommit=False)

    @staticmethod
    def quoted(path):
        return '.'.join('"' + part.replace('"', '""') + '"' for part in path)

    @staticmethod
    def query(connection, sql):
        with connection.cursor() as cursor:
            cursor.execute(sql)
            return cursor.fetch_arrow_table().to_pylist()

    def plan(self, entries):
        moves = []
        datasets = {}
        for path, entry in sorted(entries.items()):
            kind = entry['resource']['descriptor']['kind']
            if len(path) == 2 and path[0] == 'spiral_robotics' and kind == 'table':
                name = path[1] if path[1].startswith('studio_') else 'studio_' + path[1]
                moves.append({'from': list(path), 'to': [name], 'resource': entry['resource']})
            elif len(path) == 2 and path[0] == 'spiral_dataset' and kind == 'dataset':
                moves.append({'from': list(path), 'to': ['dataset_' + path[1]],
                              'resource': entry['resource']})
                resource = entry['resource']
                version = self.request(f"/datasets/{resource['descriptor']['resourceId']}/versions/{resource['versionId']}")['version']
                datasets['.'.join(path)] = version
        destinations = [tuple(move['to']) for move in moves]
        if len(set(destinations)) != len(destinations) or any(path in entries for path in destinations):
            raise ValueError('Flat resource names conflict with existing resources or one another')
        if ('dataset_source',) in entries:
            raise ValueError('dataset_source already exists; inspect before consolidating sources')
        return {'moves': moves, 'datasets': datasets,
                'source_table': 'dataset_source', 'source_discriminator': 'dataset_name',
                'source_views': {name: 'dataset_source_' + name.split('.')[-1] for name in datasets},
                'remove_containers_after_migration': ['spiral_robotics', 'spiral_dataset']}

    def run(self):
        destination = self.arguments.audit
        if destination.exists():
            raise ValueError('Audit destination exists; choose a new path to preserve prior evidence')
        before = self.snapshot()
        entries = self.entries(before)
        plan = self.plan(entries)
        counts = {}
        with self.connect() as connection:
            for path, entry in sorted(entries.items()):
                if entry['resource']['descriptor']['kind'] == 'table':
                    counts['.'.join(path)] = self.query(connection, f'SELECT COUNT(*) AS rows FROM {self.quoted(path)}')[0]['rows']
        after = self.snapshot()
        if before != after:
            raise ValueError('Catalog changed while auditing; rerun against a stable project')
        audit = {'project': self.settings['project_id'], 'catalog': before,
                 'plan': plan, 'table_counts': counts, 'read_only': True}
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(json.dumps(audit, indent=2) + '\n')
        print(json.dumps({'audit': str(destination), 'read_only': True,
                          'moves': [{'from': '.'.join(move['from']), 'to': '.'.join(move['to'])} for move in plan['moves']],
                          'table_counts': counts}, indent=2))

    @classmethod
    def main(cls):
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument('--settings', type=Path, default=Path.home() / 'Library/Application Support/spiral-robotics/backend-state.json')
        parser.add_argument('--driver', type=Path, default=Path(__file__).resolve().parents[1] / 'target/release/libadbc_driver_spiraldb.dylib')
        parser.add_argument('--audit', type=Path, required=True)
        cls(parser.parse_args()).run()


if __name__ == '__main__':
    FlatCatalogAudit.main()
