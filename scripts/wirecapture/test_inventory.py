"""Reducer canary: aliases, star expansion, missing columns/helpers and parse errors."""
import json,subprocess,sys,tempfile,unittest
from pathlib import Path
class Inventory(unittest.TestCase):
 def test_classification(self):
  root=Path(__file__).resolve().parents[2]
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp);(p/'action-connections.jsonl').write_text(json.dumps(dict(action='probe',first=1,last=1))+'\n')
   (p/'catalog-columns.json').write_text(json.dumps({'pg_catalog.pg_class':['oid','relname','relpartbound']}))
   queries=['SELECT c.relname FROM pg_catalog.pg_class c','SELECT * FROM pg_catalog.pg_class','SELECT unnest(current_schemas(true))','SELECT (']
   (p/'protocol.jsonl').write_text(''.join(json.dumps(dict(connection=1,direction='frontend',tag='Q',hex=(q+'\0').encode().hex()))+'\n' for q in queries))
   subprocess.run([sys.executable,str(root/'scripts/wirecapture/inventory.py'),str(p),str(root/'docs/decisions/0014-read-only-catalog-views.md'),str(p/'out')],check=True,capture_output=True)
   rows=json.loads((p/'out/inventory.json').read_text());by={r['object']:r for r in rows}
   self.assertEqual(by['pg_catalog.pg_class']['missing_columns'],['relpartbound'])
   self.assertEqual(by['pg_catalog.pg_class']['columns'],['oid','relname','relpartbound'])
   self.assertTrue(by['function:current_schemas']['covered']);self.assertFalse(by['function:unnest']['covered'])
   self.assertEqual(len(json.loads((p/'out/unresolved.json').read_text())),1)
   q='SELECT * FROM (SELECT c.relname FROM pg_catalog.pg_class c) c UNION SELECT relname FROM pg_catalog.pg_class'
   (p/'protocol.jsonl').write_text(json.dumps(dict(connection=1,direction='frontend',tag='Q',hex=(q+'\0').encode().hex()))+'\n')
   subprocess.run([sys.executable,str(root/'scripts/wirecapture/inventory.py'),str(p),str(root/'docs/decisions/0014-read-only-catalog-views.md'),str(p/'out')],check=True,capture_output=True)
   row=json.loads((p/'out/inventory.json').read_text())[0]
   self.assertEqual(row['columns'],['relname']);self.assertTrue(row['covered'])
if __name__=='__main__':unittest.main()
