#!/usr/bin/env python3
"""Send one UI command, saving sanitized accessibility/screenshot evidence."""
import argparse
import base64
import json
import os
from pathlib import Path
import urllib.request

def ui(command, name='latest', *, config=None, evidence=None):
    config = config or os.environ.get('CODEG_E2E_UI_CONFIG')
    evidence = evidence or os.environ.get('CODEG_E2E_UI_EVIDENCE')
    if not config or not evidence:
        raise ValueError('Provide config/evidence paths or CODEG_E2E_UI_CONFIG/CODEG_E2E_UI_EVIDENCE')
    if Path(name).name != name or name in ('', '.', '..'):
        raise ValueError('Evidence name must be a filename, not a path')
    evidence = Path(evidence)
    evidence.mkdir(parents=True, exist_ok=True)
    url = json.loads(Path(config).read_text())['url']
    request = urllib.request.Request(url + '/command', json.dumps(command).encode(), {'Content-Type':'application/json'})
    with urllib.request.urlopen(request, timeout=65) as response:
        result = json.load(response)
    png = result.pop('png', None)
    if png:
        (evidence / (name + '.png')).write_bytes(base64.b64decode(png))
    (evidence / (name + '.json')).write_text(json.dumps(result, indent=2))
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('command', help='JSON command; never include credentials')
    parser.add_argument('--name', default='latest')
    parser.add_argument('--config', default=os.environ.get('CODEG_E2E_UI_CONFIG'))
    parser.add_argument('--evidence', default=os.environ.get('CODEG_E2E_UI_EVIDENCE'))
    args = parser.parse_args()
    if not args.config or not args.evidence:
        parser.error('--config and --evidence (or their CODEG_E2E_UI_* env vars) are required')
    result = ui(json.loads(args.command), args.name, config=args.config, evidence=args.evidence)
    print(result.get('error', result.get('tree', '')))
