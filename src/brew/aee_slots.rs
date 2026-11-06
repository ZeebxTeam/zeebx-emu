//! Names of each interface's methods, in the order they occupy the vtable.
//!
//! Generated from the header declarations — the `INHERIT_*` macros of BREW SDK 4.0.2, the
//! `QINTERFACE(IGraphics)` of `AEEGraphics.h` (which uses the old style, without an inheritance
//! macro) and, for `IHID`/`IHIDDevice`, the headers of the Zeebo's own SDK. The slot order is
//! ABI: getting one wrong misaligns every following call. The `IFileMgr` table matches the vtable
//! reverse engineering extracted from the console's firmware
//! (`docs/vendor/tripleoxygen/research/brew/vtbl.ods`).

/// Methods of `IShell` (52 slots).
pub const SHELL: &[&str] = &[
    "AddRef",
    "Release",
    "CreateInstance",
    "QueryClass",
    "GetDeviceInfo",
    "StartApplet",
    "CloseApplet",
    "CanStartApplet",
    "ActiveApplet",
    "EnumAppletInit",
    "EnumNextApplet",
    "SetTimer",
    "CancelTimer",
    "GetTimerExpiration",
    "CreateDialog",
    "GetActiveDialog",
    "EndDialog",
    "LoadResString",
    "LoadResData",
    "LoadResObject",
    "FreeResData",
    "SendEvent",
    "Beep",
    "GetPrefs",
    "SetPrefs",
    "GetItemStyle",
    "Prompt",
    "MessageBox",
    "MessageBoxText",
    "SetAlarm",
    "CancelAlarm",
    "AlarmsActive",
    "GetHandler",
    "RegisterHandler",
    "RegisterNotify",
    "Notify",
    "Resume",
    "ForceExit",
    "GetPosition",
    "CheckPrivLevel",
    "IsValidResource",
    "LoadResDataEx",
    "RegisterSystemCallback",
    "DetectType",
    "GetDeviceInfoEx",
    "GetClassItemID",
    "Obsolete",
    "GetProperty",
    "SetProperty",
    "RegisterEvent",
    "Reset",
    "AppIsInGroup",
];

/// Methods of `IModule` (4 slots).
pub const MODULE: &[&str] = &["AddRef", "Release", "CreateInstance", "FreeResources"];

/// Methods of `IApplet` (3 slots).
pub const APPLET: &[&str] = &["AddRef", "Release", "HandleEvent"];

/// Methods of `IFileMgr` (21 slots).
pub const FILEMGR: &[&str] = &[
    "AddRef",
    "Release",
    "OpenFile",
    "GetInfo",
    "Remove",
    "MkDir",
    "RmDir",
    "Test",
    "GetFreeSpace",
    "GetLastError",
    "EnumInit",
    "EnumNext",
    "Rename",
    "EnumNextEx",
    "SetDescription",
    "GetInfoEx",
    "Use",
    "GetFileUseInfo",
    "ResolvePath",
    "CheckPathAccess",
    "GetFreeSpaceEx",
];

/// Methods of `IFile` (12 slots).
pub const FILE: &[&str] = &[
    "AddRef",
    "Release",
    "Readable",
    "Read",
    "Cancel",
    "Write",
    "GetInfo",
    "Seek",
    "Truncate",
    "GetInfoEx",
    "SetCacheSize",
    "Map",
];

/// Methods of `IDisplay` (26 slots).
pub const DISPLAY: &[&str] = &[
    "AddRef",
    "Release",
    "GetFontMetrics",
    "MeasureTextEx",
    "DrawText",
    "DrawRect",
    "BitBlt",
    "Update",
    "SetAnnunciators",
    "Backlight",
    "SetColor",
    "GetSymbol",
    "DrawFrame",
    "CreateDIBitmap",
    "SetDestination",
    "GetDestination",
    "GetDeviceBitmap",
    "SetFont",
    "SetClipRect",
    "GetClipRect",
    "Clone",
    "MakeDefault",
    "IsEnabled",
    "NotifyEnable",
    "CreateDIBitmapEx",
    "SetPrefs",
];

/// Methods of `IBitmap` (16 slots).
pub const BITMAP: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "RGBToNative",
    "NativeToRGB",
    "DrawPixel",
    "GetPixel",
    "SetPixels",
    "DrawHScanline",
    "FillRect",
    "BltIn",
    "BltOut",
    "GetInfo",
    "CreateCompatibleBitmap",
    "SetTransparencyColor",
    "GetTransparencyColor",
];

/// Methods of the canvas `0x0101e443`. Only slot 7 is called; the others stay as placeholders so
/// that an unexpected call shows up in the report.
pub const CANVAS: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "slot3",
    "slot4",
    "slot5",
    "slot6",
    "GetDisplay",
];

/// Methods of `ITransform` (5 slots), from `AEETransform.h`.
pub const TRANSFORM: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "TransformBltSimple",
    "TransformBltComplex",
];

/// Methods of `IHID` (8 slots).
pub const HID: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "CreateDevice",
    "GetDeviceInfo",
    "GetNextConnectEvent",
    "RegisterForConnectEvents",
    "GetConnectedDevices",
];

/// Methods of `IHIDDevice` (19 slots).
pub const HIDDEVICE: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "GetDeviceInfo",
    "GetDeviceStatus",
    "RegisterForStatusChange",
    "GetButtonInfo",
    "GetNumberOfButtons",
    "RegisterForButtonEvent",
    "GetNextButtonEvent",
    "GetPositionState",
    "GetMinPositionInfo",
    "GetMaxPositionInfo",
    "GetAxesInfo",
    "RegisterForPositionChange",
    "SetExclusiveLevel",
    "GetExclusiveLevel",
    "Rumble",
    "GetRumbleStatus",
];

/// Methods of `ISignal` (4 slots).
pub const SIGNAL: &[&str] = &["AddRef", "Release", "QueryInterface", "Set"];

/// Methods of `ISignalCtl` (6 slots).
pub const SIGNALCTL: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "Set",
    "Detach",
    "Enable",
];

/// Methods of `ISignalCBFactory` (4 slots).
pub const SIGNALCBFACTORY: &[&str] = &["AddRef", "Release", "QueryInterface", "CreateSignal"];

/// Methods of `IGraphics` (44 slots).
pub const GRAPHICS: &[&str] = &[
    "AddRef",
    "Release",
    "SetBackground",
    "GetBackground",
    "SetColor",
    "GetColor",
    "SetFillMode",
    "GetFillMode",
    "SetFillColor",
    "GetFillColor",
    "SetPointSize",
    "GetPointSize",
    "SetClip",
    "GetClip",
    "SetViewport",
    "GetViewport",
    "ClearViewport",
    "SetPaintMode",
    "GetPaintMode",
    "GetColorDepth",
    "DrawPoint",
    "DrawLine",
    "DrawRect",
    "DrawCircle",
    "DrawArc",
    "DrawPie",
    "DrawEllipse",
    "DrawTriangle",
    "DrawPolygon",
    "DrawPolyline",
    "ClearRect",
    "EnableDoubleBuffer",
    "Update",
    "Translate",
    "Pan",
    "StretchBlt",
    "SetAlgorithmHint",
    "GetAlgorithmHint",
    "SetDestination",
    "GetDestination",
    "SetStrokeStyle",
    "GetStrokeStyle",
    "DrawEllipticalArc",
    "DrawRoundRectangle",
];

/// Methods of `ISound` (15 slots), from the `INHERIT_ISound` macro in `inc/AEEISound.h`.
pub const SOUND: &[&str] = &[
    "AddRef",
    "Release",
    "RegisterNotify",
    "Set",
    "Get",
    "SetDevice",
    "PlayTone",
    "PlayToneList",
    "PlayFreqTone",
    "StopTone",
    "Vibrate",
    "StopVibrate",
    "SetVolume",
    "GetVolume",
    "GetResourceCtl",
];

/// Methods of `ILicense` (6 slots), from `QINTERFACE(ILicense)` in `sdk/inc/AEELicense.h`.
pub const LICENSE: &[&str] = &[
    "AddRef",
    "Release",
    "IsExpired",
    "GetInfo",
    "SetUsesRemaining",
    "GetPurchaseInfo",
];

/// Methods of `IMemAStream` (7 slots), from `INHERIT_IMemAStream` in `sdk/inc/AEE.h`: the
/// `IAStream` ones (`Readable`, `Read`, `Cancel`) plus `Set` and `SetEx`.
pub const MEMASTREAM: &[&str] = &[
    "AddRef", "Release", "Readable", "Read", "Cancel", "Set", "SetEx",
];

/// Methods of `IImage` (11 slots), from `INHERIT_IImage` in `inc/AEEIImage.h`.
/// Methods of `IEGLSurfaceManip` (27 slots), from `INHERIT_IEGLSurfaceManip` in
/// `sdk/inc/AEEEGLSurfaceManip.h`: the three of `IQI`, the fourteen of V1 and the ten V2 adds. V2
/// is a superset of V1 with the same prefix, so one table serves both.
pub const EGL_SURFACE_MANIP: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "SurfaceScaleEnable",
    "SetSurfaceScale",
    "GetSurfaceScale",
    "GetSurfaceScaleCaps",
    "SurfaceRotateEnable",
    "SetSurfaceRotate",
    "GetSurfaceRotate",
    "GetSurfaceRotateCaps",
    "SurfaceTransparencyEnable",
    "SetSurfaceTransparency",
    "GetSurfaceTransparency",
    "SetSurfaceTransparencyMap",
    "GetSurfaceTransparencyMap",
    "GetSurfaceTransparencyCaps",
    "SurfaceColorKeyEnable",
    "SetSurfaceColorKey",
    "GetSurfaceColorKey",
    "CreateCompositeSurface",
    "SurfaceOverlayEnable",
    "SurfaceOverlayLayerEnable",
    "SurfaceOverlayBind",
    "GetSurfaceOverlayBinding",
    "GetSurfaceOverlay",
    "GetSurfaceOverlayCaps",
];

/// Methods of `IGLESImageonExt` (27 slots), from `INHERIT_IGLESImageonExt` in
/// `sdk/inc/AEEGLESImageonEXT.h`: the ATI Imageon extras on top of OpenGL ES 1.0.
pub const GLES_IMAGEON_EXT: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "PointSizePointerOES",
    "BlendEquationSeparateEXT",
    "BlendFuncSeparateEXT",
    "BlendEquationEXT",
    "BindBufferQUALCOMM",
    "DeleteBuffersQUALCOMM",
    "GenBuffersQUALCOMM",
    "BufferDataQUALCOMM",
    "BufferSubDataQUALCOMM",
    "IsBufferQUALCOMM",
    "BufferDataATI",
    "MeshListATI",
    "DrawVertexBufferObjectATI",
    "GetPointerv",
    "TexEnvi",
    "TexEnviv",
    "TexParameteri",
    "TexParameteriv",
    "TexParameterfv",
    "TexParameterxv",
    "GetMaterialfv",
    "GetTexParameteriv",
    "GetTexParameterfv",
    "GetTexParameterxv",
];

/// Methods of `IImageDecoder` (5 slots), from `INHERIT_IImageDecoder` in
/// `inc/AEEIImageDecoder.h`: the three of `IQI` and its own two.
pub const IMAGE_DECODER: &[&str] = &["AddRef", "Release", "QueryInterface", "GetBitmap", "GetRop"];

/// Methods of `IForceFeed` (5 slots), from `INHERIT_IForceFeed` in `inc/AEEIForceFeed.h`: the
/// three of `IQI`, `Write` and `Reset`. It is how data gets into a decoder.
pub const FORCE_FEED: &[&str] = &["AddRef", "Release", "QueryInterface", "Write", "Reset"];

pub const IMAGE: &[&str] = &[
    "AddRef",
    "Release",
    "Draw",
    "DrawFrame",
    "GetInfo",
    "SetParm",
    "Start",
    "Stop",
    "SetStream",
    "HandleEvent",
    "Notify",
];

/// Methods of `IThread` (12 slots), from `INHERIT_IThread` in `sdk/inc/AEEThread.h`: the three
/// of `IQI`, the four of `IRscPool` (`inc/AEEIRscPool.h`) and the thread's own five.
pub const THREAD: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "Malloc",
    "Free",
    "HoldRsc",
    "ReleaseRsc",
    "Start",
    "Exit",
    "Join",
    "Suspend",
    "GetResumeCBK",
];

/// Methods of `IEGL11` (31 slots), from `INHERIT_IEGL10`/`INHERIT_IEGL11` in
/// `sdk/inc/AEEEGL10.h` and `AEEEGL11.h`.
///
/// It is the new form of BREW interfaces, and the one Quake uses: each method receives `this`,
/// returns an error code and delivers the result through an output pointer — the last argument.
/// The old `IEGL`, from `AEEGL.h`, has another convention and is not this one.
pub const EGL: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "GetError",
    "GetDisplay",
    "Initialize",
    "Terminate",
    "QueryString",
    "GetConfigs",
    "ChooseConfig",
    "GetConfigAttrib",
    "CreateWindowSurface",
    "CreatePixmapSurface",
    "CreatePbufferSurface",
    "DestroySurface",
    "QuerySurface",
    "CreateContext",
    "DestroyContext",
    "MakeCurrent",
    "GetCurrentContext",
    "GetCurrentSurface",
    "GetCurrentDisplay",
    "QueryContext",
    "WaitGL",
    "WaitNative",
    "SwapBuffers",
    "CopyBuffers",
    "SurfaceAttrib",
    "BindTexImage",
    "ReleaseTexImage",
    "SwapInterval",
];

/// Methods of `IGLES11` (148 slots), from `INHERIT_IGLES10`/`INHERIT_IGLES11` in
/// `sdk/inc/AEEGLES10.h` and `AEEGLES11.h` — BREW's OpenGL ES 1.1.
///
/// Same convention as [`EGL`]: `this` in the first argument, an error code as the return value and
/// the result through an output pointer. An `IGLES11` object also serves as `IGLES10`, because the
/// second table only extends the first.
/// `IGLES11ExtPak`: the package's OES extensions. See `AEEGLES11ExtPak.h`.
///
/// Three families: texture coordinate generation (`TexGen`), per-equation separate blending and
/// the framebuffer and renderbuffer objects. **It is the last door between Prey Evil and
/// drawing** — the other five have already been delivered, each one measured.
pub const GLES11_EXT_PAK: &[&str] = &[
    "AddRef", "Release", "QueryInterface",
    "GetTexGenfv", "GetTexGeniv", "GetTexGenxv",
    "TexGenf", "TexGeni", "TexGenx", "TexGenfv", "TexGeniv", "TexGenxv",
    "BlendEquation", "BlendFuncSeparate", "BlendEquationSeparate",
    "BindFramebufferOES", "BindRenderbufferOES", "CheckFramebufferStatusOES",
    "DeleteFramebuffersOES", "DeleteRenderbuffersOES",
    "FramebufferRenderbufferOES", "FramebufferTexture2DOES",
    "GenerateMipmapOES", "GenFramebuffersOES", "GenRenderbuffersOES",
    "GetFramebufferAttachmentParameterivOES", "GetRenderbufferParameterivOES",
    "IsFramebufferOES", "IsRenderbufferOES", "RenderbufferStorageOES",
];

/// `IGLES11Ext`: the OES extensions of OpenGL ES 1.1, in the order of `AEEGLES11Ext.h`.
///
/// **Prey Evil asks for this interface through `CreateInstance` and stops drawing without it**:
/// the survey of the 62 ROMs showed eleven GL methods, no drawing and a black screen. The
/// `DrawTex*` ones are what matter for a game that builds the frame in a texture.
/// `IGLES10Ext`: an OpenGL ES 1.0 extension. See `AEEGLES10Ext.h`.
///
/// A single method, and it is what Prey Evil uses to learn that the extension exists.
pub const GLES10_EXT: &[&str] = &["AddRef", "Release", "QueryInterface", "QueryMatrixxOES"];

/// `IJoystick`: the USB joystick, in the order of `AEEJoystick.h`.
///
/// **It is what Qualcomm's joystick manager asks for** (`gamepadmgr.cpp`, which Prey Evil uses):
/// it creates the interface, and without it stores null and crashes on the first `Read`. Six
/// slots.
pub const JOYSTICK: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "SetParm",
    "GetParm",
    "Read",
];

/// `IEGLGetPowerLevel`: the battery level. See `AEEEGLGetPowerLevel.h`.
pub const EGL_GET_POWER_LEVEL: &[&str] =
    &["AddRef", "Release", "QueryInterface", "GetPowerLevel"];

/// `IEGLGetColorBuffer`: EGL's color buffer. See `AEEEGLGetColorBuffer.h`.
///
/// It is the interface counterpart of the `eglGetColorBufferQUALCOMM` function, and the two share
/// the computation.
pub const EGL_GET_COLOR_BUFFER: &[&str] =
    &["AddRef", "Release", "QueryInterface", "GetColorBuffer"];

/// `IEGLOESSwapInterval`: the frame pacing the game asks for. See `AEEEGLOESSwapInterval.h`.
///
/// As a **function** the engine already answers `SwapIntervalOES`; here it is the same answer,
/// through the interface.
pub const EGL_OES_SWAP_INTERVAL: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "SwapInterval",
    "GetSwapInterval",
];

pub const GLES11_EXT: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "CurrentPaletteMatrixOES",
    "LoadPaletteFromModelViewMatrixOES",
    "MatrixIndexPointerOES",
    "WeightPointerOES",
    "DrawTexsOES",
    "DrawTexiOES",
    "DrawTexxOES",
    "DrawTexsvOES",
    "DrawTexivOES",
    "DrawTexxvOES",
    "DrawTexfOES",
    "DrawTexfvOES",
];

pub const GLES: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "AlphaFunc",
    "ClearColor",
    "ClearDepthf",
    "Color4f",
    "DepthRangef",
    "Fogf",
    "Fogfv",
    "Frustumf",
    "LightModelf",
    "LightModelfv",
    "Lightf",
    "Lightfv",
    "LineWidth",
    "LoadMatrixf",
    "Materialf",
    "Materialfv",
    "MultMatrixf",
    "MultiTexCoord4f",
    "Normal3f",
    "Orthof",
    "PointSize",
    "PolygonOffset",
    "Rotatef",
    "Scalef",
    "TexEnvf",
    "TexEnvfv",
    "TexParameterf",
    "Translatef",
    "ActiveTexture",
    "AlphaFuncx",
    "BindTexture",
    "BlendFunc",
    "Clear",
    "ClearColorx",
    "ClearDepthx",
    "ClearStencil",
    "ClientActiveTexture",
    "Color4x",
    "ColorMask",
    "ColorPointer",
    "CompressedTexImage2D",
    "CompressedTexSubImage2D",
    "CopyTexImage2D",
    "CopyTexSubImage2D",
    "CullFace",
    "DeleteTextures",
    "DepthFunc",
    "DepthMask",
    "DepthRangex",
    "Disable",
    "DisableClientState",
    "DrawArrays",
    "DrawElements",
    "Enable",
    "EnableClientState",
    "Finish",
    "Flush",
    "Fogx",
    "Fogxv",
    "FrontFace",
    "Frustumx",
    "GenTextures",
    "GetError",
    "GetIntegerv",
    "GetString",
    "Hint",
    "LightModelx",
    "LightModelxv",
    "Lightx",
    "Lightxv",
    "LineWidthx",
    "LoadIdentity",
    "LoadMatrixx",
    "LogicOp",
    "Materialx",
    "Materialxv",
    "MatrixMode",
    "MultMatrixx",
    "MultiTexCoord4x",
    "Normal3x",
    "NormalPointer",
    "Orthox",
    "PixelStorei",
    "PointSizex",
    "PolygonOffsetx",
    "PopMatrix",
    "PushMatrix",
    "ReadPixels",
    "Rotatex",
    "SampleCoverage",
    "SampleCoveragex",
    "Scalex",
    "Scissor",
    "ShadeModel",
    "StencilFunc",
    "StencilMask",
    "StencilOp",
    "TexCoordPointer",
    "TexEnvx",
    "TexEnvxv",
    "TexImage2D",
    "TexParameterx",
    "TexSubImage2D",
    "Translatex",
    "VertexPointer",
    "Viewport",
    "ClipPlanef",
    "GetClipPlanef",
    "GetFloatv",
    "GetLightfv",
    "GetMaterialfv",
    "GetTexEnvfv",
    "GetTexParameterfv",
    "PointParameterf",
    "PointParameterfv",
    "TexParameterfv",
    "BindBuffer",
    "BufferData",
    "BufferSubData",
    "ClipPlanex",
    "Color4ub",
    "DeleteBuffers",
    "GetBooleanv",
    "GetBufferParameteriv",
    "GetClipPlanex",
    "GenBuffers",
    "GetFixedv",
    "GetLightxv",
    "GetMaterialxv",
    "GetPointerv",
    "GetTexEnviv",
    "GetTexEnvxv",
    "GetTexParameteriv",
    "GetTexParameterxv",
    "IsBuffer",
    "IsEnabled",
    "IsTexture",
    "PointParameterx",
    "PointParameterxv",
    "TexEnvi",
    "TexEnviv",
    "TexParameteri",
    "TexParameteriv",
    "TexParameterxv",
    "PointSizePointerOES",
    // `GL_OES_draw_texture`. They are not part of the `IGLES` vtable: the game reaches them
    // through `eglGetProcAddress`, and so they go at the end — inserting them in the middle would
    // shift the real slots.
    "DrawTexsOES",
    "DrawTexiOES",
    "DrawTexxOES",
    "DrawTexfOES",
    "DrawTexsvOES",
    "DrawTexivOES",
    "DrawTexxvOES",
    "DrawTexfvOES",
];

/// Methods of `IMediaUtil` (6 slots), from `AEEINTERFACE(IMediaUtil)` in
/// `sdk/inc/AEEMediaUtil.h`.
pub const MEDIAUTIL: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "CreateMedia",
    "EncodeMedia",
    "CreateMediaEx",
];

/// Methods of `IMedia` (14 slots), from `INHERIT_IMedia` in `inc/AEEIMedia.h`.
pub const MEDIA: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "RegisterNotify",
    "SetMediaParm",
    "GetMediaParm",
    "Play",
    "Record",
    "Stop",
    "Seek",
    "Pause",
    "Resume",
    "GetTotalTime",
    "GetState",
];

/// Methods of `IEGL` (28 slots), from `AEEINTERFACE(IEGL)` in `sdk/inc/AEEGL.h`.
///
/// It is the **old** form: apart from the three of `IQueryInterface`, the methods do not receive
/// `this` — the SDK macro calls `AEEGETPVTBL(p,IEGL)->eglGetDisplay(a)` without passing `p` on —
/// and the result is the return value, not an output pointer. Dropping the `egl` prefix, the
/// names match those of [`EGL`], which is what lets the same code serve both.
pub const EGL_LEGACY: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "eglGetError",
    "eglGetDisplay",
    "eglInitialize",
    "eglTerminate",
    "eglQueryString",
    "eglGetProcAddress",
    "eglGetConfigs",
    "eglChooseConfig",
    "eglGetConfigAttrib",
    "eglCreateWindowSurface",
    "eglCreatePixmapSurface",
    "eglCreatePbufferSurface",
    "eglDestroySurface",
    "eglQuerySurface",
    "eglCreateContext",
    "eglDestroyContext",
    "eglMakeCurrent",
    "eglGetCurrentContext",
    "eglGetCurrentSurface",
    "eglGetCurrentDisplay",
    "eglQueryContext",
    "eglWaitGL",
    "eglWaitNative",
    "eglSwapBuffers",
    "eglCopyBuffers",
    // `EGL_QUALCOMM_get_color_buffer`. It is not part of the vtable: the game reaches it through
    // `eglGetProcAddress`, and so it goes at the end — inserting it in the middle would shift the
    // real slots, which is the same care taken with the `DrawTex*OES` in the GLES table.
    //
    // The Z-Wheel draws the stage into a **pbuffer**, not a window, and needs the color buffer's
    // pointer to composite it with the 2D. Without this function it gives up: its log says
    // `eglGetProcAddress('eglGetColorBufferQUALCOMM') failed.`, followed by tearing down the
    // context and `CreateStageWidget failed, proceeding...`.
    "eglGetColorBufferQUALCOMM",
    // `EGL_QUALCOMM_surface_scale`, by the same path and for the same reason: the game resolves
    // the four names through `eglGetProcAddress` and **tests all four against null at once**. In
    // the Z-Wheel this is at `0x5d7ac`..`0x5d7c8` of `tectoy.mod`: if any one is missing, it
    // zeroes the whole group and takes another path.
    //
    // The methods already exist in [`EGL_SURFACE_MANIP`], which is the same extension exposed as
    // an interface; here they appear as C functions, without `this` and returning the value.
    // Resolved together with the four scale ones and stored in the same block (`+0x34`), right
    // before them. Leaving it null while the others exist is worse than null in all of them: the
    // game reads that field **after** seeing the group filled in.
    "eglSwapIntervalOES",
    "eglSurfaceScaleEnableQUALCOMM",
    "eglSetSurfaceScaleQUALCOMM",
    "eglGetSurfaceScaleQUALCOMM",
    "eglGetSurfaceScaleCapsQUALCOMM",
];

/// Methods of `IGL` (80 slots), from `AEEINTERFACE(IGL)` in `sdk/inc/AEEGL.h` — OpenGL ES 1.0
/// Common-Lite —, followed by the functions only [`GLES`] has. Same old convention as
/// [`EGL_LEGACY`]; without the `gl` prefix, the names match those of [`GLES`].
pub const GL_LEGACY: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "glActiveTexture",
    "glAlphaFuncx",
    "glBindTexture",
    "glBlendFunc",
    "glClear",
    "glClearColorx",
    "glClearDepthx",
    "glClearStencil",
    "glClientActiveTexture",
    "glColor4x",
    "glColorMask",
    "glColorPointer",
    "glCompressedTexImage2D",
    "glCompressedTexSubImage2D",
    "glCopyTexImage2D",
    "glCopyTexSubImage2D",
    "glCullFace",
    "glDeleteTextures",
    "glDepthFunc",
    "glDepthMask",
    "glDepthRangex",
    "glDisable",
    "glDisableClientState",
    "glDrawArrays",
    "glDrawElements",
    "glEnable",
    "glEnableClientState",
    "glFinish",
    "glFlush",
    "glFogx",
    "glFogxv",
    "glFrontFace",
    "glFrustumx",
    "glGenTextures",
    "glGetError",
    "glGetIntegerv",
    "glGetString",
    "glHint",
    "glLightModelx",
    "glLightModelxv",
    "glLightx",
    "glLightxv",
    "glLineWidthx",
    "glLoadIdentity",
    "glLoadMatrixx",
    "glLogicOp",
    "glMaterialx",
    "glMaterialxv",
    "glMatrixMode",
    "glMultMatrixx",
    "glMultiTexCoord4x",
    "glNormal3x",
    "glNormalPointer",
    "glOrthox",
    "glPixelStorei",
    "glPointSizex",
    "glPolygonOffsetx",
    "glPopMatrix",
    "glPushMatrix",
    "glReadPixels",
    "glRotatex",
    "glSampleCoveragex",
    "glScalex",
    "glScissor",
    "glShadeModel",
    "glStencilFunc",
    "glStencilMask",
    "glStencilOp",
    "glTexCoordPointer",
    "glTexEnvx",
    "glTexEnvxv",
    "glTexImage2D",
    "glTexParameterx",
    "glTexSubImage2D",
    "glTranslatex",
    "glVertexPointer",
    "glViewport",
    // From here on it is no longer `AEEGL.h`: these are the functions only `IGLES11` has — the
    // floating-point ones, the buffer ones and the extension ones —, in the same convention
    // without `this`. They are how `eglGetProcAddress` hands out a real C function pointer. The
    // start of the table, which is the `IGL` vtable, does not change.
    "glAlphaFunc",
    "glClearColor",
    "glClearDepthf",
    "glColor4f",
    "glDepthRangef",
    "glFogf",
    "glFogfv",
    "glFrustumf",
    "glLightModelf",
    "glLightModelfv",
    "glLightf",
    "glLightfv",
    "glLineWidth",
    "glLoadMatrixf",
    "glMaterialf",
    "glMaterialfv",
    "glMultMatrixf",
    "glMultiTexCoord4f",
    "glNormal3f",
    "glOrthof",
    "glPointSize",
    "glPolygonOffset",
    "glRotatef",
    "glScalef",
    "glTexEnvf",
    "glTexEnvfv",
    "glTexParameterf",
    "glTranslatef",
    "glSampleCoverage",
    "glClipPlanef",
    "glGetClipPlanef",
    "glGetFloatv",
    "glGetLightfv",
    "glGetMaterialfv",
    "glGetTexEnvfv",
    "glGetTexParameterfv",
    "glPointParameterf",
    "glPointParameterfv",
    "glTexParameterfv",
    "glBindBuffer",
    "glBufferData",
    "glBufferSubData",
    "glClipPlanex",
    "glColor4ub",
    "glDeleteBuffers",
    "glGetBooleanv",
    "glGetBufferParameteriv",
    "glGetClipPlanex",
    "glGenBuffers",
    "glGetFixedv",
    "glGetLightxv",
    "glGetMaterialxv",
    "glGetPointerv",
    "glGetTexEnviv",
    "glGetTexEnvxv",
    "glGetTexParameteriv",
    "glGetTexParameterxv",
    "glIsBuffer",
    "glIsEnabled",
    "glIsTexture",
    "glPointParameterx",
    "glPointParameterxv",
    "glTexEnvi",
    "glTexEnviv",
    "glTexParameteri",
    "glTexParameteriv",
    "glTexParameterxv",
    "glPointSizePointerOES",
    "glDrawTexsOES",
    "glDrawTexiOES",
    "glDrawTexxOES",
    "glDrawTexfOES",
    "glDrawTexsvOES",
    "glDrawTexivOES",
    "glDrawTexxvOES",
    "glDrawTexfvOES",
];

/// Methods of `IWeb`, read from the firmware's vtable at `0x1087fde0`.
///
/// `AEEWeb.h` was dropped from SDK 4.0.2 — the interface was retired —, and for a long time this
/// table had four entries deduced from usage. There are **thirteen**, and the deduction got the
/// main thing wrong.
///
/// What the firmware corrects:
///
/// - **Slot 2 is `QueryInterface`**, not `GetResponse`. The implementation accepts
///   `0x01000001`, `0x01005004` and `0x01001031` and returns the object itself. In other words,
///   `IWeb` follows `IQI` like the other interfaces, and the `DECLARE_IBASE` hypothesis was wrong.
/// - **Slot 3 really is `AddOpt`**: `ldr r0,[r0,#0xc]; blx …`, passing on the pointer it
///   receives. It was the only one of the four deductions that was right, and the Boomerang
///   supported it.
/// - **Slot 6 is a shortcut for `AddOpt`.** It builds the descriptor `{id, value, 0}` on the
///   stack and calls the same function as slot 3; before that it checks a size and returns `0x1d`
///   if it overflows. It is what Zeeboids calls when syncing.
/// - **Slot 11 is `GetResponse`.** It is the only varargs trampoline in the vtable: it pushes
///   `r0`–`r3`, points `r3` at the rest of the arguments and branches. A method that receives the
///   URL followed by a variable list of options has exactly that shape.
///
/// The ones still without a name have not shown up in use nor been disassembled.
pub const WEB: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "AddOpt",
    "slot4",
    "slot5",
    "AddOptBuffer",
    "slot7",
    "slot8",
    "slot9",
    "slot10",
    "GetResponse",
    "slot12",
];

/// Methods of `ISQLMgr` (`AEECLSID_SQLMGR = 0x0102c4e8`), the console's database manager.
///
/// There is no header: the order came from observation with `--probe`. The Z-Wheel creates the
/// object and calls slot 3 with the file name and an output pointer — `"tt_prefs.db"` and the
/// address where it expects the database. Slots 2 and 4 onward have not shown up yet, and so they
/// have no name.
pub const SQL_MGR: &[&str] = &["AddRef", "Release", "slot2", "OpenDatabase"];

/// Methods of `ISQLDatabase`, the database [`SQL_MGR`] returns.
///
/// Same origin, same caveat. Slot 3 receives the SQL statement, a function pointer and a context —
/// the shape of `sqlite3_exec`, which is what the dialect and the console's files already said it
/// was. The first statement the Z-Wheel sends is `PRAGMA integrity_check`.
pub const SQL_DATABASE: &[&str] = &["AddRef", "Release", "slot2", "Exec"];

/// Methods of the root form (`0x01001011`), read from the firmware's vtable at `0x10a785e4`.
///
/// The first three are the usual `IQI`: slot 0 increments the counter at `+4`, slot 1
/// decrements it and slot 2 is `QueryInterface`. The other four were disassembled one by one:
///
/// - **slot 3** is a setter and nothing more: `str r1,[r0,#0x10]; str r2,[r0,#0x14]; bx lr`. It
///   stores two values in the object and returns nothing. The Z-Wheel calls it with an object and
///   the number `0xc34`, which looks like a (recipient, identifier) pair — hence the name.
/// - **slots 5 and 6** delegate: they ask the inner object at `+0xc` for interface `0x01000000`
///   and call slot 27 on it, passing the argument on. It is the gesture of putting a widget in a
///   container.
/// - **slot 4** does work of its own, with one argument.
///
/// The names 3 to 6 describe what the code does, not a header — we have no header for this
/// Methods of `ISourceUtil` (`0x01001011`), in the order of `AEESource.h`.
///
/// The firmware's vtable at `0x10a785e4` has seven, and they are these seven. See
/// [`crate::brew::aee::Interface::SourceUtil`] for how the identification was made.
pub const SOURCE_UTIL: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "PeekSourceFromSource",
    "SourceFromAStream",
    "SourceFromMemory",
    "SourceFromFile",
];

/// Methods of `ISource`. The first three belong to every interface; `Read` and `Readable` come
/// from `AEESource.h`.
pub const SOURCE: &[&str] = &["AddRef", "Release", "QueryInterface", "Read", "Readable"];

/// Methods of `IPeek`, of which we know one.
///
/// Slot 8 is what the Z-Wheel calls to read `tectoy.cfg`. The ones below stay unnamed on purpose:
/// filling the table with plausible names would hide the next discovery.
pub const PEEK: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "slot3",
    "slot4",
    "slot5",
    "slot6",
    "slot7",
    "LerLinha",
];

/// Methods of the Z-Wheel's widget (`0x01028e51`).
///
/// Only slot 3 has a name because only it has been seen in use. The first two are the `AddRef`
/// and `Release` of every BREW interface; slot 2 stays unnamed on purpose, so that a call to it
/// shows up in the report instead of passing as implemented.
pub const WIDGET: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "Acessador",
    "DefinirTratador",
    "AdicionarFilho",
    "DefinirVisivel",
    "DefinirTamanho",
    "PegarPai",
    "slot9",
    "slot10",
    "slot11",
    "PegarInterface",
    // Used by the root widget while building the bottom bar.
    "Slot13",
    // `slot14(this, object)`, at `0x8fba0`, with the return value ignored. Refusing it aborted
    // building the z-pad form halfway — the `bl` to `0x8f580` was entered 669 times and never
    // returned once. It looks like hanging a model on the widget.
    "Anexar",
    "slot15",
    // `slot16(this)`, at `0x22d58`, right after the stage is created. Refusing it does not
    // return execution to `0x22d5c`: the menu form stops being built right there, and the
    // application stays in the ten-second pulse that polls points and the download queue without
    // drawing anything.
    "Slot16",
    // `slot17(this, id, model)`, at `0x23860` and `0x23d0c` of tectoy.mod: binds the font model
    // (0x8000) or another model to the roller widget in `tectoy_rollerwidget.c`.
    "Slot17",
];

/// Methods of the `IControl` used by Zenonia's text subsystem.
pub const CONTROL: &[&str] = &[
    "AddRef",
    "Release",
    "HandleEvent",
    "Redraw",
    "SetActive",
    "IsActive",
    "SetRect",
    "GetRect",
    "SetProperties",
    "GetProperties",
    "Reset",
    "ControlMethod11",
    "ControlMethod12",
    "ControlMethod13",
    "ControlMethod14",
    "ControlMethod15",
    "ControlMethod16",
    "ControlMethod17",
    "ControlMethod18",
    "ControlMethod19",
    "ControlMethod20",
    "ControlMethod21",
    "ControlMethod22",
    "ControlMethod23",
    "ControlMethod24",
    "ControlMethod25",
    "ControlMethod26",
    "ControlMethod27",
    "ControlMethod28",
    "ControlMethod29",
    "ControlMethod30",
    "ControlMethod31",
    "ControlMethod32",
    "ControlMethod33",
    "ControlMethod34",
    "ControlMethod35",
    "ControlMethod36",
    "ControlMethod37",
    "ControlMethod38",
    "ControlMethod39",
    "ControlMethod40",
];

/// Methods of ZEEBOMCP (`0x01006c05`), from the firmware's vtable `0x102d47a8`.
///
/// There are eight. Only the first three have names because only they were read: `0x11085c5e`
/// and `0x11085c70` are the count up and down, and `0x11085c90` compares the received IID with
/// `0x01000001` and `0x01006c05` — a `QueryInterface`. The five below stay unnamed until someone
/// calls them.
pub const ZEEBO_MCP: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "ModDataCopyFromENAND",
    "slot4",
    "ModDataRemoveFromMCP",
    "slot6",
    "UserDataCopyToENAND",
];

/// Methods of `IConfig` (`0x01001027`). See [`crate::brew::aee::Interface::Config`].
///
/// Four, not twelve: the names of 2 and 3 come from the SDK's `ICONFIG_GetItem`/`ICONFIG_SetItem`,
/// and the shape of the Z-Wheel's call matches the signature. The eight above stay unnamed so that
/// a call to them shows up in the report — the firmware's vtable, which would have them, is the
/// one that has already turned out to be a facade.
pub const CONFIG: &[&str] = &["AddRef", "Release", "GetItem", "SetItem"];

/// Methods of `LCT_SIMCardCtl` (`0x01006c01`), from the firmware's vtable `0x113cf854`.
///
/// Four. See [`crate::brew::aee::Interface::SimCardCtl`].
pub const SIM_CARD_CTL: &[&str] = &["AddRef", "Release", "QueryInterface", "PedirVerificacao"];

/// Methods of the system control (`0x01006c02`), from the firmware's vtable `0x10691ea8`.
///
/// Seven: the table's eighth value is `0x86`, which is not an address. See
/// [`crate::brew::aee::Interface::SystemCtl`].
pub const SYSTEM_CTL: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "DefinirModo",
    "slot4",
    // **Slot 5 has a name, and the name comes from the firmware.** Disassembled in
    // `1.1.2_APPS.bin` (Thumb, `0x10e9fe92`): it receives `(this, mode, option)` — the option with
    // `-1` meaning "the device's", read from `0x114287ec` — and **ends by calling the body of
    // `DefinirModo`** (`bl 0x10e9fdb6`, slot 3). Measured: it is the call the Z-Wheel makes on
    // **confirm** (`0xe064`), and without it the scan stopped at `Unimplemented` on the first key.
    "DefinirModoComOpcao",
    "Consultar",
];

/// Methods of `ICM` (`0x01011810`), of which we know one.
///
/// Slot 28 is the only one the Z-Wheel calls. See [`crate::brew::aee::Interface::Cm`].
pub const CM: &[&str] = &[
    "AddRef",
    "Release",
    "slot2",
    "slot3",
    "slot4",
    "slot5",
    "slot6",
    "slot7",
    "slot8",
    "slot9",
    "slot10",
    "slot11",
    "slot12",
    "slot13",
    "slot14",
    "slot15",
    "slot16",
    "slot17",
    "slot18",
    "slot19",
    "slot20",
    "slot21",
    "slot22",
    "slot23",
    "slot24",
    "slot25",
    "slot26",
    "slot27",
    "GetSSInfo",
];

/// Methods of `0x01028e3c`, which is an `IValueModel`. See
/// [`crate::brew::aee::Interface::Class28e3c`].
pub const CLASS_28E3C: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "AddListener",
    "Notify",
    "SetValue",
    "GetValue",
];

/// Methods of the TrueType font (`0x01035156`), of which we know one.
///
/// Slot 4 is what `0x7bfc8` calls to get a usable font from the typeface. See
/// [`crate::brew::aee::Interface::Typeface`].
pub const TYPEFACE: &[&str] = &["AddRef", "Release", "slot2", "slot3", "CriarFonte"];

/// Methods of `IFont` — the system's bitmap font.
///
/// The order is that of the reference code that already implements this interface
/// (`zeebo-emulator/.../zeemu/brew/BrewFont.cpp`) and that of the six `#define`s in `AEEFont.h`:
///
/// ```text
/// AddRef(0)  Release(1)  QueryInterface(2)  DrawText(3)  GetInfo(4)  MeasureText(5)
/// ```
///
/// **This is not `ITypeface`**, and the difference is the reason this interface exists:
/// `ITypeface` creates fonts from a TTF, and `IFont` **already is** the drawable font. Double
/// Dragon, Resident Evil 4 and the Data East ports ask for the system font classes; answering them
/// as unknown made the game fall into the "Memory is insufficient" warning screen.
pub const FONT: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "DrawText",
    "GetInfo",
    "MeasureText",
];

/// Methods of the Z-Wheel's generic list (`0x01028e35`).
///
/// See [`crate::brew::aee::Interface::Vector`] for where each name was read. The six unnamed ones
/// have never been called.
pub const VECTOR: &[&str] = &[
    "AddRef",
    "Release",
    "slot2",
    "slot3",
    "slot4",
    "Tamanho",
    "PegarEm",
    "SubstituirEm",
    "InserirEm",
    "RemoverEm",
    "Esvaziar",
    "slot11",
    "DefinirLiberador",
];

/// Methods of the Z-Wheel's generic collection (`0x0102c4e8`… no: `0x0100104f`).
///
/// No header. The names came from usage, observed with `--probe`: the app creates the collection,
/// calls slot 5 once and then alternates 7 and 4 until 4 says it is done — the shape of a cursor.
/// The unnamed slots have never been called; leaving them unnamed is what makes an unexpected
/// call show up in the report instead of passing as implemented.
pub const COLLECTION: &[&str] = &[
    "AddRef",
    "Release",
    "slot2",
    "slot3",
    "AtEnd",
    "Reset",
    "slot6",
    "GetCurrent",
    "slot8",
    "slot9",
    "Definir",
];

/// Methods of `IHash` (`AEECLSID_MD5` = `0x01001015`), worked out from usage.
///
/// This class is not in the firmware table of the APPS partition, so the vtable could not be read
/// the way it was done with `IWeb`. What settled it was Zeeboids's code at `0x77b50`, where the
/// whole sequence appears:
///
/// ```text
/// 0x77b70  ldr r1, [r1, #0x10]   ; slot 4, and no argument is built before it -> Reset()
/// ...      builds a string and stores size-1 at [sp+0x38]
/// 0x77ba4  ldr r3, [r1, #8]      ; slot 2, with (buffer, size)  -> Update()
/// 0x77bb0  mov r0, #0x11         ; 17 = sixteen bytes and the terminator
/// 0x77bc4  memset(sp+0x14, 0, 0x21)
/// 0x77bdc  ldr r3, [r1, #0xc]    ; slot 3, with (buffer, &size) -> GetDigest()
/// ```
///
/// The previous order — `QueryInterface`, `Reset`, `Update`, `GetDigest` — was `IQI` plus a
/// guess, and got everything from slot 2 onward wrong: the "QueryInterface" wrote into a field
/// that was a size, and the "Update" read as a pointer what was not one. Both showed up in the
/// report as core faults, and that is how the error was found.
///
/// **There is no `QueryInterface`**: the first two slots are the `AddRef` and `Release` of
/// `DECLARE_IBASE`, and its own methods start at 2.
pub const HASH: &[&str] = &["AddRef", "Release", "Update", "GetDigest", "Reset"];

/// Methods of `IHashCTX` (6 slots): the digest with the **context in the caller's memory**. Each
/// method receives the context and its size; the object stores nothing. Order read from usage in
/// Powerboat Challenge, which calls 3 with the context, 4 with the data and 5 with the output.
pub const HASH_CTX: &[&str] = &["AddRef", "Release", "QueryInterface", "Init", "Update", "GetResult"];

/// Methods of `ICipherFactory` (6 slots), from `INHERIT_ICipherFactory` in
/// `inc/AEEICipherFactory.h`.
pub const CIPHER_FACTORY: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "CreateCipher",
    "CreateCipher2",
    "QueryCipher",
];

/// Methods of `ICipher1` (7 slots), from `INHERIT_ICipher1` in `inc/AEEICipher1.h`. The first
/// two after `IQI` come from `IParameters`, which `ICipher1` inherits from.
pub const CIPHER: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "GetParam",
    "SetParam",
    "Process",
    "ProcessLast",
];

/// Methods of `IHeap` (9 slots), from `QINTERFACE(IHeap)` in `sdk/inc/AEEHeap.h`.
///
/// The interface uses `DECLARE_IBASE`, so it has only `AddRef` and `Release` before its own
/// methods — there is no `QueryInterface`.
pub const HEAP: &[&str] = &[
    "AddRef",
    "Release",
    "Malloc",
    "Realloc",
    "Free",
    "StrDup",
    "CheckAvail",
    "GetMemStats",
    "GetModuleMemStats",
];

/// Methods of `IUnzipAStream` (6 slots), from `QINTERFACE(IUnzipAStream)` in
/// `sdk/inc/AEEUnzipStream.h`.
///
/// `DECLARE_IBASE` + `DECLARE_IASTREAM` + its own method. The `IAStream` order is the same as
/// `IMemAStream`'s, which we already used: `Readable`, `Read`, `Cancel`.
pub const UNZIP_STREAM: &[&str] = &[
    "AddRef",
    "Release",
    "Readable",
    "Read",
    "Cancel",
    "SetStream",
];
