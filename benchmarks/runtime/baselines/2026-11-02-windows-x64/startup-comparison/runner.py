import sys, json, os, tempfile, hashlib, platform, csv
from pathlib import Path
from datetime import datetime, timezone
sys.path.insert(0, r'C:\Users\14181\Desktop\axlab\ax\benchmarks\runtime')
from run import measured_process, summary
OUT = Path(__file__).resolve().parent / 'AX-market-comparison-node22'
OUT.mkdir(exist_ok=True)
BASE = Path(r'C:\Program Files\nodejs\node_cache\node_modules')
NODE = Path(r'C:\Users\14181\AppData\Roaming\nvm\v22.20.0\node.exe')
commands = {
 'AX': [r'C:\Users\14181\Desktop\axlab\ax\target\release\ax.exe', '--version'],
 'Codex native': [str(next((BASE/'@openai/codex/node_modules').rglob('codex.exe'))), '--version'],
 'OpenCode': [str(BASE/'@opencode/cli/bin/opencode.exe'), '--version'],
 'Pi': [str(NODE), str(BASE/'@earendil-works/pi-coding-agent/dist/bundle/cli.js'), '--version'],
 'Codex npm launcher': [str(NODE), str(BASE/'@openai/codex/bin/codex.js'), '--version'],
}
data = dict(timestamp_utc=datetime.now(timezone.utc).isoformat(), os=platform.platform(),
 protocol='same Windows host; --version only; 3 independent rounds; 5 warmup + 20 recorded fresh-process launches per tool per round; OS cache not flushed; shell shim excluded; native Codex and npm launcher separated; isolated homes; parent launch/wait included; Win32 whole-process CPU and working set; npm launcher memory excludes its native child and must not be compared',
 node_runtime='v22.20.0 (existing installation, explicitly selected for Pi compatibility)', commands=commands, hashes={p:hashlib.sha256(Path(p).read_bytes()).hexdigest() for args in commands.values() for p in args if Path(p).is_file()}, runs=[], failures=[])
for round_id in range(3):
 names=list(commands)
 names=names[round_id:]+names[:round_id]
 for name in names:
  print(round_id+1, name, flush=True)
  with tempfile.TemporaryDirectory(prefix='ax-peer-version-') as folder:
   env=os.environ.copy()
   for key in ('HOME','USERPROFILE','APPDATA','LOCALAPPDATA','XDG_CONFIG_HOME','XDG_CACHE_HOME','AX_HOME','CODEX_HOME'):
    directory=Path(folder)/key
    directory.mkdir()
    env[key]=str(directory)
   rows=[]
   try:
    for sample in range(25):
     out,res=measured_process(commands[name],folder,env,30)
     version=out.strip()
     if not version: raise ValueError('empty version output')
     if sample>=5: rows.append(dict(sample=sample-5,elapsed_ns=res['wall_ns'],operations=1,ns_per_op=res['wall_ns'],resources=res))
    data['runs'].append(dict(name=name, round=round_id+1, version=version,samples=rows))
   except Exception as e: data['failures'].append(dict(name=name, round=round_id+1,error=str(e)))
  (OUT/'raw.json').write_text(json.dumps(data,indent=2),encoding='utf8')
result=[]
for name in commands:
 runs=[r for r in data['runs'] if r['name']==name]
 samples=[s for r in runs for s in r['samples']]
 if not samples: continue
 stats=summary(samples)
 peak=max(s['resources']['peak_rss_bytes'] or 0 for s in samples)/2**20
 result.append(dict(name=name,version=runs[0]['version'],**stats,peak_working_set_mib=peak if name!='Codex npm launcher' else None,
 rounds_mean_ms=[summary(r['samples'])['mean_ns']/1e6 for r in runs]))
(OUT/'summary.json').write_text(json.dumps(result,indent=2),encoding='utf8')
with (OUT/'samples.csv').open('w',newline='',encoding='utf8') as f:
 writer=csv.DictWriter(f,fieldnames=['name','version','round','sample','elapsed_ns','cpu_seconds','peak_rss_bytes'])
 writer.writeheader()
 for r in data['runs']:
  for s in r['samples']:
   writer.writerow(dict(name=r['name'],version=r['version'],round=r['round'],sample=s['sample'],elapsed_ns=s['elapsed_ns'],cpu_seconds=s['resources']['cpu_seconds'],peak_rss_bytes=s['resources']['peak_rss_bytes'] if r['name']!='Codex npm launcher' else None))
lines=['# AX and installed peers: same-host --version comparison','',data['protocol'],'',f"UTC: {data['timestamp_utc']}",'']
ax=next(x for x in result if x['name']=='AX')['mean_ns']
for r in result:
 text=f"- {r['name']} ({r['version']}): mean {r['mean_ns']/1e6:.3f} ms; median {r['median_ns']/1e6:.3f} ms; P95 {r['p95_ns']/1e6:.3f} ms; min {r['min_ns']/1e6:.3f} ms; max {r['max_ns']/1e6:.3f} ms; peak working set {r['peak_working_set_mib']} MiB; mean/AX {r['mean_ns']/ax:.2f}; per-round means {r['rounds_mean_ms']}"
 lines.append(text)
lines += ['', 'These numbers cover version-command process startup only. They do not rank agent loop efficiency, full TUI readiness, Memory recall, subagent lifecycle, coding quality or long-running footprint.', '', 'Failures: '+json.dumps(data['failures'])]
(OUT/'report.md').write_text('\n'.join(lines)+'\n',encoding='utf8')
print(json.dumps(result,indent=2)); print('failures',data['failures'])
