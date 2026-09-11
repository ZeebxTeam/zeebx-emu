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
    pub(super) fn extension_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
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
                        self.gl.set_surface(width as usize, height as usize);
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
            // Os buffers de vértice da ATI e da Qualcomm. Nenhum jogo do console chegou a
            // usá-los, e responder sucesso sem guardar nada faria o desenho seguinte sair de
            // lixo — recusar é mais honesto.
            "BindBufferQUALCOMM"
            | "DeleteBuffersQUALCOMM"
            | "GenBuffersQUALCOMM"
            | "BufferDataQUALCOMM"
            | "BufferSubDataQUALCOMM"
            | "IsBufferQUALCOMM"
            | "BufferDataATI"
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
            "Consultar" => {
                self.assumptions
                    .insert("o controle de sistema respondeu zero: não há aparelho para consultar");
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
