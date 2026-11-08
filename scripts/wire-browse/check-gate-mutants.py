#!/usr/bin/env python3
"""Focused resource acceptance mutants; successful bad gate is MISSED."""
import os
from pathlib import Path
import subprocess
import sys
p=Path(__file__).with_name('test_resource_gates.py')
subprocess.run([sys.executable,str(p)],check=True)
for old,new in [('return ratio <= 1.20','return True'),('return peak <= ceiling','return True'),('rss <= idle + 64*MiB','rss <= idle + 65*MiB'),('fds == idle_fds','fds >= idle_fds'),('sessions*(16*MiB+512*1024)','sessions*(17*MiB+512*1024)')]:
    r=subprocess.run([sys.executable,str(p)],env=dict(os.environ,RESOURCE_MUTANT=old+'|||'+new),capture_output=True,text=True)
    assert r.returncode==1 and 'FAIL:' in r.stderr, ('MISSED or INVALID',old,r.stderr)
    print('CAUGHT',old)

p=Path(__file__).with_name('test_p16.py')
subprocess.run([sys.executable,str(p)],check=True)
for old,new in [('<=1.10','<=2.0'),('<=1.20','<=2.0'),("reference['twin_noise']>.05","reference['twin_noise']>.5")]:
    r=subprocess.run([sys.executable,str(p)],env=dict(os.environ,P16_MUTANT=old+'|||'+new),capture_output=True,text=True)
    assert r.returncode==1 and 'FAIL:' in r.stderr, ('MISSED or INVALID',old,r.stderr)
    print('CAUGHT',old)
