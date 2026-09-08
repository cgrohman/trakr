# SkyTrak -> Muni integration: findings and approach (2026-09-06)

## Key finding
Nobody in the community has reversed the SkyTrak device protocol. Every working
third-party connector (including the one GSPro users actually use today,
OpenSkyPlus 2) runs *inside the official SkyTrak Windows app* and hooks the
already-decrypted shot object. The device protocol is never touched.

The official `Skytrak_GSPro_Connector_1.98.exe` was a licensed integration that
used SkyTrak's SDK plus a paid SkyTrak subscription for third-party access.
SkyTrak has discontinued third-party licensing, so that path is closed for a
new product regardless of what the binary reveals.

## Three possible attack surfaces
| Layer | Feasibility | Notes |
|---|---|---|
| A. Hook inside SkyTrak app (BepInEx/Harmony) | Proven, community-standard | SkyTrak app is Unity. v4.x = Mono (OpenSkyPlus v1, hooks by obfuscated symbol names). v5.4-5.8 = IL2CPP/Unity 6 (OpenSkyPlus 2, closed source). |
| B. Device network protocol (WiFi/USB/BLE) | Unknown, likely encrypted | SkyTrak app has a `Security.*` namespace wrapping the device; strongly implies encrypted/authenticated transport. Nobody has published a reversal. High effort, brittle across firmware. |
| C. Reuse SkyTrak SDK from the GSPro connector | Legally closed | SDK needs a server-side entitlement that SkyTrak no longer sells. |

Recommendation: build on layer A. Keep layer B as a research track only if a
per-shot hook proves insufficient.

## What the hook gives you (from OpenSkyPlus v1 source)
Ball: total speed (m/s), launch angle, horizontal launch angle, backspin,
sidespin, total spin, spin axis, ball position, per-metric confidence.
Club: head speed (m/s) + confidence. Controls: arm/disarm, normal vs putting
mode, refresh connection, device ready/connected/disconnected events.

## Proposed architecture
```
SkyTrak app (Windows, Unity)
  └─ BepInEx plugin  ──TCP JSON──>  Muni connector service  ──>  Muni sim
        (hook shot object)            (normalize, filter,
                                       putting mode, heartbeat)
```
Speak GSPro Open Connect v1 on the wire between plugin and connector
(JSON over TCP 921, no auth, LaunchMonitorIsReady/IsHeartBeat/ShotNumber,
201 responses carry Handed/Club/DistanceToTarget). Reason: Muni then works
with every launch monitor that already has a GSPro connector, not just SkyTrak.

## Phase plan
1. Get the SkyTrak Windows app installer for the version you own and identify
   Mono vs IL2CPP (`GameAssembly.dll` present = IL2CPP).
2. Mono: decompile `Assembly-CSharp.dll` with ilspycmd (installed at
   ~/.dotnet/tools/ilspycmd), locate the shot class the OpenSkyPlus symbols
   point at, confirm the obfuscated names for your version.
   IL2CPP: run Il2CppDumper/Cpp2IL to recover metadata, hook via BepInEx 6
   bleeding-edge + Il2CppInterop.
3. Fork OpenSkyPlus v1 plugin framework; write `Muni4OSP` plugin (or make Muni
   itself a GSPro Open Connect server, and reuse GSPro4OSP unchanged).
4. Optional research track: capture SkyTrak app <-> device traffic with
   Wireshark on Windows (npcap) to confirm whether the transport is encrypted.
   If TLS with cert pinning, layer B is dead; move on.

## Still needed from you
- The `Skytrak_GSPro_Connector_1.98.exe` file (not present on this machine).
- Which SkyTrak unit (original / SkyTrak+ / ST MAX) and app version.
- What Muni is and what input it accepts today.

## Sources
- https://github.com/OpenSkyPlus/OpenSkyPlus (cloned to research/)
- https://github.com/OpenSkyPlus/GSPro4OSP (cloned to research/)
- https://github.com/OpenSkyPlus2/OpenSkyPlus2 (cloned to research/, binaries only)
- https://gsprogolf.com/GSProConnectV1.html
- https://openskyplus2.github.io/OpenSkyPlus2/FAQs-OpenSkyPlus2.html

## Update 2026-09-06 (later): Muni already supports this path
Muni Golf Sim (munigolfsim.com, Malkfleursby Technologies, Unreal Engine, free)
ships a .NET Avalonia companion app, `MuniGolf/Binaries/Win64/LaunchMonitorConnect/`.
Decompiled source (with shipped PDB) is in `research/muni-lmc/decompiled/`.

- Native drivers: MLM2PRO / R10 / Square over BLE, Mevo+ and Open Launch Nova over network.
- SkyTrak mode has NO native driver. Its on-screen instruction is literally:
  "Install the OpenSkyPlus mod into the SkyTrak app. Set its GSPro target to this PC:921."
- Foresight/Bushnell/OpenConnect/SkyTrak modes all share one GSPro Open Connect
  TCP listener on port 921 (`Relay/DeviceListener.cs`), relayed to the game on
  127.0.0.1:9210 (`Relay/GameServerClient.cs`).

So the working chain today is:
  SkyTrak unit -> SkyTrak app (+ OpenSkyPlus 2 mod) -> Open Connect JSON :921
  -> Muni Launch Monitor Connect -> Muni game :9210

Nothing new needs to be built for basic play. A custom app only makes sense to
(a) replace the closed-source OpenSkyPlus 2 with our own BepInEx plugin, or
(b) add features (shot logging, filtering, multi-sim fan-out).

## Windows dev box
CORI-DESKTOP = 192.168.5.2 (SMB 445 and RDP 3389 open, SSH 22 not enabled).
To dev remotely from the Mac, enable OpenSSH Server on it (admin PowerShell):
  Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
  Start-Service sshd; Set-Service sshd -StartupType Automatic

## Update 2026-09-08: GSPro connector 1.98 dissected (research/gspro-connector/)
Inno Setup installer. Payload decompiled to `research/gspro-connector/decompiled/`,
native string dumps in `platformlib.strings.txt` and `skytraksw.strings.txt`.

Layers (top to bottom):
1. GSProDeviceInterface.exe  WPF, .NET 5. Talks GSPro Open Connect to 127.0.0.1:921.
2. SkytrakPlugin.dll         Managed, ~2k lines. Discover -> connect -> arm loop,
                             putting/chipping HLA clamps, MMS license skip ("Sklp").
3. SkyTrakSW.dll             Native x64 C++ ("wrapitup" SDK). 68 cdecl exports STSW*.
                             State machine, membership (MMS) check over HTTPS to
                             connect.skygolf.com/comm/services/MMS.asmx and
                             clubsg.skygolf.com/api4/skytrak/session/, encrypted
                             SQLite box cache, OpenSSL 1.0.2m + libcurl. Loads #4 at runtime.
4. PlatformLib.dll           Native x64 "RIPE" SDK, built by Rapsodo. 34 exports RIPE*.
                             UDP broadcast discovery on every adapter, TCP+UDP box
                             transport, libusb for USB mode, AES packet crypto
                             ("RAPSODOPACKETKEY"), CRC'd packet types (SYS_CONFIG,
                             CAM_CONFIG, HOST_READY, ARM/DISARM, IMG, TRIGGER),
                             embedded STM32F4 firmware for upgrades, flight model
                             (RIPEGetFlightDataSpeedAndSpin, generateFlightData flag).
5. vtble.dll                 WinRT BLE client, unused by the SkyTrak path.

Facts that matter for a Linux port:
- Original SkyTrak only. README: will NOT work with SkyTrak+ or SkyTrak Asia.
- Host side depends on Windows only via Winsock, IPHLPAPI, libusb0/WinUSB.
  No WPF or .NET is needed below layer 2.
- Connect modes: USB, WiFi Direct (box is AP, 10.0.0.1), WiFi Network mode.
- License: SDK downloads MMS from skygolf.com but has offline fallback
  ("network is offline. Using existed data"). Connector ignores expired
  membership client-side. STSWNetworkStateType has VERIFIED_OFFLINE.
- Open question: whether ball speed/spin are measured on the box or computed
  host-side from image packets. Strings show IMG_PACKET parsing and calib data
  but no image-processing vocabulary; likely measurement on box, flight on host.

Two viable Linux designs:
A. Wine-hosted daemon (fast, days). Small Win64 console program (mingw-w64 C or
   Rust) that links SkyTrakSW.dll, runs STSWPerform loop, and emits Open Connect
   JSON to Muni :921. Runs headless under wine64. WiFi modes only (USB needs a
   Windows driver). Instrument it to log every packet: ground truth for B.
B. Clean-room RIPE reimplementation (weeks). Native Linux/macOS/RPi, no Wine.
   Needs pcap of discovery + session traffic on Windows, AES key recovery from
   PlatformLib, packet format reversal. Use A as the oracle.
Recommend A now, B as the follow-on. Both sit behind the same Open Connect
output so Muni never notices which one is running.
