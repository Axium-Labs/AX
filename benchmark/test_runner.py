import json
from pathlib import Path
import tempfile
import unittest
import runner

class MetricsTests(unittest.TestCase):
    def test_start_completion_dedup_failure_and_repeated_searches(self):
        events=[]
        for id in ['a','b']:
            events.extend([{'timestamp':12,'event':{'type':'tool_started','id':id,'name':'search','detail':'searching needle src'}},
                           {'timestamp':13,'event':{'type':'tool_finished','id':id,'success':False,'result':{'raw_output':'error'}}}])
        stats=runner.summarize(events,'ax',10)
        self.assertEqual(stats['tool_calls'],2)
        self.assertEqual(stats['failed_tool_calls'],2)
        self.assertEqual(stats['repeated_searches'],1)
        self.assertEqual(stats['time_to_first_tool'],2)

    def test_codex_unknown_rounds_and_failure_are_not_success(self):
        item={'id':'c','type':'command_execution','command':'rg -n x src','status':'failed','exit_code':1}
        stats=runner.summarize([{'event':{'type':'item.started','item':item}}, {'event':{'type':'item.completed','item':item}}],'codex',0)
        self.assertIsNone(stats['model_rounds'])
        self.assertEqual(stats['tool_calls'],1)
        self.assertEqual(stats['failed_tool_calls'],1)

    def test_missing_skipped_or_failed_acceptance_never_passes(self):
        targets=['tests/test_x.py::TestThing::test_case']
        self.assertFalse(runner.acceptance({},targets))
        self.assertFalse(runner.acceptance({('tests.test_x.TestThing','test_case'):False},targets))
        self.assertTrue(runner.acceptance({('tests.test_x.TestThing','test_case'):True},targets))

class SnapshotTests(unittest.TestCase):
    def test_two_snapshots_start_at_same_commit_and_do_not_share_edits(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);source=root/'source';source.mkdir()
            runner.checked(['git','init'],cwd=source)
            (source/'code.py').write_text('x = 1\n')
            runner.checked(['git','add','.'],cwd=source)
            runner.checked(['git','-c','user.name=Benchmark','-c','user.email=benchmark@example.invalid','commit','-m','fixture'],cwd=source)
            commit=runner.checked(['git','rev-parse','HEAD'],cwd=source).strip()
            ax=root/'ax';codex=root/'codex'
            runner.snapshot(source,ax,commit);runner.snapshot(source,codex,commit)
            (ax/'code.py').write_text('x = 2\n')
            self.assertEqual((codex/'code.py').read_text(),'x = 1\n')
            self.assertEqual(runner.changed(codex),[])
            self.assertEqual(runner.changed(ax)[0]['path'],'code.py')

    def test_timeout_is_bounded(self):
        import sys
        result=runner.run_command([sys.executable,'-c','import time;time.sleep(20)'],timeout=.1)
        self.assertTrue(result['timed_out'])
        self.assertLess(result['wall_time'],5)

    def test_locked_three_ids_exist_in_local_dataset(self):
        config=json.loads((runner.ROOT/'config.json').read_text())
        _,rows=runner.load_dataset(config)
        self.assertEqual(len(rows),3)
        self.assertEqual(config['model'],'gpt-6-luna')

if __name__=='__main__':unittest.main()
