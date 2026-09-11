//! IShell: recursos, informação do aparelho e o arranque do applet.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Os três métodos de timer do `IShell`, todos com a assinatura
    /// `(IShell *, [int32 dwMsecs,] PFNNOTIFY pfn, void *pUser)`.
    ///
    /// `PFNNOTIFY` é `void (*)(void *pUser)`; o par `(pfn, pUser)` identifica o timer, e é por
    /// ele que `CancelTimer` e `GetTimerExpiration` o encontram.
    pub(super) fn shell_timer_call(&mut self, name: &str) -> Result<u32, CpuError> {
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        Ok(match name {
            "SetTimer" => {
                let callback = Callback {
                    function: a2,
                    context: a3,
                };
                if callback.function == 0 {
                    return Ok(EBADPARM);
                }
                // Rearmar o mesmo par `(pfn, pUser)` substitui o timer anterior, em vez de
                // acumular dois — é o que o BREW faz, e o que o jogo espera ao rearmar dentro
                // do próprio callback.
                self.timers.retain(|timer| timer.callback != callback);
                self.timers.push(Timer {
                    deadline_ms: self.now_ms().saturating_add(a1),
                    callback,
                });
                SUCCESS
            }
            "CancelTimer" => {
                let (function, context) = (a1, a2);
                // `pfn` nulo cancela todos os timers daquele contexto.
                self.timers.retain(|timer| {
                    timer.callback.context != context
                        || (function != 0 && timer.callback.function != function)
                });
                SUCCESS
            }
            // Devolve quanto falta, em milissegundos; zero se não há timer armado.
            _ => {
                let callback = Callback {
                    function: a1,
                    context: a2,
                };
                self.timers
                    .iter()
                    .find(|timer| timer.callback == callback)
                    .map(|timer| timer.deadline_ms.saturating_sub(self.now_ms()))
                    .unwrap_or(0)
            }
        })
    }

    /// `int ISHELL_DetectType(IShell *, const void *cpBuf, uint32 *pdwSize,
    /// const char *cpszName, const char **pcpszMIME)`.
    ///
    /// Descobre o tipo MIME de um conteúdo. O jogo usa a resposta para achar, no registro do
    /// BREW, qual classe sabe abrir aquele arquivo.
    pub(super) fn shell_detect_type(&mut self) -> Result<u32, CpuError> {
        let (buffer, size_ptr, name_ptr, mime_ptr) =
            (self.arg(1), self.arg(2), self.arg(3), self.arg(4));

        // Sem dados e sem nome, a pergunta é "de quantos bytes você precisa?".
        if buffer == 0 && name_ptr == 0 {
            self.write_at(size_ptr, DETECT_TYPE_BYTES)?;
            return Ok(ENEEDMORE);
        }

        let available = if size_ptr == 0 {
            0
        } else {
            self.cpu.read_u32(size_ptr)?
        };
        let bytes = if buffer == 0 {
            Vec::new()
        } else {
            self.read_bytes(buffer, available.min(DETECT_TYPE_BYTES))?
        };
        let name = self.cpu.read_cstring(name_ptr, MAX_STRING);

        match detect_mime(&bytes, &name) {
            Some(mime) => {
                let text = self.intern(mime)?;
                self.write_at(mime_ptr, text)?;
                Ok(SUCCESS)
            }
            None => Ok(ENOTYPE),
        }
    }

    /// `int ISHELL_Resume(IShell *, AEECallback *pcb)` — agenda o callback para a próxima
    /// volta do laço de eventos.
    ///
    /// É o mecanismo em que as threads cooperativas se apoiam: o jogo pede a retomada por
    /// aqui e só então chama `Suspend`, para que a thread tenha como voltar.
    pub(super) fn shell_resume(&mut self) -> Result<u32, CpuError> {
        let pcb = self.cpu.read_reg(Reg::R1);
        if let Some(&thread) = self.resume_callbacks.get(&pcb) {
            if !self.pending_threads.contains(&thread) {
                self.pending_threads.push(thread);
            }
            return Ok(SUCCESS);
        }
        let call = self.resolve_notify(Callback {
            function: pcb,
            context: pcb,
        })?;
        self.queue_call(call);
        Ok(SUCCESS)
    }

    /// `IBase *ISHELL_LoadResObject(IShell *po, const char *pszResFile, uint16 nResID,
    /// AEECLSID cls)`.
    ///
    /// Com `nResID` zero o arquivo inteiro é o recurso — é assim que o Quake carrega
    /// `fs:/~/../id1/splash_title.png`. Com `nResID` diferente de zero o que ele nomeia é um
    /// `.bar`, e a imagem é a entrada daquele número lá dentro.
    ///
    /// Ignorar o `nResID` custou caro: o Tekken 2 pede a entrada 5034 do `tekken2.bar` e nós
    /// tentávamos decodificar os 734 KB do `.bar` inteiro como PNG. Falhava, devolvia nulo, e o
    /// jogo seguia com uma imagem sem tamanho — que é divisão por zero na hora de montar a
    /// tela. O relatório dizia o que estava acontecendo o tempo todo, na linha "um recurso
    /// pedido por LoadResObject não é um PNG que saibamos ler".
    ///
    /// Com `cls` zero, o BREW deduz a classe pelo conteúdo; aqui a única que sabemos produzir é
    /// a imagem, e é só o que os jogos pedem.
    pub(super) fn shell_load_res_object(&mut self) -> Result<u32, CpuError> {
        let guest_path = self
            .cpu
            .read_cstring(self.cpu.read_reg(Reg::R1), MAX_STRING);
        let id = self.cpu.read_reg(Reg::R2) as u16;
        let cls = self.cpu.read_reg(Reg::R3);
        let Some(path) = self.vfs.resolve(&guest_path) else {
            self.missing_files.insert(guest_path);
            return Ok(0);
        };
        let bytes = match id {
            0 => match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(_) => {
                    self.missing_files.insert(guest_path);
                    return Ok(0);
                }
            },
            _ => {
                let raw = self
                    .resources
                    .open(&path)
                    .and_then(|res| res.get(crate::loader::resfile::RESTYPE_IMAGE, id))
                    .map(<[u8]>::to_vec);
                let Some(raw) = raw else {
                    self.missing_files
                        .insert(format!("{guest_path} (recurso {id})"));
                    return Ok(0);
                };
                self.recursos_lidos.insert(id);
                // O cabeçalho `AEEResBlob` é nosso para pular: quem pediu foi um **objeto** de
                // imagem, não o bloco bruto que o `LoadResData` entrega.
                crate::loader::resfile::blob_data(&raw).unwrap_or(&raw).to_vec()
            }
        };
        let Some(decoded) = self.decode_resource_image(&bytes) else {
            self.assumptions
                .insert("um recurso pedido por LoadResObject veio num formato que não sabemos ler");
            return Ok(0);
        };

        // A classe pedida decide o que sai daqui. O Tekken 2 pede `AEEIID_IBITMAP` — ele quer
        // desenhar com `IDISPLAY_BitBlt`, não com `IIMAGE_Draw` —, e devolver um `IImage` fazia
        // ele chamar um método de `IBitmap` numa vtable de `IImage`: o slot 12, que em `IImage`
        // não existe.
        if cls == AEEIID_IBITMAP {
            return self.bitmap_from_decoded(&decoded);
        }
        let image = self.new_object(Interface::Image)?;
        if image != 0 {
            self.images.insert(image, std::rc::Rc::new(decoded));
        }
        Ok(image)
    }

    /// Decodifica uma imagem de recurso pelo que ela é.
    ///
    /// O PNG passa pelo caminho próprio, que traz o canal alfa que os jogos usam para recortar
    /// o sprite. O resto — BMP e JPEG — vem pelo decodificador dos ícones, que já sabe lê-los e
    /// entrega tudo opaco, que é o que esses dois formatos são.
    pub(super) fn decode_resource_image(&mut self, bytes: &[u8]) -> Option<DecodedImage> {
        if let Some(decoded) = decode_png(bytes) {
            return Some(decoded);
        }
        if let Some(gif) = crate::gif::decodifica(bytes) {
            return Some(tira_de_quadros(&gif));
        }
        let image = crate::icon::decode(bytes).ok()?;
        let count = image.width * image.height;
        let mut pixels = Vec::with_capacity(count);
        for at in (0..count * 4).step_by(4) {
            pixels.push(
                Rgb {
                    r: image.rgba[at],
                    g: image.rgba[at + 1],
                    b: image.rgba[at + 2],
                }
                .to_rgb565(),
            );
        }
        Some(DecodedImage {
            width: image.width as u32,
            height: image.height as u32,
            pixels,
            opaque: vec![true; count],
            frame_width: 0,
        })
    }

    /// Abre o arquivo de recursos que o jogo nomeou.
    ///
    /// O nome pode vir nulo, e o Peggle manda nulo: o BREW entende isso como "o arquivo de
    /// recursos deste applet". Como não há convenção de nome que sirva — o Peggle chama o dele
    /// de `resources.bar` e o Pac-Mania de `pacmania.bar` —, o que resta é o único `.bar` que
    /// existe ao lado do módulo. Havendo mais de um, não há como escolher, e ninguém abre.
    pub(super) fn open_res_file(&mut self, pointer: u32) -> Option<&crate::loader::resfile::ResFile> {
        let path = match pointer {
            0 => self.default_res_file()?,
            _ => {
                let guest_path = self.cpu.read_cstring(pointer, MAX_STRING);
                match self.vfs.resolve(&guest_path) {
                    Some(path) => path,
                    None => {
                        self.missing_files.insert(guest_path);
                        return None;
                    }
                }
            }
        };
        self.resources.open(&path)
    }

    /// O único `.bar` ao lado do módulo, se houver exatamente um.
    pub(super) fn default_res_file(&self) -> Option<std::path::PathBuf> {
        let mut found = std::fs::read_dir(self.vfs.root())
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("bar"));
        let first = found.next()?;
        match found.next() {
            None => Some(first),
            Some(_) => None,
        }
    }

    /// Escolhe um idioma quando o banco de preferências ainda não tem um.
    ///
    /// A Z-Wheel guarda em `PREFSINFO.Lang` a **etiqueta** do idioma empacotada em quatro
    /// bytes — `"pt  "` vira `0x20207470` —, e não um índice. Quem monta a tela do z-pad
    /// percorre a lista de idiomas do módulo comparando a etiqueta de cada um com esse valor;
    /// sem achar, devolve zero e o jogo repete `Couldn't create z-pad instruction form (6)`
    /// para sempre. O pacote nasce com `Lang = 0`, que é "ninguém escolheu ainda": no console
    /// quem preenche isso é a tela de primeira configuração, que ainda não alcançamos.
    ///
    /// Escolher por conta é uma hipótese, e fica anotada como tal. É o português porque é o
    /// idioma do aparelho que a TecToy vendeu, e porque `tectoy_pt.brf` está no pacote.
    pub(super) fn escolhe_idioma(&mut self, db: &crate::brew::sql::Database) {
        /// `"pt  "` lido como uma palavra de 32 bits, que é a forma como o módulo compara.
        const PORTUGUES: u32 = u32::from_le_bytes(*b"pt  ");
        let sem_escolha = db
            .exec("SELECT dwValue FROM PREFSINFO WHERE PREFSINFO.name = 'Lang'")
            .ok()
            .and_then(|linhas| linhas.into_iter().next())
            .and_then(|linha| linha.values.into_iter().next().flatten())
            .is_some_and(|valor| valor == "0");
        if !sem_escolha {
            return;
        }
        let gravou = db.exec(&format!(
            "UPDATE PREFSINFO SET dwValue = {PORTUGUES} WHERE PREFSINFO.name = 'Lang'"
        ));
        if gravou.is_ok() {
            self.assumptions
                .insert("o idioma não estava escolhido no banco e assumimos português");
        }
    }

    /// `int ISHELL_LoadResString(IShell *po, const char *pszResFile, int16 nResID,
    /// AECHAR *pBuff, int nSize)`.
    ///
    /// Devolve **quantos caracteres** foram escritos, e zero quando o recurso não existe — que
    /// é resposta legítima, não erro.
    pub(super) fn shell_load_res_string(&mut self) -> Result<u32, CpuError> {
        let (file, id) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2) as u16,
        );
        let (buffer, capacity) = (self.cpu.read_reg(Reg::R3), self.stack_arg(0)?);
        let Some(text) = self.open_res_file(file).and_then(|res| res.string(id)) else {
            return Ok(0);
        };
        if buffer == 0 || capacity < 2 {
            return Ok(0);
        }
        // `nSize` é em **bytes**, e cada `AECHAR` ocupa dois; o terminador entra na conta.
        let room = (capacity as usize / 2).saturating_sub(1);
        let written = text.len().min(room);
        let bytes: Vec<u8> = text[..written]
            .iter()
            .chain(std::iter::once(&0))
            .flat_map(|u| u.to_le_bytes())
            .collect();
        self.cpu.write_mem(buffer, &bytes)?;
        Ok(written as u32)
    }

    /// `void *ISHELL_LoadResData(IShell *po, const char *pszResFile, uint16 nResID,
    /// ResType nType)` e a variante `Ex`, que ainda recebe `void *pBuf, uint32 *pnBufSize`.
    ///
    /// A documentação é explícita: o que sai daqui é o conteúdo **bruto** do recurso, cabeçalho
    /// `AEEResBlob` incluído. Quem interpreta é o jogo, com o `RESBLOB_DATA()`.
    pub(super) fn shell_load_res_data(&mut self, with_buffer: bool) -> Result<u32, CpuError> {
        let (file, id) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2) as u16,
        );
        let kind = self.cpu.read_reg(Reg::R3) as u16;
        let Some(data) = self
            .open_res_file(file)
            .and_then(|res| res.get(kind, id))
            .map(<[u8]>::to_vec)
        else {
            return Ok(0);
        };
        let len = data.len() as u32;

        if !with_buffer {
            // Sem buffer do chamador, o BREW aloca — e quem libera é o `FreeResData`.
            let ptr = self.malloc(len)?;
            if ptr != 0 {
                self.cpu.write_mem(ptr, &data)?;
            }
            return Ok(ptr);
        }

        let (buffer, size_out) = (self.stack_arg(0)?, self.stack_arg(1)?);
        // `pBuf` valendo -1 pede só o tamanho, sem copiar nada.
        if buffer == u32::MAX {
            self.write_at(size_out, len)?;
            return Ok(u32::MAX);
        }
        if buffer == 0 {
            let ptr = self.malloc(len)?;
            if ptr != 0 {
                self.cpu.write_mem(ptr, &data)?;
            }
            self.write_at(size_out, len)?;
            return Ok(ptr);
        }
        // Com buffer do chamador, `*pnBufSize` chega dizendo o tamanho dele. Não cabendo, a
        // documentação manda devolver nulo.
        let declared = match size_out {
            0 => 0,
            _ => self.cpu.read_u32(size_out)?,
        };
        // Nem todo jogo preenche o `*pnBufSize` de entrada. O Peggle consulta o tamanho do
        // recurso, aloca os 64.629 bytes que a consulta devolveu e chama de novo com o
        // `*pnBufSize` valendo 6 — o número do tipo, que ficou na variável. Recusar por causa
        // disso deixava o buffer zerado, e o decodificador recebia sessenta mil bytes de zeros:
        // o jogo quebrava logo depois, num ponteiro nulo que ele nem confere.
        //
        // O tamanho do bloco no heap é a medida honesta: não é o número que o jogo disse, é o
        // que ele de fato reservou. Assim a cópia continua limitada ao que existe.
        let capacity = declared.max(self.heap.size_of(buffer).unwrap_or(0));
        if capacity < len {
            self.write_at(size_out, len)?;
            return Ok(0);
        }
        self.cpu.write_mem(buffer, &data)?;
        self.write_at(size_out, len)?;
        Ok(buffer)
    }

    /// `uint32 ISHELL_GetClassItemID(IShell *po, AEECLSID cls)`.
    ///
    /// O item ID é o número que a loja do BREW dá ao pacote que instalou o módulo — e é
    /// literalmente o nome do diretório em que o `.mod` vive (`mod/277083/bjt.mod`). Não é
    /// palpite: é a mesma numeração que aparece no `.mif` e no caminho da ROM.
    ///
    /// A documentação manda devolver 0 quando a classe não é de um módulo baixado, que é o
    /// que fazemos para qualquer classe que não seja a do applet carregado.
    pub(super) fn shell_get_class_item_id(&mut self) -> u32 {
        if self.cpu.read_reg(Reg::R1) != self.applet_class {
            return 0;
        }
        self.vfs
            .root()
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse().ok())
            .unwrap_or(0)
    }

    /// `ISHELL_GetDeviceInfo(IShell *po, AEEDeviceInfo *pi)`.
    ///
    /// Preenchemos só os campos do começo da struct, que têm posição inequívoca: os oito
    /// `uint16` iniciais. O que vem depois depende de como o compilador empacota bitfields, e
    /// chutar layout aqui seria pior do que deixar zerado. Os campos a partir de `wStructSize`
    /// são preenchidos pelo próprio chamador e não são tocados.
    pub(super) fn shell_get_device_info(&mut self) -> Result<u32, CpuError> {
        let info = self.cpu.read_reg(Reg::R1);
        if info == 0 {
            return Ok(SUCCESS);
        }

        // O chamador escreve `wStructSize` **antes** da chamada para pedir os campos
        // estendidos, e o Bejeweled Twist pede: manda 64. Enquanto só preenchíamos os 44
        // primeiros bytes, ele recebia `wMaxPath = 0` — nenhum caminho de arquivo caberia.
        let mut requested = [0u8; 2];
        self.cpu.read_mem(info + 44, &mut requested)?;
        let requested = u16::from_le_bytes(requested) as u32;

        // Zera até `dwLang`, o último campo antes da parte que o chamador preenche.
        self.cpu.write_mem(info, &[0u8; 44])?;

        let fields: [u16; 8] = [
            SCREEN_WIDTH,
            SCREEN_HEIGHT,
            0, // cxAltScreen: o Zeebo não tem segunda tela
            0, // cyAltScreen
            8, // cxScrollBar
            AEE_ENC_ISOLATIN1,
            0, // wMenuTextScroll
            COLOR_DEPTH,
        ];
        for (i, value) in fields.iter().enumerate() {
            self.cpu
                .write_mem(info + i as u32 * 2, &value.to_le_bytes())?;
        }
        // `dwRAM` — a documentação chama de "tamanho inicial do heap do BREW".
        self.cpu.write_u32(info + 24, loader::HEAP_SIZE as u32)?;

        if requested >= DEVICE_INFO_SIZE {
            self.cpu
                .write_mem(info + 44, &(DEVICE_INFO_SIZE as u16).to_le_bytes())?;
            self.cpu.write_u32(info + 48, 0)?; // dwNetLinger: sem rede, nada a manter aberto
            self.cpu.write_u32(info + 52, 0)?; // dwSleepDefer: o console não dorme
            self.cpu
                .write_mem(info + 56, &(AEE_MAX_FILE_NAME as u16).to_le_bytes())?;
            self.cpu.write_u32(info + 60, 0)?; // dwPlatformID: não sabemos o do Zeebo
        }
        Ok(SUCCESS)
    }

    /// `ISHELL_GetDeviceInfoEx(IShell *po, AEEDeviceItem nItem, void *pBuff, int *pnSize)`.
    ///
    /// `pnSize` é de entrada e saída: entra com o tamanho do buffer e sai com o que o item
    /// precisa. Com `pBuff` nulo a chamada serve só para perguntar esse tamanho, e é assim que
    /// o Zeebo Sports Peteca pede o IMEI — devolver sucesso sem escrever ali deixava o jogo
    /// alocar com lixo e ler fora da memória.
    pub(super) fn shell_get_device_info_ex(&mut self) -> Result<u32, CpuError> {
        let (item, buffer, size) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        if size == 0 {
            return Ok(EBADPARM);
        }
        let value = match item {
            DEVICEITEM_IMEI => IMEI,
            _ => return Ok(EUNSUPPORTED),
        };
        let capacity = self.cpu.read_u32(size)? as usize;
        self.cpu.write_u32(size, value.len() as u32)?;
        if buffer != 0 {
            // Cabendo ou não, o que couber é escrito: a documentação prevê o preenchimento
            // parcial, com `pnSize` dizendo quanto faltou.
            self.cpu
                .write_mem(buffer, &value[..capacity.min(value.len())])?;
        }
        Ok(SUCCESS)
    }

    /// Manda `EVT_APP_START` para o applet, que é o que dá partida no jogo.
    ///
    /// `boolean IAPPLET_HandleEvent(IApplet *po, AEEEvent evt, uint16 wParam, uint32 dwParam)`
    /// — slot 2 da vtable de `IApplet`. Em `EVT_APP_START`, `dwParam` aponta para um
    /// `AEEAppStart`, que montamos no heap.
    pub fn start_applet(
        &mut self,
        applet: u32,
        clsid: u32,
        budget: u64,
    ) -> Result<Outcome, CpuError> {
        let vtable = self.cpu.read_u32(applet)?;
        let handle_event = self.cpu.read_u32(vtable + 2 * 4)?;
        // A partir daqui `GetAppInstance` tem o que devolver — e passa a responder sem sair
        // da CPU.
        self.current_applet = applet;
        self.install_app_instance_stub(applet)?;

        let display = self.objects.create(Interface::Display).unwrap_or(0);
        if display != 0 {
            self.cpu
                .write_u32(display, loader::vtable_addr(Interface::Display))?;
        }

        // AEEAppStart: error, clsApp, pDisplay, rc (4 × int16), pszArgs = 24 bytes.
        let start = self.heap.alloc(24).unwrap_or(0);
        if start != 0 {
            self.cpu.write_mem(start, &[0u8; 24])?;
            self.cpu.write_u32(start + 4, clsid)?;
            self.cpu.write_u32(start + 8, display)?;
            for (i, value) in [0i16, 0, SCREEN_WIDTH as i16, SCREEN_HEIGHT as i16]
                .iter()
                .enumerate()
            {
                self.cpu
                    .write_mem(start + 12 + i as u32 * 2, &value.to_le_bytes())?;
            }
        }

        self.call_guest(handle_event, [applet, EVT_APP_START, 0, start], budget)
    }

    /// Enfileira uma tecla do teclado, pelo código virtual do BREW.
    ///
    /// Não vai direto: teclado chega ao jogo como **evento**, e evento só pode ser entregue na
    /// fronteira entre duas chamadas de API — chamar o tratador do jogo no meio de um despacho é
    /// o caminho que já derrubou o Zeeboids. A fila é esvaziada em [`Machine::deliver_signals`].
    pub fn set_installed_applets(&mut self, classes: impl IntoIterator<Item = u32>) {
        self.installed_applets = classes.into_iter().collect();
    }

    /// O host troca de sessão depois que a chamada do guest terminou.
    pub fn take_launch_request(&mut self) -> Option<u32> {
        self.pending_launch.take()
    }

    /// `ISHELL_CreateInstance(IShell *po, AEECLSID ClsId, void **ppobj)`.
    ///
    /// É a fábrica de objetos do BREW inteiro. Cada ClassID conhecido vira um objeto nosso com
    /// a vtable da interface correspondente; o que não conhecemos devolve `ECLASSNOTSUPPORT`,
    /// que é uma resposta legítima — o jogo trata classe ausente como plataforma sem aquele
    /// recurso, em vez de quebrar.
    pub(super) fn shell_create_instance(&mut self) -> Result<u32, CpuError> {
        let clsid = self.cpu.read_reg(Reg::R1);
        let out = self.cpu.read_reg(Reg::R2);
        if self.serial.is_some() {
            self.registra_serial(format!("<classe {clsid:#010x}>"));
        }

        let iface = match clsid {
            AEECLSID_DISPLAY | AEECLSID_DISPLAY1 => Interface::Display,
            AEECLSID_FILEMGR => Interface::FileMgr,
            AEECLSID_HID => Interface::Hid,
            AEECLSID_SIGNAL_CB_FACTORY => Interface::SignalCbFactory,
            AEECLSID_GRAPHICS => Interface::Graphics,
            AEECLSID_SOUND => Interface::Sound,
            AEECLSID_HEAP => Interface::Heap,
            AEECLSID_UNZIPSTREAM => Interface::UnzipStream,
            AEECLSID_LICENSE => Interface::License,
            AEECLSID_MEMASTREAM => Interface::MemAStream,
            AEECLSID_PNG => Interface::Image,
            AEECLSID_PNGDECODER | AEECLSID_PNGDECODER_BREW => Interface::ImageDecoder,
            AEECLSID_THREAD => Interface::Thread,
            AEECLSID_QEGL => Interface::Egl,
            AEECLSID_EGL => Interface::EglLegacy,
            AEECLSID_GL => Interface::GlLegacy,
            AEECLSID_MEDIAUTIL => Interface::MediaUtil,
            AEECLSID_WEB => Interface::Web,
            AEECLSID_COLLECTION => Interface::Collection,
            AEECLSID_SQLMGR => Interface::SqlMgr,
            AEECLSID_SOURCEUTIL => Interface::SourceUtil,
            _ if FAMILIA_DOS_WIDGETS.contains(&clsid) => Interface::Widget,
            AEECLSID_CONTROL => Interface::Control,
            AEECLSID_ZEEBOMCP => Interface::ZeeboMcp,
            AEECLSID_CONFIG => Interface::Config,
            AEECLSID_VETOR => Interface::Vetor,
            AEECLSID_28E3C => Interface::Classe28e3c,
            AEECLSID_CM => Interface::Cm,
            AEECLSID_SYSTEMCTL => Interface::SystemCtl,
            AEECLSID_TYPEFACE | AEECLSID_ROLLER_FONT => Interface::Typeface,
            AEECLSID_MD5 => Interface::Hash,
            AEECLSID_CIPHER_FACTORY => Interface::CipherFactory,
            AEECLSID_MEDIA | AEECLSID_MEDIAMIDI | AEECLSID_MEDIAMP3 | AEECLSID_MEDIAADPCM
            | AEECLSID_MEDIAPCM => Interface::Media,
            // A sonda entra antes da recusa: o jogo recebe um objeto que não faz nada e segue,
            // e o que ele chamar nele vai para o relatório. É como se descobre que interface a
            // classe é, sem header e sem adivinhação.
            _ if self.probe_classes.contains(&clsid) => {
                let object = self.new_object(Interface::Probe)?;
                if object == 0 {
                    return Ok(ENOMEMORY);
                }
                self.probe_objects.insert(object, clsid);
                if out != 0 {
                    self.cpu.write_u32(out, object)?;
                }
                return Ok(SUCCESS);
            }
            _ => {
                self.unknown_classes.insert(clsid);
                if out != 0 {
                    self.cpu.write_u32(out, 0)?;
                }
                return Ok(ECLASSNOTSUPPORT);
            }
        };

        let Some(obj) = self.objects.create(iface) else {
            return Ok(ENOMEMORY);
        };
        self.cpu.write_u32(obj, loader::vtable_addr(iface))?;
        // Uma coleção nasce vazia e com o cursor no começo. Sem esse registro ela não existiria
        // para os métodos, e um `AtEnd` numa coleção desconhecida responderia "acabou" por
        // acaso — a resposta certa pelo motivo errado.
        if iface == Interface::Collection {
            self.collections.insert(obj, (Vec::new(), 0));
        }
        // Pelo mesmo motivo da coleção: um widget sem registro responderia "não tenho esse
        // filho" por não existir, e não por não ter o filho.
        // Mesma razão da coleção: sem o registro, um `Tamanho` numa lista desconhecida
        // responderia zero por ela não existir, e não por estar vazia.
        if iface == Interface::Vetor {
            self.vetores.insert(obj, (Vec::new(), 0));
        }
        if iface == Interface::Widget {
            // Um widget nasce visível: o jogo só chama o slot 6 para **esconder**.
            self.proximo_serial += 1;
            self.widgets.insert(
                obj,
                Widget {
                    visivel: true,
                    classe: clsid,
                    serial: self.proximo_serial,
                    ..Widget::default()
                },
            );
        }
        if out != 0 {
            self.cpu.write_u32(out, obj)?;
        }
        Ok(SUCCESS)
    }

    /// Cria a instância do applet chamando `IModule::CreateInstance` no módulo carregado.
    ///
    /// Assinatura, de `AEEModGen.c`:
    /// `int AEEMod_CreateInstance(IModule *po, IShell *pIShell, AEECLSID ClsId, void **ppObj)`.
    /// `CreateInstance` é o slot 2 da vtable de `IModule` (depois de `AddRef` e `Release`).
    pub fn create_applet(&mut self, clsid: u32, budget: u64) -> Result<AppletResult, CpuError> {
        self.applet_class = clsid;
        let module_ptr = self.cpu.read_u32(self.module.out_module)?;
        if module_ptr == 0 {
            return Ok(AppletResult::NoModule);
        }
        let vtable = self.cpu.read_u32(module_ptr)?;
        let create_instance = self.cpu.read_u32(vtable + 2 * 4)?;

        // O ponteiro de saída fica logo depois do `IModule*`, na área de objetos.
        let out_applet = self.module.out_module + 4;
        self.cpu.write_u32(out_applet, 0)?;

        let outcome = self.call_guest(
            create_instance,
            [module_ptr, self.module.shell, clsid, out_applet],
            budget,
        )?;
        match outcome {
            Outcome::Returned { code } => Ok(AppletResult::Called {
                code,
                applet: self.cpu.read_u32(out_applet)?,
            }),
            other => Ok(AppletResult::Stopped(other)),
        }
    }
}
