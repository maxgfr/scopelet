#!/usr/bin/env python3
"""At most 30 Luna-only sessions. No provider calls without --live; no retries."""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import random
import shutil
import statistics
import subprocess
import tempfile

import compare
import run as base

MODEL='gpt-5.6-luna'
TASKS=('small','diagnostics','aggregate','repo','recovery')
ARMS=('native','baseline','candidate')
BASE_TASK={'small':'task3','diagnostics':'task4','aggregate':'task2','repo':'task1'}


def matrix():
    cases=[{'task':t,'arm':a,'repetition':r} for r in (1,2) for t in TASKS for a in ARMS]
    random.Random(20260910).shuffle(cases)
    return cases


def recovery_text():
    return ''.join('receipt audit-7139 amount=47 currency=EUR\n' if i==643 else f'progress step={i} value=unchanged\n' for i in range(1500))


def fixture(root, task):
    if task=='recovery':
        (root/'evidence.txt').write_text(recovery_text())
        return 'Read evidence.txt and recover original evidence if the view is partial. Find receipt audit-7139. Write answer.json with exactly {"amount": <integer>, "currency": <string>}. Verify the result.'
    base.create_fixture(root,BASE_TASK[task])
    if task=='diagnostics':
        p=root/'checks.py'
        p.write_text('print("error: transient retry failed\\nworking\\n" * 300)\n'+p.read_text())
    return base.prompt_for('codex',BASE_TASK[task],'baseline',Path('scopelet')).replace(' Do not invoke Scopelet.','')


def grade(root, task, hashes):
    if task=='recovery':
        try:
            valid=json.dumps(json.loads((root/'answer.json').read_text()),sort_keys=True)==json.dumps({'amount':47,'currency':'EUR'},sort_keys=True)
            return valid and base.sha256_file(root/'evidence.txt')==hashes['evidence.txt']
        except (ValueError,OSError):return False
    return base.grade_workspace(BASE_TASK[task],root,expected_fixture_hashes=hashes)['passed']


def summarize(runs):
    cells={}
    for task in TASKS:
        for arm in ARMS:
            rows=[r for r in runs if r['task']==task and r['arm']==arm]
            values=[r['usage']['logical_input_tokens']+r['usage']['output_tokens'] for r in rows if not r.get('usage',{}).get('usage_missing',True)]
            cells[f'{task}/{arm}']={'attempts':len(rows),'passes':sum(r.get('passed',False) for r in rows),
                'tokens':values,'mean_tokens':statistics.mean(values) if values else None,'missing_usage':len(rows)-len(values)}
    return cells


def run_case(case,binaries,out,timeout):
    destination=out/'runs'/f"{case['task']}-{case['arm']}-{case['repetition']}";destination.mkdir(parents=True)
    with tempfile.TemporaryDirectory(prefix='scopelet-luna-perf-') as tmp:
        workspace=Path(tmp);prompt=fixture(workspace,case['task']);base._git_init(workspace)
        state=workspace/'.scopelet';state.mkdir();(state/'.ignore').write_text('*\n');(state/'.gitignore').write_text('*\n')
        codex_home=state/'codex';codex_home.mkdir()
        original_home=Path(os.environ.get('CODEX_HOME',Path.home()/'.codex'))
        for name in ('auth.json','models_cache.json'):
            if (original_home/name).is_file():
                shutil.copyfile(original_home/name,codex_home/name);(codex_home/name).chmod(0o600)
        env=base.purged_environment()
        for key in list(env):
            if key.startswith(('SCOPELET_','RTK_','HEADROOM_')):env.pop(key)
        env.update(CODEX_HOME=str(codex_home),SCOPELET_CONFIG_DIR=str(state/'config'),SCOPELET_CACHE_DIR=str(state/'cache'))
        command=base.command_for('codex')+['--dangerously-bypass-hook-trust']
        disabled=compare.global_skill_paths()
        if disabled:command+=['-c','skills.config=['+','.join('{path='+json.dumps(str(p))+',enabled=false}' for p in disabled)+']']
        if case['arm']!='native':
            binary=binaries[case['arm']]
            if case['arm']=='candidate':env['SCOPELET_COMPACT_VERSION']='2'
            subprocess.run([str(binary),'install','--agent','codex'],env=env,check=True,capture_output=True)
            env['PATH']=str(state/'config/bin')+os.pathsep+env.get('PATH','')
            prompt+='\nScopelet is installed with automatic hooks. Its query --help describes exact local queries.\n'
            if case['task']=='recovery':
                result=subprocess.run([str(binary),'compress'],input=recovery_text().encode(),env=env,capture_output=True,check=True)
                (workspace/'evidence.txt').write_bytes(result.stdout)
                if b'[scopelet compact-' not in result.stdout:raise RuntimeError('recovery fixture was not compressed')
        else:prompt+='\nUse native tools; do not invoke Scopelet.\n'
        hashes={k:v for k,v in base.fixture_hashes(workspace).items() if not k.startswith(".scopelet/")}
        (destination/'prompt.txt').write_text(prompt)
        # Persist the attempt before launching; interrupted campaigns remain auditable.
        (destination/'attempt.json').write_text(json.dumps(case))
        invocation=compare.invoke(command,prompt,workspace,timeout,env)
        stdout=invocation.pop('stdout');stderr=invocation.pop('stderr');invocation.pop('command',None)
        (destination/'stdout.raw').write_bytes(stdout);(destination/'stderr.raw').write_bytes(stderr)
        usage=base.normalize_usage('codex',stdout)
        calls=compare.ordered_tool_calls('codex',stdout)
        trace={'completed_tool_calls':len(calls),'compact_outputs':sum('[scopelet compact-' in c['text'] for c in calls),
            'recoveries':sum('scopelet' in c.get('payload','') and 'expand' in c.get('payload','') for c in calls),
            'turns':sum(e.get('type')=='turn.completed' for e in base.parse_json_stream(stdout))}
        row={**case,**invocation,'usage':usage,'model_requested':MODEL,'passed':grade(workspace,case['task'],hashes),
            'fixture_hashes':hashes,'stdout_sha256':base.sha256_bytes(stdout),'trace':trace,
            'quota_rejected':compare.rate_limit_reset(stdout + stderr) is not None}
        shutil.rmtree(state)
        base._copy_tree_after(workspace,destination/'workspace_after')
        (destination/'result.json').write_text(json.dumps(row,indent=2)+'\n')
        return row


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--live',action='store_true')
    p.add_argument('--baseline',type=Path,required=True);p.add_argument('--candidate',type=Path,required=True)
    p.add_argument('--out',type=Path,required=True);p.add_argument('--timeout',type=float,default=240)
    options=p.parse_args();out=options.out.resolve();out.mkdir(parents=True,exist_ok=False)
    report={'model_requested':MODEL,'effort':'low','live':options.live,'matrix':matrix(),'runs':[],
        'stopped':None,'notes':'At most 30 attempts, no retries. Native tools versus frozen baseline v1 and candidate v2. OS/model caches observed, not reset. No assertion of observed model when provider omits it.'}
    def save():
        report['summary']=summarize(report['runs']);(out/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    save()
    if not options.live:print('Dry run: no model calls.');return
    frozen=out/'frozen';frozen.mkdir();binaries={}
    for arm,source in [('baseline',options.baseline),('candidate',options.candidate)]:
        binaries[arm]=frozen/arm;shutil.copy2(source.resolve(),binaries[arm])
    report['binary_sha256']={k:base.sha256_file(v) for k,v in binaries.items()}
    report['harness_sha256']=base.sha256_file(Path(__file__))
    report['codex_version']=base.agent_cli_version('codex');save()
    for case in report['matrix']:
        report['active_attempt']=case;save()
        try:row=run_case(case,binaries,out,options.timeout)
        except Exception as error:
            row={**case,'passed':False,'harness_error':type(error).__name__}
            report['stopped']='harness error; no retry'
        report['runs'].append(row);report.pop('active_attempt',None)
        if row.get('quota_rejected'):report['stopped']='quota rejected; no retry'
        elif row.get('usage',{}).get('usage_missing',True):report['stopped']=report['stopped'] or 'unknown usage; no retry'
        save();print(json.dumps({k:row.get(k) for k in ('task','arm','repetition','passed','duration_seconds')}),flush=True)
        if report['stopped']:break

if __name__=='__main__':main()
