"""Small local-only dataset benchmark. Gold patches never enter agent workspaces.

Artifacts and acceptance are produced by this harness, never by either model.
No dataset download, automatic instance replacement or model fallback exists.
"""
from __future__ import annotations
import argparse
import concurrent.futures
import csv
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import sqlite3
import statistics
import subprocess
import sys
import threading
import time
import uuid
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent
WORKSPACE = ROOT.parent
METRICS = ['total_wall_time','time_to_first_tool','time_to_first_edit','model_rounds','tool_calls',
           'search_calls','read_calls','patch_calls','shell_calls','failed_tool_calls','patch_failures',
           'repeated_searches','repeated_reads','input_tokens','output_tokens','diff_lines',
           'raw_output_bytes','large_reads','recovery_calls','max_parallel_tools']

def digest(value):
    return hashlib.sha256(value).hexdigest()

def run_command(args, cwd=None, env=None, timeout=120, log=None, stdin=None):
    started = time.monotonic()
    process = subprocess.Popen([str(a) for a in args], cwd=cwd, env=env, stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding='utf-8', errors='replace')
    timed_out = False
    try:
        output, _ = process.communicate(stdin, timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        if os.name == 'nt':
            subprocess.run(['taskkill','/PID',str(process.pid),'/T','/F'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        else:
            process.kill()
        output, _ = process.communicate()
    if log:
        Path(log).write_text(output, encoding='utf-8')
    return {'exit_code':process.returncode,'timed_out':timed_out,'wall_time':time.monotonic()-started,'output':output}

def checked(args, **kwargs):
    result = run_command(args, **kwargs)
    if result['exit_code'] != 0 or result['timed_out']:
        raise RuntimeError(f"Command failed: {args[0]} {args[1:3]}: {result['output'][-2000:]}")
    return result['output']

def load_dataset(config):
    import pyarrow.parquet as pq
    ids = {item['instance_id'] for item in config['instances']}
    matches = []
    dataset_root = (ROOT/config.get('dataset_root', '..')).resolve()
    for path in dataset_root.rglob('*.parquet'):
        if any(part in {'node_modules','target','runs','repos','.git'} for part in path.parts):
            continue
        table = pq.read_table(path)
        if 'instance_id' not in table.column_names:
            continue
        rows = {row['instance_id']:row for row in table.to_pylist() if row['instance_id'] in ids}
        if set(rows) == ids:
            matches.append((path, rows))
    if len(matches) != 1:
        raise RuntimeError(f'Expected one local dataset containing the fixed IDs; found {len(matches)}. No download or substitution allowed.')
    path, rows = matches[0]
    for item in config['instances']:
        row = rows[item['instance_id']]
        if row['repo'] != item['repo'] or row['base_commit'] != item['base_commit']:
            raise RuntimeError('Dataset repo/commit differs from locked configuration')
    return path, rows

def repo_path(item):
    return ROOT/'repos'/item['repo'].replace('/','__')

def prepare_repos(config):
    for item in config['instances']:
        path = repo_path(item)
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            checked(['git','clone','--mirror',f"https://github.com/{item['repo']}.git",path], timeout=240)
        checked(['git','--git-dir',path,'cat-file','-e',item['base_commit']+'^{commit}'])

def snapshot(source, target, commit):
    checked(['git','clone','--no-hardlinks','--no-checkout',source,target])
    checked(['git','checkout','--detach',commit], cwd=target)
    if checked(['git','status','--porcelain'], cwd=target).strip():
        raise RuntimeError('Snapshot is not clean')
    if checked(['git','rev-parse','HEAD'], cwd=target).strip() != commit:
        raise RuntimeError('Snapshot commit mismatch')

def make_environment(config, item, home):
    checked([config['python'],'-m','venv',home/'venv'])
    python = home/'venv'/('Scripts/python.exe' if os.name == 'nt' else 'bin/python')
    env = os.environ.copy()
    for name in ['AX_HOME','CODEX_HOME','AX_EVENT_LOG','PYTHONPATH','PYTHONHOME','OPENAI_API_URL']:
        env.pop(name,None)
    env.update({'AX_HOME':str(home/'ax'), 'CODEX_HOME':str(home/'codex'), 'HOME':str(home), 'USERPROFILE':str(home),
                'PIP_NO_CACHE_DIR':'1','PYTHONDONTWRITEBYTECODE':'1','PYTEST_DISABLE_PLUGIN_AUTOLOAD':'1',
                'TMP':str(home/'tmp'),'TEMP':str(home/'tmp')})
    for name in ['ax','codex','skills','tmp']:
        (home/name).mkdir(parents=True,exist_ok=True)
    # Only credentials are copied. Config, conversations, memories, model caches and hooks are not.
    if os.environ.get('OPENAI_API_KEY'):
        base=os.environ.get('OPENAI_BASE_URL','https://api.openai.com/v1').rstrip('/')
        env['OPENAI_BASE_URL']=base
        env['OPENAI_API_URL']=base+'/responses'
        (home/'codex'/'auth.json').write_text(json.dumps({'auth_mode':'apikey','OPENAI_API_KEY':os.environ['OPENAI_API_KEY']}))
    else:
        auth = Path(os.environ.get('CODEX_HOME',Path.home()/'.codex'))/'auth.json'
        if not auth.is_file():
            raise RuntimeError('Codex file credentials unavailable; no model/provider fallback allowed')
        shutil.copyfile(auth,home/'codex'/'auth.json')
    checked([python,'-m','pip','install','--no-cache-dir',*item['dependencies']], env=env, timeout=240, log=home/'dependencies.log')
    env['PATH'] = str(python.parent)+os.pathsep+env['PATH']
    env['PYTHONPATH'] = ''
    return python, env

def test_targets(row):
    def decode(value):
        return json.loads(value) if isinstance(value,str) else list(value)
    return decode(row['FAIL_TO_PASS']),decode(row['PASS_TO_PASS'])

def junit_results(path):
    if not path.exists(): return {}
    cases = {}
    for test in ET.parse(path).iter('testcase'):
        name = test.get('name','')
        classname = test.get('classname','')
        # Pytest dotted classname maps to a node id, including test classes.
        passed=not any(test.find(kind) is not None for kind in ['failure','error','skipped'])
        cases[(classname,name)] = passed
        # The source dataset's pytest parser truncates IDs at whitespace.
        # Require EVERY testcase with that canonical key to pass, not any one.
        canonical=(classname,name.split()[0])
        cases[canonical]=cases.get(canonical,True) and passed
    return cases

def acceptance(cases, targets):
    if not targets: return False
    for target in targets:
        parts=target.split('::')
        module=parts[0].removesuffix('.py').replace('/','.')
        classname='.'.join([module,*parts[1:-1]])
        if cases.get((classname,parts[-1])) is not True: return False
    return True

def evaluate(repo, python, env, row, config, directory):
    env={**env,'PYTHONPATH':str(repo/'src')+os.pathsep+str(repo)}
    # Restore gold test files, then apply ONLY the test patch. Gold solution is never used.
    patch=row['test_patch']
    files=re.findall(r'^\+\+\+ b/(.+)$',patch,re.M)
    for file in files:
        if '..' in Path(file).parts or Path(file).is_absolute(): raise RuntimeError('Unsafe test patch path')
        tracked=run_command(['git','cat-file','-e',f"{row['base_commit']}:{file}"],cwd=repo)
        if tracked['exit_code']==0: checked(['git','checkout',row['base_commit'],'--',file],cwd=repo)
        else:
            target=(repo/file).resolve()
            if not target.is_relative_to(repo.resolve()): raise RuntimeError('Unsafe test target')
            if target.exists(): target.unlink()
    checked(['git','apply','--whitespace=nowarn','-'],cwd=repo,stdin=patch)
    fail,passing=test_targets(row)
    junit=directory/'junit.xml'
    # Exactly the configured dataset acceptance set, no model grading or skips.
    test_files=list(dict.fromkeys(target.split('::')[0] for target in fail+passing))
    test=run_command([python,'-m','pytest','-o','addopts=','-rA',f'--junitxml={junit}',*test_files],
                     cwd=repo,env=env,timeout=config['test_timeout_secs'],log=directory/'tests.log')
    cases=junit_results(junit)
    return {'exit_code':test['exit_code'],'timed_out':test['timed_out'],'wall_time':test['wall_time'],
            'fail_to_pass':acceptance(cases,fail),'pass_to_pass':acceptance(cases,passing),'cases':cases}

def changed(repo):
    output=checked(['git','diff','--numstat','HEAD'],cwd=repo)
    files=[]
    for line in output.splitlines():
        a,d,path=line.split('\t',2)
        files.append({'path':path,'additions':int(a) if a.isdigit() else 0,'deletions':int(d) if d.isdigit() else 0})
    # Include new, non-ignored files without depending on git diff's tracked-only default.
    for file in checked(['git','ls-files','--others','--exclude-standard'],cwd=repo).splitlines():
        if file.startswith('.ax/'):continue
        path=repo/file
        if path.is_file(): files.append({'path':file,'additions':len(path.read_bytes().splitlines()),'deletions':0})
    return files

def classify(name,detail):
    if name=='search' or re.search(r'\b(rg|grep|Select-String)\b',detail):return 'search'
    if name=='patch' or re.search(r'\bapply_patch\b',detail):return 'patch'
    if name=='filesystem' and detail.startswith('read ') or re.search(r'\b(cat|sed|Get-Content|head|tail)\b',detail):return 'read'
    return 'shell' if name in {'shell','command_execution'} else 'other'

def summarize(events,agent,start):
    stats={key:0 for key in METRICS}
    stats.update(time_to_first_tool=None,time_to_first_edit=None,input_tokens=None,output_tokens=None)
    if agent=='codex':stats['model_rounds']=None
    seen={}; repeats={'search':set(),'read':set()}; running=set(); failures=set();recovering=False
    stats['single_search_read_rounds']=0
    round_calls=[]
    for record in events:
        event=record.get('event',record); at=record.get('timestamp',start)
        kind=event.get('type','')
        if kind=='model_started':
            if len(round_calls)==1 and round_calls[0] in {'search','read'}:stats['single_search_read_rounds']+=1
            round_calls=[];stats['model_rounds']+=1
        item=event.get('item',{})
        if kind=='tool_started' or kind in {'item.started','item.completed'} and item.get('type') in {'command_execution','file_change','mcp_tool_call'}:
            id=event.get('id',item.get('id')); name=event.get('name',item.get('type',''))
            detail=event.get('detail',item.get('command',json.dumps(item.get('changes',[]))))
            arguments=event.get('input')
            signature=json.dumps(arguments,sort_keys=True,ensure_ascii=False) if arguments is not None else ' '.join(detail.split())
            category='patch' if name=='file_change' else classify(name,detail)
            if id not in seen:
                seen[id]=(category,detail)
                round_calls.append(category)
                stats['tool_calls']+=1
                if category in {'search','read','patch'}:stats[category+'_calls']+=1
                if name in {'shell','command_execution'}:stats['shell_calls']+=1
                if recovering:stats['recovery_calls']+=1
                if stats['time_to_first_tool'] is None:stats['time_to_first_tool']=max(0,at-start)
                if category=='patch' and stats['time_to_first_edit'] is None:stats['time_to_first_edit']=max(0,at-start)
                if category in repeats:
                    if signature in repeats[category]:stats['repeated_'+category+'es' if category=='search' else 'repeated_reads']+=1
                    repeats[category].add(signature)
                running.add(id);stats['max_parallel_tools']=max(stats['max_parallel_tools'],len(running))
        if kind=='tool_finished' or kind=='item.completed':
            id=event.get('id',item.get('id'));running.discard(id)
            error=event.get('success') is False or item.get('status') in {'failed','error'} or item.get('exit_code') not in {None,0}
            if error and id not in failures:
                failures.add(id);stats['failed_tool_calls']+=1
                recovering=True
                if seen.get(id,('',))[0]=='patch':stats['patch_failures']+=1
            elif id in seen and seen[id][0] in {'patch','shell'}:recovering=False
            result=event.get('result',{})
            raw=result.get('raw_output',item.get('aggregated_output',''))
            stats['raw_output_bytes']+=len(raw.encode())
            if seen.get(id,('',))[0]=='read' and len(raw)>20000:stats['large_reads']+=1
        if kind=='turn.completed':
            usage=event.get('usage',{});stats['input_tokens']=usage.get('input_tokens');stats['output_tokens']=usage.get('output_tokens')
    return stats

def agent_command(agent,config,repo,home,prompt):
    if agent=='ax':
        binary=(ROOT/config['ax_binary']).resolve()
        return [binary,'--provider','openai' if os.environ.get('OPENAI_API_KEY') else 'codex','--model',config['model'],'--reasoning-effort',config['reasoning_effort'],
                '--codex-auth',home/'codex/auth.json','--data-dir',home/'data','--skills-dir',home/'skills',
                '--mcp-config',home/'empty-mcp.toml','--allow-dangerous','--turn-timeout-secs',str(config['timeout_secs']),'run',prompt]
    return [config['codex_binary'],'exec','--ignore-user-config','--ignore-rules','--ephemeral','--json',
            '--dangerously-bypass-approvals-and-sandbox','-m',config['model'],'-c',f'model_reasoning_effort="{config["reasoning_effort"]}"',
            *(['-c','model_provider="benchmark"','-c','model_providers.benchmark.name="Benchmark API"',
               '-c','model_providers.benchmark.wire_api="responses"','-c','model_providers.benchmark.env_key="OPENAI_API_KEY"',
               '-c','model_providers.benchmark.base_url='+json.dumps(os.environ.get('OPENAI_BASE_URL','https://api.openai.com/v1'))] if os.environ.get('OPENAI_API_KEY') else []),
            '-c','features.memories=false','-c','web_search="disabled"','-C',str(repo),prompt]

def run_agent(agent,item,row,config,run_dir,dataset_sha):
    directory=run_dir/item['instance_id']/agent
    directory.mkdir(parents=True)
    repo=directory/'repo';home=directory/'home';home.mkdir()
    result={'agent':agent,'instance_id':item['instance_id'],'model':config['model'],'reasoning_effort':config['reasoning_effort'],
            'base_commit':item['base_commit'],'dataset_sha256':dataset_sha,'resolved':False,'status':'blocked',
            'tests_passed':False,'swe_tests_passed':False,'build_passed':None,'check_passed':None,'changed_files':[],
            **{key:None for key in METRICS},'artifacts':str(directory)}
    try:
        snapshot(repo_path(item),repo,item['base_commit'])
        python,env=make_environment(config,item,home)
        (home/'empty-mcp.toml').write_text('')
        # An independent evaluation snapshot confirms the starting failure and environment.
        baseline=directory/'baseline';snapshot(repo_path(item),baseline,item['base_commit'])
        base=evaluate(baseline,python,env,row,config,directory)
        (directory/'baseline-tests.log').write_text((directory/'tests.log').read_text())
        if base['fail_to_pass'] or not base['pass_to_pass'] or base['timed_out']:
            raise RuntimeError('Baseline acceptance invalid: expected FAIL_TO_PASS failures and all PASS_TO_PASS passes; see baseline-tests.log')
        # Do not expose baseline evaluator, golden patch or prior agent output in the prompt.
        prompt='Fix the following issue in this repository. Search, read and edit the actual code, then run relevant tests. Do not access other workspaces, benchmark data/results, credentials, memories or conversations.\n\n'+row['problem_statement']
        result['prompt_sha256']=digest(prompt.encode())
        (directory/'prompt.txt').write_text(prompt,encoding='utf-8')
        event_path=directory/'events.jsonl';env['AX_EVENT_LOG']=str(event_path)
        env['PYTHONPATH']=str(repo/'src')+os.pathsep+str(repo)
        events=[]
        start=time.time();stop=threading.Event();first_edit=[]
        def watch():
            while not stop.wait(.25):
                if not first_edit and changed(repo):first_edit.append(time.time()-start)
        watcher=threading.Thread(target=watch,daemon=True);watcher.start()
        command=agent_command(agent,config,repo,home,prompt)
        # Store configuration audit without auth contents.
        (directory/'command.json').write_text(json.dumps([str(x) for x in command],indent=2))
        if agent=='codex':
            # Timestamp each event at receipt, not once after the process exits.
            def execute_codex():
                process=subprocess.Popen([str(x) for x in command],cwd=repo,env=env,stdout=subprocess.PIPE,stderr=open(directory/'stderr.log','w'),text=True,encoding='utf-8',errors='replace')
                def consume():
                    with event_path.open('w',encoding='utf-8') as sink:
                        for line in process.stdout:
                            try:event=json.loads(line)
                            except json.JSONDecodeError:continue
                            record={'timestamp':time.time(),'event':event};events.append(record);sink.write(json.dumps(record)+'\n');sink.flush()
                reader=threading.Thread(target=consume,daemon=True);reader.start()
                timed_out=False
                try:process.wait(timeout=config['timeout_secs'])
                except subprocess.TimeoutExpired:
                    timed_out=True
                    if os.name=='nt':subprocess.run(['taskkill','/PID',str(process.pid),'/T','/F'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
                    else:process.kill()
                    process.wait()
                reader.join(timeout=5)
                return {'exit_code':process.returncode,'timed_out':timed_out,'wall_time':time.time()-start}
            execution=execute_codex()
        else:
            execution=run_command(command,cwd=repo,env=env,timeout=config['timeout_secs']+5,log=directory/'stdout.log')
            if event_path.exists():events=[json.loads(line) for line in event_path.read_text().splitlines()]
        stop.set();watcher.join(timeout=5)
        result.update(summarize(events,agent,start));result['total_wall_time']=execution['wall_time']
        result['time_to_first_edit']=first_edit[0] if first_edit else None
        result['changed_files']=changed(repo);result['diff_lines']=sum(f['additions']+f['deletions'] for f in result['changed_files'])
        result['exit_code']=execution['exit_code'];result['timed_out']=execution['timed_out']
        (directory/'solution.diff').write_text(checked(['git','diff','HEAD'],cwd=repo),encoding='utf-8')
        if agent=='ax':
            database=home/'data/memory.sqlite3'
            if database.exists():
                usage=[]
                with sqlite3.connect(database) as db:
                    for (metadata,) in db.execute('select metadata from messages'):
                        value=(json.loads(metadata) or {}).get('usage',{});reported=(value or {}).get('reported')
                        if reported:usage.append(reported)
                for metric in ['input_tokens','output_tokens']:
                    values=[u.get(metric,u.get('prompt_tokens' if metric=='input_tokens' else 'completion_tokens')) for u in usage]
                    result[metric]=sum(v for v in values if v is not None) if any(v is not None for v in values) else None
        # Evaluate a separate snapshot of submitted changes. Agents can never edit acceptance artifacts.
        evaluation=directory/'evaluation';snapshot(repo_path(item),evaluation,item['base_commit'])
        patch=(directory/'solution.diff').read_text()
        if patch:checked(['git','apply','-'],cwd=evaluation,stdin=patch)
        for file in result['changed_files']:
            if run_command(['git','cat-file','-e',f"HEAD:{file['path']}"],cwd=repo)['exit_code']!=0 and (repo/file['path']).exists():
                target=evaluation/file['path'];target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(repo/file['path'],target)
        production=[f['path'] for f in result['changed_files'] if f['path'].endswith('.py') and not f['path'].startswith('tests/')]
        check=run_command([python,'-m','py_compile',*production],cwd=evaluation,env=env,timeout=config['test_timeout_secs'],log=directory/'check.log') if production else None
        result['check_passed']=check is not None and check['exit_code']==0
        tested=evaluate(evaluation,python,env,row,config,directory)
        result['tests_passed']=tested['exit_code']==0 and not tested['timed_out']
        result['swe_tests_passed']=tested['fail_to_pass'] and tested['pass_to_pass'] and result['tests_passed']
        result['resolved']=bool(result['swe_tests_passed'] and result['check_passed'] and not execution['timed_out'] and execution['exit_code']==0)
        result['status']='timeout' if execution['timed_out'] else 'agent_error' if execution['exit_code']!=0 else 'completed'
        agent_errors=[e['event'].get('error',{}).get('message') for e in events if e['event'].get('type')=='turn.failed']
        if agent_errors:result['error']='; '.join(str(error) for error in agent_errors)
        elif execution['exit_code']!=0:
            log=directory/('stdout.log' if agent=='ax' else 'stderr.log')
            result['error']=log.read_text(encoding='utf-8')[:600] if log.exists() else 'Agent process failed'
        result['acceptance']={k:v for k,v in tested.items() if k!='cases'}
    except Exception as error:
        result['error']=str(error)
    (directory/'result.json').write_text(json.dumps(result,indent=2,ensure_ascii=False),encoding='utf-8')
    return result

def report(results,config,metadata):
    (ROOT/'results.json').write_text(json.dumps({'metadata':metadata,'results':results},indent=2,ensure_ascii=False),encoding='utf-8')
    fields=['instance_id','agent','status',*METRICS,'build_passed','check_passed','tests_passed','swe_tests_passed','resolved','changed_files']
    with (ROOT/'results.csv').open('w',newline='',encoding='utf-8-sig') as file:
        writer=csv.DictWriter(file,fieldnames=fields,extrasaction='ignore');writer.writeheader()
        for row in results:writer.writerow({**row,'changed_files':json.dumps(row['changed_files'])})
    labels=[('总耗时','total_wall_time'),('首工具耗时','time_to_first_tool'),('首编辑耗时','time_to_first_edit'),('Model rounds','model_rounds'),('Tool calls','tool_calls'),('Search','search_calls'),('Read','read_calls'),('Patch failures','patch_failures'),('失败调用','failed_tool_calls'),('重复搜索','repeated_searches'),('重复读取','repeated_reads'),('测试通过','tests_passed'),('Resolved','resolved')]
    lines=['# AX vs Codex Coding Benchmark','',config['dataset_label'],'',f"Model: `{config['model']}`; reasoning: `{config['reasoning_effort']}`; timeout: {config['timeout_secs']}s.",'',
           '计时只覆盖 agent 进程；仓库/环境准备与 acceptance 测试另计。null 表示无法观测，绝不填成 0。Codex exec JSON 不公开 model rounds；其 search/read 是命令分类的观测下界。客户端状态与模型缓存不共享；服务端隐式 prompt cache 不能由此 harness 保证关闭，需结合 token usage 中缓存计数解释。','']
    def display(value):return 'N/A' if value is None else str(round(value,3)) if isinstance(value,float) else str(value)
    invalid = any(row['status'] != 'completed' for row in results)
    if invalid:
        lines.extend(['**本轮含接口或执行失败，未形成有效的三题 Coding Benchmark。下表保留真实失败记录；不能据此比较解题速度、工具效率或正确率。403/401 发生在工具调用前时，零调用不代表工具策略更高效。**',''])
    for item in config['instances']:
        lines.extend([f"## {item['instance_id']}",'','| 指标 | AX | Codex |','|---|---:|---:|'])
        pair={row['agent']:row for row in results if row['instance_id']==item['instance_id']}
        for label,key in labels:lines.append(f"| {label} | {display(pair.get('ax',{}).get(key))} | {display(pair.get('codex',{}).get(key))} |")
        for row in pair.values():
            if row.get('error'):lines.append(f"\n{row['agent']}: {row['status']} — {row['error']}")
    lines.extend(['','## Median (固定 3 题；未完成或未知值不补零)','','| 指标 | AX | Codex |','|---|---:|---:|'])
    for label,key in labels:
        cells=[]
        for agent in ['ax','codex']:
            values=[r[key] for r in results if r['agent']==agent and r.get(key) is not None and r['status']=='completed']
            cells.append(display(statistics.median(values)) if len(values)==3 else f'N/A ({len(values)}/3 observed)')
        lines.append(f'| {label} | {cells[0]} | {cells[1]} |')
    lines.extend(['','## AX 耗时诊断','',
                  '基于事件和原始输出判断；三题不足以证明因果。search/read 碎片化看连续单工具 round；并行看每个 model round 的 call 数及同时运行工具数；过大读取看 large_reads/raw_output_bytes；patch 冲突看 patch_failures；错误恢复看失败后的 call 序列；重验证看 shell 命令与测试日志。', ''])
    for row in results:
        if row['agent']=='ax':lines.append(f"- {row['instance_id']}: rounds={row['model_rounds']}, calls={row['tool_calls']}, max_parallel={row['max_parallel_tools']}, large_reads={row['large_reads']}, raw bytes={row['raw_output_bytes']}, patch failures={row['patch_failures']}, failed calls={row['failed_tool_calls']}, repeated search/read={row['repeated_searches']}/{row['repeated_reads']}. Evidence: `{row['artifacts']}`.")
    lines.extend(['','Windows 本机验收使用固定 FAIL_TO_PASS + PASS_TO_PASS 和 JUnit；原数据 Linux eval_script 保留在数据集，不在 Windows 模拟执行。build 不适用于这三个纯 Python 任务，记 N/A；check 为修改文件的 py_compile。任何 baseline 环境失败会阻止 agent 运行，不能算作模型解题失败。'])
    (ROOT/'report.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')

def main():
    parser=argparse.ArgumentParser(prog='benchmark');sub=parser.add_subparsers(dest='command',required=True)
    sub.add_parser('prepare',help='Fetch code repositories only; never download a dataset')
    run=sub.add_parser('run');run.add_argument('--agent',choices=['ax','codex','both'],default='both');run.add_argument('--instance')
    args=parser.parse_args();config=json.loads((ROOT/'config.json').read_text(encoding='utf-8'))
    if config['model']!='gpt-6-luna' or config['reasoning_effort']!='low':raise RuntimeError('Model/effort lock violated')
    path,rows=load_dataset(config)
    if args.command=='prepare':prepare_repos(config);return
    if args.instance and args.instance not in rows:parser.error('--instance must be one of the fixed three IDs')
    run_dir=ROOT/'runs'/time.strftime('%Y%m%d-%H%M%S')/uuid.uuid4().hex[:8];run_dir.mkdir(parents=True)
    config_bytes=(ROOT/'config.json').read_bytes()
    binary=(ROOT/config['ax_binary']).resolve()
    if binary.is_file():
        frozen=run_dir/'ax.exe';shutil.copyfile(binary,frozen);config['ax_binary']=str(frozen)
    agents=['ax','codex'] if args.agent=='both' else [args.agent]
    results=[];dataset_sha=digest(path.read_bytes())
    for item in config['instances']:
        if args.instance and item['instance_id']!=args.instance:continue
        with concurrent.futures.ThreadPoolExecutor(max_workers=2 if config['parallel_agents'] else 1) as pool:
            futures=[pool.submit(run_agent,agent,item,rows[item['instance_id']],config,run_dir,dataset_sha) for agent in agents]
            for future in futures:
                result=future.result();results.append(result);print(result['instance_id'],result['agent'],result['status'],result['resolved'],flush=True)
    metadata={'dataset':str(path),'dataset_sha256':dataset_sha,'config_sha256':digest(config_bytes),'platform':platform.platform(),'run_dir':str(run_dir),'created_at':time.time(),
              'auth_mode':'environment_api_key' if os.environ.get('OPENAI_API_KEY') else 'chatgpt_oauth',
              'shared_api_base_url':os.environ.get('OPENAI_BASE_URL','https://api.openai.com/v1') if os.environ.get('OPENAI_API_KEY') else 'https://chatgpt.com/backend-api/codex',
              'ax_binary_sha256':digest(binary.read_bytes()) if binary.is_file() else None,
              'codex_binary_sha256':digest(Path(config['codex_binary']).read_bytes()) if Path(config['codex_binary']).is_file() else None}
    report(results,config,metadata)

if __name__=='__main__':main()
