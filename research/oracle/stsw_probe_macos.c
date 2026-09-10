/* stsw_probe_macos: native macOS port of stsw_probe.c, dlopen'ing the
 * official SkyTrak macOS app's bundled SDK (liblibSkyTrakSW.dylib, extracted
 * from SkyTrak_5.8.27_MacOS.zip) instead of the Windows DLL under Wine.
 * Same STSW* C API, same struct layouts (plain PODs, no vendor-specific
 * packing pragmas observed, so Microsoft x64 ABI and macOS/arm64 ABI agree
 * on field offsets) -- see stsw_probe.c for the original Windows version and
 * doc comments.
 *
 * Purpose: ground truth for validating trakr's shot_decode module against
 * the vendor's own algorithm, on the *exact same real box*, natively, no
 * Wine/emulation involved.
 *
 * Build: clang -O1 -o stsw_probe_macos stsw_probe_macos.c
 * Run:   ./stsw_probe_macos [--offline] [--box NAME] [--seconds N] [--dylib PATH]
 */
#include <dlfcn.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

typedef int RIPEErr;
typedef struct { void *handle; void (*log)(void *h, int level, const char *msg); } STSWAbstLogger;
typedef struct { int operationMode; int generateFlightData; void *loggerData; int networkState; int ripeEmulatorMode; char *boxCacheFilePath; int serverUrlMode; } STSWInit;
typedef struct { int type; void *data; } STSWMsg;
typedef struct { char boxName[33]; char boxIP[16]; char adapterName[256]; char adapterIP[16]; int adapterType; int boxConnectionType; } RIPECommBoxData;
typedef struct { int handedness; int charging; float batteryPercent; float roll; float tilt; int isAPMode; int rssi; int connType; } RIPEBoxParams;
typedef struct { double x, y, z, t; } RIPEPoint;
typedef struct { float totalSpeed, totalSpeedConf, launchAngle, launchAngleConf, horizAngle, horizAngleConf; int ballPos; } RIPESpeed;
typedef struct { float totalSpin, backSpin, sideSpin, spinAxis, conf; } RIPESpin;
typedef struct { float carry, side, maxHeight, duration; int validPts; RIPEPoint pts[2010]; } RIPEFlight;
typedef struct { RIPESpeed speed; RIPESpin spin; RIPEFlight speedFlight; RIPEFlight speedSpinFlight; int status; } STSWShot;
typedef struct { float major, minor; } RIPEVersion;

#define F(ret, name, ...) typedef ret (*name##_t)(__VA_ARGS__); static name##_t name;
F(RIPEErr, STSWInitEx, void **h, STSWInit *init, STSWAbstLogger *lg)
F(RIPEErr, STSWDeInit, void *h)
F(RIPEErr, STSWPerform, void *h, int *n)
F(void *,  STSWReadMsg, void *h)
F(RIPEErr, STSWFreeMsg, void *m)
F(RIPEErr, STSWDiscover, void *h)
F(RIPEErr, STSWBoxConnect, void *h, const char *name)
F(RIPEErr, STSWBoxDisconnect, void *h)
F(RIPEErr, STSWBoxArm, void *h)
F(RIPEErr, STSWBoxDisarm, void *h)
F(RIPEErr, STSWGetState, void *h, int *s)
F(RIPEErr, STSWGetVersion, RIPEVersion *v)
F(RIPEErr, STSWGetRIPEVersion, void *h, RIPEVersion *v)
F(RIPEErr, STSWGetBoxFWVersion, void *h, RIPEVersion *v)
F(void *,  STSWBoxListGetHead, void *l)
F(void *,  STSWBoxListNodeGetNext, void *n)
F(int,     STSWBoxListNodeIsEnd, void *n)
F(void,    STSWBoxListNodeGetData, void *n, RIPECommBoxData *d)
F(RIPEErr, STSWSetBoxMMSReadMode, void *h, int mode)
F(const char *, STSWErrTypeToString, RIPEErr e)

static const char *MSGNAMES[] = {"DISCOVER_UPDATED","DISCOVER_FAIL","CONNECTED","CONNECT_FAIL","MMS_UPDATED","STARTED_DISCONNECTION","DISCONNECTED","STATUS_UPDATED","SHOT_STARTED","SHOT_ENDED","FW_UPGRADE_SUCCESS","FW_UPGRADE_ERROR","FW_UPGRADE_STARTED","FW_UPGRADE_AVAILABLE","FW_SDK_INCOMPATIBLE","FW_SDK_CUSTOMERCODE_INCOMPATIBLE","NETWORK_SCAN_LIST_UPDATED","FAILED_READ_BOX_MMS"};
static const char *CONN[] = {"UNKNOWN","DIRECT","NETWORK","USB"};

static double now(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}
static void jlog(const char *kind, const char *fmt, ...) {
  va_list ap; va_start(ap, fmt);
  printf("{\"t\":%.3f,\"kind\":\"%s\",", now(), kind); vprintf(fmt, ap); printf("}\n"); fflush(stdout); va_end(ap);
}
static void logcb(void *h, int level, const char *msg) {
  char buf[2048]; size_t j=0; for (size_t i=0; msg && msg[i] && j<sizeof buf-8; i++){ char c=msg[i]; if(c=='"'||c=='\\'){buf[j++]='\\';buf[j++]=c;} else if(c=='\n'){buf[j++]='\\';buf[j++]='n';} else if(c=='\r'){} else buf[j++]=c; } buf[j]=0;
  jlog("sdklog", "\"level\":%d,\"msg\":\"%s\"", level, buf);
}
static int load(const char *dylib_path) {
  void *m = dlopen(dylib_path, RTLD_NOW);
  if (!m) { fprintf(stderr, "dlopen %s failed: %s\n", dylib_path, dlerror()); return 0; }
#define L(n) n = (n##_t)dlsym(m, #n); if(!n){fprintf(stderr,"missing export %s: %s\n",#n,dlerror());return 0;}
  L(STSWInitEx) L(STSWDeInit) L(STSWPerform) L(STSWReadMsg) L(STSWFreeMsg) L(STSWDiscover) L(STSWBoxConnect) L(STSWBoxDisconnect)
  L(STSWBoxArm) L(STSWBoxDisarm) L(STSWGetState) L(STSWGetVersion) L(STSWGetRIPEVersion) L(STSWGetBoxFWVersion)
  L(STSWBoxListGetHead) L(STSWBoxListNodeGetNext) L(STSWBoxListNodeIsEnd) L(STSWBoxListNodeGetData) L(STSWSetBoxMMSReadMode) L(STSWErrTypeToString)
  return 1;
}

int main(int argc, char **argv) {
  int offline = 0, seconds = 600; const char *prefer = NULL;
  const char *dylib_path = "liblibSkyTrakSW.dylib";
  for (int i=1;i<argc;i++){
    if(!strcmp(argv[i],"--offline")) offline=1;
    else if(!strcmp(argv[i],"--box")&&i+1<argc) prefer=argv[++i];
    else if(!strcmp(argv[i],"--seconds")&&i+1<argc) seconds=atoi(argv[++i]);
    else if(!strcmp(argv[i],"--dylib")&&i+1<argc) dylib_path=argv[++i];
  }
  if (!load(dylib_path)) return 2;
  RIPEVersion v={0}; STSWGetVersion(&v); jlog("info","\"stsw_version\":\"%.0f.%.0f\",\"sizeof_shot\":%zu", v.major, v.minor, sizeof(STSWShot));
  void *h = NULL; STSWAbstLogger lg = { NULL, logcb };
  char cache[1024]; getcwd(cache, sizeof cache); strcat(cache, "/probe_cache.sqlite");
  /* The native init struct is ~0x8b0 bytes; the managed wrapper only declared the first 7 fields.
   * Give the SDK a zeroed page so undeclared fields read as 0/NULL. */
  unsigned char initbuf[4096]; memset(initbuf, 0, sizeof initbuf);
  STSWInit init = { 0, 1, NULL, offline ? -1 : 1, 0, NULL, 2 };
  memcpy(initbuf, &init, sizeof init);
  RIPEErr e = STSWInitEx(&h, (STSWInit *)initbuf, &lg);
  if (e) { jlog("error","\"where\":\"STSWInitEx\",\"code\":%d", e); return 3; }
  RIPEVersion rv={0}; STSWGetRIPEVersion(h, &rv); jlog("info","\"ripe_sdk_version\":\"%.0f.%.0f\"", rv.major, rv.minor);
  STSWSetBoxMMSReadMode(h, 0);
  e = STSWDiscover(h); jlog("cmd","\"name\":\"Discover\",\"code\":%d", e);
  double t0 = now(); int connected = 0; char active[33] = {0};
  while (now() - t0 < seconds) {
    int n = 0; STSWPerform(h, &n);
    while (n-- > 0) {
      STSWMsg *m = (STSWMsg *)STSWReadMsg(h); if (!m) break;
      const char *name = (m->type>=0 && m->type<18) ? MSGNAMES[m->type] : "?";
      switch (m->type) {
      case 0: { void *node = STSWBoxListGetHead(m->data); int cnt=0; RIPECommBoxData pick; memset(&pick,0,sizeof pick);
        while (!STSWBoxListNodeIsEnd(node)) { RIPECommBoxData b; memset(&b,0,sizeof b); STSWBoxListNodeGetData(node,&b);
          jlog("box","\"name\":\"%s\",\"ip\":\"%s\",\"adapter\":\"%s\",\"adapterIP\":\"%s\",\"adapterType\":%d,\"conn\":\"%s\"", b.boxName,b.boxIP,b.adapterName,b.adapterIP,b.adapterType,CONN[b.boxConnectionType&3]);
          if (!cnt || (prefer && !strcmp(b.boxName,prefer))) { pick=b; } cnt++; node = STSWBoxListNodeGetNext(node); }
        jlog("event","\"name\":\"%s\",\"count\":%d", name, cnt);
        if (cnt && !connected && !active[0]) { strncpy(active,pick.boxName,32); e=STSWBoxConnect(h,pick.boxName); jlog("cmd","\"name\":\"BoxConnect\",\"box\":\"%s\",\"code\":%d",pick.boxName,e); }
        else if (!cnt) { usleep(2000*1000); STSWDiscover(h); }
        break; }
      case 1: case 3: jlog("event","\"name\":\"%s\",\"err\":%d", name, m->data ? *(int*)m->data : -1); active[0]=0; usleep(3000*1000); STSWDiscover(h); break;
      case 2: { connected=1; RIPEVersion fw={0}; STSWGetBoxFWVersion(h,&fw); jlog("event","\"name\":\"CONNECTED\",\"box_fw\":\"%.4f.%.4f\"", fw.major, fw.minor);
        e=STSWBoxArm(h); jlog("cmd","\"name\":\"Arm\",\"code\":%d",e); break; }
      case 6: connected=0; active[0]=0; jlog("event","\"name\":\"DISCONNECTED\""); usleep(3000*1000); STSWDiscover(h); break;
      case 7: { RIPEBoxParams *p=(RIPEBoxParams*)m->data; jlog("status","\"handed\":%d,\"charging\":%d,\"battery\":%.1f,\"roll\":%.2f,\"tilt\":%.2f,\"apMode\":%d,\"rssi\":%d,\"conn\":\"%s\"", p->handedness,p->charging,p->batteryPercent,p->roll,p->tilt,p->isAPMode,p->rssi,CONN[p->connType&3]); break; }
      case 9: { STSWShot *s=(STSWShot*)m->data;
        jlog("shot","\"status\":%d,\"speed_mps\":%.3f,\"speed_conf\":%.3f,\"vla\":%.3f,\"vla_conf\":%.3f,\"hla\":%.3f,\"hla_conf\":%.3f,\"ballPos\":%d,\"totalSpin\":%.1f,\"backSpin\":%.1f,\"sideSpin\":%.1f,\"spinAxis\":%.2f,\"spin_conf\":%.3f,\"carry_m\":%.2f,\"side_m\":%.2f,\"apex_m\":%.2f,\"dur_s\":%.2f,\"pts\":%d,\"ss_carry_m\":%.2f,\"ss_side_m\":%.2f,\"ss_apex_m\":%.2f,\"ss_pts\":%d",
          s->status,s->speed.totalSpeed,s->speed.totalSpeedConf,s->speed.launchAngle,s->speed.launchAngleConf,s->speed.horizAngle,s->speed.horizAngleConf,s->speed.ballPos,
          s->spin.totalSpin,s->spin.backSpin,s->spin.sideSpin,s->spin.spinAxis,s->spin.conf,s->speedFlight.carry,s->speedFlight.side,s->speedFlight.maxHeight,s->speedFlight.duration,s->speedFlight.validPts,
          s->speedSpinFlight.carry,s->speedSpinFlight.side,s->speedSpinFlight.maxHeight,s->speedSpinFlight.validPts);
        { FILE *f=fopen("last_shot.bin","wb"); if(f){fwrite(s,sizeof *s,1,f);fclose(f);} }
        e=STSWBoxArm(h); jlog("cmd","\"name\":\"Arm\",\"code\":%d",e); break; }
      default: jlog("event","\"name\":\"%s\",\"type\":%d", name, m->type);
      }
      STSWFreeMsg(m);
    }
    usleep(50*1000);
  }
  if (connected) STSWBoxDisconnect(h);
  STSWDeInit(h); return 0;
}
