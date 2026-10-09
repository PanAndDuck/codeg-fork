#!/usr/bin/env python3
"""E1b-only lifecycle faults with the E0 PID/path/start-time ownership checks."""
import argparse
from pathlib import Path
import shutil
import subprocess
import time

from environment import (REPO, ROOT, SERVER, RELAY, clean_env, official_check,
                         relay_start, save, spawn, stop, wait_for)
from harness import api, load_state


def start_server(state, port=None):
    env = clean_env()
    env.update(CODEG_FLAVOR='fork', CODEG_HOST='127.0.0.1',
               CODEG_PORT=str(state['server_port'] if port is None else port), CODEG_BRIDGE_PORTS='off',
               CODEG_HOME=state['home_dir'], CODEG_DATA_DIR=state['data_dir'],
               CODEG_ACP_TMP_ROOT=str(Path(state['data_dir']).parent/'tmp'),
               CODEG_STATIC_DIR=str(REPO/'out'), CODEG_TOKEN=state['token'],
               CODEG_RELAY_URL=state['relay_url'],
               CODEG_RELAY_KEY=Path(state['agent_key_file']).read_text().strip(),
               CODEG_RELAY_FINGERPRINT=state['fingerprint'],
               CODEG_RELAY_ENABLED='true')
    spawn(state, 'server', [str(SERVER)], env, state['server_log'])
    wait_for(lambda: api(state['relay_url'],state['token'],'get_relay_status',
                        fingerprint=state['fingerprint'],timeout=2)['state']=='connected',45,'home reconnect')
    save(state)


def stop_server(state):
    stop(state['processes']['server'])
    stop(state['processes']['wrapper'])


if __name__ == '__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('action',choices=['stop-home','start-home','move-home','restore-home','rekey-relay'])
    args=parser.parse_args()
    official_check()
    state=load_state('e1b')
    if args.action=='stop-home':
        stop_server(state)
    elif args.action=='start-home':
        start_server(state)
    elif args.action in ('move-home','restore-home'):
        stop_server(state)
        start_server(state, 0 if args.action=='move-home' else 3094)
    elif args.action=='rekey-relay':
        stop_server(state)
        stop(state['processes']['relay'])
        tls=Path(state['relay_dir'])/'tls'
        assert tls.resolve().is_relative_to(Path(state['temp_root']).resolve())
        shutil.rmtree(tls)
        relay_start(state)
        wait_for(lambda:(tls/'cert.pem').exists(),10,'new relay certificate')
        state['fingerprint']=subprocess.check_output([str(RELAY),'fingerprint','--data-dir',state['relay_dir']],env=clean_env(),text=True).splitlines()[0]
        save(state)
        start_server(state)
    official_check()
    print('E1b owned lifecycle action completed:', args.action)
