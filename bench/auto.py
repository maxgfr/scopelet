#!/usr/bin/env python3
"""Codex Luna automatic-mode pilot. No model calls without --live; at most 60 attempts."""
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
import time
import run as base
import compare

MODEL = 'gpt-5.6-luna'
ARMS = ('native', 'default', 'caveman', 'rtk', 'headroom')
TASKS = ('task3', 'task4', 'task2')

def matrix(repetitions=4, seed=20260912, tasks=TASKS, arms=ARMS):
    rows=[dict(arm=a,task=t,repetition=r) for r in range(1,repetitions+1) for t in tasks for a in arms]
    random.Random(seed).shuffle(rows)
    return rows

def prompt(task):
    text=base.prompt_for('codex',task,'baseline',Path('scopelet'))
    return text.replace(' Do not invoke Scopelet.', '')

def summary(runs):
    cells={}
    for task in TASKS:
        for arm in ARMS:
            values=[r for r in runs if r['task']==task and r['arm']==arm]
            tokens=[r['usage']['logical_input_tokens']+r['usage']['output_tokens'] for r in values
                    if not r.get('usage',{}).get('usage_missing',True)]
            cells[f'{task}/{arm}']={'attempts':len(values),'passes':sum(r.get('acceptance',{}).get('grade')=='pass' for r in values),
                                  'tokens':tokens,'mean_tokens':statistics.mean(tokens) if tokens else None,
                                  'missing_usage':len(values)-len(tokens)}
    for task in TASKS:
        baseline=cells[f'{task}/native']['mean_tokens']
        for arm in ARMS:
            cell=cells[f'{task}/{arm}'];mean=cell['mean_tokens']
            cell['change_vs_native_percent']=(100*(mean/baseline-1)) if mean is not None and baseline else None
    return cells

def write_report(out,report):
    report['summary']=summary(report['runs'])
    temporary=out/'report.json.tmp';temporary.write_text(json.dumps(report,indent=2,sort_keys=True));temporary.replace(out/'report.json')

def run_case(case,binary,rtk,awareness,out,timeout,code_mode=False):
    run_dir=out/'runs'/f"{case['task']}-{case['arm']}-{case['repetition']}";run_dir.mkdir(parents=True)
    with tempfile.TemporaryDirectory(prefix='scopelet-luna-') as tmp:
        workspace=Path(tmp);base.create_fixture(workspace,case['task']);hashes=base.fixture_hashes(workspace);base._git_init(workspace)
        state=workspace/'.scopelet';state.mkdir();(state/'.ignore').write_text('*\n');(state/'.gitignore').write_text('*\n')
        codex_home=state/'codex';codex_home.mkdir()
        original_home=Path(os.environ.get('CODEX_HOME',Path.home()/'.codex'))
        # Authentication stays local and is never archived or logged.
        for filename in ('auth.json','models_cache.json'):
            if (original_home/filename).is_file():
                shutil.copyfile(original_home/filename,codex_home/filename);(codex_home/filename).chmod(0o600)
        env=base.purged_environment()
        for key in list(env):
            if key.startswith(('SCOPELET_','RTK_','HEADROOM_')):env.pop(key)
        env.update(CODEX_HOME=str(codex_home),SCOPELET_CONFIG_DIR=str(state/'config'),SCOPELET_CACHE_DIR=str(state/'cache'))
        command=base.command_for('codex')+['--dangerously-bypass-hook-trust']
        if code_mode:command+=['--enable','code_mode','--enable','code_mode_only']
        disabled=compare.global_skill_paths()
        if disabled:command+=['-c','skills.config=['+','.join('{path='+json.dumps(str(p))+',enabled=false}' for p in disabled)+']']
        if case['arm'] in ('default','caveman'):
            subprocess.run([str(binary),'install','--agent','codex'],env=env,check=True,stdout=subprocess.DEVNULL)
            subprocess.run([str(binary),'mode',case['arm']],env=env,check=True,stdout=subprocess.DEVNULL)
            env['PATH']=str(state/'config/bin')+os.pathsep+env.get('PATH','')
        elif case['arm']=='rtk':
            (workspace/'AGENTS.md').write_bytes(awareness.read_bytes())
            env.update(RTK_TELEMETRY_DISABLED='1',RTK_DB_PATH=str(state/'rtk.db'),RTK_RECALL_DB=str(state/'rtk-recall.db'))
            env['PATH']=str(rtk.parent)+os.pathsep+env.get('PATH','')
        text=prompt(case['task']);(run_dir/'prompt.txt').write_text(text)
        invocation=compare.invoke(command,text,workspace,timeout,env)
        stdout=invocation.pop('stdout');stderr=invocation.pop('stderr')
        (run_dir/'stdout.raw').write_bytes(stdout);(run_dir/'stderr.raw').write_bytes(stderr)
        result={**case,**invocation,'model_requested':MODEL,'effort_requested':'low',
                'usage':base.normalize_usage('codex',stdout),
                'acceptance':base.grade_workspace(case['task'],workspace,expected_fixture_hashes=hashes),
                'tools':compare.adoption('codex',stdout,case['task']),
                'stdout_sha256':base.sha256_bytes(stdout),'stderr_sha256':base.sha256_bytes(stderr),
                'fixture_hashes':hashes}
        # No authentication, user config, or cache payloads in preserved workspaces.
        shutil.rmtree(state)
        base._copy_tree_after(workspace,run_dir/'workspace_after')
        (run_dir/'result.json').write_text(json.dumps(result,indent=2,sort_keys=True))
        return result

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--live',action='store_true');parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--binary',type=Path,default=Path('target/release/scopelet'))
    parser.add_argument('--rtk',type=Path);parser.add_argument('--rtk-awareness',type=Path)
    parser.add_argument('--repetitions',type=int,default=4);parser.add_argument('--timeout',type=float,default=600)
    parser.add_argument('--max-attempts',type=int,default=60)
    parser.add_argument('--tasks',nargs='+',choices=TASKS,default=TASKS)
    parser.add_argument('--arms',nargs='+',choices=ARMS,default=ARMS)
    parser.add_argument('--code-mode',action='store_true')
    options=parser.parse_args()
    if not 1<=options.repetitions<=4 or not 1<=options.max_attempts<=60:parser.error('maximum four repetitions and 60 attempts')
    out=options.out.resolve();out.mkdir(parents=True,exist_ok=True)
    if (out/'report.json').exists():parser.error('use a fresh output directory; reports are immutable campaigns')
    report={'model':MODEL,'effort':'low','live':options.live,'code_mode':options.code_mode,'matrix':matrix(options.repetitions,tasks=options.tasks,arms=options.arms),'runs':[],
            'unmeasured':{'headroom':'Existing proxy adapter rejects Codex. Prior probes show zero routed requests; no Claude substitution.'},
            'max_attempts':options.max_attempts,'integration':{'rtk':'Pinned upstream Codex awareness instructions; no automatic rewriting hook.',
            'default':'Installed automatic hooks; no skill invocation.','caveman':'Installed hooks with telegraphic response preference.'}}
    if not options.rtk or not options.rtk_awareness:report['unmeasured']['rtk']='Provide pinned RTK binary and its Codex awareness document.'
    write_report(out,report)
    if not options.live:print('Dry run: no models called.');return
    frozen=out/'frozen';frozen.mkdir();binary=frozen/'scopelet';shutil.copy2(options.binary.resolve(),binary)
    rtk=awareness=None
    if 'rtk' not in report['unmeasured']:
        rtk=frozen/'rtk';shutil.copy2(options.rtk.resolve(),rtk)
        awareness=frozen/'RTK.md';shutil.copy2(options.rtk_awareness.resolve(),awareness)
    report['binary_sha256']=base.sha256_file(binary)
    report['harness_sha256']={p.name:base.sha256_file(p) for p in [Path(__file__),Path(base.__file__),Path(compare.__file__)]}
    report['codex_version']=subprocess.run(['codex','--version'],capture_output=True,text=True,check=True).stdout.strip()
    if rtk:report['rtk_sha256']=base.sha256_file(rtk);report['rtk_awareness_sha256']=base.sha256_file(awareness)
    for case in report['matrix']:
        if case['arm'] in report['unmeasured']:continue
        if len(report['runs'])>=options.max_attempts:break
        # Persist the attempt before invoking a model, so crashes still consume budget.
        report['runs'].append({**case,'status':'started'});write_report(out,report)
        start=time.monotonic()
        try:result=run_case(case,binary,rtk,awareness,out,options.timeout,options.code_mode)
        except Exception as error:result={**case,'status':'harness_error','error':str(error),'duration_seconds':time.monotonic()-start}
        report['runs'][-1]=result;write_report(out,report)
        print(json.dumps({**case,'grade':result.get('acceptance',{}).get('grade'),'exit':result.get('exit_code'),'seconds':round(result.get('duration_seconds',0),1)}),flush=True)
        # A transport-wide failure warrants inspection rather than burning the campaign.
        if result.get('usage',{}).get('usage_missing',True):
            report['stopped']='Missing usage: inspect failure before spending more sessions.';write_report(out,report);break

if __name__=='__main__':main()
