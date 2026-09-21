#!/usr/bin/python3
# Deterministic test-only pi protocol peer. Never used by the production defaults.
import json, os, sys
if '--version' in sys.argv:
    print('0.86.1')
    sys.exit(0)
path = sys.argv[sys.argv.index('--session') + 1]
provider = sys.argv[sys.argv.index('--provider') + 1]
model = sys.argv[sys.argv.index('--model') + 1]
entries = []
if os.path.exists(path):
    with open(path) as stream:
        entries = [json.loads(line) for line in stream]
sid = entries[0]['id'] if entries else 'fixture-session'
def emit(record):
    print(json.dumps(record), flush=True)
waiting = False
for line in sys.stdin:
    request = json.loads(line)
    kind = request['type']
    data = {}
    if kind == 'get_state':
        data = {'sessionFile': path, 'sessionId': sid, 'model': {'id': model, 'provider': provider}, 'isStreaming': False}
    elif kind == 'get_available_models':
        data = {'models': [{'id': model, 'provider': provider}]}
    elif kind == 'prompt':
        if not entries:
            entries.append({'type': 'session', 'version': 3, 'id': sid, 'cwd': os.getcwd()})
        for role in ['user', 'assistant']:
            count = len(entries)
            entries.append({'type': 'message', 'id': str(count), 'parentId': str(count-1) if count>1 else None, 'message': {'role':role, 'content':[{'type':'text','text': request['message'] if role=='user' else 'fixture result '+str(count)}]}})
        with open(path, 'w') as stream:
            for entry in entries:
                stream.write(json.dumps(entry)+'\n')
    if kind == 'prompt' and request['message'] == 'model error':
        entries[-1]['message']['stopReason'] = 'error'
        with open(path, 'w') as stream:
            for entry in entries:
                stream.write(json.dumps(entry)+'\n')
    emit({'type':'response' , 'id':request['id'], 'command':kind, 'success':True, 'data':data})
    if kind == 'prompt':
        emit({'type':'agent_end'})
        waiting = request['message'] == 'wait'
        if not waiting:
            emit({'type':'agent_settled'})
    if kind == 'abort' and waiting:
        waiting = False
        emit({'type':'agent_settled'})
