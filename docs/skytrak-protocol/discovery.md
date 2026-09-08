# SkyTrak network discovery (Rapsodo RIPE SDK, PlatformLib.dll x64)

Source: Ghidra decompile of `PlatformLib.dll` under
`research/ghidra/out/PlatformLib.dll/` (`fn/<name>.c`, `all_functions.c`,
`strings_xref.txt`). Every claim carries the function address it was read
from and a confidence tag: **confirmed** (constant or control flow read
directly), **likely** (inferred from adjacent code / naming), **guess**.

Winsock is imported by ordinal. Mapping used throughout (WS2_32 export
ordinals): `Ordinal_2`=bind, `Ordinal_3`=closesocket, `Ordinal_9`=htons,
`Ordinal_11`=inet_addr, `Ordinal_12`=inet_ntoa, `Ordinal_17`=recvfrom,
`Ordinal_18`=select, `Ordinal_20`=sendto, `Ordinal_21`=setsockopt,
`Ordinal_23`=socket, `Ordinal_111`=WSAGetLastError, `Ordinal_115`=WSAStartup,
`Ordinal_151`=__WSAFDIsSet. `FUN_180011940` is the SDK logger
(`log(level, fmt, ...)`); `FUN_180011a30`/`FUN_180011b20` are the
message-queue post/wait primitives ("sys_mqSend").

---

## 1. Architecture (who does what)

| Function | Role | Confidence |
|---|---|---|
| `RIPEDiscover` @ `180015040` | Public API. Posts internal event `0x3f` (`RIPE_INT_EVT_COMM_DISCOVER_AGAIN`) to the main SDK queue (`PTR_DAT_180419908+0x310`). Returns 0 ok, 2 busy (queue >= 51 % full, or `sys_mqSend` returned 0xb), 4 wrong state (`ctx+0x1c != 1`). | confirmed |
| `FUN_18001eb20` @ `18001eb20` | Main SDK state-machine thread. `case 0x3f:` posts `2` to the DISCT queue (`+0x268`); `0x40` -> `4`; `0x41` -> `3`; `0x42` -> `5`; `0x3e` (`DISCOVER_START`) -> `5` then `0`, with `+0xa98 = 3000` if a previous data socket existed. Also initialises the connect ports (`+0x980 = "5023"`, `+0x990 = "5024"`). | confirmed |
| `FUN_18001b8b0` @ `18001b8b0` | Creates the DISCT thread (`FUN_18001b480`) with a 10-slot queue at `+0x268`; context is `PTR_DAT_180419908+0xa70`. | confirmed |
| `FUN_18001b480` @ `18001b480` | "DISCT" discovery-controller thread. Consumes queue `+0x268`: `0`=start, `2`/`3`/`4`=rescan (both / USB-only / network-only), `5`=stop, `6`=an adapter thread finished Level1, `-9999`=exit. | confirmed |
| `FUN_18001afc0` @ `18001afc0` | Full start: clear lists, enumerate adapters, filter, then per adapter: set port string, `socket(AF_INET, SOCK_DGRAM, 0)`, spawn `FUN_180019f50`. | confirmed |
| `FUN_18001b100` @ `18001b100` | Rescan: re-enumerate (if `param_3`), add synthetic "USB" adapter (if `param_4`), reconcile with existing adapter list (`FUN_18001add0`/`FUN_18001aef0`), spawn threads (type<2 -> UDP `FUN_180019f50`; type==2 -> USB `FUN_18001a8d0`). | confirmed |
| `FUN_18000c760` @ `18000c760` | `GetAdaptersInfo` enumeration; builds a 0x188-byte adapter record incl. broadcast address. | confirmed |
| `FUN_180019f50` @ `180019f50` | "CAdapDisc" per-adapter UDP discovery thread (state machine INIT/START/WAIT/CHK_ALIVE/EXIT). **This is the wire protocol.** | confirmed |
| `FUN_180017b40` @ `180017b40` | "COMMGEN (UDP)" `select()+recvfrom()` helper with countdown timeval. | confirmed |
| `FUN_180019d30` @ `180019d30` | Post-scan reconciliation: drops boxes not seen this round (except the connected one). | confirmed |
| `FUN_180019e40` @ `180019e40` | Returns 1 iff `name == currently connected box` and the connection flag is set. | confirmed |
| `RIPEInit` @ `180012d90` | Allocates and fills all 12-byte command packets, including the discovery request (`+0xae0`). | confirmed |
| `RIPEGetDiscoveredBoxList` @ `180015100` | Flattens adapter->box lists into the public 0x14c-byte records. | confirmed |

Flow: `RIPEDiscover` -> main queue `0x3f` -> DISCT queue `2` -> DISCT closes
all adapter sockets (`+0x180=1`, `closesocket`), waits for threads, re-runs
adapter enumeration, spawns one `FUN_180019f50` thread per usable adapter.
Each thread sends **one** broadcast, listens 3 s, reconciles its box list,
posts `6` to DISCT, then idles in "check alive" until its socket is closed.
When DISCT has received `6` from every adapter it fires the user callback
`(*(ctx+0x20))(0, 0x28)` (line 16934 in `all_functions.c`).

---

## 2. UDP ports

| Item | Value | Evidence | Confidence |
|---|---|---|---|
| Destination port of the discovery broadcast | **5023/udp** | `FUN_18001afc0` @ `18001afc0`: `*(undefined4 *)(lVar1 + 0x168) = 0x33323035; *(undefined1 *)(lVar1 + 0x16c) = 0;` (LE bytes `35 30 32 33` = ASCII `"5023"`). Same in `FUN_18001b100` (`lVar2 + 0x168`). Consumed in `FUN_180019f50` `case 0`: `iVar10 = atoi((char *)(param_1 + 0x168)); uVar6 = Ordinal_9(iVar10);` (htons). | confirmed |
| Source / bind port | **0 (ephemeral)**, bound to the adapter's own IPv4 | `FUN_180019f50` `case 0`: `uStack_4e0 = CONCAT62(..., 2)` (AF_INET); `uVar7 = Ordinal_9(0)` (port 0); `uVar8 = Ordinal_11(param_1 + 0x100)` (inet_addr of adapter IP string at `adapter+0x100`); `Ordinal_2(sock, &uStack_4e0, 0x10)` (bind). A bind failure is logged `"bind() error %d, but continue"` and the state is set to 5 (exit) despite the message. | confirmed |
| Socket options | `SO_BROADCAST = 1` | `Ordinal_21(sock, 0xffff /*SOL_SOCKET*/, 0x20 /*SO_BROADCAST*/, &4-byte 1, 4)`; failure only logged. | confirmed |
| Socket type | `socket(AF_INET=2, SOCK_DGRAM=2, 0)` | `Ordinal_23(2,2,0)` in both launchers. | confirmed |
| Port the reply arrives on | the ephemeral bound port of the same socket (the box replies to the request's source address/port) | Level1/Level2 both `recvfrom` on `*(param_1+0x170)`, the same socket used for `sendto`. | confirmed |
| Destination address | per-adapter IPv4 **directed broadcast** `ip \| ~mask` | `FUN_18000c760`: `local_1d0 = ~local_1cc \| local_1d4;` then `inet_ntop(2, &local_1d0, &local_70)` -> stored at `adapter+0x158` and logged `"\tIP Broadcast: \t%s\n"`. `FUN_180019f50` `case 1`: `uVar8 = Ordinal_11(param_1 + 0x158)`. | confirmed |
| Other ports seen (not discovery) | `"5023"` at `ctx+0x980`, `"5024"` at `ctx+0x990` are the **connection** ports (`FUN_18001eb20` lines 18709-18712; `FUN_180017ea0` sends the 12-byte `0xAAAAFFFF` "connectionConfirmPkt" to `ctx+0x9a0`:5023). | confirmed (values), likely (roles) |

Note: 255.255.255.255 is never used; if the subnet mask lookup fails the
adapter is still added with an empty broadcast string, and `FUN_180019c20`
@ `180019c20` synthesises one from `ip & mask` with the last octet replaced
(`strrchr(...,'.'); sprintf(pcVar5+1,"%u")`) — argument lost by the
decompiler, presumed `net[3] | ~mask[3]` (guess).

---

## 3. Discovery request packet

Built once in `RIPEInit` @ `180012d90` (all_functions.c lines 11226-11233)
and sent verbatim:

```c
pvVar8 = malloc(0xc);
*(void **)(PTR_DAT_180419908 + 0xae0) = pvVar8;
**(undefined4 **)(PTR_DAT_180419908 + 0xae0) = 0xaaaaeeee;
*(undefined4 *)(*(longlong *)(PTR_DAT_180419908 + 0xae0) + 4) = 0xc;
*(undefined4 *)(*(longlong *)(PTR_DAT_180419908 + 0xae0) + 8) = 0;
```

Sent in `FUN_180019f50` `case 1`:

```c
iVar10 = Ordinal_20(*(undefined8 *)(param_1 + 0x170),
                    *(undefined8 *)(PTR_DAT_180419908 + 0xae0), 0xc);   // sendto(sock, pkt, 12, 0, &bcast, 16)
```

### Byte layout (12 bytes, little-endian, no CRC, no encryption) — confirmed

| Offset | Size | Value (LE u32) | On the wire (bytes) | Meaning |
|---|---|---|---|---|
| 0x00 | 4 | `0xAAAAEEEE` | `EE EE AA AA` | header signature / command id |
| 0x04 | 4 | `0x0000000C` | `0C 00 00 00` | total packet length (12) |
| 0x08 | 4 | `0x00000000` | `00 00 00 00` | reserved / payload-length 0 |

Exact wire bytes: `EE EE AA AA 0C 00 00 00 00 00 00 00`.

Endianness: the fields are written as native x64 `undefined4` stores and
compared as native ints on receive (`aiStack_438[0] == -0x44444445`), so LE
is confirmed for the SDK side. (The magic is a palindromic byte pattern
in each 16-bit half, so only the length field actually exposes byte order.)

The same 12-byte `{magic, 0xC, 0}` shape is used by every other SDK
command packet allocated in `RIPEInit` (`0xBBBBCCCC`@+0xad0,
`0xCCCCDDDD`@+0xad8, `0xAAAAFFFF`@+0xae8 connection-confirm, the 0xB8-byte
`0xBBBBAAAA`@+0xaf0 config packet with `len=0xb8`). So the generic header
is `u32 magic; u32 total_len; u32 payload_len_or_reserved;` (likely).

There is **no** generic builder function; each packet is a static struct.

---

## 4. Reply packet ("box announcement")

Received in `FUN_180019f50` `case 2` via `FUN_180017b40` into `int
aiStack_438[20]` + following stack variables, `recvfrom` max length
`local_524 = 0x400` (1024 bytes). The SDK only ever reads four fields.

```c
if (aiStack_438[0] == -0x44444445) {            // 0xBBBBBBBB header check
    if (uStack_3c8 - 1 < 0x20) {                // name_len in 1..32
        memcpy(&uStack_498, &uStack_3e8, uStack_3c8);
        *(byte*)((longlong)&uStack_498 + uStack_3c8) = 0;
    } else {                                    // name_len == 0 or > 32: copy 32 bytes, NUL at [32]
        uStack_478 = 0; uStack_498 = uStack_3e8; ... uStack_480 = uStack_3d0;
    }
    if (iStack_3b0 == 0)      uStack_444 = 2;
    else { uStack_444 = 0; if (iStack_3b0 == 1) uStack_444 = 1; }
    uStack_440 = *(undefined4 *)(param_1 + 0x120);   // index = current box count on this adapter
    _Src = (void *)Ordinal_12(uStack_4f0._4_4_);     // inet_ntoa(from.sin_addr)  -> box IP string
    ...
} else {
    FUN_180011940(1,"CAdapDisc(%d):Level1 Box(%s) header signature error\n", iVar4, inet_ntoa(from));
}
```

Stack offsets relative to the receive buffer (`aiStack_438` at rsp-0x438):
`uStack_3e8` = +0x50, `uStack_3d0` = +0x68, `uStack_3c8` = +0x70,
`iStack_3b0` = +0x88.

### Reply layout (as consumed by the SDK) — offsets confirmed, semantics per column

| Offset | Size | Type | Field | Confidence |
|---|---|---|---|---|
| 0x00 | 4 | u32 LE | header signature, must be `0xBBBBBBBB` (`BB BB BB BB`). Anything else -> `"header signature error"` and the datagram is ignored (not fatal; loop continues). | confirmed |
| 0x04 | 4 | u32 LE | presumably total length (by analogy with the request header) | guess |
| 0x08 | 4 | u32 LE | presumably payload length / reserved | guess |
| 0x0C | 0x44 | — | unknown (never read by discovery; likely firmware/HW version, serial, status) | guess |
| 0x50 | 32 | char[32] | box name, **not** guaranteed NUL-terminated; length given at 0x70. SDK stores it as `char[33]`. | confirmed |
| 0x70 | 4 | u32 LE | name length. Valid range 1..32; otherwise the SDK copies exactly 32 bytes. | confirmed |
| 0x74 | 0x14 | — | unknown | guess |
| 0x88 | 4 | i32 LE | box "mode": `0 -> 2`, `1 -> 1`, other `-> 0` in the SDK's stored `mode` field (+0x54). USB-discovered boxes are given `3` (`FUN_1800122c0` `local_484 = 3`). Interpretation of 0/1/2 unknown (probably network-mode/AP-mode indicator). | offsets confirmed / meaning guess |
| 0x8C.. | — | — | rest of datagram up to 1024 bytes ignored | confirmed |

Box IP is **not** parsed from the payload: it is `inet_ntoa(from.sin_addr)`
of the `recvfrom` source address. Firmware version is **not** extracted at
discovery time (it comes later via the connect/status packets — the same
`0xBBBBBBBB` struct is re-read in `FUN_180017ea0` @ `180017ea0` from the
global `DAT_18046c220`, again only at +0x50/+0x70 for the name compare).

The USB path (`FUN_1800122c0`, "wrapCommUSBFind") sends the same 12-byte
request over bulk EP1 and parses the same reply at +0x50/+0x70 from EP 0x81
with a 0x400 read, confirming this is one shared struct.

### SDK-internal box record (0x5c bytes, `FUN_180019f50` / `FUN_18000d160(list, rec, 0x5c)`)

| Offset | Size | Field |
|---|---|---|
| 0x00 | 33 | name (NUL-terminated copy) |
| 0x21 | up to 47 | IP string from `inet_ntoa` (dedupe key: `"already in list"` compares `rec+0x21`) |
| 0x50 | 4 | unused / uninitialised on the UDP path |
| 0x54 | 4 | mode (see +0x88 mapping; 3 = USB) |
| 0x58 | 4 | index (box count on this adapter at insertion) |

### Public record returned by `RIPEGetDiscoveredBoxList` (0x14c bytes) — confirmed

| Offset | Size | Field |
|---|---|---|
| 0x000 | 33 | box name |
| 0x021 | 16 | box IP string |
| 0x031 | 256 | adapter description (from `IP_ADAPTER_INFO.Description`) |
| 0x131 | 19 | adapter IPv4 string |
| 0x144 | 4 | adapter type: `1` = Ethernet (`IF_TYPE 6`), `0` = Wi-Fi (`IF_TYPE 0x47`), `2` = USB |
| 0x148 | 4 | box mode (+0x54 above) |

---

## 5. Timing, retries, list maintenance

All from `FUN_180019f50` @ `180019f50` unless noted.

| Behaviour | Value | Evidence | Confidence |
|---|---|---|---|
| Broadcasts per discovery per adapter | **exactly 1** | `case 1` performs a single `Ordinal_20` then sets state 2. No loop, no re-send in states 2/3. | confirmed |
| Broadcast interval | none (one-shot per `RIPEDiscover`) | as above; a new `RIPEDiscover` tears down and recreates threads (DISCT `case 2..4`). | confirmed |
| Level1 `COMM_ADAPTER_SCAN_WAIT` window | **3 s total** (countdown shared across replies) | `case 2`: `uStack_518 = 3;` (timeval `{tv_sec=3, tv_usec=0}`) passed to `FUN_180017b40`, which after each `select` rewrites the remaining time: `dVar11 = elapsed; if (dVar12 <= dVar11) iVar2=0; else remaining = dVar12 - dVar11; param_2[0]=sec; param_2[1]=usec`. The loop calls it repeatedly with the same struct, so the window is 3 s wall time regardless of how many replies arrive. | confirmed |
| Level1 exit conditions | timeout -> reconcile and go to Level2; socket error -> exit | `FUN_180017b40` returns `1` packet, `-1` when `select()==0` (timeout), `0` when `select()==SOCKET_ERROR`, `-2` on other errors (ignored, loop continues). In `case 2`: `if (iVar9 == -1) goto reconcile; } while (iVar9 != 0);` else `"Level1 (Socket Closed?)"` -> state 5. | confirmed |
| Level2 `COMM_ADAPTER_SCAN_CHK_ALIVE` | **2 s per select, looped indefinitely**, all packets discarded | `case 3`: `uStack_518 = 2;` reset before every call; `while (iVar10 != 0)` — a timeout (`-1`) simply re-arms; valid `0xBBBBBBBB` datagrams are logged `"XXXXXXXX Box Neglected XXXXXXXXX"` and dropped; only a socket error (DISCT called `closesocket`) exits to state 5. Despite its name, nothing "alive" is checked. | confirmed |
| Completion signal | after Level1 (not after Level2) | end of `case 2`: `if (*(int*)(param_1+0x180) == 0) FUN_180011a30(PTR_DAT_180419908 + 0x268, 6);`. DISCT `case 6` counts these; when `count >= adapters` -> `(*(ctx+0x20))(0, 0x28)`. So the user sees results ~3 s after `RIPEDiscover`. | confirmed |
| User event id for "discovery updated" | `0x28` = `RIPE_EVT_COMM_DISCOVER_UPDATE` | Callback ids seen: 0x1a..0x2b; with `RIPE_EVT_BOX_CONNECTED`=0x1a the string table (strings_xref lines 835-855) puts `COMM_DISCOVER_UPDATE` at 0x28 and `NETWORK_SCAN_LIST_UPDATE` at 0x2b (matches `case 0x3c: uVar9 = 0x2b`). | likely |
| Pre-scan delay | `Sleep(ctx+0xa98)`, normally 0; `3000 ms` when `DISCOVER_START` (0x3e) is processed while a previous data socket (`ctx+0x958 != -1`) existed | DISCT `case 0` path: `Sleep(param_1[10]); param_1[10] = 0;` with `param_1 = ctx+0xa70` so `param_1[10]` = `ctx+0xa98`; `FUN_18001eb20 case 0x3e`: `if (lVar17 != -1) *(ctx + 0xa98) = 3000;`. | confirmed |
| Dedupe within one scan | by **IP string** | `case 2`: `pcVar11 = (char *)(puVar12[2] + 0x21); ... "already in list"`. | confirmed |
| Dropping a box | removed if **not seen in the current 3 s round**, unless it is the currently connected box | `FUN_180019d30(param_1+0x120, &tmp_list)` called at the end of Level1: for each persisted box, `FUN_180019e40(name)==0` **and** name not present in this round's temp list -> unlink + free + `count--`. `FUN_180019e40` returns 1 only if `name == connected box (id ctx+0xa94)` **and** `*(ctx->box+8) != 0`. There is no miss counter: one missed round = dropped. | confirmed |
| Persisted list scope | per adapter (`adapter+0x120` count, `+0x128` head), survives rescans because `FUN_18001b100` reconciles rather than recreates adapter records (`FUN_18001add0` removes vanished adapters, `FUN_18001aef0` adds new ones; same-subnet test `FUN_18000cdf0` = first 3 octets of IP **and** mask equal). Full restart (`FUN_18001afc0`) calls `FUN_180019b40` which frees everything. | confirmed |
| Connected-box adapter vanished | DISCT posts `0x44` `RIPE_INT_EVT_COMM_DISCONNECTBOX` | `FUN_18001b480` label `LAB_18001b747`: `"DISCT:Sending %s\n","RIPE_INT_EVT_COMM_DISCONNECTBOX"; FUN_180011a30(...+0x310, 0x44)`. | confirmed |
| Adapter cap | 21 adapters (`if (0x14 < iVar9) break;` "reached maximum adapter handling limit") | `FUN_18000c760`. | confirmed |

### Adapter selection (`FUN_18000c760` + filters in `FUN_18001afc0`/`FUN_18001b100`)

* Only `IP_ADAPTER_INFO.Type == 6` (Ethernet -> stored type 1) and `0x47`
  (IEEE 802.11 -> stored type 0) are used. **Caveat (likely):** the flag
  `bVar1` is set `false` on the first adapter of any other type and never
  reset, so every adapter *after* a loopback/tunnel/PPP entry in
  `GetAdaptersInfo`'s list is silently skipped. A re-implementation should
  *not* copy this.
* `FUN_18001ac70` @ `18001ac70`: drop adapters whose **gateway is 0.0.0.0**
  (`"removing virtual adapter %s"`). Note for Direct mode: the box's DHCP
  must hand out a gateway (10.0.0.1) or the adapter is excluded.
* `FUN_18001ab50` @ `18001ab50`: drop adapters with IP `127.0.0.1`.
* `FUN_18001aa40` @ `18001aa40`: drop duplicate adapters on the same subnet
  (keeps the first).
* Adapter record (0x188 bytes): `+0x000` Description[256], `+0x100` IP str,
  `+0x110` type, `+0x114`.., `+0x120` box count, `+0x128` box list head,
  `+0x138` mask str, `+0x148` gateway str, `+0x158` broadcast str,
  `+0x168` port str (`"5023"`), `+0x170` SOCKET, `+0x178` thread-alive,
  `+0x17c` state, `+0x180` stop flag.

---

## 6. Direct (box-as-AP, 10.0.0.1) vs Network mode

**Discovery is identical in both modes** — confirmed by absence:

* `FUN_180019f50` has a single code path; the destination is always
  `adapter+0x158` (directed broadcast). On the box's AP network
  (10.0.0.0/24 by inference) that is `10.0.0.255:5023`.
* The string `"10.0.0.1"` is referenced exactly once, in `FUN_18001e480`
  @ `18001e480` (strings_xref line 774), which builds a manufacturing /
  telemetry identifier string and appends `"USB"` if `ctx+0x940 == 2`,
  else `"DIR"` if `strcmp(ctx+0x9a0, "10.0.0.1") == 0`, else `"NET"`.
  I.e. the SDK *infers* Direct mode after the fact from the connected
  box's IP; it never unicasts to 10.0.0.1 during discovery.
* No other literal IPs/ports exist in the binary strings besides `5023`,
  `5024`, `10.0.0.1`.

Practical consequence: in Direct mode the PC's Wi-Fi adapter must have a
non-zero gateway (else `FUN_18001ac70` drops it) and a mask so that
`ip | ~mask` reaches the box. A Rust re-implementation may additionally
unicast the same 12 bytes to `10.0.0.1:5023`; the box's reply handling
should be identical (unverified: **guess**).

---

## 7. Sequence (one adapter)

```
PC                                         Box
|-- UDP  <adapterIP>:<ephemeral> -> <bcast>:5023  [EE EE AA AA 0C 00 00 00 00 00 00 00]
|                                                (Level1: select() budget 3 s total)
|<- UDP  <boxIP>:? -> <adapterIP>:<ephemeral>     [BB BB BB BB ... name@0x50 len@0x70 mode@0x88 ...] (<= 1024 B read)
|   (repeat for every box on the segment; dedupe by src IP)
|   t=3 s: reconcile list (drop unseen, keep connected), post 6 to DISCT
|   DISCT: when all adapters posted 6 -> user callback event 0x28
|   Level2: select() 2 s loop, discard everything, until RIPEDiscover/stop closes socket
```

---

## Reimplementation checklist

1. Enumerate IPv4 adapters; keep Ethernet + Wi-Fi, skip loopback and
   adapters with gateway 0.0.0.0 (optional: relax this); dedupe by subnet.
2. Per adapter: `UDP socket`, `SO_BROADCAST=1`, `bind(adapterIP, 0)`.
3. Compute directed broadcast `ip | ~mask`; `sendto(bcast:5023,
   [EE EE AA AA 0C 00 00 00 00 00 00 00])` once. (Optionally also unicast
   to `10.0.0.1:5023` in Direct mode.)
4. Receive for 3 s wall-clock with a 1024-byte buffer. For each datagram:
   * require `len >= 0x8C` (SDK does not check; you should) and
     `u32le@0 == 0xBBBBBBBB`, else log "header signature error" and skip;
   * `name_len = u32le@0x70`; `name = bytes[0x50 .. 0x50+min(name_len,32)]`
     (if `name_len == 0 || > 32` take 32 bytes); store as `char[33]`;
   * `box_ip = source address`; `mode_raw = i32le@0x88` -> `0->2, 1->1, else 0`;
   * dedupe by `box_ip`.
5. After 3 s: remove any previously known box (per adapter) not seen this
   round unless it is the connected box; emit "discover update".
6. Keep the socket open (SDK does; not required) or close it. No periodic
   re-broadcast; each user-initiated discover repeats steps 2-5, with a
   3000 ms delay if a connection socket was just torn down.
7. Public record: name[33], ip[16], adapter desc[256], adapter ip[19],
   adapter type (0 Wi-Fi / 1 Ethernet / 2 USB), box mode.

## Open questions

* Semantics of reply bytes 0x04-0x4F, 0x74-0x87 and anything past 0x8C
  (firmware version, serial, status flags are presumably here; the SDK
  never reads them during discovery). Needs a packet capture.
* Meaning of `mode_raw` at +0x88 (0/1/other -> SDK 2/1/0). Guess: 0 =
  AP/Direct, 1 = infrastructure/Network, but nothing in the binary labels
  it. The public struct exposes the mapped value at +0x148.
* Whether the box's reply length field mirrors the request (`u32@4 =
  total_len`) — inferred only by analogy with the SDK's own 12-byte
  headers.
* Whether the box answers a unicast to `10.0.0.1:5023` the same way as a
  broadcast (expected yes; untested).
* Whether the box responds to requests from a source port other than an
  ephemeral one, or requires the request length to be exactly 12.
* The exact 5th argument of `select()` in `FUN_180017b40` is lost by the
  decompiler; the countdown logic (`sec*1e6+usec` arithmetic) makes
  `&timeval` the only sensible candidate.
* `FUN_180019c20`'s synthesised-broadcast fallback (`sprintf("%u")` after
  the last '.') — argument lost; presumed `net[3] | ~mask[3]`.
