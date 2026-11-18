r"""Minimal MPQ v1 reader for the classic Diablo II archives, with PKWARE DCL explode (port of
zlib's contrib/blast/blast.c). Used 2026-11-18 to pull data\global\ui\MENU\invps.DC6 (the tomb symbols).

Usage: python3 -I research/mpq.py <out file> <archive>... -- <path inside archive>
The first archive that holds the path wins (pass Patch_D2.mpq first).
"""
import struct
import sys
import zlib


def crypt_table():
    seed = 0x00100001
    t = [0] * 0x500
    for i in range(0x100):
        idx = i
        for _ in range(5):
            seed = (seed * 125 + 3) % 0x2AAAAB
            hi = (seed & 0xFFFF) << 16
            seed = (seed * 125 + 3) % 0x2AAAAB
            t[idx] = hi | (seed & 0xFFFF)
            idx += 0x100
    return t


CT = crypt_table()


def hash_str(s, kind):
    s1, s2 = 0x7FED7FED, 0xEEEEEEEE
    for ch in s.upper().encode():
        s1 = (CT[kind * 0x100 + ch] ^ (s1 + s2)) & 0xFFFFFFFF
        s2 = (ch + s1 + s2 + (s2 << 5) + 3) & 0xFFFFFFFF
    return s1


def decrypt(data, key):
    out = bytearray()
    s2 = 0xEEEEEEEE
    n = len(data) // 4
    for i in range(n):
        s2 = (s2 + CT[0x400 + (key & 0xFF)]) & 0xFFFFFFFF
        v = struct.unpack_from("<I", data, i * 4)[0]
        v = (v ^ (key + s2)) & 0xFFFFFFFF
        key = ((((~key) << 0x15) + 0x11111111) | (key >> 0x0B)) & 0xFFFFFFFF
        s2 = (v + s2 + (s2 << 5) + 3) & 0xFFFFFFFF
        out += struct.pack("<I", v)
    return bytes(out) + data[n * 4:]


# --- blast.c port -------------------------------------------------------------------------
LITLEN = bytes([11, 124, 8, 7, 28, 7, 188, 13, 76, 4, 10, 8, 12, 10, 12, 10, 8, 23, 8, 9, 7, 6, 7, 8, 7, 6, 55, 8, 23, 24, 12, 11, 7, 9, 11, 12, 6, 7, 22, 5, 7, 24, 6, 11, 9, 6, 7, 22, 7, 11, 38, 7, 9, 8, 25, 11, 8, 11, 9, 12, 8, 12, 5, 38, 5, 38, 5, 11, 7, 5, 6, 21, 6, 10, 53, 8, 7, 24, 10, 27, 44, 253, 253, 253, 252, 252, 252, 13, 12, 45, 12, 45, 12, 61, 12, 45, 44, 173])
LENLEN = bytes([2, 35, 36, 53, 38, 23])
DISTLEN = bytes([2, 20, 53, 230, 247, 151, 248])
BASE = [3, 2, 4, 5, 6, 7, 8, 9, 10, 12, 16, 24, 40, 72, 136, 264]
EXTRA = [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]


def construct(rep):
    lengths = []
    for r in rep:
        lengths += [r & 15] * ((r >> 4) + 1)
    count = [0] * 17
    for l in lengths:
        count[l] += 1
    offs = [0] * 18
    for l in range(1, 16):
        offs[l + 1] = offs[l] + count[l]
    symbol = [0] * len(lengths)
    for s, l in enumerate(lengths):
        if l:
            symbol[offs[l]] = s
            offs[l] += 1
    return count, symbol


LIT, LEN, DIST = construct(LITLEN), construct(LENLEN), construct(DISTLEN)


def explode(src):
    pos = 0
    bitbuf = 0
    bitcnt = 0

    def bits(need):
        nonlocal pos, bitbuf, bitcnt
        val = bitbuf
        while bitcnt < need:
            val |= src[pos] << bitcnt
            pos += 1
            bitcnt += 8
        bitbuf = val >> need
        bitcnt -= need
        return val & ((1 << need) - 1)

    def decode(h):
        count, symbol = h
        code = first = index = 0
        for l in range(1, 17):
            code |= bits(1) ^ 1
            c = count[l] if l < len(count) else 0
            if code < first + c:
                return symbol[index + (code - first)]
            index += c
            first += c
            first <<= 1
            code <<= 1
        raise ValueError("bad code")

    lit = bits(8)
    dictbits = bits(8)
    out = bytearray()
    while True:
        if bits(1):
            sym = decode(LEN)
            length = BASE[sym] + bits(EXTRA[sym])
            if length == 519:
                break
            sym = 2 if length == 2 else dictbits
            dist = (decode(DIST) << sym) + bits(sym) + 1
            for _ in range(length):
                out.append(out[-dist])
        else:
            out.append(decode(LIT) if lit else bits(8))
    return bytes(out)


# --- archive ------------------------------------------------------------------------------
def read_file(archive, name):
    with open(archive, "rb") as f:
        data = f.read()
    off = data.find(b"MPQ\x1a")
    magic, hsize, asize, ver, ssh, ht, bt, hn, bn = struct.unpack_from("<4sIIHHIIII", data, off)
    sector = 512 << ssh
    htab = decrypt(data[off + ht: off + ht + hn * 16], hash_str("(hash table)", 3))
    btab = decrypt(data[off + bt: off + bt + bn * 16], hash_str("(block table)", 3))
    a, b = hash_str(name, 1), hash_str(name, 2)
    i = hash_str(name, 0) % hn
    for _ in range(hn):
        ha, hb, loc, blk = struct.unpack_from("<IIII", htab, i * 16)
        if blk == 0xFFFFFFFF:
            return None
        if ha == a and hb == b and blk < bn:
            break
        i = (i + 1) % hn
    else:
        return None
    fpos, csize, fsize, flags = struct.unpack_from("<IIII", btab, blk * 16)
    raw = data[off + fpos: off + fpos + csize]
    key = None
    if flags & 0x10000:
        base = name.replace("/", "\\").split("\\")[-1]
        key = hash_str(base, 3)
        if flags & 0x20000:
            key = ((key + fpos) ^ fsize) & 0xFFFFFFFF
    if flags & 0x1000000:  # single unit
        return raw
    n = (fsize + sector - 1) // sector
    if flags & 0x300:
        table = raw[: (n + 1) * 4]
        if key is not None:
            table = decrypt(table, (key - 1) & 0xFFFFFFFF)
        offs = struct.unpack(f"<{n + 1}I", table)
    else:
        offs = [min(k * sector, csize) for k in range(n + 1)]
    out = bytearray()
    for k in range(n):
        chunk = raw[offs[k]: offs[k + 1]]
        if key is not None:
            chunk = decrypt(chunk, (key + k) & 0xFFFFFFFF)
        want = min(sector, fsize - k * sector)
        if len(chunk) == want:
            out += chunk
        elif flags & 0x100:
            out += explode(chunk)
        elif flags & 0x200:
            m = chunk[0]
            body = chunk[1:]
            if m == 0x08:
                out += explode(body)
            elif m == 0x02:
                out += zlib.decompress(body)
            else:
                raise ValueError(f"compression {m:#x} unsupported")
    return bytes(out)


if __name__ == "__main__":
    out_path = sys.argv[1]
    sep = sys.argv.index("--")
    archives, name = sys.argv[2:sep], sys.argv[sep + 1]
    for arc in archives:
        got = read_file(arc, name)
        if got is not None:
            open(out_path, "wb").write(got)
            print(f"{name}: {len(got)} bytes from {arc}")
            break
    else:
        sys.exit(f"{name} not found")
