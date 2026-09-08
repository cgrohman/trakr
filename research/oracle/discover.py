#!/usr/bin/env python3
"""Standalone SkyTrak discovery probe, per docs/skytrak-protocol/discovery.md.
Sends the 12-byte {0xAAAAEEEE, 0x0C, 0} request to UDP 5023 broadcast and
prints any 0xBBBBBBBB reply, decoding the fields the vendor SDK reads."""
import socket, struct, sys, time

PORT = 5023
REQ = struct.pack("<III", 0xAAAAEEEE, 0x0C, 0)
broadcasts = sys.argv[1:] or ["255.255.255.255"]

s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("0.0.0.0", 0))
s.settimeout(0.5)

print(f"local port: {s.getsockname()[1]}")
end = time.time() + 6
seen = set()
for bc in broadcasts:
    s.sendto(REQ, (bc, PORT))
    print(f"sent discovery request -> {bc}:{PORT}")

while time.time() < end:
    try:
        data, addr = s.recvfrom(1024)
    except socket.timeout:
        for bc in broadcasts:
            s.sendto(REQ, (bc, PORT))
        continue
    if addr[0] in seen:
        continue
    magic = struct.unpack_from("<I", data, 0)[0] if len(data) >= 4 else None
    print(f"\nreply from {addr[0]}:{addr[1]}  ({len(data)} bytes)")
    print(f"  magic @0x00 = 0x{magic:08X}" if magic is not None else "  (too short)")
    if magic == 0xBBBBBBBB:
        seen.add(addr[0])
        name_len = struct.unpack_from("<I", data, 0x70)[0] if len(data) >= 0x74 else None
        name_raw = data[0x50:0x70] if len(data) >= 0x70 else b""
        mode = struct.unpack_from("<i", data, 0x88)[0] if len(data) >= 0x8C else None
        print(f"  name_len @0x70 = {name_len}")
        print(f"  name @0x50 (32B) = {name_raw!r}")
        if name_len and 0 < name_len <= 32:
            print(f"  name (decoded)   = {name_raw[:name_len].decode('ascii', 'replace')!r}")
        print(f"  mode @0x88 = {mode}")
        print(f"  full hex: {data.hex()}")
    else:
        print(f"  full hex: {data.hex()}")
        print("  (does not match documented 0xBBBBBBBB header -- may be another device)")

if not seen:
    print("\nNo SkyTrak reply received on this network in 6s.")
