#!/usr/bin/env python3
"""SkyTrak session handshake probe, per docs/skytrak-protocol/session.md.
UDP connection-confirm -> TCP 5024 handshake -> SYS_CONFIG/CAM_CONFIG -> ARM.
No ball is hit; this validates the handshake and status stream only."""
import socket, struct, sys, time, binascii

BOX_IP = sys.argv[1] if len(sys.argv) > 1 else "192.168.4.61"
BOX_NAME = sys.argv[2] if len(sys.argv) > 2 else "SKYTRAK_C47F51902EE3"
UDP_PORT, TCP_PORT = 5023, 5024
RUN_SECONDS = 20

def crc32_mpeg2_swapped(data: bytes) -> int:
    assert len(data) % 4 == 0
    swapped = bytearray()
    for i in range(0, len(data), 4):
        w = data[i:i+4]
        swapped += bytes([w[3], w[2], w[1], w[0]])
    crc = 0xFFFFFFFF
    for b in swapped:
        crc = ((crc << 8) & 0xFFFFFFFF) ^ TABLE[((crc >> 24) ^ b) & 0xFF]
    return crc

TABLE = []
for i in range(256):
    c = i << 24
    for _ in range(8):
        c = ((c << 1) ^ 0x04C11DB7) & 0xFFFFFFFF if (c & 0x80000000) else (c << 1) & 0xFFFFFFFF
    TABLE.append(c)

def pkt12(magic: int) -> bytes:
    body = struct.pack("<II", magic, 0x0C)
    crc = crc32_mpeg2_swapped(body)
    return body + struct.pack("<I", crc)

def sys_config(initial: bool, handed=0, shot_mode=1, ref_laser=0) -> bytes:
    buf = bytearray(0x6C)
    struct.pack_into("<II", buf, 0x00, 0xAAAABBBB, 0x6C)
    struct.pack_into("<i", buf, 0x14, handed)
    struct.pack_into("<i", buf, 0x44, 2)          # WiFi/Ethernet
    struct.pack_into("<i", buf, 0x4C, ref_laser)
    struct.pack_into("<i", buf, 0x54, shot_mode)  # 1 normal
    struct.pack_into("<i", buf, 0x5C, 1 if initial else 0)
    struct.pack_into("<f", buf, 0x64, 2.0)
    crc = crc32_mpeg2_swapped(bytes(buf[:0x68]))
    struct.pack_into("<I", buf, 0x68, crc)
    return bytes(buf)

def cam_config(handed=0) -> bytes:
    buf = bytearray(0x84)
    struct.pack_into("<II", buf, 0x00, 0xAAAACCCC, 0x84)
    struct.pack_into("<i", buf, 0x08, 3)
    struct.pack_into("<i", buf, 0x0C, 0)
    struct.pack_into("<i", buf, 0x10, 1)
    struct.pack_into("<ii", buf, 0x20, 1, 0xFA)
    struct.pack_into("<ii", buf, 0x28, 1, 0xFA)
    struct.pack_into("<ii", buf, 0x30, 0x2D, 0x2D)
    struct.pack_into("<iiii", buf, 0x38, 0x80, 0x80, 0x80, 0x80)
    if handed == 0:
        struct.pack_into("<ii", buf, 0x48, 0x14, 0x16)
        struct.pack_into("<ii", buf, 0x50, 0x16, 0x14)
    else:
        struct.pack_into("<ii", buf, 0x48, 0x16, 0x14)
        struct.pack_into("<ii", buf, 0x50, 0x14, 0x16)
    struct.pack_into("<i", buf, 0x58, 1)
    struct.pack_into("<ii", buf, 0x5C, 1, 0xFA)
    crc = crc32_mpeg2_swapped(bytes(buf[:0x80]))
    struct.pack_into("<I", buf, 0x80, crc)
    return bytes(buf)

STATUS_CODES = {0:"RESP_OK",1:"WAIT_FOR_SYS_CONFIG_PACKET",2:"WAIT_FOR_CAM_CONFIG_PACKET",
                3:"WAIT_FOR_HOST_READY_PACKET",4:"SYS_CONFIG_PACKET_RECEIVED",
                5:"CAM_CONFIG_PACKET_RECEIVED",6:"READY_FOR_MANUFACTURING_COMMANDS",7:"FLASH_MEMORY_INTEGRITY_SUCCESS"}

def log(msg): print(f"[{time.time():.3f}] {msg}", flush=True)

# --- UDP connection confirm ---
u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
u.settimeout(4.0)
confirm = pkt12(0xAAAAFFFF)
log(f"UDP connection-confirm -> {BOX_IP}:{UDP_PORT}  {confirm.hex()}")
ok = False
for attempt in range(5):
    u.sendto(confirm, (BOX_IP, UDP_PORT))
    try:
        data, addr = u.recvfrom(0x4000)
    except socket.timeout:
        log(f"  attempt {attempt}: timeout")
        continue
    if len(data) >= 0x74 and struct.unpack_from("<I", data, 0)[0] == 0xBBBBBBBB:
        name_len = struct.unpack_from("<I", data, 0x70)[0]
        name = data[0x50:0x50+min(name_len,32)].decode("ascii", "replace")
        log(f"  reply from {addr}: name={name!r} (want {BOX_NAME!r})")
        if name == BOX_NAME:
            ok = True
            break
    else:
        log(f"  unexpected reply: {data[:16].hex()}")
u.close()
if not ok:
    log("Could not confirm box over UDP; aborting.")
    sys.exit(1)

time.sleep(1.0)  # per doc: Sleep(1000) before TCP connect

# --- TCP session ---
t = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
t.settimeout(6.0)
log(f"TCP connect -> {BOX_IP}:{TCP_PORT}")
t.connect((BOX_IP, TCP_PORT))
log("TCP connected")

state = "connected"
first_status = True
armed = False
buf = b""
end = time.time() + RUN_SECONDS
captures = []

def send(t, name, data):
    log(f"SEND {name} ({len(data)}B): {data.hex()}")
    t.sendall(data)

while time.time() < end and not armed:
    try:
        chunk = t.recv(65536)
    except socket.timeout:
        log("recv timeout (6s) -- box went quiet")
        break
    if not chunk:
        log("TCP closed by box")
        break
    buf += chunk
    while len(buf) >= 8:
        magic, length = struct.unpack_from("<II", buf, 0)
        if length < 8 or length > 0x20000 or len(buf) < length:
            break
        pkt = buf[:length]
        buf = buf[length:]
        captures.append((time.time(), pkt))
        if magic == 0xBBBBBBBB:
            code = struct.unpack_from("<i", pkt, 0x28)[0]
            batt = struct.unpack_from("<f", pkt, 0x1C)[0]
            rssi = pkt[0x48:0x50].split(b"\x00")[0].decode("ascii","replace")
            name = pkt[0x50:0x70].split(b"\x00")[0].decode("ascii","replace")
            mode = struct.unpack_from("<i", pkt, 0x88)[0]
            log(f"RECV STATUS code={code} ({STATUS_CODES.get(code,'?')}) battery={batt} rssi={rssi} mode={mode} name={name!r}")
            if first_status:
                first_status = False
                send(t, "DISARM", pkt12(0xBBBBFFFF))
                send(t, "SYS_CONFIG(initial)", sys_config(initial=True))
            elif code == 1:
                send(t, "SYS_CONFIG", sys_config(initial=False))
            elif code == 2:
                send(t, "CAM_CONFIG", cam_config())
            elif code == 3:
                arm = pkt12(0xBBBBCCCC)
                send(t, "ARM/HOST_READY", arm)
            elif code == 0:
                log("Box reports RESP_OK -- likely armed.")
                armed = True
        elif magic == 0xAAAAAAAA:
            fw = struct.unpack_from("<f", pkt, 0x14)[0] if len(pkt) >= 0x18 else None
            serial = pkt[0x164:0x170].split(b"\x00")[0] if len(pkt) >= 0x170 else b""
            log(f"RECV PARAMS len={length} fw={fw} serial={serial!r}")
            send(t, "SYS_CONFIG(post-params)", sys_config(initial=False))
            send(t, "CAM_CONFIG(post-params)", cam_config())
        else:
            log(f"RECV magic=0x{magic:08X} len={length} (unhandled here) {pkt[:32].hex()}")

log(f"Session ended. armed={armed}. Captured {len(captures)} packets.")
with open("captures/session_capture.bin", "wb") as f:
    for ts, pkt in captures:
        f.write(struct.pack("<Qd", len(pkt), ts))
        f.write(pkt)
log("Raw capture written to captures/session_capture.bin")

if armed:
    log("Waiting a moment, then disarming and disconnecting cleanly...")
    time.sleep(2)
    try:
        send(t, "DISARM", pkt12(0xBBBBFFFF))
        time.sleep(0.3)
    except Exception as e:
        log(f"disarm send failed: {e}")
t.close()
log("done")
