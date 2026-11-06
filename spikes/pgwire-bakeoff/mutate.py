from pathlib import Path
import subprocess
root=Path('/home/dev/src/nata');p=root/'spikes/pgwire-bakeoff/src/codec.rs';orig=p.read_text()
# Removing Cursor::take's only length guard is a compiling known-bad decoder.
needle='if n > self.0.len() {\n            return Err("truncated".into());\n        }'
assert needle in orig
try:
 p.write_text(orig.replace(needle,''))
 subprocess.run(['cargo','build','--manifest-path','spikes/pgwire-bakeoff/Cargo.toml'],cwd=root,check=True,capture_output=True)
 r=subprocess.run(['timeout','20s','spikes/pgwire-bakeoff/target/debug/pgwire-bakeoff','fuzz'],cwd=root,capture_output=True,text=True)
 print(r.stdout);print('hand guard mutant exit',r.returncode);assert r.returncode==101
finally:
 p.write_text(orig)
 subprocess.run(['cargo','build','--manifest-path','spikes/pgwire-bakeoff/Cargo.toml'],cwd=root,check=True,capture_output=True)
