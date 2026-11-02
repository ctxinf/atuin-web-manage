#!/usr/bin/env python3
"""Compare a running web service with Atuin 18.16.1's default fuzzy DB engine.

Uses read-only searches against the configured local history. Prints counts,
never history contents. Pass the web base URL as the optional first argument.
"""
import json
import subprocess
import sys
from urllib.parse import urlencode
from urllib.request import urlopen

base = sys.argv[1] if len(sys.argv) > 1 else 'http://127.0.0.1:28057'
version = subprocess.check_output(['atuin', '--version'], text=True)
assert version.startswith('atuin 18.16.1 '), 'This check is pinned to Atuin 18.16.1'
queries = ['', 'ssh u1 pc', 'A B C', 'git', '^git', "'git", 'git !push',
           '^git log$ | status$', 'r/^git (log|status)/', '%', "'it's", 'git  log']
for query in queries:
    cli = subprocess.run([
        'atuin', 'search', '--search-mode', 'fuzzy', '--filter-mode', 'global',
        '--limit', '200', '--author', '$all-user', '--format', '{uuid}', '--print0', '--', query,
    ], stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    assert cli.returncode in (0, 1), f'CLI failed for {query!r}'
    expected = cli.stdout.decode().split('\0')
    if expected[-1] == '':
        expected.pop()
    # Non-interactive CLI prints in reverse display order; compare engine order.
    expected.reverse()
    url = base + '/api/history?' + urlencode({'q': query, 'sort': 'relevance', 'limit': 500})
    with urlopen(url, timeout=15) as response:
        data = json.load(response)
    actual = [row['id'] for row in data['rows']]
    assert actual == expected, (
        f'CLI mismatch for {query!r}: web {len(actual)} rows, CLI {len(expected)} rows; '
        f'same membership: {set(actual) == set(expected)}'
    )
    assert data['total'] == len(expected)
    # Every page must slice the same ranked set, including the last page.
    for offset in (0, 25, 175):
        page_url = base + '/api/history?' + urlencode({
            'q': query, 'sort': 'relevance', 'limit': 25, 'offset': offset,
        })
        with urlopen(page_url, timeout=15) as response:
            page = json.load(response)
        assert [row['id'] for row in page['rows']] == expected[offset:offset + 25]
    print(f'{query!r}: {len(actual)} rows, exact CLI order and pagination match')
print('All CLI parity checks passed.')
