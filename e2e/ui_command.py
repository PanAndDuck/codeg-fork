#!/usr/bin/env python3
"""Send one UI command, saving sanitized accessibility/screenshot evidence."""
import argparse
import base64
import json
from pathlib import Path
import urllib.request

CONFIG = Path('/Users/you/Projects/codeg-ios-wt/E1b-uitests/CodegiOSUITests/E1bController.json')
EVIDENCE = Path('/Users/you/Projects/codeg-fork-plan/reports/E1b')


def ui(command, name='latest'):
    url = json.loads(CONFIG.read_text())['url']
    request = urllib.request.Request(url + '/command', json.dumps(command).encode(), {'Content-Type':'application/json'})
    with urllib.request.urlopen(request, timeout=65) as response:
        result = json.load(response)
    png = result.pop('png', None)
    if png:
        (EVIDENCE / (name + '.png')).write_bytes(base64.b64decode(png))
    (EVIDENCE / (name + '.json')).write_text(json.dumps(result, indent=2))
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('command', help='JSON command; never include credentials')
    parser.add_argument('--name', default='latest')
    args = parser.parse_args()
    result = ui(json.loads(args.command), args.name)
    print(result.get('error', result.get('tree', '')))
