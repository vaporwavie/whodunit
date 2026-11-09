#!/usr/bin/env python3
"""Deterministic RPC peer. No models, API keys or user data."""
import json
import os
from pathlib import Path
import sys
import threading
import time
import uuid

output_lock = threading.Lock()
root = Path.home() / '.pi/agent/sessions/--fake--'
root.mkdir(parents=True, exist_ok=True)
path = Path(sys.argv[sys.argv.index('--session') + 1]) if '--session' in sys.argv else root / f'{uuid.uuid4()}.jsonl'
entries = []
busy = False
aborted = threading.Event()


def emit(record):
    with output_lock:
        print(json.dumps(record), flush=True)


def response(command, data=None):
    emit({'type': 'response', 'id': command['id'], 'command': command['type'], 'success': True, 'data': data or {}})


def append(entry):
    entries.append(entry)
    with path.open('a') as file:
        file.write(json.dumps(entry, separators=(',', ':')) + '\n')


def finish(command):
    global busy
    while not path.with_suffix('.release').exists() and not aborted.is_set():
        time.sleep(0.01)
    if not aborted.is_set():
        message = {'role': 'assistant', 'content': [{'type': 'text', 'text': command['message']}], 'model': 'secret-model', 'provider': 'secret-lab'}
        append({'type': 'message', 'message': message})
        emit({'type': 'message_end', 'message': message})
    busy = False
    emit({'type': 'agent_settled'})
    response(command)


if path.exists():
    entries = [json.loads(line) for line in path.read_text().splitlines()]
else:
    append({'type': 'session', 'cwd': os.getcwd()})

for line in sys.stdin:
    command = json.loads(line)
    kind = command['type']
    if kind == 'get_state':
        response(command, {'sessionFile': str(path), 'isStreaming': busy})
    elif kind == 'get_entries':
        response(command, {'entries': entries})
    elif kind == 'get_commands':
        response(command, {'commands': []})
    elif kind == 'get_session_stats':
        response(command, {'cost': float(os.getpid()), 'contextUsage': {'percent': 12}})
    elif kind == 'prompt':
        if command['message'] == '/done':
            emit({'type': 'extension_ui_request', 'id': 'score', 'method': 'select', 'title': 'End this run', 'options': ['5 · nailed it, great']})
            response(command)
        else:
            busy = True
            aborted.clear()
            message = {'role': 'user', 'content': command['message']}
            append({'type': 'message', 'message': message})
            emit({'type': 'message_start', 'message': message})
            entry = {'type': 'custom', 'customType': 'whodunit.deal', 'data': {'role': 'builder', 'size': 'M', 'agent': 'secret-lab/secret-model'}}
            append(entry)
            emit({'type': 'entry_appended', 'entry': entry})
            emit({'type': 'agent_start'})
            threading.Thread(target=finish, args=(command,), daemon=True).start()
    elif kind == 'extension_ui_response':
        entry = {'type': 'custom', 'customType': 'whodunit.done', 'data': {'score': 5}}
        append(entry)
        emit({'type': 'entry_appended', 'entry': entry})
    elif kind == 'abort':
        aborted.set()
        response(command)
    elif kind == 'clear_queue':
        response(command, {'steering': [], 'followUp': []})
    else:
        response(command)

path.with_suffix('.closed').touch()
