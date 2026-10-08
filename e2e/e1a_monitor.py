"""Sample only explicitly owned E1a processes; never open official data."""
import argparse
import json
from pathlib import Path
import subprocess
import time

from environment import identity
from harness import load_state

p = argparse.ArgumentParser()
p.add_argument('--evidence', required=True)
a = p.parse_args()
root = Path(a.evidence)
stop = root / 'monitor.stop'
with (root / 'isolation.jsonl').open('a') as out:
    while not stop.exists():
        owned = {}
        try:
            owned.update(load_state('e1a')['processes'])
        except (FileNotFoundError, ValueError):
            pass
        extra = root / 'owned-extra.json'
        if extra.exists():
            owned.update(json.loads(extra.read_text()))
        desktop = root / 'desktop-owned.json'
        if desktop.exists():
            owned.update(json.loads(desktop.read_text()))
        rows = []
        for role, proc in owned.items():
            if identity(proc['pid']) != proc['identity']:
                continue
            result = subprocess.run(['lsof', '-nP', '-p', str(proc['pid'])], capture_output=True, text=True)
            forbidden = ['/Users/you/Library/Application Support/app.codeg/', '/Users/you/.codeg/']
            hits = [line for line in result.stdout.splitlines() if any(x in line for x in forbidden)]
            rows.append({'role': role, 'pid': proc['pid'], 'official_path_hits': hits})
            if role == 'desktop':
                (root / 'desktop-lsof-latest.txt').write_text(result.stdout)
        official = subprocess.run(['ps', '-p', '87598', '-o', 'command='], capture_output=True, text=True).stdout.strip()
        owners = subprocess.run(['lsof', '-nP', '-iTCP:3080', '-sTCP:LISTEN', '-t'], capture_output=True, text=True).stdout.split()
        out.write(json.dumps({'time': time.time(), 'official_path_ok': official == '/Applications/codeg.app/Contents/MacOS/codeg', 'port3080_pids': owners, 'owned': rows}) + '\n')
        out.flush()
        time.sleep(2)
