"""Helpers for E1b's real SwiftUI / local relay verification; no product hooks."""
import json
from pathlib import Path
import re
import subprocess
import time
import uuid

from faultctl import control
from harness import api, endpoint, load_state
from ui_command import ui, evidence_dir


def call(command, params=None, through='server', raw=False):
    state = load_state('e1b')
    url, pin = endpoint(state, through)
    return api(url, state['token'], command, params, pin, raw)


def fault(command, **params):
    return control(load_state('e1b'), command, **params)


def capture(name):
    result = ui({'action':'state', 'screenshot':True}, name)
    status = fault('status')
    (evidence_dir() / (name + '-proxy.json')).write_text(json.dumps(status, indent=2))
    return result.get('tree','')


def send(text, name):
    result = ui({'action':'type','type':'textView','text':text,'clear':True}, name+'-draft')
    if 'error' in result:
        raise RuntimeError(result['error'])
    start = time.time()
    result = ui({'action':'tap','type':'button','label':'Send','screenshot':True}, name+'-sent')
    # Keyboard layout can move the button while XCTest synthesizes the first
    # tap. Only retry a tap while the draft remains and no send pipeline began.
    if "label: 'Stop'" not in result.get('tree','') and 'TextView' in result.get('tree',''):
        tree = ui({'action':'state'}, name+'-tap-check').get('tree','')
        if "label: 'Stop'" not in tree and re.search(r'TextView,.*value:', tree):
            result = ui({'action':'tap','type':'button','label':'Send','screenshot':True}, name+'-sent-confirmed')
    (evidence_dir() / (name+'-send-time.json')).write_text(json.dumps({'started_unix':start,'returned_unix':time.time()}))
    if 'error' in result:
        raise RuntimeError(result['error'])
    return result.get('tree','')


def wait_ui(name, predicate, timeout=60):
    start = time.monotonic()
    while time.monotonic()-start < timeout:
        tree = ui({'action':'state'}, name+'-poll').get('tree','')
        if predicate(tree):
            capture(name)
            return {'condition_met':True,'elapsed_seconds':round(time.monotonic()-start,3)}
        time.sleep(1)
    capture(name)
    return {'condition_met':False,'elapsed_seconds':round(time.monotonic()-start,3)}


def finished(tree, marker=None):
    # A marker still inside the draft is not a completed assistant response.
    return "label: 'Stop'" not in tree and not re.search(r'TextView,.*value:', tree) and (marker is None or marker in tree)


def create(title):
    directory = call('create_chat_dir')['path']
    result = call('create_chat_conversation',{'agentType':'custom:e2e-mock','title':title,'existingDir':directory})
    result['workingDir'] = directory
    return result


def connect(session):
    session['connectionId'] = call('acp_connect',{'agentType':'custom:e2e-mock','workingDir':session['workingDir'],'sessionId':None})
    return session


def prompt(session, text):
    return call('acp_prompt',{'connectionId':session['connectionId'],'conversationId':session['conversationId'],
        'folderId':session['folderId'],'clientMessageId':'e1b-'+uuid.uuid4().hex,'blocks':[{'type':'text','text':text}]})


def record(name, result):
    (evidence_dir() / (name+'-result.json')).write_text(json.dumps(result, indent=2))
    print(name, json.dumps(result), flush=True)
