//! Nomes dos métodos de cada interface, na ordem em que ocupam a vtable.
//!
//! Gerado das declarações dos headers — as macros `INHERIT_*` do BREW SDK 4.0.2, o
//! `QINTERFACE(IGraphics)` do `AEEGraphics.h` (que usa o estilo antigo, sem macro de herança)
//! e, para `IHID`/`IHIDDevice`, os headers do SDK do próprio Zeebo. A ordem dos slots é ABI:
//! errar um desalinha todas as chamadas seguintes. A tabela de `IFileMgr` bate com a vtable
//! que a engenharia reversa extraiu da firmware do console
//! (`docs/vendor/tripleoxygen/research/brew/vtbl.ods`).

/// Métodos de `IShell` (52 slots).
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

/// Métodos de `IModule` (4 slots).
pub const MODULE: &[&str] = &["AddRef", "Release", "CreateInstance", "FreeResources"];

/// Métodos de `IApplet` (3 slots).
pub const APPLET: &[&str] = &["AddRef", "Release", "HandleEvent"];

/// Métodos de `IFileMgr` (21 slots).
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

/// Métodos de `IFile` (12 slots).
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

/// Métodos de `IDisplay` (26 slots).
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

/// Métodos de `IBitmap` (16 slots).
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

/// Métodos do canvas `0x0101e443`. Só o slot 7 é chamado; os outros ficam como marcador para
/// que uma chamada inesperada apareça no relatório.
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

/// Métodos de `ITransform` (5 slots), de `AEETransform.h`.
pub const TRANSFORM: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "TransformBltSimple",
    "TransformBltComplex",
];

/// Métodos de `IHID` (8 slots).
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

/// Métodos de `IHIDDevice` (19 slots).
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

/// Métodos de `ISignal` (4 slots).
pub const SIGNAL: &[&str] = &["AddRef", "Release", "QueryInterface", "Set"];

/// Métodos de `ISignalCtl` (6 slots).
pub const SIGNALCTL: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "Set",
    "Detach",
    "Enable",
];

/// Métodos de `ISignalCBFactory` (4 slots).
pub const SIGNALCBFACTORY: &[&str] = &["AddRef", "Release", "QueryInterface", "CreateSignal"];

/// Métodos de `IGraphics` (44 slots).
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

/// Métodos de `ISound` (15 slots), da macro `INHERIT_ISound` em `inc/AEEISound.h`.
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

/// Métodos de `ILicense` (6 slots), do `QINTERFACE(ILicense)` em `sdk/inc/AEELicense.h`.
pub const LICENSE: &[&str] = &[
    "AddRef",
    "Release",
    "IsExpired",
    "GetInfo",
    "SetUsesRemaining",
    "GetPurchaseInfo",
];

/// Métodos de `IMemAStream` (7 slots), de `INHERIT_IMemAStream` em `sdk/inc/AEE.h`: o
/// `IAStream` (`Readable`, `Read`, `Cancel`) mais `Set` e `SetEx`.
pub const MEMASTREAM: &[&str] = &[
    "AddRef", "Release", "Readable", "Read", "Cancel", "Set", "SetEx",
];

/// Métodos de `IImage` (11 slots), de `INHERIT_IImage` em `inc/AEEIImage.h`.
/// Métodos de `IEGLSurfaceManip` (27 slots), de `INHERIT_IEGLSurfaceManip` em
/// `sdk/inc/AEEEGLSurfaceManip.h`: os três do `IQI`, os catorze da V1 e os dez que a V2
/// acrescenta. A V2 é superconjunto da V1 com o mesmo prefixo, então uma tabela serve às duas.
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

/// Métodos de `IGLESImageonExt` (27 slots), de `INHERIT_IGLESImageonExt` em
/// `sdk/inc/AEEGLESImageonEXT.h`: os extras do ATI Imageon sobre o OpenGL ES 1.0.
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

/// Métodos de `IImageDecoder` (5 slots), de `INHERIT_IImageDecoder` em `inc/AEEIImageDecoder.h`:
/// os três do `IQI` e os dois próprios.
pub const IMAGE_DECODER: &[&str] = &["AddRef", "Release", "QueryInterface", "GetBitmap", "GetRop"];

/// Métodos de `IForceFeed` (5 slots), de `INHERIT_IForceFeed` em `inc/AEEIForceFeed.h`: os três
/// do `IQI`, o `Write` e o `Reset`. É por ela que os dados entram num decodificador.
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

/// Métodos de `IThread` (12 slots), de `INHERIT_IThread` em `sdk/inc/AEEThread.h`: os três do
/// `IQI`, os quatro do `IRscPool` (`inc/AEEIRscPool.h`) e os cinco da própria thread.
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

/// Métodos de `IEGL11` (31 slots), de `INHERIT_IEGL10`/`INHERIT_IEGL11` em `sdk/inc/AEEEGL10.h`
/// e `AEEEGL11.h`.
///
/// É a forma nova das interfaces do BREW, e a que o Quake usa: cada método recebe o `this`,
/// devolve um código de erro e entrega o resultado por um ponteiro de saída — o último
/// argumento. A `IEGL` antiga, de `AEEGL.h`, tem outra convenção e não é esta.
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

/// Métodos de `IGLES11` (148 slots), de `INHERIT_IGLES10`/`INHERIT_IGLES11` em
/// `sdk/inc/AEEGLES10.h` e `AEEGLES11.h` — o OpenGL ES 1.1 do BREW.
///
/// Mesma convenção do [`EGL`]: `this` no primeiro argumento, código de erro no retorno e o
/// resultado por ponteiro de saída. Um objeto `IGLES11` serve também como `IGLES10`, porque a
/// segunda tabela apenas estende a primeira.
/// `IGLES11ExtPak`: as extensões OES do pacote. Ver `AEEGLES11ExtPak.h`.
///
/// Três famílias: a geração de coordenadas de textura (`TexGen`), o blending separado por equação
/// e os objetos de framebuffer e renderbuffer. **É a última porta entre o Prey Evil e o desenho** —
/// as outras cinco já foram entregues, cada uma medida.
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

/// `IGLES11Ext`: as extensões OES do OpenGL ES 1.1, na ordem do `AEEGLES11Ext.h`.
///
/// **O Prey Evil pede esta interface por `CreateInstance` e para de desenhar sem ela**: o
/// levantamento das 62 ROMs mostrou onze métodos de GL, nenhum desenho e tela preta. Os
/// `DrawTex*` são os que importam para um jogo que monta o quadro numa textura.
/// `IGLES10Ext`: uma extensão do OpenGL ES 1.0. Ver `AEEGLES10Ext.h`.
///
/// Um método só, e é o que o Prey Evil usa para saber que a extensão existe.
pub const GLES10_EXT: &[&str] = &["AddRef", "Release", "QueryInterface", "QueryMatrixxOES"];

/// `IJoystick`: o joystick USB, na ordem do `AEEJoystick.h`.
///
/// **É o que o gerenciador de joystick da Qualcomm pede** (`gamepadmgr.cpp`, que o Prey Evil usa):
/// ele cria a interface, e sem ela guarda nulo e cai no primeiro `Read`. Seis slots.
pub const JOYSTICK: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "SetParm",
    "GetParm",
    "Read",
];

/// `IEGLGetPowerLevel`: o nível de bateria. Ver `AEEEGLGetPowerLevel.h`.
pub const EGL_GET_POWER_LEVEL: &[&str] =
    &["AddRef", "Release", "QueryInterface", "GetPowerLevel"];

/// `IEGLGetColorBuffer`: o buffer de cor do EGL. Ver `AEEEGLGetColorBuffer.h`.
///
/// É o par por interface da função `eglGetColorBufferQUALCOMM`, e as duas compartilham o cálculo.
pub const EGL_GET_COLOR_BUFFER: &[&str] =
    &["AddRef", "Release", "QueryInterface", "GetColorBuffer"];

/// `IEGLOESSwapInterval`: o ritmo de quadro pedido pelo jogo. Ver `AEEEGLOESSwapInterval.h`.
///
/// Por **função** o motor já responde `SwapIntervalOES`; aqui é a mesma resposta, pela interface.
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
    // `GL_OES_draw_texture`. Não fazem parte da vtable do `IGLES`: o jogo chega a elas pelo
    // `eglGetProcAddress`, e por isso ficam no fim — inserir no meio deslocaria os slots reais.
    "DrawTexsOES",
    "DrawTexiOES",
    "DrawTexxOES",
    "DrawTexfOES",
    "DrawTexsvOES",
    "DrawTexivOES",
    "DrawTexxvOES",
    "DrawTexfvOES",
];

/// Métodos de `IMediaUtil` (6 slots), de `AEEINTERFACE(IMediaUtil)` em `sdk/inc/AEEMediaUtil.h`.
pub const MEDIAUTIL: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "CreateMedia",
    "EncodeMedia",
    "CreateMediaEx",
];

/// Métodos de `IMedia` (14 slots), de `INHERIT_IMedia` em `inc/AEEIMedia.h`.
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

/// Métodos de `IEGL` (28 slots), de `AEEINTERFACE(IEGL)` em `sdk/inc/AEEGL.h`.
///
/// É a forma **antiga**: fora dos três do `IQueryInterface`, os métodos não recebem o `this`
/// — a macro do SDK chama `AEEGETPVTBL(p,IEGL)->eglGetDisplay(a)` sem repassar `p` — e o
/// resultado é o valor de retorno, não um ponteiro de saída. Tirando o prefixo `egl`, os
/// nomes coincidem com os de [`EGL`], que é o que permite atender as duas com o mesmo código.
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
    // `EGL_QUALCOMM_get_color_buffer`. Não faz parte da vtable: o jogo chega a ela pelo
    // `eglGetProcAddress`, e por isso fica no fim — inserir no meio deslocaria os slots reais,
    // que é o mesmo cuidado das `DrawTex*OES` na tabela do GLES.
    //
    // A Z-Wheel desenha o palco num **pbuffer**, não numa janela, e precisa do ponteiro do
    // buffer de cor para compor com o 2D. Sem esta função ela desiste: o log dela é
    // `eglGetProcAddress('eglGetColorBufferQUALCOMM') failed.`, seguido de desmontar o contexto
    // e de `CreateStageWidget failed, proceeding...`.
    "eglGetColorBufferQUALCOMM",
    // `EGL_QUALCOMM_surface_scale`, pelo mesmo caminho e pela mesma razão: o jogo resolve os
    // quatro nomes pelo `eglGetProcAddress` e **testa os quatro contra nulo de uma vez**. Na
    // Z-Wheel isso está em `0x5d7ac`..`0x5d7c8` do `tectoy.mod`: faltando qualquer um, ela zera
    // o grupo inteiro e segue por outro caminho.
    //
    // Os métodos já existem na [`EGL_SURFACE_MANIP`], que é a mesma extensão exposta como
    // interface; aqui eles aparecem na forma de função C, sem `this` e devolvendo o valor.
    // Resolvido junto com as quatro de escala e guardado no mesmo bloco (`+0x34`), logo antes
    // delas. Deixá-lo nulo enquanto as outras existem é pior que nulo em todas: o jogo lê esse
    // campo **depois** de ver o grupo preenchido.
    "eglSwapIntervalOES",
    "eglSurfaceScaleEnableQUALCOMM",
    "eglSetSurfaceScaleQUALCOMM",
    "eglGetSurfaceScaleQUALCOMM",
    "eglGetSurfaceScaleCapsQUALCOMM",
];

/// Métodos de `IGL` (80 slots), de `AEEINTERFACE(IGL)` em `sdk/inc/AEEGL.h` — o OpenGL ES 1.0
/// Common-Lite —, seguidos das funções que só a [`GLES`] tem. Mesma convenção antiga do [`EGL_LEGACY`]; sem o prefixo `gl`, os nomes
/// coincidem com os de [`GLES`].
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
    // Daqui em diante não é mais o `AEEGL.h`: são as funções que só a `IGLES11` tem — as de
    // ponto flutuante, as de buffer e as de extensão —, na mesma convenção sem `this`. É por
    // elas que o `eglGetProcAddress` entrega um ponteiro de função C de verdade. O começo da
    // tabela, que é a vtable do `IGL`, não muda.
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

/// Métodos de `IWeb`, lidos da vtable do firmware em `0x1087fde0`.
///
/// O `AEEWeb.h` saiu do SDK 4.0.2 — a interface foi aposentada —, e por muito tempo esta tabela
/// teve quatro entradas deduzidas do uso. São **treze**, e a dedução errava o principal.
///
/// O que o firmware corrige:
///
/// - **O slot 2 é o `QueryInterface`**, não o `GetResponse`. A implementação aceita
///   `0x01000001`, `0x01005004` e `0x01001031` e devolve o próprio objeto. Ou seja, `IWeb`
///   segue o `IQI` como as outras interfaces, e a hipótese do `DECLARE_IBASE` estava errada.
/// - **O slot 3 é mesmo o `AddOpt`**: `ldr r0,[r0,#0xc]; blx …`, repassando o ponteiro que
///   recebe. Era a única das quatro deduções que estava certa, e o Boomerang a apoiava.
/// - **O slot 6 é um atalho para o `AddOpt`.** Ele monta na pilha o descritor `{id, valor, 0}`
///   e chama a mesma função do slot 3; antes disso confere um tamanho e devolve `0x1d` se
///   estourar. É o que o Zeeboids chama ao sincronizar.
/// - **O slot 11 é o `GetResponse`.** É o único trampolim de varargs da vtable: empilha
///   `r0`–`r3`, aponta `r3` para o resto dos argumentos e desvia. Um método que recebe a URL
///   seguida de uma lista de opções variável tem exatamente essa forma.
///
/// Os que continuam sem nome não apareceram em uso nem foram desmontados.
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

/// Métodos de `ISQLMgr` (`AEECLSID_SQLMGR = 0x0102c4e8`), o gerenciador de bancos do console.
///
/// Não há header: a ordem veio da observação com o `--sonda`. O Z-Wheel cria o objeto e chama o
/// slot 3 com o nome do arquivo e um ponteiro de saída — `"tt_prefs.db"` e o endereço onde ele
/// espera o banco. Os slots 2 e 4 em diante ainda não apareceram, e por isso não têm nome.
pub const SQL_MGR: &[&str] = &["AddRef", "Release", "slot2", "OpenDatabase"];

/// Métodos de `ISQLDatabase`, o banco que o [`SQL_MGR`] devolve.
///
/// Mesma origem, mesma ressalva. O slot 3 recebe a instrução SQL, um ponteiro de função e um
/// contexto — a forma do `sqlite3_exec`, que é o que o dialeto e os arquivos do console já
/// diziam ser. A primeira instrução que o Z-Wheel manda é `PRAGMA integrity_check`.
pub const SQL_DATABASE: &[&str] = &["AddRef", "Release", "slot2", "Exec"];

/// Métodos do formulário raiz (`0x01001011`), lidos da vtable do firmware em `0x10a785e4`.
///
/// Os três primeiros são o `IQI` de sempre: o slot 0 incrementa o contador em `+4`, o 1 o
/// devolve e o 2 é o `QueryInterface`. Os outros quatro foram desmontados um a um:
///
/// - **slot 3** é um setter e nada mais: `str r1,[r0,#0x10]; str r2,[r0,#0x14]; bx lr`. Guarda
///   dois valores no objeto e não devolve nada. A Z-Wheel o chama com um objeto e o número
///   `0xc34`, que é a cara de um par (destinatário, identificador) — daí o nome.
/// - **slots 5 e 6** delegam: pedem ao objeto interno de `+0xc` a interface `0x01000000` e
///   chamam nela o slot 27, repassando o argumento. É o gesto de pôr um widget num contêiner.
/// - **slot 4** faz trabalho próprio, com um argumento.
///
/// Os nomes de 3 a 6 descrevem o que o código faz, não um header — não temos header desta
/// Métodos da `ISourceUtil` (`0x01001011`), na ordem do `AEESource.h`.
///
/// A vtable do firmware em `0x10a785e4` tem sete, e são estes sete. Ver
/// [`crate::brew::aee::Interface::SourceUtil`] para como a identificação foi feita.
pub const SOURCE_UTIL: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "PeekSourceFromSource",
    "SourceFromAStream",
    "SourceFromMemory",
    "SourceFromFile",
];

/// Métodos do `ISource`. Os três primeiros são de toda interface; o `Read` e o `Readable` vêm
/// do `AEESource.h`.
pub const SOURCE: &[&str] = &["AddRef", "Release", "QueryInterface", "Read", "Readable"];

/// Métodos do `IPeek`, dos quais conhecemos um.
///
/// O slot 8 é o que a Z-Wheel chama para ler o `tectoy.cfg`. Os de baixo ficam sem nome de
/// propósito: preencher a tabela com nomes plausíveis esconderia a próxima descoberta.
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

/// Métodos do widget da Z-Wheel (`0x01028e51`).
///
/// Só o slot 3 tem nome porque só ele foi visto em uso. Os dois primeiros são o `AddRef` e o
/// `Release` de toda interface do BREW; o 2 fica sem nome de propósito, para que uma chamada
/// nele apareça no relatório em vez de passar por implementada.
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
    // Usado pelo widget raiz durante a montagem da barra inferior.
    "Slot13",
    // `slot14(this, objeto)`, na `0x8fba0`, com o retorno ignorado. Recusá-lo abortava a
    // montagem do formulário do z-pad pela metade — o `bl` para a `0x8f580` era entrado 669
    // vezes e não voltava nenhuma. Tem cara de pendurar um modelo no widget.
    "Anexar",
    "slot15",
    // `slot16(this)`, na `0x22d58`, logo depois de o palco ser criado. Recusá-lo não devolve a
    // execução ao `0x22d5c`: o formulário do menu para de ser montado ali mesmo, e o aplicativo
    // fica no pulso de dez segundos que consulta pontos e fila de download sem desenhar nada.
    "Slot16",
    // `slot17(this, id, modelo)`, na `0x23860` e `0x23d0c` do tectoy.mod: associa o modelo de
    // fonte (0x8000) ou outro modelo ao widget do roller em `tectoy_rollerwidget.c`.
    "Slot17",
];

/// Métodos do `IControl` usado pelo subsistema de texto do Zenonia.
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

/// Métodos do ZEEBOMCP (`0x01006c05`), da vtable `0x102d47a8` do firmware.
///
/// São oito. Só os três primeiros têm nome porque só eles foram lidos: `0x11085c5e` e
/// `0x11085c70` são a contagem para cima e para baixo, e `0x11085c90` compara o IID recebido
/// com `0x01000001` e `0x01006c05` — um `QueryInterface`. Os cinco de baixo continuam sem nome
/// até alguém chamá-los.
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

/// Métodos da `IConfig` (`0x01001027`). Ver [`crate::brew::aee::Interface::Config`].
///
/// Quatro, não doze: os nomes de 2 e 3 vêm do `ICONFIG_GetItem`/`ICONFIG_SetItem` do SDK, e a
/// forma da chamada da Z-Wheel confere com a assinatura. Os oito de cima ficam sem nome para
/// que uma chamada neles apareça no relatório — a vtable do firmware, que os teria, é a que já
/// se mostrou ser de fachada.
pub const CONFIG: &[&str] = &["AddRef", "Release", "GetItem", "SetItem"];

/// Métodos do `LCT_SIMCardCtl` (`0x01006c01`), da vtable `0x113cf854` do firmware.
///
/// Quatro. Ver [`crate::brew::aee::Interface::SimCardCtl`].
pub const SIM_CARD_CTL: &[&str] = &["AddRef", "Release", "QueryInterface", "PedirVerificacao"];

/// Métodos do controle de sistema (`0x01006c02`), da vtable `0x10691ea8` do firmware.
///
/// Sete: o oitavo valor da tabela é `0x86`, que não é endereço. Ver
/// [`crate::brew::aee::Interface::SystemCtl`].
pub const SYSTEM_CTL: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "DefinirModo",
    "slot4",
    // **O slot 5 tem nome, e o nome vem do firmware.** Desmontado em `1.1.2_APPS.bin` (Thumb,
    // `0x10e9fe92`): ele recebe `(this, modo, opção)` — a opção com `-1` valendo "a do aparelho",
    // lida de `0x114287ec` — e **termina chamando o corpo do `DefinirModo`** (`bl 0x10e9fdb6`, o
    // slot 3). Medido: é a chamada que a Z-Wheel faz no **confirmar** (`0xe064`), e sem ela a
    // varredura parava em `Unimplemented` na primeira tecla.
    "DefinirModoComOpcao",
    "Consultar",
];

/// Métodos do `ICM` (`0x01011810`), dos quais conhecemos um.
///
/// O slot 28 é o único que a Z-Wheel chama. Ver [`crate::brew::aee::Interface::Cm`].
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

/// Métodos da `0x01028e3c`, que é um `IValueModel`. Ver
/// [`crate::brew::aee::Interface::Classe28e3c`].
pub const CLASSE_28E3C: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "AddListener",
    "Notify",
    "SetValue",
    "GetValue",
];

/// Métodos da fonte TrueType (`0x01035156`), dos quais conhecemos um.
///
/// O slot 4 é o que a `0x7bfc8` chama para obter uma fonte utilizável a partir do tipo. Ver
/// [`crate::brew::aee::Interface::Typeface`].
pub const TYPEFACE: &[&str] = &["AddRef", "Release", "slot2", "slot3", "CriarFonte"];

/// Métodos do `IFont` — a fonte de bitmap do sistema.
///
/// A ordem é a do código de referência que já implementa esta interface
/// (`zeebo-emulator/.../zeemu/brew/BrewFont.cpp`) e a dos seis `#define` do `AEEFont.h`:
///
/// ```text
/// AddRef(0)  Release(1)  QueryInterface(2)  DrawText(3)  GetInfo(4)  MeasureText(5)
/// ```
///
/// **Isto não é o `ITypeface`**, e a diferença é o motivo desta interface existir: o `ITypeface`
/// cria fontes a partir de um TTF, e o `IFont` **já é** a fonte desenhável. O Double Dragon, o
/// Resident Evil 4 e os ports da Data East pedem as classes de fonte do sistema; respondê-las como
/// desconhecidas fazia o jogo cair na tela de aviso "Memory is insufficient".
pub const FONT: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "DrawText",
    "GetInfo",
    "MeasureText",
];

/// Métodos da lista genérica da Z-Wheel (`0x01028e35`).
///
/// Ver [`crate::brew::aee::Interface::Vetor`] para onde cada nome foi lido. Os seis sem nome nunca
/// foram chamados.
pub const VETOR: &[&str] = &[
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

/// Métodos da coleção genérica da Z-Wheel (`0x0102c4e8`… não: `0x0100104f`).
///
/// Sem header. Os nomes saíram do uso, observado com o `--sonda`: o app cria a coleção, chama o
/// slot 5 uma vez e depois alterna o 7 e o 4 até o 4 dizer que acabou — a forma de um cursor.
/// Os slots sem nome nunca foram chamados; deixá-los sem nome é o que faz uma chamada
/// inesperada aparecer no relatório em vez de passar por implementada.
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

/// Métodos de `IHash` (`AEECLSID_MD5` = `0x01001015`), levantados do uso.
///
/// Esta classe não está na tabela do firmware da partição APPS, então não deu para ler a vtable
/// como se fez com o `IWeb`. O que decidiu foi o código do Zeeboids em `0x77b50`, onde a
/// sequência inteira aparece:
///
/// ```text
/// 0x77b70  ldr r1, [r1, #0x10]   ; slot 4, e nenhum argumento é montado antes -> Reset()
/// ...      monta uma string e guarda tamanho-1 em [sp+0x38]
/// 0x77ba4  ldr r3, [r1, #8]      ; slot 2, com (buffer, tamanho)  -> Update()
/// 0x77bb0  mov r0, #0x11         ; 17 = dezesseis bytes e o terminador
/// 0x77bc4  memset(sp+0x14, 0, 0x21)
/// 0x77bdc  ldr r3, [r1, #0xc]    ; slot 3, com (buffer, &tamanho) -> GetDigest()
/// ```
///
/// A ordem anterior — `QueryInterface`, `Reset`, `Update`, `GetDigest` — era a do `IQI` mais
/// uma suposição, e errava tudo do slot 2 em diante: o "QueryInterface" escrevia num campo que
/// era um tamanho, e o "Update" lia como ponteiro o que não era. Os dois apareciam no relatório
/// como falha de núcleo, e é assim que o erro foi achado.
///
/// **Não há `QueryInterface`**: os dois primeiros slots são o `AddRef` e o `Release` do
/// `DECLARE_IBASE`, e os métodos próprios começam no 2.
pub const HASH: &[&str] = &["AddRef", "Release", "Update", "GetDigest", "Reset"];

/// Métodos de `ICipherFactory` (6 slots), de `INHERIT_ICipherFactory` em
/// `inc/AEEICipherFactory.h`.
pub const CIPHER_FACTORY: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "CreateCipher",
    "CreateCipher2",
    "QueryCipher",
];

/// Métodos de `ICipher1` (7 slots), de `INHERIT_ICipher1` em `inc/AEEICipher1.h`. Os dois
/// primeiros depois do `IQI` vêm do `IParameters`, de onde o `ICipher1` herda.
pub const CIPHER: &[&str] = &[
    "AddRef",
    "Release",
    "QueryInterface",
    "GetParam",
    "SetParam",
    "Process",
    "ProcessLast",
];

/// Métodos de `IHeap` (9 slots), de `QINTERFACE(IHeap)` em `sdk/inc/AEEHeap.h`.
///
/// A interface usa `DECLARE_IBASE`, então tem só `AddRef` e `Release` antes dos métodos
/// próprios — não há `QueryInterface`.
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

/// Métodos de `IUnzipAStream` (6 slots), de `QINTERFACE(IUnzipAStream)` em
/// `sdk/inc/AEEUnzipStream.h`.
///
/// `DECLARE_IBASE` + `DECLARE_IASTREAM` + o método próprio. A ordem do `IAStream` é a mesma do
/// `IMemAStream`, que já usávamos: `Readable`, `Read`, `Cancel`.
pub const UNZIP_STREAM: &[&str] = &[
    "AddRef",
    "Release",
    "Readable",
    "Read",
    "Cancel",
    "SetStream",
];
