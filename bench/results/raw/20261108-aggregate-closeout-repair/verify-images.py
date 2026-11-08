"""Verify runtime-image identity across test-only rebuilds (trusted ELF64 files)."""
import hashlib
import json
from pathlib import Path
import struct
import sys

a, b = (Path(p).read_bytes() for p in sys.argv[1:])
assert len(a) == len(b)
assert a[:6] == b[:6] == b'\x7fELF\x02\x01'
h = struct.unpack_from('<16sHHIQQQIHHHHHH', a)
assert h == struct.unpack_from('<16sHHIQQQIHHHHHH', b)
shoff, shsize, shnum, shstr = h[6], h[11], h[12], h[13]
sections = [struct.unpack_from('<IIQQQQIIQQ', a, shoff+i*shsize) for i in range(shnum)]
strings = sections[shstr]
names = a[strings[4]:strings[4]+strings[5]]
differences = []
normalized_a, normalized_b = bytearray(a), bytearray(b)
for sh in sections:
    name = names[sh[0]:].split(b'\0')[0].decode()
    off, size = sh[4], sh[5]
    if sh[1] == 8:  # SHT_NOBITS
        continue
    if name == '.text':
        text_sha256 = hashlib.sha256(a[off:off+size]).hexdigest()
    if a[off:off+size] != b[off:off+size]:
        differences.append(dict(section=name, allocated=bool(sh[2]&2),
                                bytes_different=sum(x != y for x, y in zip(a[off:off+size], b[off:off+size]))))
        if sh[2]&2:  # SHF_ALLOC
            assert name == '.note.gnu.build-id', 'allocated runtime section differs'
            assert a[off:off+16] == b[off:off+16] == bytes.fromhex('040000001400000003000000474e5500')
            normalized_a[off+16:off+size] = bytes(size-16)
            normalized_b[off+16:off+size] = bytes(size-16)
loads = []
for i in range(h[10]):
    ph = struct.unpack_from('<IIQQQQQQ', a, h[5]+i*h[9])
    assert ph == struct.unpack_from('<IIQQQQQQ', b, h[5]+i*h[9])
    if ph[0] == 1:  # PT_LOAD: include padding and ELF/program headers too
        off, size = ph[2], ph[5]
        assert normalized_a[off:off+size] == normalized_b[off:off+size], 'runtime segment differs'
        loads.append(dict(offset=off, size=size,
                          normalized_sha256=hashlib.sha256(normalized_a[off:off+size]).hexdigest()))
print(json.dumps(dict(
    difference='Every PT_LOAD byte is identical except the GNU build-id. Executable .text and runtime data are byte-identical. Remaining changes are non-allocated symbol/string tables.',
    measured_sha256=hashlib.sha256(a).hexdigest(), checked_sha256=hashlib.sha256(b).hexdigest(),
    elf_size=len(a), differing_sections=differences, normalized_load_segments=loads,
    text_sha256=text_sha256), indent=2))
