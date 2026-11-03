import subprocess, shutil, sys, re
F="/home/dev/src/nata/spikes/generic-ops/src/kernels.rs"
D="/home/dev/src/nata/spikes/generic-ops"
LI="/home/dev/src/nata/data/tpch-sf1/lineitem.parquet"
orig=open(F).read()
ONLY=set(sys.argv[1:])
# (name, old, new, occurrence index (0-based) of old)
M=[
 ("filter ignores NULLs","& valid.ok(i)) as usize","& true) as usize",0),
 ("sum ignores NULLs (multiplier)","s += (v[i] as i128) * (ok as i128);","s += v[i] as i128;",0),
 ("mul error not masked by validity","bad |= o & ok;\n    });\n    if bad {\n        return overflow(\"multiplication\")","bad |= o;\n    });\n    if bad {\n        return overflow(\"multiplication\")",0),
 ("sub error not masked by validity","bad |= o & ok;\n    });\n    if bad {\n        return overflow(\"subtraction\")","bad |= o;\n    });\n    if bad {\n        return overflow(\"subtraction\")",0),
 ("mul output validity always true","valid[i] = ok as u8;\n        bad |= o & ok;\n    });\n    if bad {\n        return overflow(\"multiplication\")","valid[i] = 1;\n        bad |= o & ok;\n    });\n    if bad {\n        return overflow(\"multiplication\")",0),
 ("mul validity ignores second operand","let ok = va.ok(i) & vb.ok(i);","let ok = va.ok(i);",0),
 ("masked group sums do not skip NULL values","acc[g][j] += (cols[j].0[i] as i128) * (ok as i128);","acc[g][j] += cols[j].0[i] as i128;",0),
 ("masked group sums miscount NULLs","nulls[j][g] += !ok as u64;","nulls[j][g] += 0;",0),
 ("dense group count wrong","count[g] += 1;\n        for j in 0..K {\n            acc[g][j] += cols[j][i] as i128;","count[g] += 2;\n        for j in 0..K {\n            acc[g][j] += cols[j][i] as i128;",0),
 ("masked group count wrong","count[g] += 1;\n        for j in 0..K {\n            let ok","count[g] += 0;\n        for j in 0..K {\n            let ok",0),
 ("filter ignores the incoming selection (masked path)","each(rows, sel, |i| {\n            let x = col[i];\n            out[k] = i as u32;\n            k += ((x >= lo) & (x <= hi) & valid","each(rows, None, |i| {\n            let x = col[i];\n            out[k] = i as u32;\n            k += ((x >= lo) & (x <= hi) & valid",0),
 ("filter ignores the incoming selection (dense path)","each(rows, sel, |i| {\n            let x = col[i];\n            out[k] = i as u32;\n            k += ((x >= lo) & (x <= hi)) as usize;","each(rows, None, |i| {\n            let x = col[i];\n            out[k] = i as u32;\n            k += ((x >= lo) & (x <= hi)) as usize;",0),
 ("mul overflow never reported (all-valid path)","if bad {\n            return overflow(\"multiplication\");\n        }\n        return Ok","if false {\n            return overflow(\"multiplication\");\n        }\n        return Ok",0),
 ("mul overflow never reported (masked path)","if bad {\n        return overflow(\"multiplication\");\n    }\n    Ok(Out { v, valid: Some","if false {\n        return overflow(\"multiplication\");\n    }\n    Ok(Out { v, valid: Some",0),
 ("filter lower bound off by one","(x >= lo) & (x <= hi) & valid","(x > lo) & (x <= hi) & valid",0),
]
def run(cmds):
    for name,cmd in cmds:
        r=subprocess.run(cmd,cwd=D,shell=True,capture_output=True,text=True)
        if r.returncode!=0: return name
    return None
cmds=[("unit tests","cargo test --release 2>&1 | grep -q 'test result: ok' && ! cargo test --release 2>&1 | grep -q FAILED"),
      ("check",f"cargo run --release -q -- check {LI}"),
      ("nullcheck",f"cargo run --release -q -- nullcheck {LI}")]
# simpler: tests exit code via cargo test
cmds=[("unit tests","cargo test --release -q"),("check",f"cargo run --release -q -- check {LI}"),("nullcheck",f"cargo run --release -q -- nullcheck {LI}")]
base=run(cmds)
print("baseline fails at:",base); 
if base: sys.exit(1)
try:
  for name,old,new,idx in M:
    if ONLY and not any(o in name for o in ONLY): continue
    if orig.count(old)<=idx: print(f"SKIP (pattern not found): {name}"); continue
    parts=orig.split(old)
    mutated=old.join(parts[:idx+1])+new+old.join(parts[idx+1:])
    open(F,"w").write(mutated)
    caught=run(cmds)
    print(f"{'CAUGHT by '+caught if caught else 'SURVIVED'}: {name}",flush=True)
finally:
  open(F,"w").write(orig)
