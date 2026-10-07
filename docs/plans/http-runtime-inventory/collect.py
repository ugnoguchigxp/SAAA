"""Read-only source inventory; writes TSV/JSON evidence beside this file, never builds.
Run: python3 docs/plans/http-runtime-inventory/collect.py
Static occurrences are NOT executable test counts or a Rust/TS parser.
"""
from pathlib import Path
import csv, hashlib, json, re, subprocess
from datetime import datetime, timezone
ROOT = Path(__file__).resolve().parents[3]
OUT = Path(__file__).resolve().parent
paths = sorted(set(subprocess.check_output(['git','ls-files','-z','--cached','--others','--exclude-standard'],cwd=ROOT).decode().split('\0')) - {''})
rows = {x: [] for x in ['sources','commands','transport','tests','manifests']}
def target(path):
    if not path.startswith('src-tauri/src/'):
        return 'existing-package-or-frontend'
    x=path[len('src-tauri/src/'):].split('/')[0].removesuffix('.rs')
    groups={
      'saaa-storage': ['persistence','credentials','backup','database_backup','records'],
      'saaa-contracts': ['ipc_contract','models','voice_asr_contract'],
      'saaa-providers': ['providers'],
      'saaa-memory': ['memory'],
      'saaa-tools': ['tool_selection','generated_capabilities','generative_ui','artifact_preview'],
      'saaa-work': ['coding','worker_agents','steward','schedule','adaptive_evaluation','adaptive_improvement'],
      'saaa-conversation': ['runtime','role_routing','task_queue'],
      'saaa-voice': ['voice','voice_behavior','voice_text','tts_dictionary'],
      'saaa-diagnosis': ['diagnosis'],
      'saaa-situation': ['situation'],
      'saaa-media': ['media_generation']}
    return next((k for k,v in groups.items() if x in v),'saaa-application-or-host')
for name in paths:
    p=ROOT/name
    if not p.is_file() or not name.startswith(('src/','src-tauri/','crates/','services/','tests/','scripts/')):
        continue
    if p.suffix not in ['.rs','.ts','.tsx','.toml','.c','.h','.json']:
        continue
    if any(x in p.parts for x in ['target','node_modules','resources']):
        continue
    if p.suffix=='.json' and p.name!='tauri.conf.json':
        continue
    data=p.read_bytes()
    try: s=data.decode()
    except UnicodeDecodeError: continue
    sha=hashlib.sha256(data).hexdigest()
    lines=s.splitlines()
    rows['sources'].append([name,sha,len(lines),target(name)])
    if p.name=='Cargo.toml':
        rows['manifests'].append([name,sha])
    if p.suffix=='.rs':
        for m in re.finditer(r'#\[(?:tauri::)?command(?:\([^]]*\))?\]\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)',s):
            rows['commands'].append([name,s.count('\n',0,m.start())+1,m.group(1),target(name)])
        for m in re.finditer(r'#\[(?:(?:tokio|async_std|rstest)::)?(?:test|rstest)(?:\([^]]*\))?\]',s):
            tail=s[m.end():m.end()+1500]
            fn=re.search(r'\b(?:async\s+)?fn\s+(\w+)',tail)
            rows['tests'].append([name,s.count('\n',0,m.start())+1,'rust-test-attribute',fn.group(1) if fn else 'UNRESOLVED',target(name)])
    for i,line in enumerate(lines,1):
        if re.search(r'tauri::|@tauri-apps/|generate_handler!|with_handler\(|\.emit(?:_to|_filter)?\s*\(|\blisten\s*(?:<.*>)?\s*\(',line):
            rows['transport'].append([name,i,line.strip(),target(name)])
        if p.suffix in ['.ts','.tsx'] and re.search(r'\b(?:test|it)(?:\.(?:each|skip|todo|only))?\s*\(',line):
            rows['tests'].append([name,i,'typescript-test-expression',line.strip(),target(name)])
        if p.suffix=='.rs' and re.search(r'macro_rules!|rstest|test_case|include!|cfg_attr.*test',line):
            rows['tests'].append([name,i,'macro-or-generated-review',line.strip(),target(name)])
    if p.read_bytes()!=data:
        raise SystemExit(f'Concurrent edit while reading {name}; rerun before using inventory')
headers={
 'sources':['path','sha256','lines','proposed_owner'],
 'commands':['path','line','command','proposed_owner'],
 'transport':['path','line','source','proposed_owner'],
 'tests':['path','line','kind','source_case_or_expression','proposed_owner'],
 'manifests':['path','sha256']}
for kind,content in rows.items():
    with (OUT/f'{kind}.tsv').open('w') as f:
        w=csv.writer(f,delimiter='\t',lineterminator='\n'); w.writerow(headers[kind]); w.writerows(content)
changed=[name for name,sha,*_ in rows['sources'] if not (ROOT/name).exists() or hashlib.sha256((ROOT/name).read_bytes()).hexdigest()!=sha]
summary={'captured_at':datetime.now(timezone.utc).isoformat(),'head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT).decode().strip(),'branch':subprocess.check_output(['git','branch','--show-current'],cwd=ROOT).decode().strip(),'counts':{k:len(v) for k,v in rows.items()},'changed_during_scan':changed,'limitations':['Static occurrence inventory, not compiled registration or executed test count.','Owner is a migration default; host adapters and shared transaction orchestration follow the plan exceptions.','Resolve aliases, wrapper handlers, macros, cfg, parameterization and dynamic event names during H00/H12.','Source hashes identify the observed dirty worktree; they are not a commit snapshot.']}
(OUT/'summary.json').write_text(json.dumps(summary,ensure_ascii=False,indent=2)+'\n')
print(json.dumps(summary,ensure_ascii=False))
if changed: raise SystemExit(1)
