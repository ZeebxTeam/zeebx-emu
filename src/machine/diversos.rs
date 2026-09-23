//! As interfaces de uma chamada só: configuração, SIM, energia, licença e coleções.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// As extensões gráficas do console: `IEGLSurfaceManip` e `IGLESImageonExt`.
    ///
    /// Elas existem porque dez portes de arcade — todos sobre o mesmo emulador de Neo Geo —
    /// pedem as duas por `QueryInterface` no objeto EGL e desistem da inicialização gráfica
    /// sem elas, escrevendo "InitGLExtensions failed" na tela.
    ///
    /// Quase tudo aqui responde "consegui" sem fazer nada, e isso é deliberado: rotação,
    /// transparência e sobreposição de camadas não mudam o que o jogo desenha, só como o
    /// console compõe o resultado. Recusar faria o jogo desistir por causa de um recurso que
    /// ele nem chega a usar.
    ///
    /// A exceção é a **escala**: ali o jogo diz o tamanho da superfície em que desenha, e essa
    /// informação vale mais que a dedução por viewport que fazemos na falta dela.
    pub(super) fn extension_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            // `IQueryInterface`: um objeto responde por várias interfaces, e quem pergunta qual
            // delas quer recebe um ponteiro para **aquela** vtable.
            //
            // **É por aqui que o Prey Evil pede as extensões OES**, e não por
            // `ISHELL_CreateInstance`: ele pergunta no objeto do EGL se há `IGLES11Ext`, e recebia
            // `ECLASSNOTSUPPORT` — daí ele montar matrizes e texturas e nunca desenhar. A resposta
            // certa é um objeto com a vtable pedida; sem isto, registrar a classe na fábrica não
            // muda nada.
            "QueryInterface" if iface == Interface::Egl && self.arg(1) == AEECLSID_GLES11EXT => {
                let (out, objeto) = (self.arg(2), self.new_object(Interface::Gles11Ext)?);
                if out != 0 {
                    self.cpu.write_u32(out, objeto)?;
                }
                match objeto {
                    0 => ECLASSNOTSUPPORT,
                    _ => SUCCESS,
                }
            }
            "QueryInterface" => {
                let (iid, out) = (self.arg(1), self.arg(2));
                if out != 0 {
                    self.cpu.write_u32(out, 0)?;
                }
                self.unknown_classes.insert(iid);
                ECLASSNOTSUPPORT
            }
            // int SetSurfaceScale(pMe, dpy, surf, AEEEGLSurfaceScaleRect *src, *dst,
            //                     AEEEGLBoolean *ret)
            "SetSurfaceScale" => {
                let source = self.arg(3);
                if source != 0 {
                    let width = self.cpu.read_u32(source + 8)? as i32;
                    let height = self.cpu.read_u32(source + 12)? as i32;
                    if width > 0 && height > 0 {
                        self.scale_source = Some((width, height));
                        self.gl.set_surface_esticada(width as usize, height as usize);
                    }
                }
                self.write_egl_true(4)?
            }
            // int GetSurfaceScale(pMe, dpy, surf, EGLBoolean *enabled, *src, *dst, *ret)
            "GetSurfaceScale" => {
                let (enabled, source, dest) = (self.arg(3), self.arg(4), self.arg(5));
                self.write_at(enabled, u32::from(self.scale_source.is_some()))?;
                let (width, height) = match self.scale_source {
                    Some(size) => size,
                    None => {
                        let (w, h) = self.gl.surface();
                        (w as i32, h as i32)
                    }
                };
                for (rect, size) in [
                    (source, (width, height)),
                    (dest, (SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32)),
                ] {
                    if rect != 0 {
                        self.cpu.write_u32(rect, 0)?;
                        self.cpu.write_u32(rect + 4, 0)?;
                        self.cpu.write_u32(rect + 8, size.0 as u32)?;
                        self.cpu.write_u32(rect + 12, size.1 as u32)?;
                    }
                }
                self.write_egl_true(6)?
            }
            // int GetSurfaceScaleCaps(pMe, dpy, surf, AEEEGLSurfaceScaleCaps *param, *ret)
            //
            // O console amplia da superfície do jogo para a tela; anunciamos exatamente essa
            // faixa. Os fatores são ponto fixo 16.16, como manda o `AEEEGLfixed`.
            "GetSurfaceScaleCaps" => {
                let caps = self.arg(3);
                if caps != 0 {
                    let fields: [u32; 12] = [
                        1 << 16, // MinXScaleFactor: nunca reduz
                        8 << 16, // MaxXScaleFactor
                        1 << 16, // MinYScaleFactor
                        8 << 16, // MaxYScaleFactor
                        1,       // MinSrcWidth
                        SCREEN_WIDTH as u32,
                        1, // MinSrcHeight
                        SCREEN_HEIGHT as u32,
                        1, // MinDstWidth
                        SCREEN_WIDTH as u32,
                        1, // MinDstHeight
                        SCREEN_HEIGHT as u32,
                    ];
                    for (index, value) in fields.iter().enumerate() {
                        self.cpu.write_u32(caps + index as u32 * 4, *value)?;
                    }
                }
                self.write_egl_true(4)?
            }
            // O resto da manipulação de superfície: aceitar sem fazer é honesto porque nada
            // disso muda o que o jogo desenha. O último argumento é sempre o `EGLBoolean *ret`.
            "SurfaceScaleEnable"
            | "SurfaceRotateEnable"
            | "SetSurfaceRotate"
            | "SurfaceTransparencyEnable"
            | "SetSurfaceTransparency"
            | "SetSurfaceTransparencyMap"
            | "SurfaceColorKeyEnable"
            | "SetSurfaceColorKey"
            | "SurfaceOverlayEnable"
            | "SurfaceOverlayLayerEnable"
            | "SurfaceOverlayBind" => {
                let last = EXTENSION_RESULT_SLOT
                    .iter()
                    .find(|(method, _)| *method == name)
                    .map(|(_, slot)| *slot)
                    .unwrap_or(4);
                self.write_egl_true(last)?
            }
            // As consultas que não temos como responder de verdade: zeram a saída e dizem que
            // o recurso não está ligado, que é a verdade.
            "GetSurfaceRotate"
            | "GetSurfaceRotateCaps"
            | "GetSurfaceTransparency"
            | "GetSurfaceTransparencyMap"
            | "GetSurfaceTransparencyCaps"
            | "GetSurfaceColorKey"
            | "GetSurfaceOverlayBinding"
            | "GetSurfaceOverlay"
            | "GetSurfaceOverlayCaps"
            | "CreateCompositeSurface" => {
                for index in 3..8 {
                    let out = self.arg(index);
                    if out != 0 {
                        self.cpu.write_u32(out, 0)?;
                    }
                }
                SUCCESS
            }
            // `IGLESImageonExt` repete métodos do OpenGL ES com outra assinatura: aqui o `this`
            // é a extensão, então os argumentos vêm um lugar à frente.
            "TexEnvi" | "TexEnviv" | "TexParameteri" | "TexParameteriv" | "TexParameterfv"
            | "TexParameterxv" => {
                let (pname, value) = (self.arg(2), self.arg(3));
                match name.ends_with('v') {
                    true if pname == gles::GL_TEXTURE_CROP_RECT_OES => {
                        let mut crop = [0i32; 4];
                        for (index, item) in crop.iter_mut().enumerate() {
                            *item = self.cpu.read_u32(value + index as u32 * 4)? as i32;
                        }
                        self.gl.set_texture_crop(crop);
                    }
                    true => {
                        let value = self.cpu.read_u32(value)?;
                        self.apply_texture_setting(name, pname, value);
                    }
                    false => self.apply_texture_setting(name, pname, value),
                }
                SUCCESS
            }
            "BlendEquationEXT"
            | "BlendEquationSeparateEXT"
            | "BlendFuncSeparateEXT"
            | "PointSizePointerOES" => SUCCESS,
            // **Os buffers de vértice da Qualcomm são os objetos de buffer de sempre.** O
            // Ridge Racer preenche cem deles por partida — cem `GenBuffersQUALCOMM`, trezentos
            // `BufferSubDataQUALCOMM` —, e enquanto a extensão os recusava toda essa geometria
            // ia para o lixo com a hipótese "usou um buffer que não temos" no relatório.
            //
            // A extensão passa o próprio objeto no primeiro argumento, então tudo vem um lugar
            // à frente; o resto é o mesmo nome, o mesmo alvo e o mesmo conteúdo do
            // `glBindBuffer` e companhia, e é no mesmo lugar que eles ficam guardados.
            "GenBuffersQUALCOMM" => {
                let (quantos, saida) = (self.arg(1), self.arg(2));
                for i in 0..quantos {
                    self.gles_next_name += 1;
                    if saida != 0 {
                        self.cpu.write_u32(saida + i * 4, self.gles_next_name)?;
                    }
                }
                SUCCESS
            }
            "BindBufferQUALCOMM" => {
                let (alvo, nome) = (self.arg(1), self.arg(2));
                if nome != 0 {
                    self.gl_buffers.entry(nome).or_default();
                }
                match alvo {
                    gles::GL_ARRAY_BUFFER => self.gl_array_buffer = nome,
                    gles::GL_ELEMENT_ARRAY_BUFFER => self.gl_element_buffer = nome,
                    _ => {}
                }
                SUCCESS
            }
            "BufferDataQUALCOMM" => {
                let (alvo, tamanho, dados) = (self.arg(1), self.arg(2), self.arg(3));
                match self.buffer_ligado(alvo) {
                    None => SUCCESS,
                    Some(nome) => {
                        let conteudo = match dados {
                            0 => vec![0u8; tamanho as usize],
                            _ => self.read_bytes(dados, tamanho)?,
                        };
                        self.gl_buffers.insert(nome, conteudo);
                        SUCCESS
                    }
                }
            }
            "BufferSubDataQUALCOMM" => {
                let (alvo, inicio, tamanho, dados) =
                    (self.arg(1), self.arg(2), self.arg(3), self.arg(4));
                match self.buffer_ligado(alvo) {
                    None => SUCCESS,
                    Some(nome) => {
                        let novo = self.read_bytes(dados, tamanho)?;
                        if let Some(buffer) = self.gl_buffers.get_mut(&nome) {
                            let fim = inicio as usize + novo.len();
                            if fim <= buffer.len() {
                                buffer[inicio as usize..fim].copy_from_slice(&novo);
                            }
                        }
                        SUCCESS
                    }
                }
            }
            "DeleteBuffersQUALCOMM" => {
                let (quantos, lista) = (self.arg(1), self.arg(2));
                for i in 0..quantos {
                    let nome = self.cpu.read_u32(lista + i * 4)?;
                    self.gl_buffers.remove(&nome);
                    if self.gl_array_buffer == nome {
                        self.gl_array_buffer = 0;
                    }
                    if self.gl_element_buffer == nome {
                        self.gl_element_buffer = 0;
                    }
                }
                SUCCESS
            }
            "IsBufferQUALCOMM" => u32::from(self.gl_buffers.contains_key(&self.arg(1))),
            // Os buffers da ATI, que nenhum jogo do console chegou a usar. Responder sucesso sem
            // guardar nada faria o desenho seguinte sair de lixo — recusar é mais honesto.
            "BufferDataATI"
            | "MeshListATI"
            | "DrawVertexBufferObjectATI"
            | "GetPointerv"
            | "GetMaterialfv"
            | "GetTexParameteriv"
            | "GetTexParameterfv"
            | "GetTexParameterxv" => {
                self.assumptions
                    .insert("o jogo usou um buffer de vértices da extensão, que não temos");
                EUNSUPPORTED
            }
            // `int QueryMatrixxOES(pMe, AEEGLfixed *mantissa, AEEGLint *exponent, bitfield *ret)`
            //
            // A matriz corrente em ponto fixo, como o GLES a representa: um `fixed` por elemento e
            // o expoente de cada um. **Respondemos a identidade**, e é honesto: o ponto fixo da
            // nossa matriz vive na etapa de vértice, e converter de volta introduziria erro onde o
            // jogo espera exatamente o que ele mandou. Nenhum jogo do acervo lê esta matriz para
            // desenhar — o Prey Evil a pede para saber se a extensão existe.
            "QueryMatrixxOES" if iface == Interface::Gles10Ext => {
                let (mantissa, expoente) = (self.arg(1), self.arg(2));
                for i in 0..16u32 {
                    let identidade = u32::from(i % 5 == 0) * (1 << 16);
                    if mantissa != 0 {
                        self.cpu.write_u32(mantissa + i * 4, identidade)?;
                    }
                    if expoente != 0 {
                        self.cpu.write_u32(expoente + i * 4, 0)?;
                    }
                }
                SUCCESS
            }
            // `int GetPowerLevel(pMe, int *ret)`. O console é portátil; o emulador não tem
            // bateria para consultar, e "cheia" é a resposta que não faz o jogo pedir para
            // carregar — nem abrir uma tela de aviso que ninguém pediu.
            "GetPowerLevel" if iface == Interface::EglGetPowerLevel => {
                let out = self.arg(1);
                if out != 0 {
                    self.cpu.write_u32(out, 100)?;
                }
                SUCCESS
            }
            // `int SwapInterval(pMe, dpy, interval, EGLBoolean *ret)` e o `Get` dele.
            //
            // **Aceitar e não guardar é o que o caminho de função já faz**, e é o certo: o ritmo
            // de quadro aqui é o do relógio virtual, e prometer um intervalo que não controlamos
            // seria pior que não prometer nada.
            "SwapInterval" if iface == Interface::EglOesSwapInterval => self.write_egl_true(3)?,
            "GetSwapInterval" if iface == Interface::EglOesSwapInterval => {
                let out = self.arg(2);
                if out != 0 {
                    self.cpu.write_u32(out, 1)?;
                }
                SUCCESS
            }
            // `int GetColorBuffer(pMe, void **ret)` — o mesmo buffer de cor que a
            // `eglGetColorBufferQUALCOMM` entrega por função, pelo mesmo cálculo.
            "GetColorBuffer" if iface == Interface::EglGetColorBuffer => {
                let (out, buffer) = (self.arg(1), self.egl_color_da_tela()?);
                if out != 0 {
                    self.cpu.write_u32(out, buffer)?;
                }
                SUCCESS
            }
            // `IGLES11ExtPak`: `TexGen`, blending separado e objetos de framebuffer.
            //
            // **O tratamento é o mesmo das outras extensões gráficas** — responder "consegui" ao
            // que não muda o que o jogo desenha, e dar resposta plausível ao que ele consulta.
            // Nenhuma dessas famílias altera o traço no nosso rasterizador: a geração de
            // coordenadas de textura é a única que mexeria, e nenhum jogo do acervo a usa para
            // desenhar — o Prey Evil a pede para saber que o pacote existe. Os objetos de
            // framebuffer ganham identificadores de mentira e respondem "completo": o alvo aqui é
            // um só, e é o certo.
            "TexGenf" | "TexGeni" | "TexGenx" | "TexGenfv" | "TexGeniv" | "TexGenxv"
            | "BlendEquation" | "BlendFuncSeparate" | "BlendEquationSeparate"
            | "BindRenderbufferOES" | "DeleteFramebuffersOES" | "DeleteRenderbuffersOES"
            | "FramebufferRenderbufferOES" | "FramebufferTexture2DOES" | "GenerateMipmapOES"
            | "RenderbufferStorageOES"
                if iface == Interface::Gles11ExtPak =>
            {
                self.assumptions.insert(concat!(
                    "o jogo usou o pacote de extensões OES (IGLES11ExtPak); o alvo de desenho ",
                    "continua sendo único, e o que ele pediu foi atendido sem mudar o traço"
                ));
                SUCCESS
            }
            // Os `Gen*OES` devolvem um identificador pelo ponteiro de saída: zero é "acabou", e o
            // jogo desiste da família inteira. **Devolvemos sempre o mesmo `1`**, e é coerente: o
            // alvo de desenho aqui é um só, então há um framebuffer e um renderbuffer, e pedir
            // mais devolve o mesmo. Um contador daria identificadores que não levam a lugar nenhum.
            "GenFramebuffersOES" | "GenRenderbuffersOES" if iface == Interface::Gles11ExtPak => {
                let out = self.arg(1);
                if out != 0 {
                    self.cpu.write_u32(out, 1)?;
                }
                SUCCESS
            }
            "IsFramebufferOES" | "IsRenderbufferOES" if iface == Interface::Gles11ExtPak => {
                self.write_egl_true(1)?
            }
            // `GL_FRAMEBUFFER_COMPLETE_OES` é 0x8CD5, e é o que um alvo único sempre é.
            "CheckFramebufferStatusOES" if iface == Interface::Gles11ExtPak => 0x8cd5,
            "GetTexGenfv" | "GetTexGeniv" | "GetTexGenxv"
            | "GetFramebufferAttachmentParameterivOES" | "GetRenderbufferParameterivOES"
                if iface == Interface::Gles11ExtPak =>
            {
                // Zerar o que se consulta é melhor que deixar lixo na memória do jogo, e é o que
                // o resto do motor faz com os `Get` que não têm o que devolver.
                self.write_at(self.arg(2), 0)?;
                SUCCESS
            }
            // `IJoystick`: o joystick USB que o gerenciador de joystick da Qualcomm procura.
            //
            // `int SetParm(pMe, int16 nParmID, int32 p1, int32 p2)` — calibração e configuração.
            // Aceitar e não guardar é o certo: o controle que temos é o Z-Pad, e não há parâmetro
            // dele que o jogo possa mudar por aqui.
            "SetParm" if iface == Interface::Joystick => SUCCESS,
            // `int GetParm(pMe, int16 nParmID, int32 *pP1)` — responder zero é melhor que deixar
            // lixo, que o jogo leria como calibração.
            "GetParm" if iface == Interface::Joystick => {
                self.write_at(self.arg(2), 0)?;
                SUCCESS
            }
            // `int Read(pMe, int16 *px, int16 *py)` — **o estado do controle, de verdade**.
            //
            // É a mesma leitura que o `IHIDDevice::GetPositionState` entrega, na faixa do console
            // (repouso em 128): o joystick e o Z-Pad são o mesmo aparelho para quem joga, e é o
            // que o gerenciador de joystick do jogo espera ler.
            "Read" if iface == Interface::Joystick => {
                let (px, py) = (self.arg(1), self.arg(2));
                let pad = self.pads[0];
                for (onde, eixo) in [(px, 0), (py, 1)] {
                    if onde != 0 {
                        let valor = pad.eixo_do_console(eixo) as i16;
                        self.cpu.write_mem(onde, &valor.to_le_bytes())?;
                    }
                }
                SUCCESS
            }
            // `IGLES11Ext`: as extensões OES do OpenGL ES 1.1.
            //
            // **O Prey Evil não desenha sem elas.** O levantamento das 62 ROMs o pegou com a tela
            // de uma cor só: onze métodos de GL no relatório — `MatrixMode`, `PushMatrix`,
            // `BindTexture` dezesseis mil vezes — e **nenhum** `Draw` ou `eglSwapBuffers`. Ele
            // monta o estado e para, porque pede estas extensões por `CreateInstance` e recebia
            // nulo. Sem a interface, o caminho de desenho dele nunca começa.
            //
            // Os `DrawTex*` desenham um retângulo de textura em coordenadas de tela, sem passar
            // pela matriz de modelo — é o que um jogo faz para compor o quadro numa textura. Aqui
            // eles ainda respondem "consegui" sem desenhar: é o passo que faz o jogo **chegar** ao
            // desenho, e o efeito dele é medido pela contagem de cores do relatório. O retângulo
            // de verdade é o passo seguinte, e a referência para ele é o `gles_draw`.
            "CurrentPaletteMatrixOES" | "LoadPaletteFromModelViewMatrixOES"
            | "MatrixIndexPointerOES" | "WeightPointerOES" | "DrawTexsOES" | "DrawTexiOES"
            | "DrawTexxOES" | "DrawTexsvOES" | "DrawTexivOES" | "DrawTexxvOES" | "DrawTexfOES"
            | "DrawTexfvOES"
                if iface == Interface::Gles11Ext =>
            {
                // O nome do método já aparece em "chamadas que mais pesaram"; aqui basta nomear a
                // causa, porque as hipóteses são um conjunto de textos fixos.
                self.assumptions.insert(concat!(
                    "o jogo desenhou por uma extensão OES (IGLES11Ext), cujo ",
                    "retângulo de textura ainda não desenhamos"
                ));
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    pub(super) fn license_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::License.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "IsExpired" => FALSE,
            // AEELicenseType GetInfo(ILicense *, uint32 *pdwExpire)
            //
            // Com `LT_NONE` a documentação diz que não há valor associado, mas o jogo passa
            // um ponteiro e vai ler o que estiver lá — então escrevemos `BV_UNLIMITED`.
            "GetInfo" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, BV_UNLIMITED)?;
                }
                LT_NONE
            }
            // Só faz sentido em licença por uso; a própria documentação manda devolver
            // `EFAILED` quando o tipo não é `LT_USES`.
            "SetUsesRemaining" => EFAILED,
            // AEEPriceType GetPurchaseInfo(ILicense *, AEELicenseType *plt, uint32 *pdwExpire,
            //                              uint32 *pdSeq)
            "GetPurchaseInfo" => {
                if a1 != 0 {
                    self.cpu.write_mem(a1, &[LT_NONE as u8])?;
                }
                if a2 != 0 {
                    self.cpu.write_u32(a2, BV_UNLIMITED)?;
                }
                if a3 != 0 {
                    self.cpu.write_u32(a3, 0)?;
                }
                PT_PURCHASE
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// A coleção genérica da interface da Z-Wheel.
    ///
    /// O app a percorre como um cursor: `Reset` uma vez, e depois `GetCurrent`/`AtEnd` até o
    /// fim. Enquanto o `AtEnd` respondia "ainda não" — que é o que a sonda fazia ao devolver
    /// sucesso —, ele girava quinze milhões de vezes.
    ///
    /// Os slots sem nome ainda não apareceram; se aparecerem, o relatório avisa em vez de
    /// fingir que foram atendidos. É por isso que eles não têm nome na tabela.
    /// Atende a `IConfig`. Ver [`Interface::Config`].
    ///
    /// `int ICONFIG_GetItem(IConfig *pMe, ConfigItem nItem, void *pBuff, int nSize)` e o
    /// `SetItem` de mesma forma. Os itens vivem enquanto o emulador roda, como as preferências
    /// do `ISHELL_GetPrefs`: gravá-los em disco seria inventar um formato que o console tinha e
    /// nós não conhecemos. O que precisa valer é que quem grava releia o que gravou.
    pub(super) fn config_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Config.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.config_items.remove(&this);
                }
                restantes
            }
            "GetItem" => {
                let (item, buffer, tamanho) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3) as usize,
                );
                match self
                    .config_items
                    .get(&this)
                    .and_then(|itens| itens.get(&item))
                {
                    // Devolver menos do que foi pedido seria deixar o resto do buffer com o
                    // que já estava lá, e o jogo leria lixo achando que leu configuração.
                    Some(dados) if dados.len() >= tamanho => {
                        let recorte = dados[..tamanho].to_vec();
                        self.cpu.write_mem(buffer, &recorte)?;
                        SUCCESS
                    }
                    _ => EFAILED,
                }
            }
            "SetItem" => {
                let (item, buffer, tamanho) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3) as usize,
                );
                if tamanho == 0 || tamanho > MAX_STRING {
                    return Ok(Some(EFAILED));
                }
                let mut dados = vec![0u8; tamanho];
                self.cpu.read_mem(buffer, &mut dados)?;
                self.config_items
                    .entry(this)
                    .or_default()
                    .insert(item, dados);
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende o ZEEBOMCP. Ver [`Interface::ZeeboMcp`].
    ///
    /// Só os três slots lidos no firmware são atendidos; os cinco de baixo caem fora e viram
    /// relatório, que é o que queremos quando a Z-Wheel finalmente usar um deles.
    pub(super) fn zeebo_mcp_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::ZeeboMcp.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            // O do firmware aceita dois IIDs: o da própria classe e o `0x01000001`. Aceitar
            // qualquer um seria dizer que este objeto é toda interface do sistema.
            "QueryInterface" => {
                let (iid, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                if iid != AEECLSID_ZEEBOMCP && iid != 0x0100_0001 {
                    return Ok(Some(ECLASSNOTSUPPORT));
                }
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // **Os três slots que a Z-Wheel usa para lançar um jogo mexem na cópia de trabalho
            // do módulo, e aqui ela não existe.** No console o jogo mora no eNAND, em
            // `fs:/card3/mod/<pasta>`, e roda de uma cópia em `fs:/mcp/mod/<pasta>`. Os nomes e
            // os caminhos saem do firmware (`0x110860f0`, `0x11086342`, `0x11086466`) e da
            // mensagem de erro da própria Z-Wheel, `ModDataCopyFromENAND failure, folder='%s'`:
            //
            // - slot 3 copia do eNAND para o MCP antes de rodar;
            // - slot 7 leva o `udata` — os saves — do MCP de volta ao eNAND;
            // - slot 5 apaga a cópia do MCP.
            //
            // Aqui cada jogo roda e grava na própria pasta, então não há o que copiar nem apagar.
            // Com os três recusados, a Z-Wheel desistia de lançar o jogo escolhido na grade.
            "ModDataCopyFromENAND" | "UserDataCopyToENAND" | "ModDataRemoveFromMCP" => SUCCESS,
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende o controle do cartão SIM. Ver [`Interface::SimCardCtl`].
    pub(super) fn sim_card_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::SimCardCtl.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "QueryInterface" => {
                let (iid, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                if iid != AEECLSID_SIMCARDCTL && iid != 0x0100_0001 {
                    return Ok(Some(ECLASSNOTSUPPORT));
                }
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // Guarda o par e não avisa ninguém: não há cartão para verificar, e chamar o
            // retorno seria afirmar que há.
            // Guarda o par e não avisa ninguém: não há cartão para verificar, e chamar o
            // retorno seria afirmar que há. Hoje não chega aqui — a classe não é oferecida.
            "PedirVerificacao" => {
                self.assumptions
                    .insert("uma verificação de cartão SIM foi aceita e nunca respondida");
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende o `IValueModel` (`0x01028e3c`). Ver [`Interface::Classe28e3c`].
    pub(super) fn modelo_de_valor_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        /// `EVT_MDL_VALUE`, o evento de "o valor mudou". O número é o que o ouvinte da grade
        /// confere em `0x37710` antes de pegar o item.
        const EVT_MDL_VALUE: u32 = 0x1000;
        /// O tamanho do `ModelEvent`: `{ evCode, pModel, dwParam }`.
        const MODEL_EVENT: u32 = 12;

        let Some(name) = Interface::Classe28e3c.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let restam = self.objects.release(this);
                if restam == 0 {
                    self.modelos_de_valor.remove(&this);
                }
                restam
            }
            "QueryInterface" => {
                if a2 != 0 {
                    self.cpu.write_u32(a2, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // void AddListener(IValueModel *, ModelListener *pl)
            //
            // O `ModelListener` é `{ pNext, pPrev, pfnListener, pListenerData, pfnCancel,
            // pCancelData }`. O jogo preenche a função e o contexto; o resto é do modelo. O
            // cancelamento fica nulo: quem cancela um ouvinte chama o `pfnCancel` se houver, e
            // sem ele o ouvinte continua na nossa lista — por isso a conferência na hora de avisar.
            "AddListener" => {
                if a1 != 0 {
                    let funcao = self.cpu.read_u32(a1 + 8)?;
                    let contexto = self.cpu.read_u32(a1 + 12)?;
                    for campo in [0, 4, 16, 20] {
                        self.cpu.write_u32(a1 + campo, 0)?;
                    }
                    let modelo = self.modelos_de_valor.entry(this).or_default();
                    modelo.ouvintes.retain(|&(onde, _, _)| onde != a1);
                    modelo.ouvintes.push((a1, funcao, contexto));
                }
                SUCCESS
            }
            // void Notify(IValueModel *, ModelEvent *pev)
            "Notify" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1 + 4, this)?;
                    self.avisa_ouvintes(this, a1)?;
                }
                SUCCESS
            }
            // int SetValue(IValueModel *, void *pvValue, int nLen, PFNVALUEFREE pfn)
            //
            // O liberador do valor anterior não é chamado: é código do jogo, e chamá-lo no meio
            // de um despacho é o caminho que já derrubou jogo antes.
            "SetValue" => {
                let modelo = self.modelos_de_valor.entry(this).or_default();
                modelo.valor = a1;
                modelo.tamanho = a2;
                if let Some(evento) = self.heap.alloc(MODEL_EVENT) {
                    self.cpu.write_u32(evento, EVT_MDL_VALUE)?;
                    self.cpu.write_u32(evento + 4, this)?;
                    self.cpu.write_u32(evento + 8, 0)?;
                    self.avisa_ouvintes(this, evento)?;
                    self.heap.free(evento);
                }
                SUCCESS
            }
            // void *GetValue(IValueModel *, int *pnLen)
            "GetValue" => {
                let modelo = self.modelos_de_valor.get(&this).cloned().unwrap_or_default();
                if a1 != 0 {
                    self.cpu.write_u32(a1, modelo.tamanho)?;
                }
                modelo.valor
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Chama cada ouvinte do modelo com o evento, conferindo que ele ainda é o que foi registrado.
    fn avisa_ouvintes(&mut self, modelo: u32, evento: u32) -> Result<(), CpuError> {
        let ouvintes = self
            .modelos_de_valor
            .get(&modelo)
            .map(|m| m.ouvintes.clone())
            .unwrap_or_default();
        for (onde, funcao, contexto) in ouvintes {
            let vivo = self.cpu.read_u32(onde + 8).ok() == Some(funcao)
                && self.cpu.read_u32(onde + 12).ok() == Some(contexto);
            if !vivo || funcao == 0 {
                if let Some(m) = self.modelos_de_valor.get_mut(&modelo) {
                    m.ouvintes.retain(|&(o, _, _)| o != onde);
                }
                continue;
            }
            let orcamento = self.orcamento.max(1);
            let _ = self.call_guest_aninhado(funcao, [contexto, evento, 0, 0], orcamento)?;
        }
        Ok(())
    }

    /// Atende o controle de sistema. Ver [`Interface::SystemCtl`].
    ///
    /// O `QueryInterface` do firmware aceita dois IIDs, o `0x01000001` e o da própria classe, e
    /// é isso que fazemos aqui — aceitar qualquer um seria dizer que este objeto é toda
    /// interface do sistema.
    pub(super) fn system_ctl_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::SystemCtl.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "QueryInterface" => {
                let (iid, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                if iid != AEECLSID_SYSTEMCTL && iid != 0x0100_0001 {
                    return Ok(Some(ECLASSNOTSUPPORT));
                }
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // `slot6(this)` lê o modo atual por uma função do OEM (`0x10e9ff84` no firmware), e
            // `slot3(this, modo, ligado)` o grava: o firmware aceita os modos 0 a 3 e acende
            // sinalizadores de hardware para cada um (`0x10e9fdb6`). A Z-Wheel lê e grava de
            // volta em `0x83594`, logo antes de lançar um jogo — com o slot 3 recusado, o
            // lançamento parava ali.
            //
            // Não há hardware para ajustar aqui, então o modo só é guardado para ser lido de
            // volta. Antes de qualquer gravação, a leitura é zero, como sempre foi.
            "Consultar" => {
                self.assumptions
                    .insert("o controle de sistema respondeu zero: não há aparelho para consultar");
                self.modo_do_sistema
            }
            "DefinirModo" => {
                self.modo_do_sistema = self.cpu.read_reg(Reg::R1);
                SUCCESS
            }
            // O slot 5 guarda o modo **pelo mesmo caminho** do slot 3 — o firmware desmontado
            // mostra que o corpo é o mesmo (`0x10e9fdb6`) — e recebe ainda um terceiro argumento,
            // a opção. Não há hardware para acender, então o modo é guardado e o slot 6 o devolve,
            // como no slot 3; a opção fica registrada na hipótese.
            "DefinirModoComOpcao" => {
                self.modo_do_sistema = self.cpu.read_reg(Reg::R1);
                self.assumptions.insert(
                    "o controle de sistema recebeu modo e opção; só o modo é guardado",
                );
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende o `ICM`. Ver [`Interface::Cm`].
    pub(super) fn cm_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        /// Deslocamento do estado do serviço dentro do `AEECMSSInfo`.
        const ESTADO_DO_SERVICO: u32 = 0x0;
        /// Deslocamento do modo de operação, lido em `0x87cb0`.
        const MODO_DE_OPERACAO: u32 = 0xc;
        /// Deslocamento da intensidade do sinal, lido em `0x696f8` como meia palavra.
        const INTENSIDADE: u32 = 0x28;
        /// `AEECM_SRV_STATUS_SRV`. A `0x696e8` aceita 1, 2 ou 3 e recusa o resto com
        /// `Service status is NOT available!`; 2 é "serviço pleno".
        const COM_SERVICO: u32 = 2;
        /// `SYS_OPRT_MODE_ONLINE`. É com este número que a `0x77564` compara.
        const NO_AR: u32 = 5;
        /// A `0x69830` transforma a intensidade em barras por faixas de nove: `0x45..=0x4d`
        /// são quatro barras, e é onde este número cai.
        const SINAL: u16 = 0x48;
        /// O menor buffer que responde às três leituras que conhecemos.
        const MINIMO: usize = INTENSIDADE as usize + 2;

        let Some(name) = Interface::Cm.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            // int ICM_GetSSInfo(ICM *, AEECMSSInfo *pInfo, uint32 nSize)
            //
            // Uma chamada, dois leitores: a `0x87c90` quer o modo de operação em `+0xc`, e a
            // `0x696a0` quer o estado do serviço em `+0` e, quando ele é 2, a intensidade do
            // sinal em `+0x28`. Quem só respondia ao primeiro deixava o segundo repetindo
            // `Service status is NOT available!` para sempre.
            "GetSSInfo" => {
                let (info, tamanho) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2) as usize,
                );
                if info == 0 || tamanho < MINIMO {
                    return Ok(Some(EBADPARM));
                }
                // Zerar o resto é parte da resposta: o jogo passa um buffer que ele mesmo
                // zerou, mas quem chama esta função não pode contar com isso.
                self.cpu.write_mem(info, &vec![0u8; tamanho])?;
                self.cpu.write_u32(info + ESTADO_DO_SERVICO, COM_SERVICO)?;
                self.cpu.write_u32(info + MODO_DE_OPERACAO, NO_AR)?;
                self.cpu
                    .write_mem(info + INTENSIDADE, &SINAL.to_le_bytes())?;
                self.assumptions.insert(
                    "o ICM respondeu rádio no ar e serviço pleno, com o resto da AEECMSSInfo zerado",
                );
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende a lista genérica da Z-Wheel. Ver [`Interface::Vetor`].
    pub(super) fn vetor_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        /// O índice que o jogo passa para dizer "no fim".
        const NO_FIM: u32 = u32::MAX;

        let Some(name) = Interface::Vetor.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.vetores.remove(&this);
                }
                restantes
            }
            "Tamanho" => self
                .vetores
                .get(&this)
                .map_or(0, |(itens, _)| itens.len() as u32),
            "PegarEm" => {
                let (indice, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let item = self
                    .vetores
                    .get(&this)
                    .and_then(|(itens, _)| itens.get(indice as usize).copied());
                match item {
                    Some(item) => {
                        if saida != 0 {
                            self.cpu.write_u32(saida, item)?;
                        }
                        SUCCESS
                    }
                    // Fora da faixa não escreve nada: deixar a saída como estava é o que
                    // permite ao chamador distinguir "não tem" de "tem e é nulo".
                    None => EBADPARM,
                }
            }
            // O `ReplaceAt` do `IVector`, e é o slot 7 do BREW, entre o `GetAt` e o `InsertAt`.
            // A Z-Wheel o usa num ordenamento por inserção em `0x38e5c`: desloca cada item
            // uma posição e grava o novo no lugar certo. Faltava, e a lista de jogos que
            // "Jogar" abre ficava sem ordenar — e sem sair.
            "SubstituirEm" => {
                let (indice, item) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                match self
                    .vetores
                    .get_mut(&this)
                    .and_then(|(itens, _)| itens.get_mut(indice as usize))
                {
                    Some(lugar) => {
                        *lugar = item;
                        SUCCESS
                    }
                    None => EBADPARM,
                }
            }
            "InserirEm" => {
                let (indice, item) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let Some((itens, _)) = self.vetores.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let onde = match indice {
                    NO_FIM => itens.len(),
                    n => (n as usize).min(itens.len()),
                };
                itens.insert(onde, item);
                SUCCESS
            }
            // O par `RemoverEm(0)` + `PegarEm(0)` em `0x7d788` é um laço que drena a lista: o
            // jogo tira o primeiro, pega o novo primeiro e repete até não haver mais. Sem o
            // `RemoverEm` de verdade ele nunca acaba — foram sete milhões de voltas até o
            // orçamento de instruções estourar.
            "RemoverEm" => {
                let indice = self.cpu.read_reg(Reg::R1) as usize;
                let Some((itens, _)) = self.vetores.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                if indice >= itens.len() {
                    return Ok(Some(EBADPARM));
                }
                itens.remove(indice);
                SUCCESS
            }
            // O liberador é ponteiro de função do módulo, e é para ele que o `Esvaziar` do
            // console entrega cada item. Aqui ele só é guardado — ver a nota no `Esvaziar`.
            "DefinirLiberador" => {
                if let Some((_, liberador)) = self.vetores.get_mut(&this) {
                    *liberador = self.cpu.read_reg(Reg::R1);
                }
                SUCCESS
            }
            // Esvaziar **sem** chamar o liberador de cada item é uma dívida consciente: quem
            // alocou os itens foi o jogo, e chamar código dele no meio de um despacho é o
            // caminho que já derrubou o Zeeboids uma vez. O custo é memória que não volta ao
            // heap do jogo enquanto ele roda, e é por isso que a hipótese fica registrada.
            "Esvaziar" => {
                if let Some((itens, liberador)) = self.vetores.get_mut(&this)
                    && !itens.is_empty()
                    && *liberador != 0
                {
                    itens.clear();
                    self.assumptions.insert(
                        "uma lista foi esvaziada sem chamar o liberador que o jogo registrou",
                    );
                } else if let Some((itens, _)) = self.vetores.get_mut(&this) {
                    itens.clear();
                }
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    pub(super) fn collection_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Collection.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let a1 = self.cpu.read_reg(Reg::R1);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.collections.remove(&this);
                    self.parametros_de_colecao
                        .retain(|(obj, _), _| *obj != this);
                }
                restantes
            }
            "Reset" => {
                if let Some((_, cursor)) = self.collections.get_mut(&this) {
                    *cursor = 0;
                }
                SUCCESS
            }
            // O fim é verdade quando o cursor passou do último item — e uma coleção que
            // ninguém preencheu está no fim desde o começo.
            "AtEnd" => {
                let (itens, cursor) = self
                    .collections
                    .get(&this)
                    .map(|(itens, cursor)| (itens.len(), *cursor))
                    .unwrap_or((0, 0));
                u32::from(cursor >= itens)
            }
            // `slot10(this, id, ponteiro, tamanho)`, visto em `0x7d0c4` com
            // `(0, &{0x01070798}, 4)` — o número passado é o ClassID do próprio applet.
            //
            // O que ele **significa** não dá para dizer: a função que o chama cria a coleção,
            // faz esta chamada e solta o objeto em seguida, sem ler nada de volta. Pode ser
            // "guarde este parâmetro" ou "acrescente este item"; as duas leituras têm o mesmo
            // efeito observável, que é nenhum. Guardar os bytes cobre as duas e não inventa
            // comportamento.
            "Definir" => {
                let (id, ponteiro, tamanho) = (
                    a1,
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3) as usize,
                );
                if ponteiro == 0 || tamanho == 0 || tamanho > MAX_STRING {
                    return Ok(Some(EBADPARM));
                }
                let mut dados = vec![0u8; tamanho];
                self.cpu.read_mem(ponteiro, &mut dados)?;
                self.parametros_de_colecao.insert((this, id), dados);
                SUCCESS
            }
            // O item corrente sai pelo ponteiro de saída, e o cursor anda. Sem item, `EFAILED`.
            "GetCurrent" => {
                let item = self.collections.get_mut(&this).and_then(|(itens, cursor)| {
                    let item = itens.get(*cursor).copied();
                    if item.is_some() {
                        *cursor += 1;
                    }
                    item
                });
                match item {
                    Some(item) => {
                        if a1 != 0 {
                            self.cpu.write_u32(a1, item)?;
                        }
                        SUCCESS
                    }
                    None => EFAILED,
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }
}
