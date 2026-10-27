//! Structure offsets, constants and library vector (LVO) names, Kickstart 1.3 (V34) layouts.
//!
//! Written first from the public documentation as remembered (second round), now checked
//! mechanically (third round): `scripts/ndk-check.sh` compiles `scripts/ndk-offsets.c` (offsetof /
//! sizeof of every structure field used here) with an m68k-amigaos gcc against the NDK include
//! files and compares each value with the constant of the same name in this file, and
//! `scripts/lvo-check.sh` compares every LVO name table with the NDK `.fd` files. The NDK used is
//! 3.2 (the only one on this machine); fields that exist in V34 sit at the same offsets there
//! (the structures only grew at the end or into reserved space), and sizes are given as the V34
//! end of the structure where it grew. Everything that is still *not* covered by the two scripts is
//! marked `UNSURE`. Rocket Ranger's code agrees too: its Manx startup reads `ThisTask`/`AttnFlags`
//! (276/296), `pr_CLI` (172), `pr_CurrentDir` (152), `pr_MsgPort` (92), `cli_Module` (60),
//! `cli_DefaultStack` (52) and `cli_CommandName` (16), and its pause menu reads Window.RPort (50),
//! Window.UserPort (86), IntuiMessage Class/Code/IAddress (20/24/28), Gadget.GadgetID (38) and
//! IntuitionBase.FirstScreen (60).

// ---------------------------------------------------------------- exec nodes and lists
pub const LN_SUCC: u32 = 0;
pub const LN_PRED: u32 = 4;
pub const LN_TYPE: u32 = 8;
pub const LN_PRI: u32 = 9;
pub const LN_NAME: u32 = 10;
pub const LN_SIZE: u32 = 14;

pub const LH_HEAD: u32 = 0;
pub const LH_TAIL: u32 = 4;
pub const LH_TAILPRED: u32 = 8;
pub const LH_TYPE: u32 = 12;
pub const LH_SIZE: u32 = 14;

// Node types (exec/nodes.h).
pub const NT_UNKNOWN: u8 = 0;
pub const NT_TASK: u8 = 1;
pub const NT_INTERRUPT: u8 = 2;
pub const NT_DEVICE: u8 = 3;
pub const NT_MSGPORT: u8 = 4;
pub const NT_MESSAGE: u8 = 5;
pub const NT_FREEMSG: u8 = 6;
pub const NT_REPLYMSG: u8 = 7;
pub const NT_RESOURCE: u8 = 8;
pub const NT_LIBRARY: u8 = 9;
pub const NT_MEMORY: u8 = 10;
pub const NT_PROCESS: u8 = 13;

// ---------------------------------------------------------------- Library
pub const LIB_FLAGS: u32 = 14;
pub const LIB_NEGSIZE: u32 = 16;
pub const LIB_POSSIZE: u32 = 18;
pub const LIB_VERSION: u32 = 20;
pub const LIB_REVISION: u32 = 22;
pub const LIB_IDSTRING: u32 = 24;
pub const LIB_SUM: u32 = 28;
pub const LIB_OPENCNT: u32 = 32;
pub const LIB_SIZE: u32 = 34;

// ---------------------------------------------------------------- ExecBase (V34, sizeof 558)
pub const EB_SOFTVER: u32 = 34;
pub const EB_MAXLOCMEM: u32 = 62;
pub const EB_MAXEXTMEM: u32 = 78;
pub const EB_INTVECTS: u32 = 84; // 16 x IntVector (iv_Data 0, iv_Code 4, iv_Node 8; 12 bytes)
pub const EB_THISTASK: u32 = 276;
pub const EB_IDLECOUNT: u32 = 280;
pub const EB_DISPCOUNT: u32 = 284;
pub const EB_QUANTUM: u32 = 288;
pub const EB_ELAPSED: u32 = 290;
pub const EB_SYSFLAGS: u32 = 292;
pub const EB_IDNESTCNT: u32 = 294;
pub const EB_TDNESTCNT: u32 = 295;
pub const EB_ATTNFLAGS: u32 = 296;
pub const EB_TASKTRAPCODE: u32 = 304;
pub const EB_TASKEXCEPTCODE: u32 = 308;
pub const EB_TASKEXITCODE: u32 = 312;
pub const EB_TASKSIGALLOC: u32 = 316;
pub const EB_TASKTRAPALLOC: u32 = 320;
pub const EB_MEMLIST: u32 = 322;
pub const EB_RESOURCELIST: u32 = 336;
pub const EB_DEVICELIST: u32 = 350;
pub const EB_INTRLIST: u32 = 364;
pub const EB_LIBLIST: u32 = 378;
pub const EB_PORTLIST: u32 = 392;
pub const EB_TASKREADY: u32 = 406;
pub const EB_TASKWAIT: u32 = 420;
pub const EB_SOFTINTS: u32 = 434;
pub const EB_VBLANKFREQ: u32 = 530;
pub const EB_POWERFREQ: u32 = 531;
pub const EB_SEMAPHORELIST: u32 = 532;
pub const EB_SIZE: u32 = 558;

// ---------------------------------------------------------------- memory
pub const MEMF_ANY: u32 = 0;
pub const MEMF_PUBLIC: u32 = 1;
pub const MEMF_CHIP: u32 = 2;
pub const MEMF_FAST: u32 = 4;
pub const MEMF_CLEAR: u32 = 0x1_0000;
pub const MEMF_LARGEST: u32 = 0x2_0000;
pub const MEMF_REVERSE: u32 = 0x4_0000; // V36
pub const MEMF_TOTAL: u32 = 0x8_0000; // V36

pub const MH_ATTRIBUTES: u32 = 14;
pub const MH_FIRST: u32 = 16;
pub const MH_LOWER: u32 = 20;
pub const MH_UPPER: u32 = 24;
pub const MH_FREE: u32 = 28;
pub const MH_SIZE: u32 = 32;
pub const MC_NEXT: u32 = 0;
pub const MC_BYTES: u32 = 4;

// MemList / MemEntry (AllocEntry)
pub const ML_NUMENTRIES: u32 = 14;
pub const ML_ME: u32 = 16;
pub const ME_SIZE: u32 = 8;

// ---------------------------------------------------------------- Task / Process
pub const TC_FLAGS: u32 = 14;
pub const TC_STATE: u32 = 15;
pub const TC_IDNESTCNT: u32 = 16;
pub const TC_TDNESTCNT: u32 = 17;
pub const TC_SIGALLOC: u32 = 18;
pub const TC_SIGWAIT: u32 = 22;
pub const TC_SIGRECVD: u32 = 26;
pub const TC_SIGEXCEPT: u32 = 30;
pub const TC_TRAPALLOC: u32 = 34;
pub const TC_TRAPABLE: u32 = 36;
pub const TC_EXCEPTDATA: u32 = 38;
pub const TC_EXCEPTCODE: u32 = 42;
pub const TC_TRAPDATA: u32 = 46;
pub const TC_TRAPCODE: u32 = 50;
pub const TC_SPREG: u32 = 54;
pub const TC_SPLOWER: u32 = 58;
pub const TC_SPUPPER: u32 = 62;
pub const TC_SWITCH: u32 = 66;
pub const TC_LAUNCH: u32 = 70;
pub const TC_MEMENTRY: u32 = 74;
pub const TC_USERDATA: u32 = 88;
pub const TC_SIZE: u32 = 92;

pub const TS_INVALID: u8 = 0;
pub const TS_ADDED: u8 = 1;
pub const TS_RUN: u8 = 2;
pub const TS_READY: u8 = 3;
pub const TS_WAIT: u8 = 4;
pub const TS_REMOVED: u8 = 6;

pub const PR_MSGPORT: u32 = 92;
pub const PR_SEGLIST: u32 = 128;
pub const PR_STACKSIZE: u32 = 132;
pub const PR_GLOBVEC: u32 = 136;
pub const PR_TASKNUM: u32 = 140;
pub const PR_STACKBASE: u32 = 144;
pub const PR_RESULT2: u32 = 148;
pub const PR_CURRENTDIR: u32 = 152;
pub const PR_CIS: u32 = 156;
pub const PR_COS: u32 = 160;
pub const PR_CONSOLETASK: u32 = 164;
pub const PR_FILESYSTEMTASK: u32 = 168;
pub const PR_CLI: u32 = 172;
pub const PR_RETURNADDR: u32 = 176;
pub const PR_PKTWAIT: u32 = 180;
pub const PR_WINDOWPTR: u32 = 184;
pub const PR_SIZE: u32 = 188;

// CommandLineInterface (all BPTR/BSTR/LONG)
pub const CLI_RESULT2: u32 = 0;
pub const CLI_SETNAME: u32 = 4;
pub const CLI_COMMANDDIR: u32 = 8;
pub const CLI_RETURNCODE: u32 = 12;
pub const CLI_COMMANDNAME: u32 = 16;
pub const CLI_FAILLEVEL: u32 = 20;
pub const CLI_PROMPT: u32 = 24;
pub const CLI_STANDARDINPUT: u32 = 28;
pub const CLI_CURRENTINPUT: u32 = 32;
pub const CLI_COMMANDFILE: u32 = 36;
pub const CLI_INTERACTIVE: u32 = 40;
pub const CLI_BACKGROUND: u32 = 44;
pub const CLI_CURRENTOUTPUT: u32 = 48;
pub const CLI_DEFAULTSTACK: u32 = 52;
pub const CLI_STANDARDOUTPUT: u32 = 56;
pub const CLI_MODULE: u32 = 60;
pub const CLI_SIZE: u32 = 64;

// ---------------------------------------------------------------- MsgPort / Message / IORequest
pub const MP_FLAGS: u32 = 14;
pub const MP_SIGBIT: u32 = 15;
pub const MP_SIGTASK: u32 = 16;
pub const MP_MSGLIST: u32 = 20;
pub const MP_SIZE: u32 = 34;
pub const PA_SIGNAL: u8 = 0;
pub const PA_SOFTINT: u8 = 1;
pub const PA_IGNORE: u8 = 2;

pub const MN_REPLYPORT: u32 = 14;
pub const MN_LENGTH: u32 = 18;
pub const MN_SIZE: u32 = 20;

pub const IO_DEVICE: u32 = 20;
pub const IO_UNIT: u32 = 24;
pub const IO_COMMAND: u32 = 28;
pub const IO_FLAGS: u32 = 30;
pub const IO_ERROR: u32 = 31;
pub const IO_ACTUAL: u32 = 32;
pub const IO_LENGTH: u32 = 36;
pub const IO_DATA: u32 = 40;
pub const IO_OFFSET: u32 = 44;
pub const IOSTD_SIZE: u32 = 48;
pub const IOF_QUICK: u8 = 1;

pub const IOERR_OPENFAIL: i8 = -1;
pub const IOERR_ABORTED: i8 = -2;
pub const IOERR_NOCMD: i8 = -3;

pub const CMD_RESET: u16 = 1;
pub const CMD_READ: u16 = 2;
pub const CMD_WRITE: u16 = 3;
pub const CMD_UPDATE: u16 = 4;
pub const CMD_CLEAR: u16 = 5;
pub const CMD_STOP: u16 = 6;
pub const CMD_START: u16 = 7;
pub const CMD_FLUSH: u16 = 8;
// trackdisk.device (devices/trackdisk.h); TDF_EXTCOM = $8000 marks the ETD_ variants.
pub const TD_MOTOR: u16 = 9;
pub const TD_SEEK: u16 = 10;
pub const TD_FORMAT: u16 = 11;
pub const TD_REMOVE: u16 = 12;
pub const TD_CHANGENUM: u16 = 13;
pub const TD_CHANGESTATE: u16 = 14;
pub const TD_PROTSTATUS: u16 = 15;
pub const TD_RAWREAD: u16 = 16;
pub const TD_GETDRIVETYPE: u16 = 18;
pub const TD_GETNUMTRACKS: u16 = 19;
pub const TD_ADDCHANGEINT: u16 = 20;
pub const TD_REMCHANGEINT: u16 = 21;
pub const TDF_EXTCOM: u16 = 0x8000;

// Interrupt structure (is_Node 14, is_Data 14, is_Code 18)
pub const IS_DATA: u32 = 14;
pub const IS_CODE: u32 = 18;

// ---------------------------------------------------------------- DOS
pub const DOSTRUE: u32 = 0xFFFF_FFFF;
pub const MODE_READWRITE: u32 = 1004;
pub const MODE_OLDFILE: u32 = 1005;
pub const MODE_NEWFILE: u32 = 1006;
pub const OFFSET_BEGINNING: i32 = -1;
pub const OFFSET_CURRENT: i32 = 0;
pub const OFFSET_END: i32 = 1;
pub const SHARED_LOCK: i32 = -2;
pub const EXCLUSIVE_LOCK: i32 = -1;

pub const ERROR_NO_FREE_STORE: u32 = 103;
pub const ERROR_OBJECT_IN_USE: u32 = 202;
pub const ERROR_OBJECT_NOT_FOUND: u32 = 205;
pub const ERROR_ACTION_NOT_KNOWN: u32 = 209;
pub const ERROR_INVALID_LOCK: u32 = 211;
pub const ERROR_OBJECT_WRONG_TYPE: u32 = 212;
pub const ERROR_DISK_WRITE_PROTECTED: u32 = 214;
pub const ERROR_DEVICE_NOT_MOUNTED: u32 = 218;
pub const ERROR_SEEK_ERROR: u32 = 219;
pub const ERROR_NO_MORE_ENTRIES: u32 = 232;
pub const ERROR_FILE_NOT_OBJECT: u32 = 121;

// FileHandle (dos/dosextens.h)
pub const FH_LINK: u32 = 0;
pub const FH_PORT: u32 = 4; // non-zero = interactive
pub const FH_TYPE: u32 = 8;
pub const FH_BUF: u32 = 12;
pub const FH_POS: u32 = 16;
pub const FH_END: u32 = 20;
pub const FH_FUNCS: u32 = 24;
pub const FH_FUNC2: u32 = 28;
pub const FH_FUNC3: u32 = 32;
pub const FH_ARGS: u32 = 36;
pub const FH_ARG2: u32 = 40;
pub const FH_SIZE: u32 = 44;

// FileLock
pub const FL_LINK: u32 = 0;
pub const FL_KEY: u32 = 4;
pub const FL_ACCESS: u32 = 8;
pub const FL_TASK: u32 = 12;
pub const FL_VOLUME: u32 = 16;
pub const FL_SIZE: u32 = 20;

// FileInfoBlock
pub const FIB_DISKKEY: u32 = 0;
pub const FIB_DIRENTRYTYPE: u32 = 4;
pub const FIB_FILENAME: u32 = 8; // 108 bytes, BCPL-style (length byte) in 1.3
pub const FIB_PROTECTION: u32 = 116;
pub const FIB_ENTRYTYPE: u32 = 120;
pub const FIB_SIZE: u32 = 124;
pub const FIB_NUMBLOCKS: u32 = 128;
pub const FIB_DATE: u32 = 132;
pub const FIB_COMMENT: u32 = 144; // 80 bytes
pub const FIB_STRUCT_SIZE: u32 = 260;
pub const ST_ROOT: i32 = 1;
pub const ST_USERDIR: i32 = 2;
pub const ST_FILE: i32 = -3;

// InfoData
pub const ID_NUMSOFTERRORS: u32 = 0;
pub const ID_UNITNUMBER: u32 = 4;
pub const ID_DISKSTATE: u32 = 8;
pub const ID_NUMBLOCKS: u32 = 12;
pub const ID_NUMBLOCKSUSED: u32 = 16;
pub const ID_BYTESPERBLOCK: u32 = 20;
pub const ID_DISKTYPE: u32 = 24;
pub const ID_VOLUMENODE: u32 = 28;
pub const ID_INUSE: u32 = 32;
pub const ID_WRITE_PROTECTED: u32 = 80;

// DosLibrary / RootNode / DosInfo / DeviceList (volume node). Offsets checked by ndk-check.sh;
// UNSURE: DOSLIB_SIZE, RN_SIZE, DI_SIZE (only the V34 ends of structures that grew later).
pub const DL_ROOT: u32 = 34;
pub const DOSLIB_SIZE: u32 = 54;
pub const RN_TASKARRAY: u32 = 0;
pub const RN_TIME: u32 = 8;
pub const RN_INFO: u32 = 24;
pub const RN_SIZE: u32 = 32;
pub const DI_DEVINFO: u32 = 4;
pub const DI_SIZE: u32 = 20;
pub const DOL_NEXT: u32 = 0;
pub const DOL_TYPE: u32 = 4;
pub const DOL_TASK: u32 = 8;
pub const DOL_LOCK: u32 = 12;
pub const DOL_VOLUMEDATE: u32 = 16;
pub const DOL_LOCKLIST: u32 = 28;
pub const DOL_DISKTYPE: u32 = 32;
pub const DOL_STARTUP: u32 = 28; // device nodes: BPTR to FileSysStartupMsg
pub const DOL_NAME: u32 = 40;
// FileSysStartupMsg (dos/filehandler.h)
pub const FSSM_UNIT: u32 = 0;
pub const FSSM_DEVICE: u32 = 4; // BSTR
pub const FSSM_ENVIRON: u32 = 8;
pub const FSSM_FLAGS: u32 = 12;
pub const FSSM_SIZE: u32 = 16;
pub const DOL_SIZE: u32 = 44;
pub const DLT_DEVICE: u32 = 0;
pub const DLT_VOLUME: u32 = 2;

// ---------------------------------------------------------------- graphics / intuition
/// GfxBase->DisplayFlags (NTSC = 1, GENLOC = 2, PAL = 4). Offset $CE: Rocket Ranger's init_load
/// reads this word. Other GfxBase fields are not modelled.
pub const GFX_DISPLAYFLAGS: u32 = 206;
pub const GFX_ACTIVIEW: u32 = 34;
pub const GFX_COPINIT: u32 = 38;
/// GfxBase->NormalDisplayRows / NormalDisplayColumns (read by Rocket Ranger's display_open to
/// centre its view).
pub const GFX_NORMALDISPLAYROWS: u32 = 216;
pub const GFX_NORMALDISPLAYCOLUMNS: u32 = 218;
/// IntuitionBase->ViewLord (a View at offset 34): DyOffset at 46, DxOffset at 48.
pub const IB_VIEWLORD: u32 = 34;
pub const GFX_SIZE: u32 = 0x200; // UNSURE: the V34 GfxBase is smaller than this (ends near $F0); extra is zero

// BitMap
pub const BM_BYTESPERROW: u32 = 0;
pub const BM_ROWS: u32 = 2;
pub const BM_FLAGS: u32 = 4;
pub const BM_DEPTH: u32 = 5;
pub const BM_PLANES: u32 = 8;
pub const BM_SIZE: u32 = 40;

// Remember (intuition)
pub const RM_NEXT: u32 = 0;
pub const RM_SIZE: u32 = 4;
pub const RM_MEMORY: u32 = 8;
pub const RM_STRUCT_SIZE: u32 = 12;

// ---------------------------------------------------------------- verified layouts (third round)
// Every constant below was generated from, and is checked by, scripts/ndk-check.sh (offsetof/sizeof
// in the NDK include files compiled with m68k-amigaos gcc). Sizes are the V34 sizes where a later
// NDK grew the structure (noted per structure).
// IntVector (exec/interrupts.h)
pub const IV_DATA: u32 = 0;
pub const IV_CODE: u32 = 4;
pub const IV_NODE: u32 = 8;
pub const IV_SIZE: u32 = 12;
// GfxBase (graphics/gfxbase.h)
pub const GFX_LOFLIST: u32 = 50;
pub const GFX_SHFLIST: u32 = 54;
pub const GFX_VBSRV: u32 = 74;
pub const GFX_TEXTFONTS: u32 = 140;
pub const GFX_DEFAULTFONT: u32 = 154;
pub const GFX_MODES: u32 = 158;
pub const GFX_SYSTEM_BPLCON0: u32 = 164;
pub const GFX_BLITLOCK: u32 = 170;
pub const GFX_BLITOWNER: u32 = 188;
pub const GFX_TOF_WAITQ: u32 = 192;
pub const GFX_MAXDISPLAYROW: u32 = 212;
pub const GFX_MAXDISPLAYCOLUMN: u32 = 214;
pub const GFX_MICROSPERLINE: u32 = 232;
pub const GFX_MINDISPLAYCOLUMN: u32 = 234;
// RastPort (graphics/rastport.h)
pub const RP_LAYER: u32 = 0;
pub const RP_BITMAP: u32 = 4;
pub const RP_AREAPTRN: u32 = 8;
pub const RP_TMPRAS: u32 = 12;
pub const RP_AREAINFO: u32 = 16;
pub const RP_GELSINFO: u32 = 20;
pub const RP_MASK: u32 = 24;
pub const RP_FGPEN: u32 = 25;
pub const RP_BGPEN: u32 = 26;
pub const RP_AOLPEN: u32 = 27;
pub const RP_DRAWMODE: u32 = 28;
pub const RP_AREAPTSZ: u32 = 29;
pub const RP_LINPATCNT: u32 = 30;
pub const RP_FLAGS: u32 = 32;
pub const RP_LINEPTRN: u32 = 34;
pub const RP_CPX: u32 = 36;
pub const RP_CPY: u32 = 38;
pub const RP_MINTERMS: u32 = 40;
pub const RP_PENWIDTH: u32 = 48;
pub const RP_PENHEIGHT: u32 = 50;
pub const RP_FONT: u32 = 52;
pub const RP_ALGOSTYLE: u32 = 56;
pub const RP_TXFLAGS: u32 = 57;
pub const RP_TXHEIGHT: u32 = 58;
pub const RP_TXWIDTH: u32 = 60;
pub const RP_TXBASELINE: u32 = 62;
pub const RP_TXSPACING: u32 = 64;
pub const RP_USER: u32 = 66;
pub const RP_SIZE: u32 = 100;
// TextFont (graphics/text.h)
pub const TF_YSIZE: u32 = 20;
pub const TF_STYLE: u32 = 22;
pub const TF_FLAGS: u32 = 23;
pub const TF_XSIZE: u32 = 24;
pub const TF_BASELINE: u32 = 26;
pub const TF_BOLDSMEAR: u32 = 28;
pub const TF_ACCESSORS: u32 = 30;
pub const TF_LOCHAR: u32 = 32;
pub const TF_HICHAR: u32 = 33;
pub const TF_CHARDATA: u32 = 34;
pub const TF_MODULO: u32 = 38;
pub const TF_CHARLOC: u32 = 40;
pub const TF_CHARSPACE: u32 = 44;
pub const TF_CHARKERN: u32 = 48;
pub const TF_SIZE: u32 = 52;
// TextAttr (graphics/text.h)
pub const TA_NAME: u32 = 0;
pub const TA_YSIZE: u32 = 4;
pub const TA_STYLE: u32 = 6;
pub const TA_FLAGS: u32 = 7;
pub const TA_SIZE: u32 = 8;
// ViewPort (graphics/view.h)
pub const VP_NEXT: u32 = 0;
pub const VP_COLORMAP: u32 = 4;
pub const VP_DSPINS: u32 = 8;
pub const VP_SPRINS: u32 = 12;
pub const VP_CLRINS: u32 = 16;
pub const VP_UCOPINS: u32 = 20;
pub const VP_DWIDTH: u32 = 24;
pub const VP_DHEIGHT: u32 = 26;
pub const VP_DXOFFSET: u32 = 28;
pub const VP_DYOFFSET: u32 = 30;
pub const VP_MODES: u32 = 32;
pub const VP_SPRITEPRIORITIES: u32 = 34;
pub const VP_EXTENDEDMODES: u32 = 35;
pub const VP_RASINFO: u32 = 36;
pub const VP_SIZE: u32 = 40;
// View (graphics/view.h)
pub const VIEW_VIEWPORT: u32 = 0;
pub const VIEW_LOF: u32 = 4;
pub const VIEW_SHF: u32 = 8;
pub const VIEW_DYOFFSET: u32 = 12;
pub const VIEW_DXOFFSET: u32 = 14;
pub const VIEW_MODES: u32 = 16;
pub const VIEW_SIZE: u32 = 18;
// RasInfo (graphics/view.h)
pub const RI_NEXT: u32 = 0;
pub const RI_BITMAP: u32 = 4;
pub const RI_RXOFFSET: u32 = 8;
pub const RI_RYOFFSET: u32 = 10;
pub const RI_SIZE: u32 = 12;
// ColorMap (graphics/view.h; the V34 structure ends at ColorTable)
pub const CM_FLAGS: u32 = 0;
pub const CM_TYPE: u32 = 1;
pub const CM_COUNT: u32 = 2;
pub const CM_COLORTABLE: u32 = 4;
pub const CM_SIZE: u32 = 8;
// cprlist (graphics/copper.h)
pub const CPR_NEXT: u32 = 0;
pub const CPR_START: u32 = 4;
pub const CPR_MAXCOUNT: u32 = 8;
pub const CPR_SIZE: u32 = 10;
// CopList (graphics/copper.h)
pub const CL_NEXT: u32 = 0;
pub const CL_COPINS: u32 = 12;
pub const CL_COPPTR: u32 = 16;
pub const CL_COPLSTART: u32 = 20;
pub const CL_COPSSTART: u32 = 24;
pub const CL_COUNT: u32 = 28;
pub const CL_MAXCOUNT: u32 = 30;
pub const CL_DYOFFSET: u32 = 32;
// UCopList (graphics/copper.h)
pub const UCL_NEXT: u32 = 0;
pub const UCL_FIRSTCOPLIST: u32 = 4;
pub const UCL_COPLIST: u32 = 8;
pub const UCL_SIZE: u32 = 12;
// CopIns (graphics/copper.h)
pub const COPINS_SIZE: u32 = 6;
// Rectangle (graphics/gfx.h)
pub const RECT_MINX: u32 = 0;
pub const RECT_MINY: u32 = 2;
pub const RECT_MAXX: u32 = 4;
pub const RECT_MAXY: u32 = 6;
// Layer (graphics/clip.h)
pub const LR_FRONT: u32 = 0;
pub const LR_BACK: u32 = 4;
pub const LR_CLIPRECT: u32 = 8;
pub const LR_RP: u32 = 12;
pub const LR_BOUNDS: u32 = 16;
pub const LR_FLAGS: u32 = 30;
pub const LR_SUPERBITMAP: u32 = 32;
pub const LR_WINDOW: u32 = 40;
pub const LR_SCROLL_X: u32 = 44;
pub const LR_SCROLL_Y: u32 = 46;
pub const LR_LAYERINFO: u32 = 68;
pub const LR_LOCK: u32 = 72;
// Layer_Info (graphics/layers.h)
pub const LI_TOP_LAYER: u32 = 0;
pub const LI_SIZE: u32 = 102;
// IntuitionBase (intuition/intuitionbase.h)
pub const IB_ACTIVEWINDOW: u32 = 52;
pub const IB_ACTIVESCREEN: u32 = 56;
pub const IB_FIRSTSCREEN: u32 = 60;
pub const IB_FLAGS: u32 = 64;
pub const IB_MOUSEY: u32 = 68;
pub const IB_MOUSEX: u32 = 70;
pub const IB_SECONDS: u32 = 72;
pub const IB_MICROS: u32 = 76;
// NewScreen (intuition/screens.h)
pub const NS_LEFTEDGE: u32 = 0;
pub const NS_TOPEDGE: u32 = 2;
pub const NS_WIDTH: u32 = 4;
pub const NS_HEIGHT: u32 = 6;
pub const NS_DEPTH: u32 = 8;
pub const NS_DETAILPEN: u32 = 10;
pub const NS_BLOCKPEN: u32 = 11;
pub const NS_VIEWMODES: u32 = 12;
pub const NS_TYPE: u32 = 14;
pub const NS_FONT: u32 = 16;
pub const NS_DEFAULTTITLE: u32 = 20;
pub const NS_GADGETS: u32 = 24;
pub const NS_CUSTOMBITMAP: u32 = 28;
pub const NS_SIZE: u32 = 32;
// Screen (intuition/screens.h)
pub const SC_NEXTSCREEN: u32 = 0;
pub const SC_FIRSTWINDOW: u32 = 4;
pub const SC_LEFTEDGE: u32 = 8;
pub const SC_TOPEDGE: u32 = 10;
pub const SC_WIDTH: u32 = 12;
pub const SC_HEIGHT: u32 = 14;
pub const SC_MOUSEY: u32 = 16;
pub const SC_MOUSEX: u32 = 18;
pub const SC_FLAGS: u32 = 20;
pub const SC_TITLE: u32 = 22;
pub const SC_DEFAULTTITLE: u32 = 26;
pub const SC_BARHEIGHT: u32 = 30;
pub const SC_BARVBORDER: u32 = 31;
pub const SC_BARHBORDER: u32 = 32;
pub const SC_MENUVBORDER: u32 = 33;
pub const SC_MENUHBORDER: u32 = 34;
pub const SC_WBORTOP: u32 = 35;
pub const SC_WBORLEFT: u32 = 36;
pub const SC_WBORRIGHT: u32 = 37;
pub const SC_WBORBOTTOM: u32 = 38;
pub const SC_FONT: u32 = 40;
pub const SC_VIEWPORT: u32 = 44;
pub const SC_RASTPORT: u32 = 84;
pub const SC_BITMAP: u32 = 184;
pub const SC_LAYERINFO: u32 = 224;
pub const SC_FIRSTGADGET: u32 = 326;
pub const SC_DETAILPEN: u32 = 330;
pub const SC_BLOCKPEN: u32 = 331;
pub const SC_SAVECOLOR0: u32 = 332;
pub const SC_BARLAYER: u32 = 334;
pub const SC_EXTDATA: u32 = 338;
pub const SC_USERDATA: u32 = 342;
pub const SC_SIZE: u32 = 346;
// NewWindow (intuition/intuition.h)
pub const NW_LEFTEDGE: u32 = 0;
pub const NW_TOPEDGE: u32 = 2;
pub const NW_WIDTH: u32 = 4;
pub const NW_HEIGHT: u32 = 6;
pub const NW_DETAILPEN: u32 = 8;
pub const NW_BLOCKPEN: u32 = 9;
pub const NW_IDCMPFLAGS: u32 = 10;
pub const NW_FLAGS: u32 = 14;
pub const NW_FIRSTGADGET: u32 = 18;
pub const NW_CHECKMARK: u32 = 22;
pub const NW_TITLE: u32 = 26;
pub const NW_SCREEN: u32 = 30;
pub const NW_BITMAP: u32 = 34;
pub const NW_MINWIDTH: u32 = 38;
pub const NW_MINHEIGHT: u32 = 40;
pub const NW_MAXWIDTH: u32 = 42;
pub const NW_MAXHEIGHT: u32 = 44;
pub const NW_TYPE: u32 = 46;
pub const NW_SIZE: u32 = 48;
// Window (intuition/intuition.h; the V34 structure ends at IFont)
pub const WD_NEXTWINDOW: u32 = 0;
pub const WD_LEFTEDGE: u32 = 4;
pub const WD_TOPEDGE: u32 = 6;
pub const WD_WIDTH: u32 = 8;
pub const WD_HEIGHT: u32 = 10;
pub const WD_MOUSEY: u32 = 12;
pub const WD_MOUSEX: u32 = 14;
pub const WD_MINWIDTH: u32 = 16;
pub const WD_MINHEIGHT: u32 = 18;
pub const WD_MAXWIDTH: u32 = 20;
pub const WD_MAXHEIGHT: u32 = 22;
pub const WD_FLAGS: u32 = 24;
pub const WD_MENUSTRIP: u32 = 28;
pub const WD_TITLE: u32 = 32;
pub const WD_FIRSTREQUEST: u32 = 36;
pub const WD_DMREQUEST: u32 = 40;
pub const WD_REQCOUNT: u32 = 44;
pub const WD_WSCREEN: u32 = 46;
pub const WD_RPORT: u32 = 50;
pub const WD_BORDERLEFT: u32 = 54;
pub const WD_BORDERTOP: u32 = 55;
pub const WD_BORDERRIGHT: u32 = 56;
pub const WD_BORDERBOTTOM: u32 = 57;
pub const WD_BORDERRPORT: u32 = 58;
pub const WD_FIRSTGADGET: u32 = 62;
pub const WD_PARENT: u32 = 66;
pub const WD_DESCENDANT: u32 = 70;
pub const WD_POINTER: u32 = 74;
pub const WD_PTRHEIGHT: u32 = 78;
pub const WD_PTRWIDTH: u32 = 79;
pub const WD_XOFFSET: u32 = 80;
pub const WD_YOFFSET: u32 = 81;
pub const WD_IDCMPFLAGS: u32 = 82;
pub const WD_USERPORT: u32 = 86;
pub const WD_WINDOWPORT: u32 = 90;
pub const WD_MESSAGEKEY: u32 = 94;
pub const WD_DETAILPEN: u32 = 98;
pub const WD_BLOCKPEN: u32 = 99;
pub const WD_CHECKMARK: u32 = 100;
pub const WD_SCREENTITLE: u32 = 104;
pub const WD_GZZMOUSEX: u32 = 108;
pub const WD_GZZMOUSEY: u32 = 110;
pub const WD_GZZWIDTH: u32 = 112;
pub const WD_GZZHEIGHT: u32 = 114;
pub const WD_EXTDATA: u32 = 116;
pub const WD_USERDATA: u32 = 120;
pub const WD_WLAYER: u32 = 124;
pub const WD_IFONT: u32 = 128;
pub const WD_SIZE: u32 = 132;
// IntuiMessage (intuition/intuition.h)
pub const IM_CLASS: u32 = 20;
pub const IM_CODE: u32 = 24;
pub const IM_QUALIFIER: u32 = 26;
pub const IM_IADDRESS: u32 = 28;
pub const IM_MOUSEX: u32 = 32;
pub const IM_MOUSEY: u32 = 34;
pub const IM_SECONDS: u32 = 36;
pub const IM_MICROS: u32 = 40;
pub const IM_IDCMPWINDOW: u32 = 44;
pub const IM_SPECIALLINK: u32 = 48;
pub const IM_SIZE: u32 = 52;
// Gadget (intuition/intuition.h)
pub const GG_NEXTGADGET: u32 = 0;
pub const GG_LEFTEDGE: u32 = 4;
pub const GG_TOPEDGE: u32 = 6;
pub const GG_WIDTH: u32 = 8;
pub const GG_HEIGHT: u32 = 10;
pub const GG_FLAGS: u32 = 12;
pub const GG_ACTIVATION: u32 = 14;
pub const GG_GADGETTYPE: u32 = 16;
pub const GG_GADGETRENDER: u32 = 18;
pub const GG_SELECTRENDER: u32 = 22;
pub const GG_GADGETTEXT: u32 = 26;
pub const GG_MUTUALEXCLUDE: u32 = 30;
pub const GG_SPECIALINFO: u32 = 34;
pub const GG_GADGETID: u32 = 38;
pub const GG_USERDATA: u32 = 40;
pub const GG_SIZE: u32 = 44;
// IntuiText (intuition/intuition.h)
pub const IT_FRONTPEN: u32 = 0;
pub const IT_BACKPEN: u32 = 1;
pub const IT_DRAWMODE: u32 = 2;
pub const IT_LEFTEDGE: u32 = 4;
pub const IT_TOPEDGE: u32 = 6;
pub const IT_ITEXTFONT: u32 = 8;
pub const IT_ITEXT: u32 = 12;
pub const IT_NEXTTEXT: u32 = 16;
pub const IT_SIZE: u32 = 20;
// InputEvent (devices/inputevent.h)
pub const IE_NEXTEVENT: u32 = 0;
pub const IE_CLASS: u32 = 4;
pub const IE_SUBCLASS: u32 = 5;
pub const IE_CODE: u32 = 6;
pub const IE_QUALIFIER: u32 = 8;
pub const IE_X: u32 = 10;
pub const IE_Y: u32 = 12;
pub const IE_TIMESTAMP: u32 = 14;
pub const IE_SIZE: u32 = 22;

// timer.device (devices/timer.h)
pub const TR_TIME: u32 = 32;
pub const TV_SECS: u32 = 0;
pub const TV_MICRO: u32 = 4;
pub const TR_SIZE: u32 = 40;
pub const UNIT_MICROHZ: u32 = 0;
pub const UNIT_VBLANK: u32 = 1;
pub const TR_ADDREQUEST: u32 = 9;
pub const TR_GETSYSTIME: u32 = 10;
pub const TR_SETSYSTIME: u32 = 11;
/// NT_SOFTINT: a soft interrupt queued by Cause (exec/nodes.h).
pub const NT_SOFTINT: u32 = 11;

// Image (intuition/intuition.h)
pub const IMG_LEFTEDGE: u32 = 0;
pub const IMG_TOPEDGE: u32 = 2;
pub const IMG_WIDTH: u32 = 4;
pub const IMG_HEIGHT: u32 = 6;
pub const IMG_DEPTH: u32 = 8;
pub const IMG_IMAGEDATA: u32 = 10;
pub const IMG_PLANEPICK: u32 = 14;
pub const IMG_PLANEONOFF: u32 = 15;
pub const IMG_NEXTIMAGE: u32 = 16;
pub const IMG_SIZE: u32 = 20;
// Border (intuition/intuition.h)
pub const BD_LEFTEDGE: u32 = 0;
pub const BD_TOPEDGE: u32 = 2;
pub const BD_FRONTPEN: u32 = 4;
pub const BD_BACKPEN: u32 = 5;
pub const BD_DRAWMODE: u32 = 6;
pub const BD_COUNT: u32 = 7;
pub const BD_XY: u32 = 8;
pub const BD_NEXTBORDER: u32 = 12;
pub const BD_SIZE: u32 = 16;

// Intuition flag and code values (intuition/intuition.h, intuition/screens.h, devices/inputevent.h)
pub const IDCMP_MOUSEBUTTONS: u32 = 0x8;
pub const IDCMP_MOUSEMOVE: u32 = 0x10;
pub const IDCMP_GADGETDOWN: u32 = 0x20;
pub const IDCMP_GADGETUP: u32 = 0x40;
pub const IDCMP_MENUPICK: u32 = 0x100;
pub const IDCMP_CLOSEWINDOW: u32 = 0x200;
pub const IDCMP_RAWKEY: u32 = 0x400;
pub const IDCMP_ACTIVEWINDOW: u32 = 0x40000;
pub const IDCMP_INACTIVEWINDOW: u32 = 0x80000;
pub const IDCMP_INTUITICKS: u32 = 0x400000;
pub const WFLG_REPORTMOUSE: u32 = 0x200;
pub const WFLG_BACKDROP: u32 = 0x100;
pub const WFLG_GIMMEZEROZERO: u32 = 0x400;
pub const WFLG_BORDERLESS: u32 = 0x800;
pub const WFLG_ACTIVATE: u32 = 0x1000;
pub const WFLG_WINDOWACTIVE: u32 = 0x2000;
pub const WFLG_RMBTRAP: u32 = 0x10000;
pub const GACT_RELVERIFY: u32 = 0x1;
pub const GACT_IMMEDIATE: u32 = 0x2;
pub const GACT_TOGGLESELECT: u32 = 0x100;
pub const GFLG_RELBOTTOM: u32 = 0x8;
pub const GFLG_RELRIGHT: u32 = 0x10;
pub const GFLG_RELWIDTH: u32 = 0x20;
pub const GFLG_RELHEIGHT: u32 = 0x40;
pub const GFLG_SELECTED: u32 = 0x80;
pub const GFLG_DISABLED: u32 = 0x100;
pub const WBENCHSCREEN: u32 = 0x1;
pub const CUSTOMSCREEN: u32 = 0xF;
pub const SCREENTYPE: u32 = 0xF;
pub const SHOWTITLE: u32 = 0x10;
pub const CUSTOMBITMAP: u32 = 0x40;
pub const SCREENBEHIND: u32 = 0x80;
pub const SCREENQUIET: u32 = 0x100;
pub const SELECTDOWN: u32 = 0x68;
pub const SELECTUP: u32 = 0xE8;
pub const MENUDOWN: u32 = 0x69;
pub const MENUUP: u32 = 0xE9;
pub const MENUNULL: u32 = 0xFFFF;
pub const IEQUALIFIER_RBUTTON: u32 = 0x2000;
pub const IEQUALIFIER_LEFTBUTTON: u32 = 0x4000;

// ---------------------------------------------------------------- LVO name tables
// Each table lists the functions from LVO -30 downwards in steps of 6; "" = reserved slot.
// The four standard library vectors -6..-24 are Open, Close, Expunge, Reserved (ExtFunc).

pub const EXEC_LVOS: &[&str] = &[
    "Supervisor", "ExitIntr", "Schedule", "Reschedule", "Switch", "Dispatch", "Exception", "InitCode",
    "InitStruct", "MakeLibrary", "MakeFunctions", "FindResident", "InitResident", "Alert", "Debug",
    "Disable", "Enable", "Forbid", "Permit", "SetSR", "SuperState", "UserState", "SetIntVector",
    "AddIntServer", "RemIntServer", "Cause", "Allocate", "Deallocate", "AllocMem", "AllocAbs",
    "FreeMem", "AvailMem", "AllocEntry", "FreeEntry", "Insert", "AddHead", "AddTail", "Remove",
    "RemHead", "RemTail", "Enqueue", "FindName", "AddTask", "RemTask", "FindTask", "SetTaskPri",
    "SetSignal", "SetExcept", "Wait", "Signal", "AllocSignal", "FreeSignal", "AllocTrap", "FreeTrap",
    "AddPort", "RemPort", "PutMsg", "GetMsg", "ReplyMsg", "WaitPort", "FindPort", "AddLibrary",
    "RemLibrary", "OldOpenLibrary", "CloseLibrary", "SetFunction", "SumLibrary", "AddDevice",
    "RemDevice", "OpenDevice", "CloseDevice", "DoIO", "SendIO", "CheckIO", "WaitIO", "AbortIO",
    "AddResource", "RemResource", "OpenResource", "RawIOInit", "RawMayGetChar", "RawPutChar",
    "RawDoFmt", "GetCC", "TypeOfMem", "Procure", "Vacate", "OpenLibrary", "InitSemaphore",
    "ObtainSemaphore", "ReleaseSemaphore", "AttemptSemaphore", "ObtainSemaphoreList",
    "ReleaseSemaphoreList", "FindSemaphore", "AddSemaphore", "RemSemaphore", "SumKickData",
    "AddMemList", "CopyMem", "CopyMemQuick",
    // V36+ (provided because CreateMsgPort/AllocVec are asked for; a 1.3 program never calls them)
    "CacheClearU", "CacheClearE", "CacheControl", "CreateIORequest", "DeleteIORequest",
    "CreateMsgPort", "DeleteMsgPort", "ObtainSemaphoreShared", "AllocVec", "FreeVec",
];

pub const DOS_LVOS: &[&str] = &[
    "Open", "Close", "Read", "Write", "Input", "Output", "Seek", "DeleteFile", "Rename", "Lock",
    "UnLock", "DupLock", "Examine", "ExNext", "Info", "CreateDir", "CurrentDir", "IoErr",
    "CreateProc", "Exit", "LoadSeg", "UnLoadSeg", "GetPacket", "QueuePacket", "DeviceProc",
    "SetComment", "SetProtection", "DateStamp", "Delay", "WaitForChar", "ParentDir",
    "IsInteractive", "Execute",
];

pub const GRAPHICS_LVOS: &[&str] = &[
    "BltBitMap", "BltTemplate", "ClearEOL", "ClearScreen", "TextLength", "Text", "SetFont",
    "OpenFont", "CloseFont", "AskSoftStyle", "SetSoftStyle", "AddBob", "AddVSprite", "DoCollision",
    "DrawGList", "InitGels", "InitMasks", "RemIBob", "RemVSprite", "SetCollision", "SortGList",
    "AddAnimOb", "Animate", "GetGBuffers", "InitGMasks", "DrawEllipse", "AreaEllipse", "LoadRGB4",
    "InitRastPort", "InitVPort", "MrgCop", "MakeVPort", "LoadView", "WaitBlit", "SetRast", "Move",
    "Draw", "AreaMove", "AreaDraw", "AreaEnd", "WaitTOF", "QBlit", "InitArea", "SetRGB4",
    "QBSBlit", "BltClear", "RectFill", "BltPattern", "ReadPixel", "WritePixel", "Flood",
    "PolyDraw", "SetAPen", "SetBPen", "SetDrMd", "InitView", "CBump", "CMove", "CWait",
    "VBeamPos", "InitBitMap", "ScrollRaster", "WaitBOVP", "GetSprite", "FreeSprite",
    "ChangeSprite", "MoveSprite", "LockLayerRom", "UnlockLayerRom", "SyncSBitMap", "CopySBitMap",
    "OwnBlitter", "DisownBlitter", "InitTmpRas", "AskFont", "AddFont", "RemFont", "AllocRaster",
    "FreeRaster", "AndRectRegion", "OrRectRegion", "NewRegion", "ClearRectRegion", "ClearRegion",
    "DisposeRegion", "FreeVPortCopLists", "FreeCopList", "ClipBlit", "XorRectRegion",
    "FreeCprList", "GetColorMap", "FreeColorMap", "GetRGB4", "ScrollVPort", "UCopperListInit",
    "FreeGBuffers", "BltBitMapRastPort", "OrRegionRegion", "XorRegionRegion", "AndRegionRegion",
    "SetRGB4CM", "BltMaskBitMapRastPort", "", "", "AttemptLockLayerRom",
];

pub const INTUITION_LVOS: &[&str] = &[
    "OpenIntuition", "Intuition", "AddGadget", "ClearDMRequest", "ClearMenuStrip", "ClearPointer",
    "CloseScreen", "CloseWindow", "CloseWorkBench", "CurrentTime", "DisplayAlert", "DisplayBeep",
    "DoubleClick", "DrawBorder", "DrawImage", "EndRequest", "GetDefPrefs", "GetPrefs",
    "InitRequester", "ItemAddress", "ModifyIDCMP", "ModifyProp", "MoveScreen", "MoveWindow",
    "OffGadget", "OffMenu", "OnGadget", "OnMenu", "OpenScreen", "OpenWindow", "OpenWorkBench",
    "PrintIText", "RefreshGadgets", "RemoveGadget", "ReportMouse", "Request", "ScreenToBack",
    "ScreenToFront", "SetDMRequest", "SetMenuStrip", "SetPointer", "SetWindowTitles", "ShowTitle",
    "SizeWindow", "ViewAddress", "ViewPortAddress", "WindowToBack", "WindowToFront",
    "WindowLimits", "SetPrefs", "IntuiTextLength", "WBenchToBack", "WBenchToFront", "AutoRequest",
    "BeginRefresh", "BuildSysRequest", "EndRefresh", "FreeSysRequest", "MakeScreen",
    "RemakeDisplay", "RethinkDisplay", "AllocRemember", "AlohaWorkbench", "FreeRemember",
    "LockIBase", "UnlockIBase",
    // V33/V34 additions (1.2/1.3)
    "GetScreenData", "RefreshGList", "AddGList", "RemoveGList", "ActivateWindow",
    "RefreshWindowFrame", "ActivateGadget", "NewModifyProp",
];

/// layers.library (checked against layers_lib.fd by lvo-check.sh).
pub const LAYERS_LVOS: &[&str] = &[
    "InitLayers", "CreateUpfrontLayer", "CreateBehindLayer", "UpfrontLayer", "BehindLayer",
    "MoveLayer", "SizeLayer", "ScrollLayer", "BeginUpdate", "EndUpdate", "DeleteLayer",
    "LockLayer", "UnlockLayer", "LockLayers", "UnlockLayers", "LockLayerInfo",
    "SwapBitsRastPortClipRect", "WhichLayer", "UnlockLayerInfo", "NewLayerInfo",
    "DisposeLayerInfo", "FattenLayerInfo", "ThinLayerInfo", "MoveLayerInFrontOf",
];

pub const MATHFFP_LVOS: &[&str] = &["SPFix", "SPFlt", "SPCmp", "SPTst", "SPAbs", "SPNeg", "SPAdd", "SPSub", "SPMul", "SPDiv", "SPFloor", "SPCeil"];

/// icon.library (checked against icon_lib.fd; the first six slots are private there, names from 1.3).
pub const ICON_LVOS: &[&str] = &[
    "GetWBObject", "PutWBObject", "GetIcon", "PutIcon", "FreeFreeList", "FreeWBObject",
    "AllocWBObject", "AddFreeList", "GetDiskObject", "PutDiskObject", "FreeDiskObject",
    "FindToolType", "MatchToolValue", "BumpRevision",
];

/// diskfont.library (checked against diskfont_lib.fd).
pub const DISKFONT_LVOS: &[&str] = &["OpenDiskFont", "AvailFonts"];

/// Devices: -30 BeginIO, -36 AbortIO (after the four standard vectors).
pub const DEVICE_LVOS: &[&str] = &["BeginIO", "AbortIO"];

/// Name of a standard vector (-6..-24) or of `lvo` in `table`.
pub fn lvo_name(table: &[&'static str], lvo: u32) -> &'static str {
    match lvo {
        6 => "Open",
        12 => "Close",
        18 => "Expunge",
        24 => "Reserved",
        _ if lvo >= 30 && lvo % 6 == 0 => table.get(((lvo - 30) / 6) as usize).copied().filter(|s| !s.is_empty()).unwrap_or("?"),
        _ => "?",
    }
}

/// LVO of `name` in `table` (inverse of [`lvo_name`]).
pub fn lvo_of(table: &[&'static str], name: &str) -> Option<u32> {
    table.iter().position(|n| *n == name).map(|i| 30 + 6 * i as u32)
}
