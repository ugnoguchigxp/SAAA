#!/usr/bin/python3
"""Credentials-free CLI protocol fixture; the runner/MCP/hooks/ledger remain real."""
import sys, json, subprocess, pathlib
if '--version' in sys.argv:
    print('Claude Code 2.1.289' if 'claude' in pathlib.Path(sys.argv[0]).name else 'codex-cli 0.155.1')
    sys.exit(0)
claude = '--settings' in sys.argv
prompt = sys.stdin.read()
if claude:
    directory = pathlib.Path(sys.argv[sys.argv.index('--settings') + 1]).parent
else:
    config = next(arg.split('=', 1)[1] for arg in sys.argv if arg.startswith('mcp_servers.saaa.args='))
    directory = pathlib.Path(json.loads(config)[-1])
spec = json.loads((directory / 'spec.json').read_text())
helper = spec['helper']
session = 'fixture-exact-session'
print(json.dumps({'type': 'system', 'subtype': 'init', 'session_id': session} if claude else {'type': 'thread.started', 'thread_id': session}), flush=True)
def call(name, arguments):
    requests = [dict(jsonrpc='2.0', id=1, method='initialize', params={'protocolVersion': '2024-11-05'}), dict(jsonrpc='2.0', id=2, method='tools/call', params={'name': name, 'arguments': arguments})]
    outcome = subprocess.run([helper, '--saaa-terminal-agent', 'mcp', str(directory)], input=''.join(json.dumps(x)+'\n' for x in requests), text=True, capture_output=True, check=True)
    return json.loads(outcome.stdout.splitlines()[-1])
if spec['resume'] is None:
    if claude:
        tool = {'questions': [{'question': 'Which color?', 'options': [{'label': 'Blue'}, {'label': 'Red'}], 'multiSelect': False}]}
        result = subprocess.run([helper, '--saaa-terminal-agent', 'hook', str(directory)], input=json.dumps({'hook_event_name': 'PreToolUse', 'tool_name': 'AskUserQuestion', 'tool_use_id': 'question-1', 'tool_input': tool}), text=True, capture_output=True, check=True)
        assert json.loads(result.stdout)['hookSpecificOutput']['permissionDecision'] == 'defer'
        print(json.dumps({'type': 'result', 'subtype': 'success', 'is_error': False, 'session_id': session, 'stop_reason': 'tool_deferred'}), flush=True)
    else:
        call('saaa_consult', {'question': 'Which color?', 'options': ['Blue', 'Red']})
        print(json.dumps({'type': 'turn.completed'}), flush=True)
else:
    assert spec['resume'] == session
    assert 'Blue' in prompt
    if claude:
        result = subprocess.run([helper, '--saaa-terminal-agent', 'hook', str(directory)], input=json.dumps({'hook_event_name': 'PreToolUse', 'tool_name': 'AskUserQuestion', 'tool_use_id': 'question-1', 'tool_input': spec['answer']['input']}), text=True, capture_output=True, check=True)
        assert json.loads(result.stdout)['hookSpecificOutput']['updatedInput']['answers']['Which color?'] == 'Blue'
    (pathlib.Path(spec['workspace']) / 'result.txt').write_text('Blue\n')
    call('saaa_finish', {'summary': 'Created result.txt with Blue', 'remainingManualChecks': []})
    print(json.dumps({'type': 'result', 'subtype': 'success', 'is_error': False, 'session_id': session, 'stop_reason': 'end_turn'} if claude else {'type': 'turn.completed'}), flush=True)
