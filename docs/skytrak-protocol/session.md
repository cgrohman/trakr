# SkyTrak session transport (WiFi) — connect, handshake, keepalive, disconnect

Source: Ghidra decompile of the vendor SDK `PlatformLib.dll` (x64, Rapsodo "RIPE" SDK v2.0 —
`RIPEInit` logs `"RAPSODO SDK(64bit) Version %f"` with `0x4000000000000000` = 2.0).
Decompiled functions live in `research/ghidra/out/PlatformLib.dll/fn/<name>.c`; all addresses below are
image-relative (`0x1800xxxxx`). Two jump tables were resolved from the binary
(`research/oracle/PlatformLib.dll`) to name events and box status codes; everything else is from the
decompile.

Confidence legend: **confirmed** = literal constant / direct code path quoted; **likely** = inferred
from consistent usage in more than one place; **guess** = single weak inference.

Conventions used throughout:

* `ctx` = the SDK global context (`PTR_DAT_180419908`, allocated `malloc(0xb90)` in `RIPEInit`).
* `ctx10` = `*(ctx+0x10)` (public handle), `ctx10_40` = `*(ctx10+0x40)` (connection-state block).
* `ctx40` = `*(ctx+0x40)` (engine data; decoded box packets are copied here).
* `psr` = `*(ctx+0x28)` (byte-stream parser state, `malloc(0x12ce20)`).
* All multi-byte integers on the wire are **little-endian** (x64 host writes native dwords straight into
  the send buffer; the CRC pre-swap in §6 exists precisely because the wire is LE).

---

## 1. Sockets and ports

| Item | Value | Where | Confidence |
|---|---|---|---|
| Box UDP port (discovery + connection-confirm) | **5023** | `FUN_18001eb20` L59: `*(undefined4 *)(puVar2 + 0x980) = 0x33323035;` (= ASCII `"5023"`, little-endian), used via `atoi(puVar15 + 0x980)` in `FUN_180017ea0`. Same constant written to the discovery thread's port string: `FUN_18001afc0` L21 / `FUN_18001b100` L67 `*(undefined4 *)(lVar + 0x168) = 0x33323035;` | confirmed |
| Box TCP port (session) | **5024** | `FUN_18001eb20` L62: `*(undefined4 *)(puVar2 + 0x990) = 0x34323035;` (= `"5024"`), used via `atoi(puVar15 + 0x990)` → `Ordinal_9` (htons) → `Ordinal_4` (connect) in `FUN_180017ea0`. | confirmed |
| Host UDP socket | `socket(AF_INET, SOCK_DGRAM, 0)`; bound to **port 0** (ephemeral) on the adapter IP string at `ctx+0x9e1` | `FUN_1800186d0`: `uVar4 = Ordinal_23(2,2,0); *(ctx+0x950) = uVar4;` — `FUN_180017ea0`: `uVar4 = Ordinal_9(0); ... uVar6 = Ordinal_11(puVar15 + 0x9e1); ... Ordinal_2(*(ctx+0x950), &local_2c0, 0x10)` | confirmed |
| Host TCP socket | `socket(AF_INET, SOCK_STREAM, …)` at `ctx+0x958`; connected to box IP string `ctx+0x9a0`:5024 | `FUN_1800186d0`: `lVar5 = Ordinal_23(2); *(ctx+0x958) = lVar5;` (trailing args not recovered; usage is `connect/send/recv`) | confirmed |
| Box IP / name source | Copied from the discovery record: name → `ctx+0x9b0`, IP → `ctx+0x9a0`, adapter IP → `ctx+0x9e1` | `FUN_18001eb20` L482–514: `memcpy(puVar2 + 0x9b0, plVar15, …); memcpy(puVar2 + 0x9a0, plVar15 + 0x21, …); memcpy(puVar2 + 0x9e1, plVar6 + 0x20 /*adapter IP*/, …)` | confirmed |

### What each socket carries

| Socket | Direction | Content | Evidence |
|---|---|---|---|
| UDP → box:5023 (broadcast) | host→box | Discovery probe, 12 bytes `{0xAAAAEEEE, 0x0C, 0}` | `FUN_180019f50` L179: `Ordinal_20(sock, *(ctx+0xae0), 0xc)`; `RIPEInit`: `**(ctx+0xae0) = 0xaaaaeeee; +4 = 0xc; +8 = 0` — **confirmed** |
| UDP ← box | box→host | Discovery reply = a **box status packet** (`0xBBBBBBBB` header), recv buffer 0x400 | `FUN_180019f50` L210–216: `local_524 = 0x400; … if (aiStack_438[0] == -0x44444445 /*0xBBBBBBBB*/)` — **confirmed** |
| UDP → box:5023 (unicast) | host→box | Connection-confirm, 12 bytes `{0xAAAAFFFF, 0x0C, 0}` | `FUN_180017ea0`: `Ordinal_20(*(ctx+0x950), *(ctx+0xae8), 0xc)`; `RIPEInit` L297–299: `**(ctx+0xae8) = 0xaaaaffff; +4 = 0xc; +8 = 0` — **confirmed** |
| UDP ← box | box→host | Confirm reply = box status packet (`0xBBBBBBBB`), name compared at `+0x50` | `FUN_180017ea0`: `if (DAT_18046c220 == -0x44444445) { … memcpy(&local_2a8, &DAT_18046c270 /*+0x50*/, DAT_18046c290 /*+0x70*/) …strcmp with ctx+0x9b0 }` — **confirmed** |
| TCP box:5024 | host→box | Every command packet: SYS_CONFIG, CAM_CONFIG, ARM/HOST_READY, DISARM, MANU, NW-config, user-persistent-data, FW upgrade | `FUN_180019160`: `Ordinal_19(*(ctx+0x958), param_1, param_2, 0)` (send) — **confirmed** |
| TCP box:5024 | box→host | Status packets, box-params packet, shot data (header/trigger/image), user data, WiFi scan list | `FUN_180017ea0` recv loop feeds `FUN_18000c180` (parser, §5) — **confirmed** |

After the confirm exchange the UDP socket is kept open but never used again; it is closed together with
the TCP socket when the comm thread exits (`Ordinal_3(*(ctx+0x950)); Ordinal_3(*(ctx+0x958));`).

---

## 2. Thread / event architecture (needed to read the sequence)

* **Manager thread** `FUN_18001eb20` ("MGR") pops events from queue `ctx+0x310` (depth 0x14) and
  drives everything. Internal events are posted via the function pointer at `ctx+0x20`
  (`FUN_18001fc20` L11: `*(ctx+0x20) = &LAB_18001ea70`, a thunk of `FUN_18001ea40` which does
  `FUN_180011a30(ctx+0x310, evt)`). User-visible events go out through `ctx+0x18`.
* **WiFi comm thread** `FUN_180017ea0` ("COMMT") — owns the sockets (§3). Started by `FUN_1800186d0`,
  stopped by `FUN_180018820` → `FUN_180018790`.
* **USB comm thread** `FUN_180018890` ("COMMUSBT") — same state machine over USB bulk; used here only
  to disambiguate timeouts.
* **Byte parser** `FUN_18000c180` + callback `FUN_1800165d0` ("PSR") — reassembles packets from the
  TCP byte stream and posts internal events.

### Event IDs (jump table at `0x1c380`, used by `FUN_18001c070`; resolved from the binary — confirmed)

| ID | Name | ID | Name |
|---|---|---|---|
| 0x00–0x19 | `RIPE_CMD_*` user commands (0x0f `ARM`, 0x10 `DISARM`, 0x0b/0x0c `PLAYER_HAND_LEFT/RIGHT`, 0x03/0x04 `SHOT_MODE_NORMAL/PUTTING`, 0x13 `GET_USERPERSISTENTDATA`, 0x17 `GET_NETWORK_SCAN_LIST_RESULT`, …) | 0x33 | `RIPE_INT_EVT_BOX_RESP_BOXPARAMSDATA` |
| 0x1a | `RIPE_EVT_BOX_CONNECTED` | 0x34 | `RIPE_INT_EVT_BOX_RESP_BOX_STATUS_DATA` |
| 0x1b | `RIPE_EVT_BOX_DISCONNECTED` | 0x35 | `RIPE_INT_EVT_SEND_SYS_CONFIG` |
| 0x1c | `RIPE_EVT_BOX_STATUS_UPDATE` | 0x36 | `RIPE_INT_EVT_SEND_CAM_CONFIG` |
| 0x1f | `RIPE_EVT_DATA_TRIGGERDETECTED` | 0x37 | `RIPE_INT_EVT_SEND_MANU_CONFIG` |
| 0x22 | `RIPE_EVT_DATA_USERPERSISTENTDATA_READY` | 0x38 | `RIPE_INT_EVT_SEND_ARM_CONFIG` |
| 0x23/0x24/0x25 | `FW_UPGRADE_AVAILABLE` / `FW_SDK_INCOMPATIBLE` / `FW_SDK_CUSTOMERCODE_INCOMPATIBLE` | 0x39 | `RIPE_INT_EVT_SEND_DISARM_CONFIG` |
| 0x29 | `RIPE_EVT_BOX_STATIC_UPDATE` | 0x3a / 0x3b | `RIPE_INT_EVT_BOX_ARMED` / `BOX_DISARMED` |
| 0x2a | `RIPE_EVT_BOX_ERROR` | 0x3c | *(unnamed; "WiFi scan list received")* |
| 0x2b | `RIPE_EVT_NETWORK_SCAN_LIST_UPDATE` | 0x3d | `RIPE_INT_EVT_PROCESSING_SHOT_DATA` |
| 0x30 | `RIPE_INT_EVT_WIFI_THREAD_TERMINATED` | 0x3e–0x42 | `COMM_DISCOVER_START/AGAIN/AGAIN_NETWORK/AGAIN_USB/STOP` |
| 0x31 | `RIPE_INT_EVT_USB_THREAD_TERMINATED` | 0x43 | `RIPE_INT_EVT_COMM_CONNECT2BOX` |
| 0x32 | *(unnamed; "initial configure after first status")* | 0x44 | `RIPE_INT_EVT_COMM_DISCONNECTBOX` |
| | | 0x45 / 0x46 | `COMM_CONFIGUREBOX` / `COMM_BOX_NORESP` (no handler in MGR switch) |

### Connection state (`*ctx10_40 + 0`, int) — likely names, transitions confirmed

| Value | Meaning | Set where |
|---|---|---|
| 0 | disconnected | MGR on 0x1b |
| 1 | TCP connected, not configured | MGR on 0x1a (`**(ctx10+0x40) = 1`) |
| 2 | error (cam-config invalid / FW incompatible) | MGR case 0x33 |
| 5 | configured, static data delivered | MGR case 0x33 end (`= 5`) |
| 6 | disarmed | MGR case 0x39 (`if (4 < state) state = 6`) |
| 7 | box waiting for HOST_READY (ready to arm) | MGR case 0x3b / user event 0x1f path |
| 8 | ARM sent | MGR case 0x38 |
| 9 | armed (box answered `RESP_OK`) | MGR case 0x34: `if (code==0 && status+0x30==0 && state==8) state = 9` |

`RIPESendBoxCommand` only accepts commands while state ∈ [5,9] (`iVar3 - 5U < 5`); state 0 → error 5
(`RIPE_ERR_BOX_NOT_CONNECTED`), 1/2 → 6, 3 → 4.

Comm-thread mode (`ctx10_40 + 0x1c`): 1 normal, 2 processing shot, 3 FW-upgrade, 4 debug, 5 waiting
for WiFi scan list, 6 discovering/disconnected, 7 calibrate. Comm error code (`ctx10_40 + 0xc`):
-2 TCP socket invalid, -3 connect failed, -4 recv failed, -5 select timeout in normal mode, -6 select
error. `ctx10_40 + 8` = "link up" flag (1 while TCP connected; `FUN_180019160` refuses to send unless 1).

---

## 3. Connection sequence

### 3.1 Trigger

`RIPEBoxConnect(handle, nodeId)` (`0x180015310`) posts event **0x43** after storing the node id at
`ctx+0xaa0`. Preconditions: message-queue fill < 51 % (`iVar5 < 0x33`) and comm mode ≤ 1.

MGR case 0x43: `*(ctx+0xa94) = *(ctx+0xaa0); post(0x30)`. Case 0x30/0x31 (shared with "thread
terminated") is the (re)connect entry point:

```
FUN_180018820();            // stop WiFi thread + close sockets
FUN_180018ee0();            // stop USB thread
if (mode != 6) {
    find discovery record whose node id == *(ctx+0xa94)   // list at ctx+0xa80
    copy name -> ctx+0x9b0, boxIP -> ctx+0x9a0, adapterIP -> ctx+0x9e1
    if (adapter type == 2 /*USB*/) FUN_180018e30() else FUN_1800186d0()   // start comm thread
}
post user event 0x1b (BOX_DISCONNECTED) if no record matched
```

Because the same case handles `WIFI_THREAD_TERMINATED`, **any comm-thread exit while `ctx+0xa94 != 0`
automatically reconnects** to the same box, running the full handshake again (confirmed; there is no
retry cap).

### 3.2 The WiFi comm thread `FUN_180017ea0` step by step

1. `bind(udp, {AF_INET, port 0, adapterIP})`. Failure logs `"COMMT:(UDP)bind() error %s"` but
   **continues** (`uVar13 = 0`). Confirmed.
2. **Connection-confirm loop** (runs until match or hard error — no attempt cap; the counter is only
   logged as `"COMMT:(UDP) Connection confirm retires %d"`):
   * `sendto(udp, *(ctx+0xae8) /*{AAAAFFFF,0C,00}*/, 12, → boxIP:5023)`; error → exit with -1.
   * `FUN_180017940(udp, &timeval{4,0}, …)`: `select` with **4 s** timeout (`local_2e0 = 4`), then
     `recvfrom` up to **0x4000** bytes (`local_2e8[0] = 0x4000`) into `DAT_18046c220`.
   * Reply accepted iff `dword[0] == 0xBBBBBBBB` **and** the NUL-terminated string at `+0x50`
     (length at `+0x70`, clamped to 32) equals the expected box name (`ctx+0x9b0`). Header mismatch →
     `"COMMT:(UDP)statuspkt resp header error"` and retry; timeout → `"…sock timeout"` and retry;
     socket error → `"…sock error (socker closed?)"` and exit.
3. `Sleep(1000)` (confirmed: `Sleep(1000);` immediately after the loop).
4. `connect(tcp, {AF_INET, htons(5024), boxIP})` (blocking). Failure → `"COMMT:(TCP) connect err %d"`,
   error code -3, exit.
5. Set link-up (`ctx10_40+8 = 1`), post **0x1a BOX_CONNECTED**. MGR then sets state 1 and resets the
   cached FW version to -1.0 (`*(ctx40 + 0x940) = 0xbf800000`).
6. **Receive loop** (§4).

### 3.3 Post-connect configuration exchange (driven by box status codes)

The host is reactive: each incoming status packet's code (§7.1, offset `+0x28`) tells it what the box
wants. Sequence as implemented in MGR case 0x34 (`BOX_RESP_BOX_STATUS_DATA`):

| Box says (`status+0x28`) | Host state | Host does | Where |
|---|---|---|---|
| any first status | state < 2 | post **0x32** → send **DISARM** `{BBBBFFFF,0C,crc}` then **SYS_CONFIG** (`FUN_180016da0(ctx,1)`), set `SYS_CONFIG+0x5c = 1` ("initial") | case 0x34 `if (iVar4 < 2) post 0x32`; case 0x32 |
| 1 `WAIT_FOR_SYS_CONFIG_PACKET` | ≥2 | post 0x35 → zero `SYS_CONFIG+0x18/+0x1c`, send **SYS_CONFIG** | case 0x34 `iVar1 == 1`; case 0x35 |
| 2 `WAIT_FOR_CAM_CONFIG_PACKET` | ≥2 | post 0x36 → send **CAM_CONFIG** | case 0x36: `FUN_180019160(*(ctx+0xac8), len)` |
| *(box params packet `0xAAAAAAAA` arrives, usually after SYS_CONFIG)* | | post **0x33** → decode params (§7.2), recompute CAM_CONFIG (`FUN_1800173f0(0)`), log FW/Serial/SSID, send **SYS_CONFIG** *and* **CAM_CONFIG**, run compatibility checks, state = 5 (or 2 on error), emit 0x29 `BOX_STATIC_UPDATE` | case 0x33 |
| 3 `WAIT_FOR_HOST_READY_PACKET` | 6 | post 0x3b → state 7 (ready) | case 0x34 `iVar1 == 3 && iVar4 == 6` |
| 3 `WAIT_FOR_HOST_READY_PACKET` | 8 or 9, seen >2× | resend **ARM/HOST_READY** `{BBBBCCCC,0C,crc}`; log `"MGR: Error ArmPktSent DATA LOSS DETECTED CASE-recover box failed"` on send failure | case 0x34 `(iVar4 - 8U < 2) && ++count > 2` |
| 0 `RESP_OK` with `status+0x30 == 0` | 8 | state 9 (armed) | case 0x34 |
| 0 `RESP_OK` | < 5 | log `"MGR: Box config error"`, post 0x39 (DISARM) | case 0x34 |

The **HOST_READY packet is the ARM packet** `0xBBBBCCCC`: user `RIPE_CMD_ARM` (0x0f) →
`FUN_180016f80` case 0xf → post 0x38 → `**(ctx+0xad0) = 0xbbbbcccc; FUN_180019160(...)`, state 8.
`RIPE_CMD_DISARM` (0x10) → 0x39 → `0xbbbbffff`, state 6. Confirmed.

The box does not send its params packet unless configured; the GSPro plugin calls `BoxConnect`, waits
for `BOX_STATIC_UPDATE`/status, then `BoxArm()` (`SimpleSkytrakDevice.cs`).

Every status packet also triggers user event **0x1c `BOX_STATUS_UPDATE`**; MGR's 0x1c handler syncs
handedness: `if (*(ctx40+0xe44) == 0) hand = 0 else hand = 1; FUN_1800173f0(hand)` (status `+0x38`).

---

## 4. Keepalive / timeouts / loss detection

There is **no host-initiated keepalive or status request**. The box pushes status packets on its own
(box-firmware strings embedded in the DLL: `"Status Packet sent"`, `"Box Params Packet sent"`,
`"AFE Packet sent"` at `0x180446674..0x1804466b8`). The host merely requires the TCP stream to never
go silent:

| Parameter | Value | Evidence | Confidence |
|---|---|---|---|
| TCP `select` timeout per iteration | **6 s** | `FUN_180017ea0`: `local_2f0 = 6; … Ordinal_18(sock+1, &fdset, 0, 0 /*, &timeval*/)` | confirmed |
| Silence tolerated in normal mode | **one select timeout (6 s) → disconnect** with error -5 | `if (iVar7 != 5) { … +0xc = 0xfffffffb; goto exit }` (executed when mode is neither 3 nor 5) | confirmed |
| Silence tolerated in FW-upgrade mode (mode 3) | 12 s cumulative | `uVar12 += 6000; if (11999 < uVar12) "TIMEOUT during FW mode"` | confirmed |
| Silence tolerated while waiting for WiFi scan list (mode 5) | 24 s cumulative | `uVar12 += 6000; if (23999 < uVar12) "TIMEOUT during wait for WIFI Scan list mode"` | confirmed |
| USB variant (for comparison) | 1000 ms reads; 6000 ms cumulative silence in normal mode → `"TIMEOUT during normal mode"` | `FUN_180018890`: `local_b0 = 1000; … uVar8 = uVar9 + 1000; … while (uVar8 < 6000)` | confirmed |
| Box status period | not visible in host code; must be < 6 s | — | open |
| `recv` returns ≤ 0 | `"COMMT:(TCP)Recv error %d(socket closed?)"`, error -4, exit | confirmed |
| `select` returns -1 | error -6, exit | confirmed |

Recovery: on exit the thread clears link-up, resets parser state (`**(ctx+0x28) = 0; DAT_18046b20c = 0;
DAT_18046b218 = 0`), closes both sockets, and — unless the manager asked it to stop
(`*(ctx+0x948) == 1`) — posts **0x30**, which the manager treats as "reconnect to `ctx+0xa94`" (§3.1).
So a dropped box is re-handshaked from the UDP confirm step onward, and the app sees no
`BOX_DISCONNECTED` unless the box is gone for good (the loop simply keeps retrying the UDP confirm).

Received data path: every byte is appended to a 1,232,416-byte (`0x12ce20`) ring buffer at
`*(ctx+0x38)` (write index `ctx+0x30`, wraps at `0x12ce1f`) **and** fed to the parser
`FUN_18000c180(psr, &byte, 1)`.

---

## 5. Common packet framing and the stream parser (`FUN_18000c180`)

All packets, both directions:

| Offset | Size | Field |
|---|---|---|
| 0x00 | u32 LE | magic (see table) |
| 0x04 | u32 LE | **total length including this 8-byte header** (e.g. 0x0C for the 12-byte packets) |
| 0x08 | … | payload |
| len-4 | u32 LE | CRC-32 (host→box packets only; see §6 for exact coverage) |

Parser sync (`*psr == 0` state): shift each incoming byte into a u32 as
`uVar1 = byte << 24 | uVar1 >> 8` (so the wire bytes `BB BB BB BB` match `0xBBBBBBBB`); recognised
magics: `0xBBBBBBBB, 0xCCCCCCCC, 0xDDDDDDDD, 0xEEEEEEEE, 0xFFFFAAAA, 0xAAAAAAAA, 0xFFFFBBBB,
0xBBBBAABB`. The next 4 bytes are the length (`DAT_18046b200`), then payload bytes are copied until
`bytes_seen == length` (`if (uVar1 == DAT_18046b200) …`). There is **no CRC check on received
packets**. Receive buffers inside `psr`:

| Magic | Meaning | Copied to | Callback (`FUN_1800165d0` case) | Confidence |
|---|---|---|---|---|
| `0xBBBBBBBB` | box status | `psr+0x1200a2` (0xA0 bytes) → `ctx40+0xe0c`; posts **0x34** | 2 start / **3 end** | confirmed |
| `0xAAAAAAAA` | box params | `psr+0x12c6f6` (≤0x4E0 bytes) → `FUN_180016110` → `ctx40+0x92c…`; posts **0x33** | 0xc / **0xd** | confirmed |
| `0xCCCCCCCC` | shot header packet start | `psr+0x2c…` | 0 / 1 (emits 0x1f `DATA_TRIGGERDETECTED`) | confirmed |
| `0xDDDDDDDD` | shot image chunk n (`psr+0x54 + n*0x6001a`) | | 6 / 7 | confirmed |
| `0xEEEEEEEE` | trigger packet | `psr+0x120142` | 4 / 5 | confirmed |
| `0xFFFFAAAA` | user persistent data (0x40C) | `psr+0x12c2ea` → `*(ctx+0xb08)`; posts 0x22 | 10 / 0xb | confirmed |
| `0xBBBBAABB` | WiFi scan list (0x2010) | `psr+0x12a2da` → `*(ctx+0xaf8)`; posts 0x3c → user 0x2b | 8 / 9 | confirmed |
| `0xFFFFBBBB` | recognised, no handler | — | — | confirmed |

Encryption: in payload state the parser AES-decrypts 16-byte blocks (`FUN_180010bc0`, AES-NI,
128/192/256 by round count 0xa0/0xc0/0xe0) **only** when `psr[0x4b383] != 0` and the magic is **not**
one of `AAAAAAAA, BBBBBBBB, EEEEEEEE, FFFFAAAA, FFFFBBBB, BBBBAABB` — i.e. only shot-data
(`CCCCCCCC`/`DDDDDDDD`) can be encrypted. **All session packets in this document are plaintext.**
Confirmed (`FUN_18000c180` state-5 branch).

Host→box magics and their fixed sizes (all allocated/initialised in `RIPEInit`, sent by
`FUN_180019160`):

| Magic | Packet | Size | Buffer | CRC offset |
|---|---|---|---|---|
| `0xAAAAFFFF` | connection confirm (UDP) | 0x0C | `ctx+0xae8` | none (field = 0) |
| `0xAAAAEEEE` | discovery probe (UDP bcast) | 0x0C | `ctx+0xae0` | none (field = 0) |
| `0xAAAABBBB` | SYS_CONFIG | 0x6C | `ctx+0xac0` | 0x68 |
| `0xAAAACCCC` | CAM_CONFIG | 0x84 | `ctx+0xac8` | 0x80 |
| `0xBBBBCCCC` | ARM / HOST_READY | 0x0C | `ctx+0xad0` | 0x08 |
| `0xBBBBFFFF` | DISARM | 0x0C | `ctx+0xad0` (magic overwritten) | **not recomputed** (stale) |
| `0xBBBBDDDD` | MANU/AFE config | 0x3BC | `ctx40+0xa50` | 0x3B8 (also forces `+0x1c8..0x1cf = 0xFF…`) |
| `0xBBBBAAAA` | network config | 0xB8 | `ctx+0xaf0` | 0xB4 |
| `0xFFFFAAAA` | set user persistent data | 0x40C | `ctx+0xb00` | 0x408 |
| `0xAAAADDDD` | firmware chunk | var | `DAT_180470a10` | 0x10, CRC over `[0x14, len)` |
| `0xCCCCDDDD` | USB-only "closing" packet | 0x0C | `ctx+0xad8` | none |

---

## 6. CRC-32 algorithm (`FUN_1800196a0` table, `FUN_180019600` compute, `FUN_1800194b0` pre-swap)

* Table: `uVar1 = uVar3 << 0x18; 8× { if ((int)uVar1 < 0) uVar1 = uVar1*2 ^ 0x4c11db7; else uVar1 *= 2; }`
  → MSB-first polynomial **0x04C11DB7**. Confirmed.
* Compute: `uVar2 = 0xffffffff; for each byte: uVar2 = table[uVar2 >> 24 ^ byte] ^ uVar2 << 8;` →
  init **0xFFFFFFFF**, no reflection, **no final XOR**. Confirmed.
* Pre-swap: `FUN_1800194b0(src, tmp, len>>2)` byte-reverses every 32-bit word
  (`w>>24 | (w>>8)&0xff00 | (w&0xff00)<<8 | w<<24`) before hashing. Confirmed.
* Coverage (from disassembly of `FUN_180019160` at `0x180019229–0x180019248`):
  `lea r8d,[rsi-4]; mov rdx,rbx; call FUN_180019600; mov [rdi],eax` → CRC over the **first `len-4`
  bytes** (header included), stored at `len-4`. For `0xAAAADDDD` firmware packets:
  `lea r8d,[rsi-0x14]; lea rdx,[rbx+0x14]` → CRC over `[0x14, len)`, stored at `+0x10`. Confirmed.

Net effect: this is **CRC-32/MPEG-2 fed with the packet's little-endian u32 words in big-endian byte
order** — exactly what an STM32 hardware CRC unit produces when fed 32-bit words. Rust equivalent:

```rust
// crc = "3"
const MPEG2: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_MPEG_2);
fn skytrak_crc(pkt: &[u8]) -> u32 {           // pkt = bytes [0, len-4), len % 4 == 0
    let swapped: Vec<u8> = pkt.chunks_exact(4).flat_map(|w| [w[3], w[2], w[1], w[0]]).collect();
    MPEG2.checksum(&swapped)
}
```

Note `FUN_180019160` computes a CRC only for the seven magics in its `if` chain; for `0xBBBBFFFF`
(DISARM) the 12-byte buffer keeps whatever CRC the last ARM left there (0 on a fresh SDK). The box
evidently accepts that (there is a `HOST_READY_PKT_CRC_ERROR` code for `0xBBBBCCCC` but the DISARM
path never fails in practice). A reimplementation should compute the correct CRC for both; it costs
nothing and matches the ARM behaviour.

---

## 7. Packet layouts

### 7.1 Box status packet `0xBBBBBBBB` (box→host, 0xA0 bytes expected; also the UDP discovery/confirm reply)

Host copies 0xA0 bytes to `ctx40+0xe0c` (`FUN_1800165d0` case 3: `memcpy(ctx40+0xe0c, psr+0x1200a2, 0xa0)`),
so `ctx40+0xe0c+X` ⇔ packet offset `X`. Public struct is `RIPEBoxParamsType` (from the connector's
C# binding): `{handednessState, chargingState, batteryPercent(float), boxRoll(float), boxTilt(float),
isBoxInAPMode, rssiLevel, ConnectionType}` filled by `RIPEGetBoxStatusData` (`0x1800144d0`).

| Offset | Type | Field | Evidence | Confidence |
|---|---|---|---|---|
| 0x00 | u32 | magic `0xBBBBBBBB` | parser | confirmed |
| 0x04 | u32 | length (0xA0 observed by copy size) | parser copies 0xA0 | likely |
| 0x08 | i32 | accelerometer X | `FUN_1800140e0`: `*(int*)(ctx40+0xe14)` used with Y,Z in `atan2`/`sqrt` to compute roll/tilt; zero-initialised in `FUN_18001c720` | confirmed (accel), axis naming likely |
| 0x0C | i32 | accelerometer Y | `ctx40+0xe18` | confirmed |
| 0x10 | i32 | accelerometer Z | `ctx40+0xe1c` | confirmed |
| 0x14 | u32 | unknown | — | — |
| 0x18 | u32 | unknown (reset to 0 at start: `ctx40+0xe24 = 0`) | `FUN_18001c720` | guess |
| 0x1C | f32 | **battery percent** | `RIPEGetBoxStatusData`: `param_2[2] = *(ctx40+0xe28)` → `batteryPercent`; default `0x42c80000` = 100.0f in `FUN_18001c720` | confirmed |
| 0x20 | u32 | unknown | — | — |
| 0x24 | i32 | **charging** (non-zero = charging) | `param_2[1] = (*(int*)(ctx40+0xe30) != 0)` → `chargingState` | confirmed |
| 0x28 | i32 | **box status/response code** (table below) | MGR case 0x34 reads `psr+0x1200ca` (= `+0x28`), logs `"MGR:Box status packet(%d)=%s"` | confirmed |
| 0x2C | i32 | status-payload-present flag (if non-zero the 0xA0 status block is appended to the shot header dump) | `FUN_1800165d0` case 0: `if (*(psr+0x1200ce) != 0) memcpy(…,psr+0x1200a2,0xa0)` | likely |
| 0x30 | i32 | busy / shot-in-progress flag: non-zero → wake IPE queue `ctx+0x348`; must be 0 for state 8→9 (armed) | MGR case 0x34: `if (*(psr+0x1200d2) != 0) FUN_180011a30(ctx+0x348,0)` | guess |
| 0x34 | u32 | unknown | — | — |
| 0x38 | i32 | **handedness reported by box** (0 = right, else left) | MGR 0x1c handler: `if (*(ctx40+0xe44) == 0) hand=0 else hand=1` | confirmed |
| 0x3C–0x47 | 3×u32 | unknown | — | — |
| 0x48 | char[8] | **RSSI as ASCII decimal** | `param_2[6] = atoi((char*)(ctx40+0xe54))` → `rssiLevel` | confirmed |
| 0x50 | char[32] | **box name / ESN string** | `FUN_180017ea0` & `FUN_180019f50`: string at `DAT_18046c270` (= buf+0x50) compared to discovery name | confirmed |
| 0x70 | u32 | length of the name string (clamped to 32) | `DAT_18046c290` (= buf+0x70): `if (len-1 < 0x20) memcpy(…,len)` | confirmed |
| 0x74–0x87 | | unknown | — | — |
| 0x88 | i32 | **connection mode**: 0 → `RIPE_BOX_CONNECTION_NETWORK_MODE`(2), 1 → `DIRECT_MODE`(1), else `UNKNOWN`(0) | `FUN_180019f50`: `if (iStack_3b0 == 0) type=2; else type = (iStack_3b0==1)?1:0` where `iStack_3b0` = buf+0x88 | confirmed |
| 0x8C–0x9F | | unknown | — | — |

Roll/tilt (`boxRoll`,`boxTilt`) are *derived on the host* by `FUN_1800140e0` from the three accel ints
plus calibration from the params packet (1-axis reference at params `+0x158..0x160`, or a 12-float
6-axis matrix at params `+0x33C` if valid). `isBoxInAPMode = (*(ctx40+0x958) == 1)` comes from the
**params** packet (`+0x2C`, see 7.2), not the status packet.

Box status codes (`status+0x28`; jump table at `0x1c640`, index = code + 0x2F; resolved from binary — confirmed):

| Code | Name | Code | Name |
|---|---|---|---|
| 7 | `FLASH_MEMORY_INTEGRITY_SUCCESS` | -1 | `RESP_ERR_IMGTRANSFER_TIMEOUT` |
| 6 | `READY_FOR_MANUFACTURING_COMMANDS` | -2 | `RESP_ERR_FPGATRIGGER_TIMEOUT` |
| 5 | `CAM_CONFIG_PACKET_RECEIVED` | -3 | `WIFI_RX_PACKET_HEADER_ERROR` |
| 4 | `SYS_CONFIG_PACKET_RECEIVED` | -4 | `WIFI_RX_PACKET_SIZE_ERROR` |
| 3 | `WAIT_FOR_HOST_READY_PACKET` | -5…-9 | flash errors |
| 2 | `WAIT_FOR_CAM_CONFIG_PACKET` | -10 | `SYS_CONFIG_PKT_CRC_ERROR` |
| 1 | `WAIT_FOR_SYS_CONFIG_PACKET` | -11 | `CAM_CONFIG_PKT_CRC_ERROR` |
| 0 | `RESP_OK` | -12 | `HOST_READY_PKT_CRC_ERROR` |
| | | -13/-14/-15 | `FW_PKT_SEQUENCE/CRC/TIMEOUT_ERROR` |
| | | -16 | `APP_BOX_BAD_POSITION_ERROR` |
| | | -17 | `APP_POWER_CRITICAL_ERROR` |
| | | -18 | `APP_LASER_SAFETY_ALARM_ERROR` |
| | | -19 | `WIFI_RECONNECT_ERROR` |
| | | -20…-41 | `FLASH_WRITE_ERASE_ERROR_SECTOR` |
| | | -42 | `USB_RECONNECT_ERROR` |
| | | -43/-44 | `ENCRYPTION_INIT/DATA_FAILURE` |
| | | -45/-46 | `WIFI_NW_SCAN_LIST_RESPONSE_UNAVAILABLE/TIMEOUT` |
| | | -47 | `MANU_PKT_CRC_ERROR` |

### 7.2 Box params packet `0xAAAAAAAA` (box→host, ≤ 0x4E0 bytes)

`FUN_180016110` copies the packet dword-by-dword into `ctx40+0x92c…` (`min(len,0x4e0)`), skipping any
embedded `0xBBBBDDDD` block (copied to `ctx40+0xa50`, ≤0x3BC). In practice the used fields are read
from both the raw buffer (`psr+0x12c6f6+X`) and the copy at the same relative offsets, so no such block
precedes them; treat `ctx40+0x92c+X` ⇔ packet offset `X`.

| Offset | Type | Field | Evidence | Confidence |
|---|---|---|---|---|
| 0x00 | u32 | magic `0xAAAAAAAA` | | confirmed |
| 0x04 | u32 | length | `FUN_180016110`: `if (*(psr+0x12c6fa) < 0x4e0) len = …` | confirmed |
| 0x14 | f32 | **firmware version** (e.g. 1.21, 2.0) | `RIPEGetBoxStaticData` → `FUN_180014be0(*(ctx40+0x940), out)` splits into major/minor; `FUN_18001ea90` logs `"BOX: FW Ver- %4.4f"`; reset to -1.0 on connect | confirmed |
| 0x2C | i32 | **AP (direct) mode flag** (1 = AP mode) | `RIPEGetBoxStatusData`: `param_2[5] = (*(int*)(ctx40+0x958) == 1)` → `isBoxInAPMode` | confirmed |
| 0x56 | u16 | box model/type (valid 1..11, else 1) | `RIPEGetBoxStaticData`: `uVar2 = *(ushort*)(ctx40+0x982); if (uVar2-1 < 0xb) out+0x40 = uVar2 else 1` | likely |
| 0xF8 | u8[16] | AES-encrypted customer code | `FUN_180019310`: `FUN_180010bc0(ctx40+0xa24, out, 0x10, key)` then compared to expected plaintext | confirmed |
| 0x108 | u32 | static-data field `+0x3c` (unknown meaning; also gates `FUN_180013e20`) | `RIPEGetBoxStaticData`: `out+0x3c = *(ctx40+0xa34)` | confirmed (position) |
| 0x130 | f32 | tilt calibration offset (invalid if `|x| > 25`) | `FUN_1800173f0`: `bVar10 = 25.0 < ABS(*(float*)(ctx40+0xa5c))` | likely |
| 0x140 | f32 | roll calibration (×1000 when FW<1.05; valid 40..85 after scaling) | `FUN_1800173f0` | likely |
| 0x158 | i32×3 | accelerometer reference (level) X,Y,Z; sanity `|v| < 0x4FFF` | `FUN_180016110`: `*(psr+0x12c84e)…`; `FUN_1800140e0` uses `ctx40+0xa84/0xa88/0xa8c` | confirmed |
| 0x164 | char[12] | **serial / ESN** (NUL appended by host) | `RIPEGetBoxStaticData`: `out+8,+0xc,+0x10 = *(ctx40+0xa90/0xa94/0xa98); out[0x14] = 0`; `FUN_18001ea90` logs `"BOX: Serial - %s"` | confirmed |
| 0x170 | f32 | hardware/firmware variant used for compatibility (1.02 ≤ v < 3.0) | `FUN_1800192b0`, `FUN_1800173f0` (`fVar11 = *(float*)(ctx40+0xa9c)`) | confirmed (usage) |
| 0x194, 0x198 | u32 | camera exposure/timing inputs for CAM_CONFIG | `FUN_1800173f0`: `*(uint*)(ctx40+0xac0)`, `+0xac4` | likely |
| 0x1B0 | u32 | must equal `0x18A89` (100 999) when variant ∈ [1.05, 3.0) | `FUN_1800192b0`: `if (*(int*)(ctx40+0xadc) == 0x18a89) return 1` | confirmed |
| 0x33C | f32×12 | optional 6-axis calibration matrix (validated by `1/x` ranges 14744.7..18021.3 or 920.7..1125.3) | `FUN_180016110` reads `psr+0x12ca32…0x12ca5e` | confirmed |

**SSID is not in this packet**: `"BOX: SSID - %s"` in `FUN_18001ea90` prints `RIPEGetBoxStaticData+0x18`,
which `FUN_180014ce0` fills from the *discovery record's box name*. The box name/ESN string in the
status packet (`+0x50`) is what the SDK calls the box name (the connector passes it to `BoxConnect`).

Compatibility checks run in MGR case 0x33 (all confirmed):
* `FUN_1800173f0(0)` fails → state 2, user event 0x2a `BOX_ERROR`.
* `FUN_1800192b0()==0` (variant rule above) → state 2, event 0x25.
* `FUN_180019310()`: customer code. `fw == -1 || fw < 1.21f` (`**(ctx10+0x48) = 0x3f9ae148`) →
  fail; else AES-128 decrypt `+0xF8` with a key selected by FW (`FUN_18000cf10`: index 0 for <1.2,
  1 for [1.2,2.0), 2 for ≥2.0) and compare to `00 01 … 0F` (FW<2.0) or
  `31 70 50 06 78 21 54 65 19 34 18 98 51 61 40 68` (FW≥2.0). Fail → event 0x24, still state 5.
* `FUN_18001baf0` → event 0x23 `FW_UPGRADE_AVAILABLE` if the embedded FW blob is newer.

### 7.3 SYS_CONFIG `0xAAAABBBB` (host→box, 0x6C bytes; buffer `ctx+0xac0`)

Initial values from `RIPEInit` L192–214; per-field writers noted.

| Offset | Type | Value / meaning | Writer | Confidence |
|---|---|---|---|---|
| 0x00 | u32 | `0xAAAABBBB` | `RIPEInit` | confirmed |
| 0x04 | u32 | `0x6C` | `RIPEInit` | confirmed |
| 0x08–0x13 | | 0 | memset | confirmed |
| 0x14 | i32 | handedness/mode: 0 right, 1 left, 4 debug | `RIPEInit` (`= (**(ctx10+0x38)==1)`), `FUN_1800173f0`, `FUN_180016f80` case 0xd (`= 4`) | confirmed |
| 0x18, 0x1C | i32 | 0 (cleared before every SYS_CONFIG resend) | MGR 0x32/0x35 | confirmed |
| 0x20–0x3B | | 0 | | confirmed |
| 0x3C | i32 | 1 = request user persistent data (`RIPE_CMD_GET_USERPERSISTENTDATA`) | `FUN_180016f80` case 0x13 | confirmed |
| 0x40 | i32 | 0 | | |
| 0x44 | i32 | host interface: 1 = USB, 2 = WiFi/Ethernet, 0 other | `FUN_180016da0` case 1: `if (*(ctx+0x940)==2) =1; else if (<2) =2; else =0` | confirmed |
| 0x48 | i32 | 0 | | |
| 0x4C | i32 | 1 = enable reference laser (`RIPE_CMD_ENABLE_REFERENCE_LASER`) | `FUN_180016f80` case 0x14 | confirmed |
| 0x50 | i32 | 0 | | |
| 0x54 | i32 | shot mode: 1 normal (default), 2 putting | `RIPEInit` (`+0x54 = 1`), `FUN_180016f80` cases 3/4 | confirmed |
| 0x58 | i32 | PSC/JP mode 1/0 | `FUN_180016f80` cases 0x15/0x16 | confirmed |
| 0x5C | i32 | 1 on the very first SYS_CONFIG after connect, else 0 | MGR case 0x32: `*(pkt+0x5c) = 1`; `FUN_180016da0` clears after send | confirmed |
| 0x60 | i32 | special request: 10 = WiFi scan list (`RIPE_CMD_GET_NETWORK_SCAN_LIST_RESULT`), 6/7/8 for events 0x47/0x48/0x49 (unknown), 0 otherwise | `FUN_180016f80` case 0x17; MGR 0x47–0x49 | confirmed (values) |
| 0x64 | f32 | `0x40000000` = **2.0f** SDK/protocol version | `RIPEInit`: `*(pkt + 100) = 0x40000000` | confirmed |
| 0x68 | u32 | CRC over bytes 0..0x67 | `FUN_180019160` | confirmed |

After each send `FUN_180016da0(…,1)` zeroes 0x3C, 0x40, 0x48, 0x4C, 0x50, 0x54, 0x5C, 0x60 (one-shot
request flags; note 0x54 shot-mode is also cleared — the SDK re-sets it on the next mode command).

### 7.4 CAM_CONFIG `0xAAAACCCC` (host→box, 0x84 bytes; buffer `ctx+0xac8`)

Defaults from `RIPEInit` L216–244; recomputed from box params by `FUN_1800173f0(hand)` before the
case-0x33 send. Semantics of the timing fields are camera-internal; values are reproduced so a
reimplementation can send byte-identical packets.

| Offset | Default | Notes / writer |
|---|---|---|
| 0x00 | `0xAAAACCCC` | |
| 0x04 | `0x84` | |
| 0x08 | 3 | constant |
| 0x0C / 0x10 | 0 / 1 | swapped to 1/0 by `FUN_180016f80` case 7 (`RIPE_CMD_MODE_DEBUG`), back by case 5 |
| 0x1C | 0 | |
| 0x20 / 0x24 | 1 / 0xFA | coarse/fine pair from `FUN_180016040(25.0, 846.0, expA)` (params `+0x194`); on invalid params reset to 1/0xFA |
| 0x28 / 0x2C | 1 / 0xFA | pair from `FUN_180016040(25.0, 846.0, expB)` (params `+0x198`) |
| 0x30 / 0x34 | 0x2D / 0x2D | `expA+2..5`, `expB+2..5` clamped (≤0x41); 0x2D on failure |
| 0x38–0x44 | 0x80 ×4 | constants |
| 0x48 / 0x4C | 0x14 (right) or 0x16 (left) | handedness-dependent, set by `RIPEInit` and `FUN_1800173f0` |
| 0x50 / 0x54 | 0x16 (right) or 0x14 (left) | mirror of above |
| 0x58 | 1 | `FUN_1800173f0`: `*(pkt+0x58) = 1` |
| 0x5C / 0x60 | 1 / 0xFA | pair from `FUN_180016040(37.5, 666.0, exp + {2|5})`; 1/0xFA on failure |
| 0x80 | CRC | over 0..0x7F |

`FUN_180016040(clk, base, v, &coarse, &fine)`: `fine_max = (clk==25.0) ? 0x34D : 0x299;
coarse = floor(v / (base/clk)); fine = min(fine_max, (v - coarse*base/clk) * clk)`.

### 7.5 12-byte control packets (host→box)

| Packet | Bytes (LE dwords) |
|---|---|
| Connection confirm (UDP) | `FF FF AA AA` `0C 00 00 00` `00 00 00 00` |
| Discovery probe (UDP bcast) | `EE EE AA AA` `0C 00 00 00` `00 00 00 00` |
| ARM / HOST_READY (TCP) | `CC CC BB BB` `0C 00 00 00` `crc32(first 8 bytes)` |
| DISARM (TCP) | `FF FF BB BB` `0C 00 00 00` `crc` (DLL leaves stale value; compute it anyway) |

### 7.6 Discovery record (host-side, 0x5C bytes; `FUN_180019f50` L268–279) — for context

| Offset | Field | Source |
|---|---|---|
| 0x00 | `boxName[0x21]` | status reply `+0x50` |
| 0x21 | `boxIP[0x10]` | `inet_ntoa(recvfrom addr)` |
| 0x54 | connection type (2 network / 1 direct / 0 unknown) | status reply `+0x88` |
| 0x58 | adapter node id | discovery thread index |

The public `RIPECommBoxDataParamsType` is `{boxName[33], boxIP[16], adapterName[256], adapterIP[16],
adapterType, boxConnectionType}`.

---

## 8. Disconnect sequence

`RIPEBoxDisconnect()` (`0x180015450`): requires comm mode == 1, posts **0x44**. MGR case 0x44:
`*(ctx+0xa94) = 0; post(0x30)`. Case 0x30 then:

1. `FUN_180018820()` → `FUN_180018790()`: `*(ctx+0x948) = 1` (tells the comm thread *not* to post
   0x30 on exit), `closesocket(udp); closesocket(tcp)`, clear link-up (`ctx10_40+8 = 0`), set both
   handles to -1; then joins the thread (`FUN_1802bb210`) and frees thread objects.
2. `FUN_180018ee0()` (USB counterpart; on USB it first sends the 12-byte `0xCCCCDDDD` packet with a
   500 ms timeout and sleeps 100 ms — **no such packet exists for WiFi**).
3. Box lookup for id 0 finds nothing → falls through to `local_218 = 0x1b` → state 0, user event
   `RIPE_EVT_BOX_DISCONNECTED`.

So over WiFi the disconnect is simply: close the TCP socket (and the idle UDP socket). No DISARM or
goodbye packet is sent. The box notices via TCP close/its own timeout. (Confirmed: no send path
exists between 0x44 and the socket close.)

`RIPEDeInit`/shutdown path (`uVar22 == 0xffffd8f1` = -9999 in MGR) does the same closes plus frees
everything.

---

## 9. Reimplementation checklist (Rust, no DLL)

1. Inputs from discovery: `box_ip: Ipv4Addr`, `box_name: String` (status `+0x50`), `adapter_ip`.
2. `UdpSocket::bind((adapter_ip, 0))`. Loop: `send_to([FF FF AA AA 0C 00 00 00 00 00 00 00], (box_ip, 5023))`;
   `recv_from` with 4 s timeout, buffer ≥ 0xA0 (DLL uses 0x4000); accept when `u32_le(buf[0..4]) == 0xBBBBBBBB`
   and `cstr(buf[0x50..0x70])` (len `u32_le(buf[0x70..0x74])`, clamp 32) == `box_name`. Retry otherwise.
   Parse the same buffer as a status packet (battery, charging, RSSI, mode) for free.
3. `sleep(1 s)`; `TcpStream::connect((box_ip, 5024))`. Emit `Connected`.
4. Reader task: read with a 6 s idle timeout (extend to 12 s during FW upgrade, 24 s while awaiting a
   scan list). Frame by 4-byte magic + 4-byte total length (includes header); dispatch on magic.
   Idle timeout / EOF / error → close both sockets and go back to step 2 (the DLL reconnects forever).
5. Packet builders: fixed-size LE buffers from §7.3–7.5; CRC = MPEG-2 over word-swapped bytes
   `[0, len-4)` written at `len-4` (§6). Keep one mutable SYS_CONFIG/CAM_CONFIG state, clear one-shot
   flags (0x3C, 0x4C, 0x5C, 0x60) after each send.
6. Status handler (magic `0xBBBBBBBB`): read `code = i32_le[0x28]`. First status after connect →
   send DISARM then SYS_CONFIG with `+0x5C = 1`. `code == 1` → SYS_CONFIG; `code == 2` → CAM_CONFIG;
   `code == 3` when armed and seen 3× → resend ARM; `code == 0` after ARM → armed. Publish battery
   `f32_le[0x1C]`, charging `i32[0x24] != 0`, handedness `i32[0x38]`, RSSI `atoi(cstr[0x48..0x50])`,
   accel `i32[8..0x14]`.
7. Params handler (magic `0xAAAAAAAA`): FW version `f32[0x14]`, AP mode `i32[0x2C]==1`, ESN
   `str[0x164..0x170]`, model `u16[0x56]`, exposure inputs `u32[0x194], u32[0x198]`, accel reference
   `i32[0x158..0x164]`. Recompute CAM_CONFIG (§7.4), send SYS_CONFIG + CAM_CONFIG, mark "configured".
   (You may skip the customer-code AES check; it only gates a user event.)
8. `arm()`: send `CC CC BB BB 0C 00 00 00 <crc>`; `disarm()`: `FF FF BB BB 0C 00 00 00 <crc>`.
9. `disconnect()`: drop the TCP stream (and UDP socket). Nothing is sent.
10. Do **not** validate CRCs on inbound packets (the box does not append one the host checks); do not
    expect encryption on any packet described here.

---

## 10. Open questions

1. **Box status packet period.** Not observable in host code; the host only enforces <6 s silence.
   Capture a live session to measure it (and to see whether status is also sent while armed/idle).
2. **Status fields at 0x14, 0x18, 0x20, 0x34, 0x3C–0x47, 0x74–0x87, 0x8C–0x9F** are never read by the
   host. The `+0x30` "busy" and `+0x2C` "present" interpretations are single-inference guesses.
3. **Does the box check the CRC on `0xBBBBFFFF` (DISARM)?** The DLL sends a stale/zero CRC there.
   Compute the real one; if the box then NAKs, that would indicate it expects the stale behaviour
   (unlikely).
4. **Byte-level structure of `0xAAAAAAAA` beyond the fields used**, and whether the `0xBBBBDDDD`
   manufacturing block is ever actually embedded (the copy loop supports it; observed offsets suggest
   it is not).
5. **Whether the box tolerates skipping the UDP confirm** and connecting straight to TCP 5024. The DLL
   always does UDP first; the box firmware strings (`WIFI_RX_PACKET_HEADER_ERROR`) suggest a state
   machine that may require it.
6. **SO_BROADCAST / socket options on the session sockets**: none are set in `FUN_1800186d0`
   (only the discovery socket sets `SO_BROADCAST`); `select` is the only timeout mechanism.
7. **Events 0x47/0x48/0x49** set `SYS_CONFIG+0x60` to 6/7/8 — no name strings exist for them
   (table has 0x47 entries, max named 0x46); meaning unknown.
8. `RIPEBoxConnect` returns `RIPE_ERR_UNAVAILABLE` when comm mode > 1 — i.e. connect is refused while
   a shot is being processed or during FW upgrade; a reimplementation should mirror that guard.
9. The embedded box firmware image (strings at `0x180446xxx`, `fpgaLoaderInit`, `Status Packet sent`)
   is the authoritative source for box-side timing and packet layouts; disassembling it would close
   questions 1, 2 and 4.
