# research/

Reverse-engineering workspace. Nothing here ships. Proprietary binaries and
files derived from them are git-ignored; the notes and scripts are kept.

| Path | What | Regenerate |
|---|---|---|
| `gspro-connector/` | GSPro SkyTrak connector 1.98 payload | `innoextract -d extracted SkyTrak_GSPro_Connector_1.98.exe`, then `ilspycmd -p -o decompiled/<name> extracted/app/<name>.dll` |
| `ghidra/` | Ghidra project + decompiled C for PlatformLib.dll and SkyTrakSW.dll | `tools/ghidra_*/support/analyzeHeadless research/ghidra proj -import <dlls> -scriptPath tools/ghidra_scripts -postScript ExportDecomp.java research/ghidra/out` |
| `oracle/` | `stsw_probe.c`, Win64 host for the original SDK, run under Wine to capture ground truth | `x86_64-w64-mingw32-gcc -O1 -o stsw_probe.exe stsw_probe.c` |
| `muni-lmc/` | Muni Golf Sim Launch Monitor Connect (Open Connect listener reference) | blobs from `d1n1mva1fc0qah.cloudfront.net/game-blobs/<sha256>`, zstd-compressed |
| `OpenSkyPlus*`, `GSPro4OSP` | community BepInEx connectors | `git clone` from github.com/OpenSkyPlus |

Findings and the protocol notes live in `../claudedocs/` and `../docs/`.
