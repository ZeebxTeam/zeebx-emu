//! IEGL: contextos, superfícies e a troca de buffer que fecha o quadro.

use super::*;

/// A chave da vigia do color buffer do pbuffer, em [`CpuBackend::watch_dirty`].
///
/// As outras chaves são endereços de bitmap, que nunca são zero: o color buffer não pertence a
/// nenhum deles e fica com o valor que sobra.
const VIGIA_COLOR_BUFFER: u32 = 0;

impl<C: CpuBackend> Machine<C> {
    /// Escreve `EGL_TRUE` no `AEEEGLBoolean *ret` do argumento `slot` e devolve `SUCCESS`.
    pub(super) fn write_egl_true(&mut self, slot: usize) -> Result<u32, CpuError> {
        let out = self.arg(slot);
        if out != 0 {
            self.cpu.write_u32(out, gles::EGL_TRUE)?;
        }
        Ok(SUCCESS)
    }

    /// O EGL, nas duas formas em que o BREW o expõe.
    ///
    /// A nova (`IEGL11`, de `sdk/inc/AEEEGL10.h` e `AEEEGL11.h`) recebe o `this` no primeiro
    /// argumento e entrega o resultado num ponteiro de saída — sempre o último argumento —,
    /// reservando o retorno ao código de erro. A antiga (`IEGL`, de `sdk/inc/AEEGL.h`) não
    /// recebe o `this` e devolve o resultado direto.
    ///
    /// Fora isso as duas são a mesma API, na mesma ordem: tirando o prefixo `egl` dos nomes
    /// antigos, os métodos coincidem. Por isso um único tradutor atende as duas, com um
    /// deslocamento nos argumentos e uma decisão no fim sobre onde pôr a resposta.
    pub(super) fn egl_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(full) = iface.method(slot) else {
            return Ok(None);
        };
        let legacy = iface == Interface::EglLegacy;
        let name = full.strip_prefix("egl").unwrap_or(full);
        if self.serial.is_some() {
            let args: Vec<String> = (0..6).map(|i| format!("{:#x}", self.arg(i))).collect();
            let msg = format!("<egl {full} {}>", args.join(" "));
            self.registra_serial(msg);
        }
        // Os argumentos, já sem o `this` quando ele existe. Ler alguns a mais do que o método
        // usa é inofensivo: `arg` responde zero para o que não conseguir ler.
        let base = usize::from(!legacy);
        let a: [u32; 8] = std::array::from_fn(|i| self.arg(base + i));
        let this = self.arg(0);
        // A tela é a superfície do "device bitmap" quando o jogo pediu uma; o `self.screen`
        // vira um marcador de 1×1 nesse caso, e responder as dimensões dele fazia o Crash
        // montar uma viewport de um pixel e desenhar o jogo inteiro dentro dela.
        let (width, height) = {
            let target = self.screen();
            (target.width(), target.height())
        };
        let (out, value) = match name {
            "AddRef" => return Ok(Some(self.objects.add_ref(this))),
            "Release" => return Ok(Some(self.objects.release(this))),
            "QueryInterface" => return Ok(Some(self.egl_query_interface(iface)?)),
            // Ler o erro também o limpa, como manda a spec do EGL.
            "GetError" => (0, std::mem::replace(&mut self.egl_error, gles::EGL_SUCCESS)),
            // Há um display só, e ele é a tela do console.
            "GetDisplay" => (1, EGL_DISPLAY),
            "Initialize" => {
                self.write_at(a[1], 1)?;
                self.write_at(a[2], 0)?;
                (3, gles::EGL_TRUE)
            }
            "Terminate" => (1, gles::EGL_TRUE),
            "QueryString" => {
                let text = match a[1] {
                    gles::EGL_VENDOR => "Zeebx",
                    gles::EGL_VERSION => "1.1",
                    // A escala de superfície do console. Os jogos procuram o nome por
                    // substring, com o espaço no fim como delimitador — é assim que a string
                    // aparece no binário deles.
                    // Os jogos procuram o nome por substring, com o espaço no fim como
                    // delimitador — é assim que a string aparece no binário deles. O
                    // `get_color_buffer` entrou porque a Z-Wheel o procura antes de pedir o
                    // ponteiro da função: o literal está em `0x43a5c` do `tectoy.mod`.
                    gles::EGL_EXTENSIONS => {
                        "EGL_QUALCOMM_surface_scale EGL_QUALCOMM_get_color_buffer "
                    }
                    _ => {
                        self.egl_error = gles::EGL_BAD_ATTRIBUTE;
                        return Ok(Some(if legacy { 0 } else { SUCCESS }));
                    }
                };
                let ponteiro = self.intern(text)?;
                if self.serial.is_some() {
                    let lido = self.cpu.read_cstring(ponteiro, 200);
                    self.registra_serial(format!("<egl string {ponteiro:#x} = {lido:?}>"));
                }
                (2, ponteiro)
            }
            // void (*eglGetProcAddress(const char *procname))()
            //
            // A faixa de trampolim **é** um ponteiro de função: o endereço codifica interface e
            // slot, e é assim que toda chamada de API chega aqui. Então basta achar o método
            // pelo nome, tirando o `gl` da frente que o OpenGL usa e a vtable não.
            "GetProcAddress" => {
                let name = self.cpu.read_cstring(a[0], MAX_STRING);
                // A busca cobre a tabela inteira, e não só os slots da vtable real: as funções
                // de extensão ficam no fim dela e é só por aqui que o jogo chega a elas.
                let slot = name.strip_prefix("gl").and_then(|method| {
                    (0..crate::brew::aee_slots::GLES.len() as u32)
                        .find(|&s| Interface::Gles.method(s) == Some(method))
                });
                // Um nome `egl*` procura na tabela **antiga**, a `IEGL` de `AEEGL.h`, e não na
                // `IEGL11` que o jogo usa pela vtable. Não é escolha de gosto: o que o
                // `eglGetProcAddress` devolve é uma função C, sem `this` no primeiro argumento,
                // e é exatamente essa a convenção da tabela antiga. Procurar na `IEGL11`, além
                // de comer o primeiro argumento, nunca acertava nome nenhum — lá eles estão sem
                // o prefixo `egl`, e aqui se procura com ele.
                let egl = (name.starts_with("egl") && slot.is_none())
                    .then(|| {
                        (0..crate::brew::aee_slots::EGL_LEGACY.len() as u32)
                            .find(|&s| Interface::EglLegacy.method(s) == Some(name.as_str()))
                    })
                    .flatten();
                match (slot, egl) {
                    (Some(slot), _) => (1, aee::encode(Interface::Gles, slot)),
                    (_, Some(slot)) => (1, aee::encode(Interface::EglLegacy, slot)),
                    _ => {
                        self.bad_pointers
                            .insert(format!("o jogo pediu o endereço de {name}, que não temos"));
                        (1, 0)
                    }
                }
            }
            // `GetConfigs` lista as configurações e `ChooseConfig` filtra por atributos. Como
            // só existe uma, e ela é a nativa da tela, as duas respondem o mesmo.
            "GetConfigs" | "ChooseConfig" => {
                let first = usize::from(name == "ChooseConfig");
                let (configs, size, num) = (a[1 + first], a[2 + first], a[3 + first]);
                let fits = size >= 1;
                if configs != 0 && fits {
                    self.cpu.write_u32(configs, EGL_CONFIG)?;
                }
                self.write_at(num, u32::from(configs == 0 || fits))?;
                (4 + first, gles::EGL_TRUE)
            }
            "GetConfigAttrib" => match gles::config_attrib(a[2], width, height) {
                Some(answer) => {
                    self.write_at(a[3], answer)?;
                    (4, gles::EGL_TRUE)
                }
                None => {
                    self.egl_error = gles::EGL_BAD_ATTRIBUTE;
                    (4, gles::EGL_FALSE)
                }
            },
            "CreateWindowSurface" | "CreatePixmapSurface" | "CreatePbufferSurface" => {
                let pbuffer = name == "CreatePbufferSurface";
                let handle = self.new_egl_handle();
                // **O pbuffer tem o tamanho que a lista de atributos disser, não o da tela.**
                // A Z-Wheel desenha o palco num de 640×330 — a área do widget da roda — e é
                // esse recorte que o `eglGetColorBufferQUALCOMM` devolve. Enquanto o tamanho
                // aqui era o da tela, o traço saía deslocado: o cilindro passava da borda e o
                // painel de baixo era cortado.
                //
                // A janela e o pixmap não trazem tamanho na lista; para eles a tela continua
                // valendo.
                let atributos = a[if pbuffer { 2 } else { 3 }];
                let medida = match pbuffer {
                    true => self.medida_dos_atributos(atributos, (width, height))?,
                    false => (width, height),
                };
                self.egl_surfaces.insert(handle, medida);
                (if pbuffer { 3 } else { 4 }, handle)
            }
            "DestroySurface" => {
                self.egl_surfaces.remove(&a[1]);
                (2, gles::EGL_TRUE)
            }
            "QuerySurface" => {
                let (w, h) = self
                    .egl_surfaces
                    .get(&a[1])
                    .copied()
                    .unwrap_or((width, height));
                let answer = match a[2] {
                    gles::EGL_WIDTH => Some(w),
                    gles::EGL_HEIGHT => Some(h),
                    attribute => gles::config_attrib(attribute, width, height),
                };
                match answer {
                    Some(answer) => {
                        self.write_at(a[3], answer)?;
                        (4, gles::EGL_TRUE)
                    }
                    None => {
                        self.egl_error = gles::EGL_BAD_ATTRIBUTE;
                        (4, gles::EGL_FALSE)
                    }
                }
            }
            "CreateContext" => {
                let handle = self.new_egl_handle();
                self.egl_context = handle;
                (4, handle)
            }
            "DestroyContext" => (2, gles::EGL_TRUE),
            "MakeCurrent" => {
                self.egl_surface = a[1];
                self.egl_context = a[3];
                (4, gles::EGL_TRUE)
            }
            "GetCurrentContext" => (0, self.egl_context),
            "GetCurrentSurface" => (1, self.egl_surface),
            "GetCurrentDisplay" => (0, EGL_DISPLAY),
            "QueryContext" => {
                self.write_at(a[3], 0)?;
                (4, gles::EGL_TRUE)
            }
            "WaitGL" => (0, gles::EGL_TRUE),
            "WaitNative" => (1, gles::EGL_TRUE),
            // Apresentar o quadro: o buffer de trás vira o da frente.
            "SwapBuffers" => {
                self.sync_egl_color_from_guest()?;
                self.egl_swaps += 1;
                self.present_gl();
                self.wait_for_vsync();
                (2, gles::EGL_TRUE)
            }
            // As quatro da `EGL_QUALCOMM_surface_scale`, na forma de função C: os argumentos
            // chegam sem `this` e o resultado é o retorno, não um ponteiro de saída. O
            // comportamento é o mesmo já implementado na [`Interface::EglSurfaceManip`] — ver
            // `extension_call` —, e está aqui porque é por ponteiro de função que a Z-Wheel
            // chega a ele.
            //
            // **Anunciar a extensão e não entregar as funções é pior que não anunciar**: o
            // `eglQueryString` já dizia `EGL_QUALCOMM_surface_scale`, e o jogo, achando os
            // ponteiros nulos, descartava o grupo.
            // `EGLBoolean eglSwapIntervalOES(EGLDisplay dpy, EGLint interval)`. O ritmo de quadro
            // aqui é o do relógio virtual, no `wait_for_vsync`; aceitar e não guardar é o que
            // deixa o jogo seguir sem prometer um intervalo que não controlamos.
            "SwapIntervalOES" => (2, gles::EGL_TRUE),
            "SurfaceScaleEnableQUALCOMM" => (3, gles::EGL_TRUE),
            // `EGLBoolean eglSetSurfaceScaleQUALCOMM(dpy, surf, const rect *src, const rect *dst)`
            "SetSurfaceScaleQUALCOMM" => {
                let origem = a[2];
                if origem != 0 {
                    let largura = self.cpu.read_u32(origem + 8)? as i32;
                    let altura = self.cpu.read_u32(origem + 12)? as i32;
                    if largura > 0 && altura > 0 {
                        self.scale_source = Some((largura, altura));
                        self.gl.set_surface(largura as usize, altura as usize);
                    }
                }
                (4, gles::EGL_TRUE)
            }
            // `EGLBoolean eglGetSurfaceScaleQUALCOMM(dpy, surf, EGLBoolean *on, rect *src, *dst)`
            "GetSurfaceScaleQUALCOMM" => {
                let (ligado, origem, destino) = (a[2], a[3], a[4]);
                self.write_at(ligado, u32::from(self.scale_source.is_some()))?;
                let (largura, altura) = match self.scale_source {
                    Some(tamanho) => tamanho,
                    None => {
                        let (w, h) = self.gl.surface();
                        (w as i32, h as i32)
                    }
                };
                for (retangulo, tamanho) in [
                    (origem, (largura, altura)),
                    (destino, (SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32)),
                ] {
                    if retangulo != 0 {
                        self.cpu.write_u32(retangulo, 0)?;
                        self.cpu.write_u32(retangulo + 4, 0)?;
                        self.cpu.write_u32(retangulo + 8, tamanho.0 as u32)?;
                        self.cpu.write_u32(retangulo + 12, tamanho.1 as u32)?;
                    }
                }
                (5, gles::EGL_TRUE)
            }
            // `EGLBoolean eglGetSurfaceScaleCapsQUALCOMM(dpy, surf, AEEEGLSurfaceScaleCaps *)`
            //
            // Mesmos valores da interface: ampliar da superfície do jogo até a tela, com os
            // fatores em ponto fixo 16.16.
            "GetSurfaceScaleCapsQUALCOMM" => {
                let caps = a[2];
                if caps != 0 {
                    let campos: [u32; 12] = [
                        1 << 16,
                        8 << 16,
                        1 << 16,
                        8 << 16,
                        1,
                        SCREEN_WIDTH as u32,
                        1,
                        SCREEN_HEIGHT as u32,
                        1,
                        SCREEN_WIDTH as u32,
                        1,
                        SCREEN_HEIGHT as u32,
                    ];
                    for (indice, valor) in campos.iter().enumerate() {
                        self.cpu.write_u32(caps + indice as u32 * 4, *valor)?;
                    }
                }
                (3, gles::EGL_TRUE)
            }
            // `void *eglGetColorBufferQUALCOMM(void)` — **sem argumento nenhum**.
            //
            // A chamada é `blx r0` puro, em `0x76cbc`, com o ponteiro lido de `[r4+0x34]`: os
            // registradores que chegam aqui são sobra, e o único que importa é o que sai. Zero
            // é falha — o `cmp r0,#0` logo depois desvia para `0x76e30` e o palco desiste.
            //
            // O que sai é o endereço cru dos pixels. O jogo já sabe as dimensões: guarda
            // largura e altura em `[r4+0x18]` e `[r4+0x1c]` e monta com elas o descritor do
            // traço. Por isso não há saída de tamanho aqui, e por isso o formato tem de ser o
            // da tela — RGB565, o mesmo que o `present_gl` entrega.
            //
            // É esta função que existe porque a Z-Wheel desenha o palco num **pbuffer** e não
            // numa janela: sem um ponteiro para o resultado, não há como compor o 3D com o 2D.
            "GetColorBufferQUALCOMM" => {
                self.sync_egl_color_from_guest()?;
                let (largura, altura) = match self.egl_surfaces.get(&self.egl_surface) {
                    Some(&(l, a)) => (l as usize, a as usize),
                    None => {
                        let alvo = self.screen();
                        (alvo.width() as usize, alvo.height() as usize)
                    }
                };
                // O vetor de bytes é o mesmo de uma chamada para a outra: são quatrocentos
                // kilobytes por leitura, duas leituras por quadro, e alocar isso sessenta vezes
                // por segundo não paga nada.
                //
                // Guardar o quadro convertido para servir a segunda leitura **não** funciona:
                // medido, zero de mil e duzentas e noventa e oito leituras puderam ser
                // reaproveitadas, porque o jogo desenha entre uma e outra.
                let mut bytes = std::mem::take(&mut self.egl_color_bytes);
                self.gl.frame_rgb565(largura, altura, &mut bytes);
                if self.egl_color_buffer.1 < bytes.len() {
                    match self.surface_alloc(bytes.len() as u32) {
                        Some(onde) => {
                            self.egl_color_buffer = (onde, bytes.len());
                            // A faixa mudou de lugar: o watchpoint acompanha.
                            self.cpu
                                .watch_dirty(VIGIA_COLOR_BUFFER, onde, bytes.len() as u32)?;
                        }
                        None => {
                            self.egl_color_bytes = bytes;
                            return Ok(Some(0));
                        }
                    }
                }
                self.cpu.write_mem(self.egl_color_buffer.0, &bytes)?;
                self.egl_color_bytes = bytes;
                self.egl_color_dimensions = Some((largura, altura));
                (0, self.egl_color_buffer.0)
            }
            "CopyBuffers" => (3, gles::EGL_TRUE),
            "SurfaceAttrib" => (4, gles::EGL_TRUE),
            "BindTexImage" | "ReleaseTexImage" => (3, gles::EGL_TRUE),
            "SwapInterval" => (2, gles::EGL_TRUE),
            _ => return Ok(None),
        };
        if legacy {
            return Ok(Some(value));
        }
        self.write_at(a[out], value)?;
        Ok(Some(SUCCESS))
    }

    /// Lê largura e altura de uma lista de atributos do EGL, com o padrão para o que faltar.
    ///
    /// A lista é um vetor de pares `(atributo, valor)` terminado por `EGL_NONE`. O teto de
    /// pares existe porque a lista vem do jogo: uma sem terminador não pode virar laço eterno.
    pub(super) fn medida_dos_atributos(
        &mut self,
        lista: u32,
        padrao: (u32, u32),
    ) -> Result<(u32, u32), CpuError> {
        /// Quantos pares ler antes de desistir.
        const TETO: u32 = 64;

        let (mut largura, mut altura) = padrao;
        if lista == 0 {
            return Ok(padrao);
        }
        for par in 0..TETO {
            let atributo = self.cpu.read_u32(lista + par * 8)?;
            if atributo == gles::EGL_NONE {
                break;
            }
            let valor = self.cpu.read_u32(lista + par * 8 + 4)?;
            match atributo {
                gles::EGL_WIDTH => largura = valor,
                gles::EGL_HEIGHT => altura = valor,
                _ => {}
            }
        }
        Ok((largura, altura))
    }

    /// Escreve uma palavra num ponteiro de saída, ignorando o nulo.
    pub(super) fn write_at(&mut self, pointer: u32, value: u32) -> Result<(), CpuError> {
        if pointer != 0 {
            self.cpu.write_u32(pointer, value)?;
        }
        Ok(())
    }

    /// `QueryInterface` do objeto do EGL.
    ///
    /// O `AEECLSID_QEGL` é um objeto só que responde por várias interfaces: o EGL propriamente
    /// dito e o OpenGL ES. Devolver `this` para tudo, como fazíamos, entregava a vtable do EGL
    /// para quem pediu a do GL — e a primeira chamada caía num slot que não existe.
    pub(super) fn egl_query_interface(&mut self, iface: Interface) -> Result<u32, CpuError> {
        let (iid, out) = (self.arg(1), self.arg(2));
        // Um `QueryInterface` na forma antiga devolve a forma antiga do OpenGL: as duas
        // convenções não se misturam dentro de um mesmo objeto.
        let gl = match iface {
            Interface::EglLegacy => Interface::GlLegacy,
            _ => Interface::Gles,
        };
        let object = match iid {
            AEEIID_GLES10 | AEEIID_GLES11 => {
                if self.gles_object == 0 {
                    self.gles_object = self.new_object(gl)?;
                }
                self.gles_object
            }
            AEEIID_EGL10 | AEEIID_EGL11 => self.arg(0),
            // As extensões do console. A V2 é superconjunto da V1 com o mesmo prefixo de
            // vtable, então o mesmo objeto atende as duas IIDs.
            AEEIID_EGL_SURFACE_MANIP | AEEIID_EGL_SURFACE_MANIP_V1 => {
                if self.surface_manip == 0 {
                    self.surface_manip = self.new_object(Interface::EglSurfaceManip)?;
                }
                self.surface_manip
            }
            AEEIID_GLES_IMAGEON_EXT | AEEIID_GLES_IMAGEON_EXT_V1 => {
                if self.imageon_ext == 0 {
                    self.imageon_ext = self.new_object(Interface::GlesImageonExt)?;
                }
                self.imageon_ext
            }
            _ => {
                self.unknown_classes.insert(iid);
                if out != 0 {
                    self.cpu.write_u32(out, 0)?;
                }
                return Ok(ECLASSNOTSUPPORT);
            }
        };
        self.objects.add_ref(object);
        if out != 0 {
            self.cpu.write_u32(out, object)?;
        }
        Ok(SUCCESS)
    }

    /// Importa somente pixels modificados pelo guest; o restante pode ter sido
    /// atualizado pelo GL desde a última exposição e não deve ser sobrescrito.
    pub(super) fn sync_egl_color_from_guest(&mut self) -> Result<(), CpuError> {
        let Some((width, height)) = self.egl_color_dimensions else {
            return Ok(());
        };
        // **Só lê quando o jogo escreveu.** Esta função é chamada em todo `Draw*`, `Clear` e
        // `ReadPixels` — 93 mil vezes em treze segundos da Z-Wheel —, e a versão anterior lia
        // 400 KB do guest e os comparava byte a byte em cada uma delas, só para descobrir que
        // quase nunca havia mudança. O watchpoint de escrita responde a mesma pergunta de graça.
        if !self.cpu.take_dirty(VIGIA_COLOR_BUFFER) {
            return Ok(());
        }
        self.egl_color_readback
            .resize(self.egl_color_bytes.len(), 0);
        self.cpu
            .read_mem(self.egl_color_buffer.0, &mut self.egl_color_readback)?;
        if self.egl_color_readback != self.egl_color_bytes {
            self.gl.import_rgb565_changes(
                width,
                height,
                &self.egl_color_bytes,
                &self.egl_color_readback,
            );
            std::mem::swap(&mut self.egl_color_bytes, &mut self.egl_color_readback);
        }
        Ok(())
    }

    /// Próximo identificador de superfície ou contexto do EGL.
    pub(super) fn new_egl_handle(&mut self) -> u32 {
        let handle = self.egl_next_handle;
        self.egl_next_handle += 1;
        handle
    }
}
