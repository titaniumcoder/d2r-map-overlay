"""Probe: dump the live player's skill list (unit+0x100 -> first skill, chain +0x08,
skills.txt id at *(+0x00), hard points at +0x38). Read-only.

Usage: sudo python3 -I research/skillprobe.py <pid> <unit table addr hex> [skills offset hex]
"""
import struct,sys
pid=int(sys.argv[1]); UT=int(sys.argv[2],16); SK=int(sys.argv[3],16) if len(sys.argv)>3 else 0x100
f=open(f"/proc/{pid}/mem","rb")
def rd(a,n):
    f.seek(a); return f.read(n)
def u16(a): return struct.unpack("<H",rd(a,2))[0]
def u32(a): return struct.unpack("<I",rd(a,4))[0]
def u64(a): return struct.unpack("<Q",rd(a,8))[0]
def units(t):
    out=[]
    for s in range(128):
        p=u64(UT+(t*128+s)*8); n=0
        while p and n<5000:
            out.append(p); p=u64(p+0x158); n+=1
    return out
for a in units(0):
    if not u64(a+0x20): continue
    print(f"PLAYER {a:#x} cls={u32(a+4)} id={u32(a+8):#x}")
    sk=u64(a+SK); print(f" skills@{sk:#x}")
    for o in range(0,0x40,8): print(f"  +{o:#x}: {u64(sk+o):#x}")
    s=u64(sk); n=0; tot=0
    while s and n<200:
        rec=u64(s); sid=u16(rec)
        raw=rd(s,0x60)
        dw=struct.unpack("<24I",raw)
        print(f" skill {s:#x} id={sid:4d} rec={rec:#x} next={u64(s+8):#x} " + " ".join(f"{i*4:02x}:{v:x}" for i,v in enumerate(dw) if i>=4))
        s=u64(s+8); n+=1
