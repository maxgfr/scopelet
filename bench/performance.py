#!/usr/bin/env python3
"""Offline release-binary comparisons. No models. Fresh, immutable output directory."""
from __future__ import annotations
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import tempfile
import time


def sha(data):
    return hashlib.sha256(data).hexdigest()


def fixtures(root, stress=False):
    root.mkdir()
    (root/'small').write_bytes(b'small exact output\r\n')
    (root/'rejected').write_text('é'*6000)
    log = ''.join(f'progress {i:05d} '+ 'x'*300+'\n' for i in range(10000))
    (root/'log').write_text(log+'error: late failure\nfinal\n')
    rows = [{'id':i,'status':'failed' if i % 10 == 0 else 'passed','suite':i%4,'payload':'stable '*20} for i in range(12000)]
    (root/'rows').write_text('\n'.join(json.dumps(row, separators=(',',':')) for row in rows))
    repo=root/'repo';repo.mkdir()
    for i in range(400):
        (repo/f'{i:05d}.txt').write_text(f'file {i}\n'+'ordinary line\n'*100+('needle\n' if i%40==0 else ''))
    (root/'recovery').write_text(''.join(f'line {i:05d} original\r\n' for i in range(90000)))
    cases = {
        'small': (['compress'], root/'small'),
        'rejected': (['compress'], root/'rejected'),
        'log': (['compress'], root/'log'),
        'json_table': (['compress'], root/'rows'),
        'aggregate': (['query','--file',str(root/'rows'),'--format','jsonl','--filter','/status','--equals','"failed"','--group','/suite'], None),
        'repo': (['query','--repo',str(repo),'--find','needle','--count'], None),
        'recovery': ([], None),
    }
    if stress:
        (root/'limit').write_text(('ordinary '+'x'*621+'\n')*53000+'error: boundary\n')
        assert (root/'limit').stat().st_size <= 32*1024*1024
        cases['limit_32m']=(['compress'],root/'limit')
    return cases


def measure(binary, args, stdin, cache, version):
    command=[str(binary),'--cache-dir',str(cache),*args]
    env=os.environ.copy();env.pop('SCOPELET_COMPACT_VERSION',None)
    if version is not None:command+=['--compact-version',str(version)]
    timer=['/usr/bin/time','-l'] if platform.system()=='Darwin' else ['/usr/bin/time','-v']
    start=time.perf_counter()
    with open(stdin or os.devnull,'rb') as source:
        result=subprocess.run(timer+command,stdin=source,capture_output=True,env=env,timeout=120)
    elapsed=time.perf_counter()-start
    stderr=result.stderr.decode(errors='replace')
    pattern=r'(\d+)\s+maximum resident set size' if platform.system()=='Darwin' else r'Maximum resident set size \(kbytes\):\s*(\d+)'
    match=re.search(pattern,stderr)
    rss=int(match[1])*(1 if platform.system()=='Darwin' else 1024) if match else None
    return {'seconds':elapsed,'rss_bytes':rss,'exit_code':result.returncode,'stdout_sha256':sha(result.stdout),'output_bytes':len(result.stdout)}, result.stdout


def summarize(samples):
    seconds=sorted(s['seconds'] for s in samples)
    rss=[s['rss_bytes'] for s in samples if s['rss_bytes'] is not None]
    return {'median_seconds':statistics.median(seconds),'p95_seconds':seconds[max(0,math.ceil(.95*len(seconds))-1)],
            'median_rss_bytes':statistics.median(rss) if rss else None,
            'failures':sum(s['exit_code'] != 0 for s in samples),'samples':len(samples)}


def run(options):
    out=options.out.resolve();out.mkdir(parents=True,exist_ok=False)
    frozen=out/'frozen';frozen.mkdir()
    binaries={}
    for name, source in [('baseline',options.baseline),('candidate',options.candidate)]:
        binaries[name]=frozen/name;shutil.copy2(source,binaries[name])
    baseline_version = getattr(options, 'baseline_version', None)
    comparison_arm = 'candidate_v2' if baseline_version == 2 else 'candidate'
    report={'kind':'offline process measurements; not model tokens','platform':platform.platform(),
            'baseline_compact_version': baseline_version, 'comparison_arm': comparison_arm,
            'warmups':options.warmups,'repetitions':options.repetitions,'stress':options.stress,
            'cache_note':'cold means empty application cache, not flushed OS page cache',
            'binary_sha256':{k:sha(p.read_bytes()) for k,p in binaries.items()},'cases':{},'v1_mismatches':[], 'output_mismatches':[]}
    with tempfile.TemporaryDirectory(prefix='scopelet-perf-') as temp:
        root=Path(temp);cases=fixtures(root/'fixtures',options.stress)
        if options.cases:
            selected=options.cases.split(',')
            if set(selected)-cases.keys():raise ValueError('unknown case')
            cases={key:cases[key] for key in selected}
        report['fixture_sha256']={str(p.relative_to(root/'fixtures')):sha(p.read_bytes()) for p in (root/'fixtures').rglob('*') if p.is_file()}
        for name,(args,stdin) in cases.items():
            for state in ['cold','warm']:
                key=f'{name}/{state}';report['cases'][key]={}
                hashes={}
                for rep in range(-options.warmups,options.repetitions):
                    arms=['baseline','candidate','candidate_v2']
                    if rep%2:arms.reverse()
                    for arm in arms:
                        binary=binaries['baseline' if arm=='baseline' else 'candidate'];version=baseline_version if arm=='baseline' else (2 if arm=='candidate_v2' else 1)
                        cache=root/f'cache-{arm}-{name}-{state}'
                        if state=='cold':shutil.rmtree(cache,ignore_errors=True)
                        actual_args=args
                        if name=='recovery':
                            raw=(root/'fixtures'/'recovery').read_bytes();blob=sha(raw)
                            (cache/'blobs').mkdir(parents=True,exist_ok=True)
                            (cache/'blobs'/blob).write_bytes(raw)
                            actual_args=['expand','blob:'+blob,'--start','70000','--end','70004']
                        sample,stdout=measure(binary,actual_args,stdin,cache,version)
                        if sample['exit_code']:
                            (out/f'{name}-{state}-{arm}-failure.stdout').write_bytes(stdout)
                        if rep>=0:
                            report['cases'][key].setdefault(arm,{'samples':[]})['samples'].append(sample)
                            hashes[arm]=sample['stdout_sha256']
                        if rep==options.repetitions-1:
                            report['cases'][key][arm]['cache_bytes']=sum(p.stat().st_size for p in cache.rglob('*') if p.is_file())
                    if rep>=0 and hashes['baseline']!=hashes[comparison_arm]:
                        mismatch = {'case':key,'repetition':rep}
                        report['output_mismatches'].append(mismatch)
                        if comparison_arm == 'candidate':
                            report['v1_mismatches'].append(mismatch)
                for cell in report['cases'][key].values():cell['summary']=summarize(cell['samples'])
                print(json.dumps({'case':key,**{a:c['summary'] for a,c in report['cases'][key].items()}}),flush=True)
                (out/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    return report


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--baseline',type=Path,required=True);p.add_argument('--candidate',type=Path,required=True)
    p.add_argument('--baseline-version', type=int, choices=(1, 2), help='Pin the reference presentation; use 2 for released 0.3.0.')
    p.add_argument('--out',type=Path,required=True);p.add_argument('--repetitions',type=int,default=30)
    p.add_argument('--warmups',type=int,default=5);p.add_argument('--stress',action='store_true')
    p.add_argument('--cases',help='comma-separated subset for a component follow-up')
    options=p.parse_args()
    if options.repetitions<1 or options.warmups<0:p.error('positive repetitions and nonnegative warmups required')
    report=run(options)
    if report['output_mismatches'] or any(c['summary']['failures'] for case in report['cases'].values() for c in case.values()):raise SystemExit(1)

if __name__=='__main__':main()
