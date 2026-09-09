#!/usr/bin/env python3
"""Bounded small-task comparison. No model calls without --live; private raw traces."""
from __future__ import annotations
import argparse
import concurrent.futures
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

TASKS = ('python', 'javascript', 'json')
ARMS = ('native', 'released', 'candidate')


def fixture(root, task):
    if task == 'python':
        base.create_fixture(root, 'task3')
        return 'Fix normalize_identifier in src/helpers.py so it trims, lowercases and collapses whitespace. Verify the change.'
    if task == 'javascript':
        (root/'options.mjs').write_text('export function option(value, fallback) { return value || fallback; }\n')
        (root/'check.mjs').write_text("import {option} from './options.mjs';\nimport assert from 'node:assert/strict';\nassert.equal(option(0, 3), 0);\nassert.equal(option(undefined, 3), 3);\nconsole.log('passed');\n")
        return 'Fix option in options.mjs: default only for null or undefined, preserving explicit false, zero and empty string. Verify the change.'
    if task == 'json':
        (root/'config.json').write_text('{"retries":3,"enabled":false,"quota":0,"label":"keep me","nested":{"retries":2}}\n')
        return 'Change only the top-level retries in config.json to 0. Preserve every other setting and verify the result.'
    raise ValueError(task)


def grade(root, task, hashes):
    if task == 'python':
        return base.grade_workspace('task3', root, expected_fixture_hashes=hashes)['passed']
    if task == 'json':
        try:
            expected = {'retries':0,'enabled':False,'quota':0,'label':'keep me','nested':{'retries':2}}
            return json.dumps(json.loads((root/'config.json').read_text()),sort_keys=True) == json.dumps(expected,sort_keys=True)
        except (ValueError, OSError):
            return False
    if base.sha256_file(root/'check.mjs') != hashes['check.mjs']:
        return False
    code = "import {option} from './options.mjs'; import assert from 'node:assert/strict'; for(const v of [0,false,'',7,'yes']) assert.equal(option(v,3),v); for(const v of [null,undefined]) assert.equal(option(v,3),3);"
    return subprocess.run(['node','--input-type=module','-e',code],cwd=root,capture_output=True,timeout=15).returncode == 0


def matrix(repetitions):
    cases=[{'task':t,'arm':a,'repetition':r} for r in range(1,repetitions+1) for t in TASKS for a in ARMS]
    random.Random(20260909).shuffle(cases)
    return cases


def summarize(rows):
    result={}
    for task in TASKS:
        for arm in ARMS:
            selected=[r for r in rows if r['task']==task and r['arm']==arm]
            values=[r['usage']['logical_input_tokens']+r['usage']['output_tokens'] for r in selected if r.get('usage',{}).get('logical_input_tokens') is not None and r.get('usage',{}).get('output_tokens') is not None]
            result[f'{task}/{arm}']={'attempts':len(selected),'passes':sum(r.get('passed',False) for r in selected),'tokens':values,'median_tokens':statistics.median(values) if values else None,'mean_tokens':statistics.mean(values) if values else None,'missing_usage':len(selected)-len(values)}
    return result


def run_case(case, binaries, out, disabled):
    destination=out/f"{case['task']}-{case['arm']}-{case['repetition']}";destination.mkdir()
    with tempfile.TemporaryDirectory(prefix='scopelet-small-') as tmp:
        workspace=Path(tmp);prompt=fixture(workspace,case['task']);hashes=base.fixture_hashes(workspace);base._git_init(workspace)
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
        if disabled:command+=['-c','skills.config=['+','.join('{path='+json.dumps(str(p))+',enabled=false}' for p in disabled)+']']
        if case['arm'] != 'native':
            binary=binaries[case['arm']]
            subprocess.run([str(binary),'install','--agent','codex'],env=env,check=True,stdout=subprocess.DEVNULL)
        prompt+='\nWork in this isolated synthetic workspace. Save the requested change and report the result.\n'
        (destination/'prompt.txt').write_text(prompt)
        result=compare.invoke(command,prompt,workspace,240,env)
        raw=result.pop('stdout');err=result.pop('stderr');result.pop('command',None)
        (destination/'stdout.raw').write_bytes(raw);(destination/'stderr.raw').write_bytes(err)
        usage=base.normalize_usage('codex',raw)
        calls=compare.ordered_tool_calls('codex',raw)
        messages=[e['item'].get('text','') for e in base.parse_json_stream(raw) if e.get('type')=='item.completed' and isinstance(e.get('item'),dict) and e['item'].get('type')=='agent_message']
        row={**case,**result,'usage':usage,'passed':grade(workspace,case['task'],hashes),'fixture_hashes':hashes,'stdout_sha256':base.sha256_bytes(raw),'trace':{'completed_shell_calls':len(calls),'compact_outputs':sum('[scopelet compact-v1' in c['text'] for c in calls),'final_reply_bytes':len(messages[-1].encode()) if messages else None,'final_reply_words':len(messages[-1].split()) if messages else None}}
        shutil.rmtree(state);base._copy_tree_after(workspace,destination/'workspace_after')
        (destination/'result.json').write_text(json.dumps(row,indent=2)+'\n')
        return row


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--live',action='store_true');p.add_argument('--out',type=Path,required=True)
    p.add_argument('--released',type=Path,required=True);p.add_argument('--candidate',type=Path,required=True)
    p.add_argument('--repetitions',type=int,default=2);p.add_argument('--workers',type=int,choices=[1,2],default=2)
    a=p.parse_args()
    if not 1<=a.repetitions<=2:p.error('one or two repetitions; at most 18 sessions')
    out=a.out.resolve();out.mkdir(parents=True,exist_ok=True)
    if any(out.iterdir()):p.error('output must be empty')
    cases=matrix(a.repetitions);report={'model_requested':'gpt-5.6-luna','effort':'low','live':a.live,'matrix':cases,'runs':[],'notes':['Equal prompts and isolated host configs; no skill invocation.','Logical input plus output includes cached input and is not billed cost.','Two repetitions per cell cannot establish general or statistically reliable savings.','All failures retained; missing usage stops further batches.']}
    def save():
        report['summary']=summarize(report['runs']);(out/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    save()
    if not a.live:return
    binaries={}
    for arm,path in [('released',a.released),('candidate',a.candidate)]:
        binaries[arm]=out/f'scopelet-{arm}';shutil.copy2(path.resolve(),binaries[arm])
    report['binary_sha256']={a:base.sha256_file(b) for a,b in binaries.items()}
    report['harness_sha256']={Path(p).name:base.sha256_file(Path(p)) for p in [__file__,base.__file__,compare.__file__]}
    report['codex_version']=subprocess.check_output(['codex','--version'],text=True).strip();save()
    disabled=compare.global_skill_paths()
    with concurrent.futures.ThreadPoolExecutor(max_workers=a.workers) as pool:
        for i in range(0,len(cases),a.workers):
            batch=cases[i:i+a.workers];start=len(report['runs'])
            report['runs'].extend({**case,'status':'started'} for case in batch);save()
            futures=[pool.submit(run_case,c,binaries,out,disabled) for c in batch]
            for index,future in enumerate(futures):
                try:row=future.result()
                except Exception as error:row={**batch[index],'status':'harness_error','error':str(error)}
                report['runs'][start+index]=row;save()
                print(json.dumps({k:row.get(k) for k in ['task','arm','repetition','passed','duration_seconds']}),flush=True)
            if any(r.get('usage',{}).get('usage_missing',True) for r in report['runs'][start:]):
                report['stopped']='Missing usage; inspect before further calls.';save();break


if __name__=='__main__':main()
