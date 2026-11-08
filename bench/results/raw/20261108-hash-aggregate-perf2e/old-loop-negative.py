from pathlib import Path
import subprocess,os
p=Path('crates/nata-exec/src/aggregate.rs'); s=p.read_text()
old='''                sink_calls += deliver_output(&projected, &select, &mut sink)?;
                output += select.len() as u64;'''
new='''                for i in select {
                    sink(&projected.slice(i, 1))?;
                    sink_calls += 1;
                    output += 1;
                }'''
assert s.count(old)==1
try:
    p.write_text(s.replace(old,new))
    env=dict(os.environ,RUST_TEST_THREADS='1')
    result=subprocess.run(['cargo','test','--locked','--release','-j','4','-p','nata-exec','--features','nata-parquet/default','--test','aggregate','output_batches_gather_having_survivors_and_apply_offset_limit'],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
    Path('/tmp/perf2e-old-loop-negative.log').write_text(result.stdout)
    assert result.returncode!=0 and 'sink must receive batches' in result.stdout, result.stdout
    print('old one-row output loop: CAUGHT by batch-delivery regression')
finally:
    p.write_text(s)
