"""Run from repo root: acceptance truth table and deliberate gate mutants."""
import ast
import json
from pathlib import Path

source = Path('scripts/wire-skeleton/matrix.py').read_text()
expr = ast.unparse(ast.parse(source).body[-1].exc.args[0])
limits = json.loads(Path('scripts/wire-skeleton/api-limits.json').read_text())
assert len(limits) == 9
cases = [({'ok': 'PASS'}, 0), ({'ok': 'PASS driver=42.7.14'}, 0)]
for name, limit in limits.items():
    assert limit['reason'] and limit['source'].startswith('https://')
    cases.append(({'ok': 'PASS', name: 'not run: ' + limit['reason']}, 1))
cases.append(({'ok': 'PASS', 'bad': 'FAIL 08P01'}, 1))

def run(expression):
    return all(eval(expression, {'results': rows}) == expected for rows, expected in cases)

assert run(expr)
print(f'control PASS: {len(cases)} cases, all nine individual gaps reject acceptance')
mutants = {
    'all-instead-of-any': expr.replace('any(', 'all('),
    'ignore-not-run': expr.replace("v != 'PASS'", "not v.startswith('not run:') and v != 'PASS'"),
}
for name, mutant in mutants.items():
    assert mutant != expr
    assert not run(mutant), name
    print(name + ': CAUGHT')
