# SkyTrak device link: packet framing, CRC and encryption (Rapsodo RIPE SDK, PlatformLib.dll x64)

Source: Ghidra decompile of `PlatformLib.dll` under
`research/ghidra/out/PlatformLib.dll/` (`fn/<name>.c`, `all_functions.c`,
`strings_xref.txt`). Every claim carries the function address it was read
from and a confidence tag: **confirmed** (constant or control flow read
directly), **likely** (inferred from adjacent code / naming), **guess**.

Companion document: `discovery.md` (UDP discovery flow). This document covers
what goes on the wire after a box has been found: the byte framing shared by
UDP/TCP/USB, the CRC-32 trailer on host-to-box packets, the AES-128-ECB layer
on two box-to-host packet types, and how `RIPESendBoxCommand` turns the
command enum into packets.

Conventions: `G` = the SDK global context (`PTR_DAT_180419908`);
`parser` = the 0x12CE20-byte RX parser state block at `G+0x28`;
`boxinfo` = the box data block at `G+0x40`. All multi-byte integers on the wire
are **little-endian** (the packet templates are written with plain x64 32-bit
stores and the RX shift register assembles bytes LSB-first, see 1.2).

---

## 0. TL;DR for the Rust implementation

| Item | Value | Confidence |
|---|---|---|
| Frame | `u32 magic`, `u32 total_len` (includes the 8-byte header), payload `total_len-8` bytes | confirmed |
| Endianness | little-endian everywhere | confirmed |
| Host->box trailer | last 4 bytes of every command packet = CRC-32 | confirmed |
| CRC algorithm | poly `0x04C11DB7`, init `0xFFFFFFFF`, no reflection, no final XOR, fed 32-bit words **byte-swapped** (i.e. the STM32 hardware CRC unit emulated in software) | confirmed (algorithm), likely (covered range = everything before the CRC word) |
| Encryption | AES-128-ECB, no IV, no padding, applied only to the payload of box->host `0xCCCCCCCC` (shot record) and `0xDDDDDDDD` (image) packets | confirmed |
| Key | constant, picked by box firmware version float: `<1.195` all-zero key; `1.2..2.0` = `00 01 02 .. 0F`; `>=2.0` = `42 53 89 18 77 42 87 05 81 58 55 36 86 58 61 08` | confirmed |
| Host->box packets are **not** encrypted | | confirmed |
| Ports | UDP 5023 (discovery broadcast + connection confirm), TCP 5024 (data) | confirmed |
| USB | libusb bulk OUT ep 0x01 / IN ep 0x81, same framing, VID 0x4003 PID 0x0001 (as decompiled) | confirmed / likely |

---

## 1. Where the device link is built and parsed

### 1.1 Function map

| Role | Function | Notes |
|---|---|---|
| Packet templates allocated | `RIPEInit @ 180012d90` (all_functions.c 11130-11260) | one malloc per host->box packet type, magic + len written once |
| Host->box send + CRC | `FUN_180019160 @ 180019160` | picks CRC slot from magic, calls CRC, then `send()` (TCP, `Ordinal_19`) or `FUN_180019030` (USB bulk) |
| CRC table generator | `FUN_1800196a0 @ 1800196a0` | fills `DAT_180470220[256]` lazily |
| CRC compute | `FUN_180019600 @ 180019600` | byte-swap words via `FUN_1800194b0`, then table CRC |
| RX byte parser (state machine) | `FUN_18000c180 @ 18000c180` | fed one buffer at a time by comm threads; owns AES decrypt |
| RX event dispatch ("PSR") | `FUN_1800165d0 @ 1800165d0` | callback installed at `parser+8` by RIPEInit (`*(code **)(lVar7 + 8) = FUN_1800165d0`) |
| WiFi comm thread | `FUN_180017ea0 @ 180017ea0` | UDP confirm handshake then TCP stream -> parser |
| USB comm thread | `FUN_180018890 @ 180018890` | identical logic over libusb |
| Discovery thread | `FUN_180019f50 @ 180019f50` | broadcast `0xAAAAEEEE`, expects `0xBBBBBBBB` ("header signature") |
| USB discovery | `FUN_1800122c0 @ 1800122c0` | writes the 12-byte probe, reads 0x400, checks `0xBBBBBBBB` |
| Manager thread ("MGR") | `FUN_18001eb20 @ 18001eb20` | pops the `G+0x310` message queue; internal events 0x30.. and user commands 0..0x19 |
| User command -> packet fields | `FUN_180016f80 @ 180016f80` | the `RIPESendBoxCommand` switch |
| Packet group sender | `FUN_180016da0 @ 180016da0` | "send sys config", "send cam config", ... |
| AES context init (device link) | `FUN_18000cf10 @ 18000cf10` | picks key by FW version, calls key schedule |
| AES key schedule (AES-NI) | `FUN_180011750 @ 180011750` -> `FUN_180010d30 @ 180010d30` (+`aesimc` for decrypt keys); SW fallback `FUN_180010100` when CPUID.1:ECX bit 25 clear |
| AES block decrypt | `FUN_180010bc0 @ 180010bc0` (SW fallback `FUN_18000e340`) |
| AES block encrypt | `FUN_180010a60 @ 180010a60` (SW fallback `FUN_18000d320`) — only used by the telemetry path |
| Encryption verification | `FUN_180019310 @ 180019310` | decrypts a 16-byte challenge block from the box static-data packet and compares |
| Static-data packet handler | `FUN_180016110 @ 180016110` | extracts embedded `0xBBBBDDDD` sub-record, then initialises the parser's AES ctx |
| FW upgrade sender | `FUN_18001bc90 @ 18001bc90` / chunker `FUN_18001bb60 @ 18001bb60` | `0xAAAADDDD` packets |

The telemetry path (`FUN_18001dd40`, `FUN_18001e480`, `FUN_18001e910`,
strings `RAPSODOPACKETKEY` / `FOdOspAr00000000`, file `./Skytrak.dat`) shares
only the AES primitives; it never touches the device sockets. It is not
described further here.

### 1.2 Framing (confirmed, `FUN_18000c180`)

The RX parser is byte-oriented. In state 0 it shifts each byte into a 32-bit
little-endian window and looks for a magic:

```c
uVar1 = (uint)*(byte *)(lVar7 + param_2) << 0x18 | uVar1 >> 8;
if (uVar1 == 0xbbbbbbbb || uVar1 == 0xcccccccc || uVar1 == 0xdddddddd ||
    uVar1 == 0xeeeeeeee || uVar1 == 0xffffaaaa || uVar1 == 0xaaaaaaaa ||
    uVar1 == 0xffffbbbb || uVar1 == 0xbbbbaabb) { DAT_18046a500 = 1; DAT_18046a508 = 0; DAT_18046b1f8 = uVar1; }
else if (DAT_18046a500 != 0 && DAT_18046a508 == 4) { _DAT_18046b200 = uVar1; ... uVar4 = DAT_18046b1f8; }
```

So after the magic, the next 4 bytes (LE) are the total length. The parser then
writes magic and length into the destination buffer, sets the running counter
`DAT_18046b204 = 8`, and enters state 5 where it copies payload bytes until

```c
uVar1 = DAT_18046b204 + 1;
if (uVar1 == DAT_18046b200) { ...packet complete... *param_1 = 0; }
else if (DAT_18046b200 < uVar1) { do { } while(true); }   /* deliberate hang on overrun */
```

Hence **`total_len` counts the 8 header bytes**; payload = `total_len - 8`.
Host->box templates use the same convention (e.g. a 12-byte packet has
`len = 0xc` and a 4-byte payload). Because the magic is assembled LSB-first,
the bytes on the wire for magic `0xFFFFAAAA` are `AA AA FF FF`.

Common header layout (both directions):

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0x00 | 4 | magic / packet type | one of the values in 1.3 / 1.4 |
| 0x04 | 4 | total length | includes header; LE |
| 0x08 | n | payload | `total_len - 8` bytes |
| end-4 | 4 | CRC-32 | host->box only, see section 2 |

There is no version, sequence, node-id or opcode field in the common header;
the magic **is** the opcode. A sequence number exists only inside the
`0xAAAADDDD` firmware-chunk payload (1.4).

### 1.3 Box -> host packet types (confirmed, `FUN_18000c180` + `FUN_1800165d0`)

| Magic | Meaning | Total len | Destination in `parser` | PSR events (start/end) | Encrypted? |
|---|---|---|---|---|---|
| `0xBBBBBBBB` | box status / discovery + connection-confirm response | 0xA0 (PSR case 3 copies 0xa0) | `+0x1200A2` | 2 / 3 | no |
| `0xCCCCCCCC` | shot record ("header packet") | variable, `+0x30` printed as size | `+0x2C` | 0 / 1 | **yes** |
| `0xDDDDDDDD` | camera image n | variable | `+0x54 + n*0x6001A` | 6 / 7 | **yes** |
| `0xEEEEEEEE` | trigger detected | variable (`+0x120146`) | `+0x120142` | 4 / 5 | no |
| `0xBBBBAABB` | WiFi network scan list | 0x2010 | `+0x12A2DA` | 8 / 9 | no |
| `0xFFFFAAAA` | user persistent data (read-back) | 0x40C | `+0x12C2EA` | 10 / 11 | no |
| `0xAAAAAAAA` | box static data ("box params"/sys-config response) | up to 0x4E0 copied | `+0x12C6F6` | 12 / 13 | no |
| `0xFFFFBBBB` | recognised as a start marker but has no dispatch branch | — | — | — | no |

Only `0xCCCCCCCC` and `0xDDDDDDDD` are decrypted; the exclusion list in state 5
is explicit:

```c
if ((param_1[0x4b383] == 0) || (DAT_18046b208 == 0xaaaaaaaa) || (DAT_18046b208 == 0xbbbbbbbb) ||
    (DAT_18046b208 == 0xeeeeeeee) || (DAT_18046b208 == 0xffffaaaa) ||
    (DAT_18046b208 == 0xffffbbbb) || (DAT_18046b208 == 0xbbbbaabb)) { /* plain copy */ }
else { /* accumulate 16 bytes, FUN_180010bc0(...) decrypt, then copy */ }
```

`param_1[0x4b383]` is the "encryption enabled" flag = `ctx+0x22C` of the AES
context that lives at `parser+0x12CBE0` (`0x4b383*4 = 0x12CE0C = 0x12CBE0 + 0x22C`).

**`0xBBBBBBBB` status packet payload** (offsets from packet start; likely, from
`RIPEGetBoxStatusData @ 1800144d0`, `FUN_1800140e0`, `FUN_18001eb20`,
`FUN_180017ea0`, `FUN_180019f50`):

| Offset | Type | Field |
|---|---|---|
| 0x08,0x0C,0x10 | i32 x3 | accelerometer / tilt (used for "6 axis" calc) |
| 0x1C | i32 | copied to status[2] (battery level, guess) |
| 0x24 | i32 | non-zero -> status[1] = 1 (charging/low-batt flag, guess) |
| 0x28 | i32 | box status / error code (`parser+0x1200CA`; printed via `"MGR:Box status packet(%d)=%s"`) |
| 0x2C | i32 | non-zero -> box is configured/ready (`+0x1200CE`; PSR appends the 0xA0 status block to the saved shot header when set) |
| 0x30 | i32 | non-zero -> shot data pending (`+0x1200D2`) |
| 0x48 | ASCII | numeric string, `atoi()` -> status[6] |
| 0x50 | char[32] | box name (also used in discovery) |
| 0x70 | u32 | box name length (1..31 valid) |
| 0x88 | i32 | connection type: 0 -> 2, 1 -> 1, else 0 (`FUN_180019f50`) |

**`0xAAAAAAAA` static-data payload** (confirmed offsets only): `FUN_180016110`
copies the packet (magic included) to `boxinfo+0x92C` word by word, but when a
word equals `0xBBBBDDDD` it treats `[magic][len]...` as an embedded
manufacturing record (<= 0x3BC bytes), copies it to `boxinfo+0xA50` and skips
it (so offsets after it shrink). Known fields after extraction:
`boxinfo+0x940` = **firmware version float** (packet offset 0x14 if the
sub-record comes later); `boxinfo+0xA24` = 16-byte **encryption challenge
block**; `boxinfo+0xA9C` = camera/board version float (1.04/1.05/…);
`boxinfo+0xADC == 0x18A89` customer-code check (`FUN_1800192b0`).

### 1.4 Host -> box packet types (confirmed, `RIPEInit` lines 11130-11260, `FUN_180019160`)

| Magic | Meaning | Total len | CRC at | Sender |
|---|---|---|---|---|
| `0xAAAAEEEE` | discovery probe (UDP broadcast / USB probe) | 0x0C | none (not routed through `FUN_180019160`) | `FUN_180019f50`, `FUN_1800122c0` |
| `0xAAAAFFFF` | connection confirm | 0x0C | none | `FUN_180017ea0`, `FUN_180018890` |
| `0xAAAABBBB` | system config | 0x6C | 0x68 | `FUN_180016da0` case 1/3/8 |
| `0xAAAACCCC` | camera config | 0x84 | 0x80 | `FUN_180016da0` case 2/3 |
| `0xBBBBCCCC` | ARM | 0x0C | 0x08 | `FUN_180016da0` case 4, MGR 0x38 |
| `0xBBBBFFFF` | DISARM | 0x0C | 0x08 | `FUN_180016da0` case 5, MGR 0x32/0x39 |
| `0xCCCCDDDD` | allocated, never observed sent (host-ready / ack?) | 0x0C | none listed | — |
| `0xBBBBAAAA` | network config | 0xB8 | 0xB4 | `FUN_180016da0` case 6 |
| `0xFFFFAAAA` | user persistent data write (`G+0xB00`) / read-back buffer (`G+0xB08`) | 0x40C | 0x408 | `FUN_180016da0` case 7 |
| `0xBBBBDDDD` | manufacturing config (echo of the extracted sub-record at `boxinfo+0xA50`) | 0x3BC | 0x3B8 | MGR 0x37 |
| `0xAAAADDDD` | firmware chunk | 0x14 + n (+pad) | 0x10 | `FUN_18001bc90` |
| `0xBBBBAABB` | WiFi list buffer (`G+0xAF8`) | 0x2010 | — | RX only, allocated for copy-out |

CRC slot selection, quoted from `FUN_180019160`:

```c
if (iVar3 == -0x55552223)      piVar5 = param_1 + 4;      /* 0xAAAADDDD -> byte 0x10 */
else if (iVar3 == -0x55554445) piVar5 = param_1 + 0x1a;   /* 0xAAAABBBB -> byte 0x68 */
else if (iVar3 == -0x55553334) piVar5 = param_1 + 0x20;   /* 0xAAAACCCC -> byte 0x80 */
else if (iVar3 == -0x44443334) piVar5 = param_1 + 2;      /* 0xBBBBCCCC -> byte 0x08 (also 0xBBBBFFFF? see below) */
else if (iVar3 == -0x44445556) piVar5 = param_1 + 0x2d;   /* 0xBBBBAAAA -> byte 0xB4 */
else if (iVar3 == -0x5556)     piVar5 = param_1 + 0x102;  /* 0xFFFFAAAA -> byte 0x408 */
else if (iVar3 == -0x44442223) { param_1[0x72] = -1; param_1[0x73] = -1; piVar5 = param_1 + 0xee; } /* 0xBBBBDDDD -> byte 0x3B8 */
```

Note `0xBBBBFFFF` (= `-0x44440001`) is **not** in this list, so the DISARM
packet is sent with whatever is in bytes 8..11 (zero from init, or the CRC left
over from the last ARM since the same 12-byte buffer at `G+0xAD0` is reused for
both). Whether the box checks a CRC on DISARM is unknown (open question).

#### System config `0xAAAABBBB` (0x6C bytes) — field semantics

| Offset | Value / meaning | Source |
|---|---|---|
| 0x08..0x13 | zero | init |
| 0x14 | player hand / special mode: 0 = right, 1 = left, 4 = assist-alignment | `FUN_1800173f0`, `FUN_180016f80` case 0xD |
| 0x18 | 1 = firmware upgrade follows | `FUN_18001bc90` |
| 0x1C | firmware image total size | `FUN_18001bc90` |
| 0x20..0x3B | zero | |
| 0x3C | 1 = request user persistent data (GET_USERPERSISTENTDATA) | case 0x13 |
| 0x40 | cleared after send | |
| 0x44 | link type: 1 if USB (`G+0x940 == 2`), 2 if `< 2`, else 0 | `FUN_180016da0` case 1 |
| 0x48 | cleared after send | |
| 0x4C | 1 = enable reference laser | case 0x14 |
| 0x50 | cleared after send | |
| 0x54 | shot mode: 1 = normal, 2 = putting (0 = unchanged) | cases 3/4 |
| 0x58 | PSC JP mode 1/0 | cases 0x15/0x16 |
| 0x5C | 1 on the first config after connect (MGR 0x32) | |
| 0x60 | request code: 10 = WiFi network scan; 6/7/8 from MGR 0x47/0x48/0x49 | case 0x17 |
| 0x64 | `0x40000000` = float 2.0 (host protocol/SDK version) | init |
| 0x68 | CRC-32 | `FUN_180019160` |

#### Camera config `0xAAAACCCC` (0x84 bytes) — defaults from `RIPEInit`

`+0x08=3, +0x0C=0/1 (debug mode), +0x10=1/0 (normal mode), +0x1C=0, +0x20=1, +0x24=0xFA, +0x28=1, +0x2C=0xFA, +0x30=0x2D, +0x34=0x2D, +0x38..0x44=0x80, +0x48/0x4C=0x14 or 0x16, +0x50/0x54=0x16 or 0x14 (swapped by hand), +0x58=1, +0x5C/+0x60 exposure results, +0x80=CRC`. Values at 0x20-0x34 are recomputed by `FUN_1800173f0` from the box's calibration floats.

#### Network config `0xBBBBAAAA` (0xB8 bytes) — from `RIPEBoxSetNetworkConfig @ 1800157d0`

| Offset | Meaning |
|---|---|
| 0x08 | boot-force flag (1 for BOOT_FORCE_NETWORKCONNECT / DIRECTCONNECT) |
| 0x0C | 0 = network connect, 1 = direct connect |
| 0x10 | 1 = node 0 SSID present |
| 0x14 | SSID length (node 0) |
| 0x18.. | SSID bytes (node 0) |
| 0x38 | 1 = node 1 (secured) config present |
| 0x3C / 0x40.. | SSID length / bytes (node 1) |
| 0x60 / 0x64.. | passphrase length / bytes |
| 0x84, 0x88 | security type codes (+0x30 = ASCII digit) |
| 0x8C / 0x90 | 1 = channel present / channel (1..11) |
| 0xB4 | CRC-32 |

#### User persistent data `0xFFFFAAAA` (0x40C bytes)

`+0x04 = 0x40C`, `+0x08..0x407` = 1024 bytes of opaque user data, `+0x408` = CRC.
`RIPEBoxSetUserPersistentData` requires the caller token `-0xa98ac7`
(`0xFF567539`) and stores it at `cfg+0x24`; `RIPESendBoxCommand(0x12)` is
rejected unless that token is present.

#### Firmware chunk `0xAAAADDDD` (`FUN_18001bb60`, confirmed)

| Offset | Meaning |
|---|---|
| 0x00 | `0xAAAADDDD` |
| 0x04 | total len = `0x14 + n + pad` (`n <= 1000`, padded to 4) |
| 0x08 | sequence number (`DAT_18046b1e8`, starts at 0, increments) |
| 0x0C | 1 if this is the last chunk |
| 0x10 | CRC-32 |
| 0x14 | first data byte verbatim, then **delta-encoded**: `out[i] = in[i] - in[i-1]` (byte-wise) |

The upgrade flow: send sys-config with `+0x18=1, +0x1C=size`, then chunks,
waiting on queue `G+0x348` between chunks (12 s first, 6 s after); the box
reports `FW_PKT_SEQUENCE_ERROR` / `FW_PKT_CRC_ERROR` / `FW_PKT_TIMEOUT_ERROR`
through the status packet.

### 1.5 Connection handshake (confirmed, `FUN_180017ea0`)

1. UDP socket bound to local adapter IP (`G+0x9E1`), port 0.
2. `sendto(box_ip, port atoi(G+0x980) = "5023")` the 12-byte `0xAAAAFFFF` packet (`G+0xAE8`).
3. `recvfrom` with 4 s timeout into `DAT_18046c220` (0x4000 bytes). Accept iff
   `DAT_18046c220 == -0x44444445` (`0xBBBBBBBB`) **and** the name at `+0x50`
   (length at `+0x70`) equals the target box name at `G+0x9B0`; otherwise
   log `"COMMT:(UDP)statuspkt resp header error"` and retry.
4. TCP `connect(box_ip, atoi(G+0x990) = "5024")`, then `recv()` into
   `DAT_18046c220` and push every byte into `FUN_18000c180`. Idle timeouts:
   12 s in FW mode (`mode 3`), 24 s while waiting for the WiFi scan list
   (`mode 5`), otherwise any `select()` timeout terminates the link.

Discovery (`FUN_180019f50`) broadcasts `0xAAAAEEEE` (`G+0xAE0`) to UDP 5023
(`*(lVar1 + 0x168) = 0x33323035`) and applies the same `0xBBBBBBBB` /
`+0x50` / `+0x70` / `+0x88` parse ("header signature error" otherwise).
USB (`FUN_1800122c0`) does the same over libusb with `bulk_transfer(ep 0x01,
12 bytes)` then `bulk_transfer(ep 0x81, 0x400)` and matches device descriptor
`idVendor == 0x4003, idProduct == 1, iSerialNumber != 0`.

---

## 2. CRC

### 2.1 Algorithm (confirmed)

Table generator `FUN_1800196a0`:

```c
uVar1 = (uint)uVar3 << 0x18;
do { if ((int)uVar1 < 0) uVar1 = uVar1 * 2 ^ 0x4c11db7; else uVar1 = uVar1 * 2; } while (--lVar2);
*puVar4++ = uVar1;   /* 256 entries at DAT_180470220 */
```

Compute `FUN_180019600(unused, data, nbytes)`:

```c
uVar2 = 0xffffffff;
FUN_1800194b0(param_2, local_fb8, param_3 >> 2);            /* byte-swap each u32 into a 4000-byte temp */
do { bVar1 = *pbVar3++; uVar2 = (&DAT_180470220)[uVar2 >> 0x18 ^ (uint)bVar1] ^ uVar2 << 8; } while (--uVar4);
return uVar2;                                                /* no final XOR */
```

`FUN_1800194b0` swaps every 32-bit word (`b0 b1 b2 b3 -> b3 b2 b1 b0`, both in
the unrolled and the tail loop). Therefore:

* Polynomial `0x04C11DB7`, init `0xFFFFFFFF`, refin = false, refout = false,
  xorout = 0 — i.e. **CRC-32/MPEG-2** applied to the packet viewed as
  big-endian 32-bit words. This is exactly what the STM32 hardware CRC unit
  produces when fed the packet's little-endian `u32` words, which is almost
  certainly what the box does on its side (the box-side error names are
  `*_PKT_CRC_ERROR`).
* Equivalent formulation for Rust: iterate the packet as `u32::from_le_bytes`
  words and, for each word, run the MSB-first CRC over its 4 bytes in
  big-endian order (or use a word-at-a-time STM32 CRC implementation).
* The temp buffer is 4000 bytes, so `nbytes <= 4000`; all host->box packets
  are `<= 0x40C`.
* The CRC value is stored little-endian in the slot given in 1.4.

### 2.2 Covered range (likely)

Ghidra could not recover the argument setup for the `FUN_180019600()` call in
`FUN_180019160` (the call is shown with no arguments). Evidence for the range:

* The CRC slot is always the last 4 bytes of the packet (1.4).
* Packet buffers are reused across sends and the CRC slot is never cleared
  before recomputation (only `0xBBBBDDDD` pre-sets two other words to -1).
  A CRC that included its own slot would therefore vary between identical
  sends, which would break the box's check on the second send.

Conclusion: CRC covers bytes `[0, total_len - 4)`, i.e. header + payload up to
but excluding the CRC word.

**Test vectors** (computed from the algorithm above; the standard check value
`crc("123456789") = 0x0376E6E7` confirms it is CRC-32/MPEG-2):

| Input | CRC | Meaning |
|---|---|---|
| `BB BB CC CC 0C 00 00 00` fed as byte-swapped words (`BB BB CC CC 00 00 00 0C`) | `0xE465E6F1` | expected ARM trailer under the `[0, len-4)` hypothesis -> wire packet `BB BB CC CC 0C 00 00 00 F1 E6 65 E4` |
| same 8 bytes, no word swap | `0xFEE15FE4` | what you would get if the byte-swap step were omitted (wrong) |

Verify against a captured ARM packet: if its last four bytes are
`F1 E6 65 E4` the range hypothesis is confirmed.

### 2.3 Box -> host packets

No CRC check is performed by the SDK on inbound packets; framing is validated
by magic + length only (`FUN_18000c180`). Inbound packets may carry a CRC in
their last word (e.g. status packet 0xA0 total), but the SDK never verifies it.

---

## 3. Encryption

### 3.1 Is the link encrypted? (confirmed)

Partially. Host->box: never. Box->host: only the payload bytes of
`0xCCCCCCCC` (shot record) and `0xDDDDDDDD` (image) packets, and only once
`ctx+0x22C` has been set by `FUN_18000cf10`, which happens in
`FUN_180016110` after the first `0xAAAAAAAA` static-data packet:

```c
FUN_180019310();
FUN_18000cf10(*(longlong *)(PTR_DAT_180419908 + 0x28) + 0x12cbe0,
              *(undefined4 *)(*(longlong *)(PTR_DAT_180419908 + 0x40) + 0x940));   /* ctx, fw_version */
```

The header (magic + length) of those packets is in the clear; the
per-packet byte count is exact (payload = `total_len - 8`).

### 3.2 Cipher, mode, padding (confirmed)

* **AES-128**. Key schedule `FUN_180010d30` emits exactly 10 `aeskeygenassist`
  rounds with rcon `1,2,4,8,0x10,0x20,0x40,0x80,0x1b,0x36` (192/256 paths were
  dead-code-eliminated); the round-count byte at `ctx+0xF0` is `0xA0`
  (`(Nr=10) << 4`), and `FUN_180010bc0` dispatches on `0xa0/0xc0/0xe0`.
* **ECB**. `FUN_180010bc0(in, out, nbytes, ctx)` loops `nbytes >> 4` blocks,
  each `auVar5 = *param_1 ^ *pauVar4` (XOR with last round key) followed by
  `aesdec x9 + aesdeclast`, with no chaining variable and no IV.
* Decrypt keys are the standard equivalent-inverse-cipher (`aesimc` applied to
  round keys 1..9 in `FUN_180011750`).
* **No padding.** The parser accumulates exactly 16 bytes:

  ```c
  *(byte *)(ctx + 0x218 + count) = in_byte; count++;
  if (count != ctx->block_size /* 0x10 at ctx+0x204 */) continue;
  if ((count & 0xf) == 0) FUN_180010bc0(ctx + 0x218, ctx + 0x208, count, ctx);
  count = 0; copy 16 decrypted bytes from ctx+0x208 to destination
  ```

  A trailing partial block is never flushed, so an encrypted payload must be a
  multiple of 16 bytes (`total_len = 8 + 16k`).
* The software fallback (`FUN_180010100`, `FUN_18000d320`, `FUN_18000e340`)
  is selected when `cpuid(1).ECX & 0x2000000` (AES-NI) is 0 and uses eight
  1 KiB tables at `DAT_1803fd260..` / `DAT_1803ff260..` with rcon `0x1b`; this
  is the classic 4x T-table AES layout (**likely** standard AES; not
  byte-verified).

AES context layout (`FUN_18000cf10`, `memset(ctx, 0, 0x240)`):

| Offset | Meaning |
|---|---|
| 0x000..0x0EF | round keys (decrypt-transformed) |
| 0x0F0 | rounds byte `0xA0` |
| 0x204 | block size = 0x10 |
| 0x208 | 16-byte decrypted block scratch |
| 0x218 | 16-byte ciphertext accumulator |
| 0x228 | key index 0/1/2 |
| 0x22C | encryption enabled (1) |
| 0x230 | 0 |

### 3.3 Key derivation (confirmed) — constants, selected by firmware version

`FUN_18000cf10(ctx, fw_version_float, flag)`:

```c
local_58[0] = 0; local_58[1] = 0;                                   /* key 0: 16 x 0x00 */
local_48 = 0x3020100; local_44 = 0x7060504; local_40 = 0xb0a0908; local_3c = 0xf0e0d0c;  /* key 1 */
local_38 = 0x18895342; local_34 = 0x5874277; local_30 = 0x36555881; local_2c = 0x8615886; /* key 2 */
...
if (1.195 <= param_2) { if (2.0 <= param_2) idx = 2; else if (1.2 <= param_2) idx = 1; else idx = 0; } else idx = 0;
*(int *)(ctx + 0x228) = idx;
if (param_3 == 0) { *(int *)(ctx + 0x22c) = 1; FUN_180011750(local_58 + idx * 2, ctx); }
```

| FW version `v` | Key index | Key bytes (as fed to the key schedule, in memory order) |
|---|---|---|
| `v < 1.2` (incl. `< 1.195`) | 0 | `00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00` |
| `1.2 <= v < 2.0` | 1 | `00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F` |
| `v >= 2.0` | 2 | `42 53 89 18 77 42 87 05 81 58 55 36 86 58 61 08` |

(The stack locals are contiguous: `local_58[2]` at -0x58, `local_48..local_3c`
at -0x48, `local_38..local_2c` at -0x38, so `local_58 + idx*2` indexes them as
three consecutive 16-byte keys.) The key is **not** derived from the serial,
the box name, the obfuscated strings, or negotiated; it is compiled in. The
strings `$4,8-9'66.:$?#1*HhXpAeS~ZrNlS` / `8$4,6-9'$6.:*?#1pHhX~AeSlZrNbS`
have no code cross-references in `strings_xref.txt` and play no role in the
device link. `"AES %c CLMUL %c\n"` likewise has no xref (leftover from a
statically linked crypto self-test).

### 3.4 Challenge / verification (confirmed, `FUN_180019310`)

Before enabling the link the SDK proves the key against a 16-byte block the
box sent in the static-data packet (`boxinfo+0xA24`):

```c
fVar1 = fw_version;
if (fVar1 == -1.0 || fVar1 < min_supported) return 1;          /* incompatible */
else if (fVar1 < 1.21) return 0;                                /* no check for old FW */
else {
  expected = (fVar1 < 2.0) ? "00 01 .. 0F" : {0x6507031,0x65542178,0x98183419,0x68406151};
  FUN_18000cf10(tmpctx, fVar1, 0);
  FUN_180010bc0(boxinfo + 0xa24, plain, 0x10, tmpctx);           /* ECB-decrypt one block */
  return memcmp(plain, expected, 16) != 0;
}
```

Expected plaintext for FW >= 2.0 (LE bytes of the four constants):
`31 70 50 06 78 21 54 65 19 34 18 98 51 61 40 68`.
Expected plaintext for 1.21 <= FW < 2.0: `00 01 02 .. 0F`.
On mismatch MGR raises event `0x24` (`RIPE_EVT_FW_SDK_INCOMPATIBLE`) but still
proceeds to state 5. A Rust implementation can use this block as a self-test
that it selected the right key.

Firmware flag inside `FUN_18000cf10`'s third parameter: in `FUN_180016110`
the call is shown with two arguments, so `param_3` is whatever `r8` held.
Given every FW >= 1.21 box observed by the vendor is encrypted, treat
`ctx+0x22C = 1` as always set (open question for FW < 1.2 boxes).

---

## 4. Command encoding (`RIPESendBoxCommand`)

### 4.1 Pipeline (confirmed)

`RIPESendBoxCommand(ctx, cmd) @ 180013eb0`:

```c
if ((param_2 - 5U < 3) || (param_2 == 0x12 && cfg->token != -0xa98ac7)) return 4;  /* MODE_* and un-tokened SET_USERPERSISTENTDATA rejected */
... state checks (returns 2/4/5/6) ...
return FUN_18001ea40(param_2);           /* -> FUN_180011a30(G+0x310, cmd): push cmd id onto MGR queue */
```

The MGR thread (`FUN_18001eb20`) pops the id; ids `< 0x1a` go to
`FUN_180016f80(_, cmd)`, whose `switch` writes the packet fields and calls
`FUN_180016da0(G, group)`; `FUN_180016da0` calls `FUN_180019160(pkt, pkt->len)`
which stamps the CRC and sends. The command id space is the string table order
(`UCMD: RIPE_CMD_*`, `strings_xref.txt` 809-834, and the `FUN_180016f80` cases):

### 4.2 Numeric table (confirmed unless marked)

| Id | RIPE_CMD name | `FUN_180016f80` action | Packets sent (in order) |
|---|---|---|---|
| 0 | NONE | default: clear one-shot fields (sys +0x3C,+0x40,+0x48,+0x50,+0x54,+0x5C; nw +0x08,+0x10,+0x38,+0x8C) | none |
| 1 | SHUTDOWNBOX | returns 4, clears fields | none (not implemented) |
| 2 | RESTART | returns 4, clears fields | none (not implemented) |
| 3 | SHOT_MODE_NORMAL | sys `+0x54 = 1` | `0xAAAABBBB` |
| 4 | SHOT_MODE_PUTTING | sys `+0x54 = 2` | `0xAAAABBBB` |
| 5 | MODE_NORMAL | rejected by `RIPESendBoxCommand`; internally cam `+0x0C=0,+0x10=1` | `0xAAAACCCC` |
| 6 | MODE_CALIBRATE | rejected; internally sets mode 7 | none |
| 7 | MODE_DEBUG | rejected; internally cam `+0x0C=1,+0x10=0` | `0xAAAACCCC` |
| 8 | BOOT_FORCE_NETWORKCONNECT | nw `+0x08=1, +0x0C=0` | `0xBBBBAAAA` |
| 9 | BOOT_FORCE_DIRECTCONNECT | nw `+0x08=1, +0x0C=1` | `0xBBBBAAAA` |
| 10 (0x0A) | UPGRADE_FIRMWARE | if newer FW available (`FUN_18001baf0`) start `FUN_18001beb0` thread, mode 3 | `0xAAAABBBB`(+0x18=1,+0x1C=size) then `0xAAAADDDD` chunks |
| 11 (0x0B) | PLAYER_HAND_LEFT | `FUN_1800173f0(1)`: sys `+0x14=1`, cam gains swapped | `0xAAAABBBB`, `0xAAAACCCC` |
| 12 (0x0C) | PLAYER_HAND_RIGHT | `FUN_1800173f0(0)`: sys `+0x14=0` | `0xAAAABBBB`, `0xAAAACCCC` |
| 13 (0x0D) | ASSIST_ALIGNMENT_ON | sys `+0x14=4`, link mode 4 | `0xAAAABBBB` |
| 14 (0x0E) | ASSIST_ALIGNMENT_OFF | `FUN_1800173f0(current hand)`, link mode 1 | `0xAAAABBBB`, `0xAAAACCCC` |
| 15 (0x0F) | ARM | callback `(0,0x38)` -> MGR 0x38 -> `G+0xAD0` magic `0xBBBBCCCC` | `0xBBBBCCCC` (12 bytes, CRC at +8) |
| 16 (0x10) | DISARM | callback `(0,0x39)` -> MGR 0x39 -> `G+0xAD0` magic `0xBBBBFFFF` | `0xBBBBFFFF` (12 bytes) |
| 17 (0x11) | SET_NWCONFIG | fields pre-filled by `RIPEBoxSetNetworkConfig` | `0xBBBBAAAA` |
| 18 (0x12) | SET_USERPERSISTENTDATA | needs token; clears token | `0xFFFFAAAA` (`G+0xB00`) |
| 19 (0x13) | GET_USERPERSISTENTDATA | zero RX buffer `G+0xB08+8`, sys `+0x3C=1` | `0xAAAABBBB` (group 8) |
| 20 (0x14) | ENABLE_REFERENCE_LASER | sys `+0x4C=1` | `0xAAAABBBB` |
| 21 (0x15) | ENABLE_PSC_JP_MODE | sys `+0x58=1` | `0xAAAABBBB` |
| 22 (0x16) | DISABLE_PSC_JP_MODE | sys `+0x58=0` | `0xAAAABBBB` |
| 23 (0x17) | GET_NETWORK_SCAN_LIST_RESULT | zero list buffers, sys `+0x60=10`, link mode 5 | `0xAAAABBBB` |
| 24, 25 | BIT_RESERVED_4/5 | default | none |

`FUN_180016da0` groups: 1 = sys config (also sets `+0x44` link type and clears
one-shot fields after send), 2 = cam config, 3 = sys then cam, 4/5 = set ARM /
DISARM magic then send `G+0xAD0`, 6 = network config (clears `+0x08,+0x10,+0x38`
after), 7 = persistent-data write, 8 = sys config then clear `+0x3C`.

### 4.3 Internal MGR queue ids seen on the wire path (confirmed behaviour; names likely)

User events (`RIPE_EVT_*`) are ids 0x1A..0x2E (0x1A BOX_CONNECTED, 0x1B
DISCONNECTED, 0x1C STATUS_UPDATE, 0x1F TRIGGERDETECTED, 0x22
USERPERSISTENTDATA_READY, 0x24 FW_SDK_INCOMPATIBLE, 0x29 BOX_STATIC_UPDATE,
0x2A BOX_ERROR, 0x2B NETWORK_SCAN_LIST_UPDATE). Internal ids handled by
`FUN_18001eb20`:

| Id | Behaviour | Likely name |
|---|---|---|
| 0x30/0x31 | comm thread ended: tear down, auto-reconnect to the stored box (`FUN_180018e30` USB / `FUN_1800186d0` WiFi) | WIFI/USB_THREAD_TERMINATED |
| 0x32 | send `0xBBBBFFFF` (DISARM), reset sys config, `+0x5C=1`, send sys config | (post-connect configure) |
| 0x33 | static-data packet received: compute cam config, print FW/serial/SSID, send sys + cam config, run key check `FUN_180019310`, then event 0x29 | BOX_RESP_BOXPARAMSDATA |
| 0x34 | status packet received: react to `+0x28` code (1 -> FW upgrade success, 2 -> error, 3 -> …), re-ARM after data loss, event 0x1C | BOX_RESP_BOX_STATUS_DATA |
| 0x35 | send sys config (group 1) | SEND_SYS_CONFIG |
| 0x36 | send cam config | SEND_CAM_CONFIG |
| 0x37 | send `0xBBBBDDDD` manufacturing record (`boxinfo+0xA50`) | SEND_MANU_CONFIG |
| 0x38 | send `0xBBBBCCCC` | SEND_ARM_CONFIG |
| 0x39 | send `0xBBBBFFFF` | SEND_DISARM_CONFIG |
| 0x3B | state 7 | BOX_DISARMED |
| 0x3C | WiFi list complete -> event 0x2B, link mode 1 | — |
| 0x3E | disconnect: link mode 6, DISCT queue 5 then 0, event 0x1B | COMM_DISCONNECTBOX |
| 0x3F | `RIPEDiscover` -> DISCT queue 2 | COMM_DISCOVER_* |
| 0x43 | `RIPEBoxConnect` -> set target node id, callback 0x30 | COMM_CONNECT2BOX |
| 0x44 | `RIPEBoxDisconnect` -> clear node id, callback 0x30 | — |
| 0x47/0x48/0x49 | sys `+0x60 = 6/7/8`, send sys config | (extended requests) |

The IEVT string table has 22 names for 24 slots (`< 0x47` bound), so the exact
name/number alignment for ids >= 0x30 is unresolved; the wire behaviour above
is what matters for reimplementation.

---

## 5. Reimplementation checklist (Rust)

1. **Frame codec**: `struct Frame { magic: u32, payload: Vec<u8> }`; encode as
   `magic.to_le_bytes() ++ (8 + payload.len() as u32).to_le_bytes() ++ payload`.
   Decoder: byte-wise 32-bit LE window scanning for one of the eight inbound
   magics, then 4-byte LE length, then `len - 8` payload bytes. Reject/resync
   if `len < 8` or `len > 0x12CE20`.
2. **CRC**: implement CRC-32/MPEG-2 (`poly 0x04C11DB7, init 0xFFFFFFFF, no
   reflect, xorout 0`) over the packet's `u32` words with each word fed
   big-endian (byte-swapped), covering `[0, len-4)`; write result LE into the
   last 4 bytes. Unit test: the ARM packet `BBBBCCCC 0000000C` must reproduce
   the CRC observed in a capture (see Open questions for the alternative range).
3. **Packet builders** for `0xAAAABBBB` (0x6C, template above; `+0x64 =
   0x40000000`), `0xAAAACCCC` (0x84, defaults above), `0xBBBBCCCC`/`0xBBBBFFFF`
   (12 bytes), `0xBBBBAAAA` (0xB8), `0xFFFFAAAA` (0x40C), `0xAAAADDDD` (FW).
   Keep the "first config after connect" order: DISARM, sys config, (on static
   data) sys config + cam config.
4. **Handshake**: UDP `0xAAAAEEEE` broadcast to 5023 -> parse `0xBBBBBBBB`
   (name at +0x50, len at +0x70). Connect: UDP `0xAAAAFFFF` to 5023, expect
   `0xBBBBBBBB` with matching name, then TCP 5024 stream.
5. **AES-128-ECB decrypt** for `0xCCCCCCCC` / `0xDDDDDDDD` payloads once the
   static-data packet arrived: parse FW version float at packet offset 0x14
   (after skipping an embedded `0xBBBBDDDD` sub-record if it precedes it),
   choose key: `<1.2` zero key, `1.2..2.0` `00..0F`, `>=2.0`
   `42 53 89 18 77 42 87 05 81 58 55 36 86 58 61 08`. Payload length must be a
   multiple of 16; no IV, no padding.
6. **Key self-test**: ECB-decrypt the 16 bytes at `boxinfo+0xA24` (static-data
   record) and compare with `31 70 50 06 78 21 54 65 19 34 18 98 51 61 40 68`
   (FW >= 2.0) or `00..0F` (1.21 <= FW < 2.0).
7. **Command API**: map the 26 `RIPE_CMD_*` ids to the field writes in 4.2;
   only ARM/DISARM produce a dedicated packet, everything else is a sys/cam/nw
   config re-send with one field toggled, then cleared after send.
8. **Timeouts**: TCP idle = fatal except FW mode (12 s) and WiFi-scan mode
   (24 s); UDP confirm retry loop with 4 s recv timeout.
9. Do not implement telemetry (`RAPSODOPACKETKEY`, `Skytrak.dat`, upload to
   `54.169.95.169`) — it is host-side only.

---

## 6. Open questions

1. **CRC covered range.** The decompiler dropped the arguments to
   `FUN_180019600` inside `FUN_180019160`. `[0, len-4)` is the consistent
   choice (buffers are reused without clearing the CRC slot), but
   `[0, len)`-with-slot-zeroed or `[8, len-4)` (payload only) cannot be
   excluded without a capture or the raw disassembly at `0x180019160`.
2. **DISARM (`0xBBBBFFFF`) has no CRC slot** in `FUN_180019160`; bytes 8..11
   carry stale data from the last ARM. Does the box validate it? (There is no
   `DISARM_PKT_CRC_ERROR` string, which supports "not checked".)
3. **`0xCCCCDDDD` and `0xFFFFBBBB`** are allocated/recognised but never
   sent/dispatched in this SDK build (possibly "host ready" per
   `HOST_READY_PKT_CRC_ERROR`, and a reserved inbound type).
4. **Encryption for FW < 1.2 boxes**: `ctx+0x22C` is set whenever `param_3 ==
   0`, and the live call in `FUN_180016110` passes no visible third argument.
   Either such boxes encrypt with the all-zero key or the flag is not set;
   needs a real 1.1x box or the disassembly of `0x180016110`.
5. **Static-data (`0xAAAAAAAA`) full layout** — serial and SSID strings
   (`RIPEGetBoxStaticData` -> `FUN_180014be0`, `FUN_180014ce0`) were not
   mapped to packet offsets here; only FW version (+0x14), challenge block
   (`boxinfo+0xA24`) and version/customer-code words are pinned.
6. **Box status code enum** (`"BOX: ..."` strings at `180403ab0..180403f30`)
   is indexed by `payload+0x20` via an unrecovered jump table
   (`func_0x00018001c4a0`); numeric values are not confirmed.
7. **USB VID/PID** read as `0x4003 / 0x0001` from the libusb descriptor at
   +8/+10; confirm against a real device (unusual VID).
8. The two obfuscated strings (`...HhXpAeS~ZrNlS`) are unreferenced by any
   function; they may belong to a statically linked library and are not part
   of the protocol.
