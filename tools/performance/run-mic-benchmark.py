"""Interleave old policy, framing fix, and optimized policy; keep every run."""
import argparse
import json
from pathlib import Path
import statistics
import subprocess

parser=argparse.ArgumentParser()
parser.add_argument('--baseline',type=Path,required=True)
parser.add_argument('--optimized',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
rows=[]
for rate in [44100,48000,96000]:
    for opus in [False,True]:
        for run in range(5):
            variants=[('before',args.baseline,'legacy'),('mic-fix-only',args.baseline,'fixed'),('optimized',args.optimized,'fixed')]
            if run%2: variants.reverse()
            for name,exe,mode in variants:
                row=json.loads(subprocess.check_output([str(exe.resolve()),mode,str(rate),'2','5000','opus' if opus else 'pcm'],text=True))
                row.update(variant=name,run=run+1)
                rows.append(row)
args.output.mkdir(parents=True,exist_ok=True)
(args.output/'mic-runs.json').write_text(json.dumps(rows,indent=2))
summary=[]
for rate in [44100,48000,96000]:
    for opus in [False,True]:
        for variant in ['before','mic-fix-only','optimized']:
            matching=[r for r in rows if r['variant']==variant and r['rate']==rate and r['opus']==opus]
            summary.append({'rate':rate,'opus':opus,'variant':variant,'runs':len(matching),
                            'median_run_p50_us':statistics.median(r['p50_us'] for r in matching),
                            'median_run_p95_us':statistics.median(r['p95_us'] for r in matching),
                            'median_run_p99_us':statistics.median(r['p99_us'] for r in matching),
                            'worst_run_max_us':max(r['max_us'] for r in matching),
                            'excess_consumed_frames':sorted(set(r['excess_consumed_frames'] for r in matching))})
(args.output/'mic-summary.json').write_text(json.dumps(summary,indent=2))
print(json.dumps(summary,indent=2))
