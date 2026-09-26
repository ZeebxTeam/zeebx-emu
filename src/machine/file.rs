//! IFileMgr e IFile: o VFS visto pelo jogo.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Raiz do sistema de arquivos que o jogo enxerga.
    pub fn file_root(&self) -> &std::path::Path {
        self.vfs.root()
    }

    /// Arquivos que o jogo pediu e não foram encontrados.
    ///
    /// Um recurso que outro arquivo acabou fornecendo não conta como falta. A Z-Wheel guarda o
    /// que independe de idioma no `tectoyli.brf` e o resto no `tectoy_<idioma>.brf`, e pede
    /// primeiro ao do idioma: a imagem 5035 não está lá, o jogo recua para o `li` e a encontra.
    /// Anotar a primeira tentativa como arquivo faltando faz um recuo normal parecer defeito —
    /// e fez: fui atrás dessa linha achando que era uma imagem que não tínhamos.
    pub fn missing_files(&self) -> Vec<String> {
        self.missing_files
            .iter()
            .filter(|falta| match id_do_recurso(falta) {
                Some(id) => !self.recursos_lidos.contains(&id),
                None => true,
            })
            .cloned()
            .collect()
    }

    /// `IFileMgr` e `IFile`, sobre o diretório do módulo.
    pub(super) fn file_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        // O que o jogo perguntou ao sistema de arquivos, com o retorno: é o instrumento para a
        // classe de problema em que a tela de erro do jogo não diz qual chamada falhou — o
        // Double Dragon mostra "Memory is insufficient" quando qualquer verificação de espaço ou
        // de arquivo não responde o que ele espera.
        let anotar = |maquina: &mut Self, linha: String| {
            if maquina.fs_log.len() >= MAX_FS_LOG {
                maquina.fs_log.pop_front();
            }
            maquina.fs_log.push_back(linha);
        };
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.open_files.remove(&this);
                    self.enumerations.remove(&this);
                }
                remaining
            }
            // IFile *OpenFile(IFileMgr *, const char *pszName, OpenFileMode mode).
            // Devolve o ponteiro do arquivo, não um código de erro.
            "OpenFile" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                self.open_file(&guest_path, a2)?
            }
            // int GetInfo(IFileMgr *, const char *pszName, FileInfo *pInfo)
            "GetInfo" if iface == Interface::FileMgr => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                // A Z-Wheel pergunta pelo metadado do preload antes de decidir se o abre.
                // Ele é estado do aparelho, não um arquivo distribuído dentro do pacote.
                let path = if let Some(cfg) = self.cfg_da_z_wheel(&guest_path) {
                    Some(cfg)
                } else if guest_path.eq_ignore_ascii_case("preloaded.cfg") {
                    let path = self.vfs.profile_file("z-wheel", "preloaded.cfg");
                    if !path.exists() {
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::File::create(&path);
                    }
                    Some(path)
                } else {
                    self.vfs.resolve(&guest_path)
                };
                match path.and_then(|p| std::fs::metadata(&p).ok()) {
                    Some(meta) => {
                        self.write_file_info(a2, &guest_path, &meta)?;
                        SUCCESS
                    }
                    None => EFAILED,
                }
            }
            // int GetInfoEx(IFile *, AEEFileInfoEx *pInfo)
            //
            // **O tamanho fica em `+0xc`**, e isto foi lido, não suposto: em `0x89068` a Z-Wheel
            // chama este método e, na instrução seguinte, lê `sp[0xc]` como o tamanho e o passa
            // ao `malloc` — **sem conferir o retorno**. Recusar não é resposta neutra aqui: o
            // que ela lia era pilha por inicializar, e o que sobrava lá era um ponteiro. Daí o
            // `check_malloc: Malloc failed` que aparecia no log.
            //
            // Escrevemos dezesseis bytes e nada mais, e o limite tem motivo. O quadro daquela
            // função é de `0x2c` e ela guarda um local em `sp[0x28]`; a tentativa anterior
            // preenchia com o formato do `FileInfo`, **nome de arquivo incluído**, e o nome
            // passava por cima do vizinho. O app lia `0x6f6369d5` — texto — como tamanho e
            // pedia 1,8 GB.
            //
            // Os três primeiros campos vão zerados porque não sabemos o que são. Zero é uma
            // resposta que o jogo sabe tratar; lixo não.
            "GetInfoEx" if iface == Interface::File => {
                /// Onde o tamanho mora na `AEEFileInfoEx`, lido em `0x89078`.
                const TAMANHO: u32 = 0xc;
                /// Quanto da struct preenchemos. Ver acima o porquê de não ser mais.
                const QUANTO: usize = 0x10;

                let tamanho = self
                    .open_files
                    .get(&this)
                    .and_then(|aberto| aberto.file.metadata().ok())
                    .map(|meta| meta.len() as u32);
                let Some(tamanho) = tamanho else {
                    return Ok(Some(EBADPARM));
                };
                if a1 != 0 {
                    self.cpu.write_mem(a1, &[0u8; QUANTO])?;
                    self.cpu.write_u32(a1 + TAMANHO, tamanho)?;
                }
                SUCCESS
            }
            // int GetInfo(IFile *, FileInfo *pInfo)
            "GetInfo" if iface == Interface::File => match self.open_files.get(&this) {
                Some(open) => {
                    let (path, meta) = (open.guest_path.clone(), open.file.metadata().ok());
                    match meta {
                        Some(meta) => {
                            self.write_file_info(a1, &path, &meta)?;
                            SUCCESS
                        }
                        None => EFAILED,
                    }
                }
                None => EBADPARM,
            },
            "Test" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                // O preload é estado virtual do aparelho: o pacote oficial não o traz, mas a
                // Z-Wheel testa sua existência antes de abri-lo. O `OpenFile` o materializa no
                // perfil logo em seguida.
                if guest_path.eq_ignore_ascii_case("preloaded.cfg") {
                    return Ok(Some(SUCCESS));
                }
                match self.vfs.resolve(&guest_path) {
                    Some(path) if path.exists() => SUCCESS,
                    _ => EFAILED,
                }
            }
            "Remove" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                // Com overlay, só o que o jogo gravou pode ser apagado: o recurso do pacote é
                // imutável, e apagá-lo alteraria o conteúdo original.
                let alvo = match self.vfs.overlay_path(&guest_path) {
                    Some(caminho) => caminho,
                    None => match self.vfs.resolve(&guest_path) {
                        Some(caminho) => caminho,
                        None => return Ok(Some(EFAILED)),
                    },
                };
                match std::fs::remove_file(&alvo) {
                    Ok(()) => SUCCESS,
                    Err(_) => EFAILED,
                }
            }
            // int IFILEMGR_Rename(IFileMgr *, const char *pszSrc, const char *pszDest)
            //
            // O destino é resolvido pelo caminho exato, sem a busca sem caixa: renomear é criar
            // um nome novo, e casar com um arquivo existente de caixa diferente sobrescreveria
            // o arquivo errado.
            "Rename" => {
                let origem = self.cpu.read_cstring(a1, MAX_STRING);
                let destino = self.cpu.read_cstring(a2, MAX_STRING);
                // O destino novo vai para o overlay quando ele existe; a origem pode estar no
                // pacote, e nesse caso o rename a tira de lá — o que já é o comportamento sem
                // overlay e continua valendo com ele.
                let para = self
                    .vfs
                    .overlay_path(&destino)
                    .or_else(|| self.vfs.resolve_new(&destino));
                match (self.vfs.resolve(&origem), para) {
                    (Some(de), Some(para)) => {
                        if let Some(pai) = para.parent() {
                            let _ = std::fs::create_dir_all(pai);
                        }
                        // Origem no pacote: ela é imutável, então o destino nasce no overlay e a
                        // origem permanece. Origem no overlay: o arquivo é movido de verdade.
                        let resultado = match self.vfs.is_overlay_path(&de) {
                            true => std::fs::rename(&de, &para),
                            false => std::fs::copy(&de, &para).map(|_| ()),
                        };
                        match resultado {
                            Ok(()) => SUCCESS,
                            Err(_) => EFAILED,
                        }
                    }
                    _ => EFAILED,
                }
            }
            "MkDir" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                match self.vfs.resolve(&guest_path) {
                    Some(path) if std::fs::create_dir_all(&path).is_ok() => SUCCESS,
                    _ => EFAILED,
                }
            }
            "RmDir" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                match self.vfs.resolve(&guest_path) {
                    Some(path) if std::fs::remove_dir(&path).is_ok() => SUCCESS,
                    _ => EFAILED,
                }
            }
            // uint32 GetFreeSpace(IFileMgr *, uint32 *pdwTotal)
            //
            // Devolve o **livre** e escreve o **total** no ponteiro, se houver.
            "GetFreeSpace" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, FS_TOTAL_BYTES)?;
                }
                FS_FREE_BYTES
            }
            // int GetFreeSpaceEx(IFileMgr *, const char *cpszPath, uint32 *pdwTotal,
            //                    uint32 *pdwFree)
            //
            // **Não é o mesmo método.** Tem quatro argumentos, e o primeiro deles **não é um
            // ponteiro de saída**: é o caminho do sistema de arquivos perguntado. Tratando os dois
            // igual, o total era escrito por cima da string do jogo e os dois ponteiros de saída
            // ficavam sem resposta — e o Double Dragon, que pergunta o espaço antes de abrir os
            // dados, concluía que não havia memória e mostrava "Memory is insufficient. Please
            // delete some files." em vez do jogo.
            "GetFreeSpaceEx" => {
                let caminho = self.cpu.read_cstring(a1, MAX_STRING);
                let out_total = a2;
                let out_livre = self.cpu.read_reg(Reg::R3);
                // Cartão periférico não existe neste aparelho: o Zeebo guarda tudo na NAND, e
                // `fs:/card0/` é a forma de o jogo perguntar por um. `EUNSUPPORTED` é a resposta
                // documentada, e é o que faz o jogo usar o sistema principal.
                if !caminho.is_empty() && !caminho.starts_with("fs:/") {
                    return Ok(Some(EUNSUPPORTED));
                }
                if out_total != 0 {
                    self.cpu.write_u32(out_total, FS_TOTAL_BYTES)?;
                }
                if out_livre != 0 {
                    self.cpu.write_u32(out_livre, FS_FREE_BYTES)?;
                }
                SUCCESS
            }
            // int EnumInit(IFileMgr *, const char *pszDir, boolean bDirs)
            //
            // A listagem inteira sai daqui, de uma vez. O BREW deixa o estado dentro do
            // `IFileMgr`, e é o que fazemos: a fila é do gerenciador, não global.
            "EnumInit" => {
                let guest_dir = self.cpu.read_cstring(a1, MAX_STRING);
                let entries = self.list_dir(&guest_dir, a2 != 0);
                self.enumerations.insert(this, entries);
                SUCCESS
            }
            // boolean EnumNext(IFileMgr *, FileInfo *pInfo)
            //
            // Falso encerra a enumeração, e o `GetLastError` de depois devolve `EFAILED` mesmo
            // quando tudo correu bem — a documentação chama isso de compatibilidade com o
            // cliente 1.0, e há jogo que confere.
            "EnumNext" => {
                let next = self
                    .enumerations
                    .get_mut(&this)
                    .and_then(std::collections::VecDeque::pop_front);
                match next.and_then(|guest| {
                    let meta = self
                        .vfs
                        .resolve_dir(&guest)
                        .and_then(|p| p.metadata().ok())?;
                    Some((guest, meta))
                }) {
                    Some((guest, meta)) => {
                        self.write_file_info(a1, &guest, &meta)?;
                        TRUE
                    }
                    None => {
                        self.file_error = EFAILED;
                        FALSE
                    }
                }
            }
            "GetLastError" => self.file_error,
            // int ResolvePath(IFileMgr *, const char *cpszIn, char *pszOut, int *pnOutLen)
            "ResolvePath" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                let limit = if a3 != 0 {
                    self.cpu.read_u32(a3)? as usize
                } else {
                    0
                };
                self.write_cstring_limited(a2, &guest_path, limit)?;
                if a3 != 0 {
                    self.cpu.write_u32(a3, guest_path.len() as u32 + 1)?;
                }
                SUCCESS
            }
            // int32 Read(IFile *, void *pBuffer, uint32 dwCount)
            // uint32 Read(IFile *, void *pBuffer, uint32 dwCount)
            //
            // **Uma leitura curta não é o fim do arquivo.** O `read` do Rust pode devolver menos
            // do que se pediu sem que nada tenha acabado, e o `IFILE_Read` do BREW entrega o que
            // foi pedido enquanto houver arquivo. Insistindo só até o fim de verdade, o pacote do
            // Iron Sight — 16 MB lidos em pedaços grandes — para de chegar cortado.
            "Read" => {
                // O tamanho vem do jogo: conferido antes de alocar, ou um pedido absurdo derruba o
                // processo em vez de virar erro de API.
                let count = tamanho_do_guest(a2 as usize)?;
                let mut buffer = vec![0u8; count];
                let read = match self.open_files.get_mut(&this) {
                    Some(open) => {
                        let mut lidos = 0;
                        while lidos < count {
                            match std::io::Read::read(&mut open.file, &mut buffer[lidos..]) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => lidos += n,
                            }
                        }
                        lidos
                    }
                    None => 0,
                };
                if read > 0 {
                    self.cpu.write_mem(a1, &buffer[..read])?;
                }
                read as u32
            }
            // uint32 Write(IFile *, const void *pBuffer, uint32 dwCount)
            "Write" => {
                let bytes = self.read_bytes(a1, a2)?;
                match self.open_files.get_mut(&this) {
                    Some(open) => std::io::Write::write(&mut open.file, &bytes).unwrap_or(0) as u32,
                    None => 0,
                }
            }
            // int32 Seek(IFile *, FileSeekType seek, int32 position)
            "Seek" => {
                let position = a2 as i32 as i64;
                let from = match a1 {
                    SEEK_END => std::io::SeekFrom::End(position),
                    SEEK_CURRENT => std::io::SeekFrom::Current(position),
                    _ => std::io::SeekFrom::Start(position.max(0) as u64),
                };
                match self.open_files.get_mut(&this) {
                    Some(open) => match std::io::Seek::seek(&mut open.file, from) {
                        // Documentado em `IFILE_Seek.htm`: o retorno é `SUCCESS`, exceto no caso
                        // especial de `_SEEK_CURRENT` com deslocamento zero, que devolve a
                        // posição atual.
                        Ok(offset) if a1 == SEEK_CURRENT && a2 == 0 => offset as u32,
                        Ok(_) => SUCCESS,
                        Err(_) => EFAILED,
                    },
                    None => EFAILED,
                }
            }
            "Truncate" => match self.open_files.get_mut(&this) {
                Some(open) if open.file.set_len(a1 as u64).is_ok() => SUCCESS,
                _ => EFAILED,
            },
            // Sem cache próprio e sem mapeamento de arquivo em memória.
            "SetCacheSize" => 0,
            "Map" => 0,
            "Cancel" => SUCCESS,
            _ => return Ok(None),
        };
        // O caminho entra na linha quando a chamada tem um: `Test`, `OpenFile`, `MkDir`, `Remove`,
        // `Rename` e `EnumInit` são as que decidem onde o jogo escreve.
        let tem_caminho = matches!(
            name,
            "Test" | "OpenFile" | "MkDir" | "Remove" | "Rename" | "EnumInit" | "GetFreeSpaceEx"
        );
        let caminho = match tem_caminho && a1 != 0 {
            true => format!(" \"{}\"", self.cpu.read_cstring(a1, MAX_STRING)),
            false => String::new(),
        };
        anotar(
            self,
            format!("{name}{caminho}  ({a1:#x} {a2:#x} {a3:#x}) -> {result}"),
        );
        Ok(Some(result))
    }

    /// A cópia ajustada da `tectoy.cfg`, quando é ela que a Z-Wheel deve ler.
    ///
    /// `None` para qualquer outro arquivo, outro applet, ou com as opções de fábrica — aí vale
    /// o arquivo do pacote. Ver [`Machine::configura_z_wheel`].
    fn cfg_da_z_wheel(&self, guest_path: &str) -> Option<std::path::PathBuf> {
        let de_fabrica = self.z_wheel.fim_de_vida && !self.z_wheel.transicoes_sempre;
        if de_fabrica || self.applet_class != crate::session::Z_WHEEL {
            return None;
        }
        let nome = guest_path.rsplit(['/', '\\']).next().unwrap_or(guest_path);
        if !nome.eq_ignore_ascii_case("tectoy.cfg") {
            return None;
        }
        let original = std::fs::read_to_string(self.vfs.resolve(guest_path)?).ok()?;
        let copia = self.vfs.profile_file("z-wheel", "tectoy.cfg");
        std::fs::create_dir_all(copia.parent()?).ok()?;
        std::fs::write(&copia, ajusta_cfg(&original, self.z_wheel)).ok()?;
        Some(copia)
    }

    /// Abre um arquivo do jogo, devolvendo o `IFile*` ou zero se não deu.
    pub(super) fn open_file(&mut self, guest_path: &str, mode: u32) -> Result<u32, CpuError> {
        let preloaded = guest_path.eq_ignore_ascii_case("preloaded.cfg");
        let path = if let Some(cfg) = self.cfg_da_z_wheel(guest_path) {
            Some(cfg)
        } else if preloaded {
            let path = self.vfs.profile_file("z-wheel", "preloaded.cfg");
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                // Sem NAND, a resposta fiel é uma lista vazia de jogos pré-instalados. O
                // catálogo de ROMs locais entra pelo banco de perfil, não por este arquivo.
                let _ = std::fs::File::create(&path);
            }
            Some(path)
        } else {
            None
        };
        if let Some(cfg) = path {
            // Arquivo de perfil do emulador: mesma semântica de modo do caminho comum.
            return self.open_resolved(guest_path, cfg, Self::open_options(mode));
        }
        // A partir daqui vale a resolução do VFS, que decide entre conteúdo e overlay e diz se
        // o arquivo do pacote precisa ser copiado antes de receber escrita.
        let intent = match (mode & OFM_CREATE != 0, mode & (OFM_READWRITE | OFM_APPEND) != 0) {
            (true, _) => crate::brew::vfs::OpenIntent::Create,
            (false, true) if mode & OFM_APPEND != 0 => crate::brew::vfs::OpenIntent::Append,
            (false, true) => crate::brew::vfs::OpenIntent::ReadWrite,
            (false, false) => crate::brew::vfs::OpenIntent::Read,
        };
        let Some(alvo) = self.vfs.open_target(guest_path, intent) else {
            self.file_error = EFAILED;
            return Ok(0);
        };
        let path = alvo.path;
        // O pacote nunca é alterado: a primeira escrita copia o recurso para o overlay.
        if let Some(origem) = alvo.copy_from {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(&origem, &path).is_err() {
                self.file_error = EFAILED;
                return Ok(0);
            }
        }
        // **O diretório do módulo já vem pronto no console; aqui não.** Vários jogos abrem
        // `udata/algo` com `OFM_CREATE` sem chamar `MkDir` antes — o Double Dragon é um deles, e
        // quando a criação do diretório se perdeu num refactor, ele falhava ao abrir o save e
        // mostrava a tela "Memory is insufficient. Please delete some files." em vez do jogo.
        if mode & OFM_CREATE != 0
            && let Some(parent) = path.parent()
        {
            let _ = std::fs::create_dir_all(parent);
        }
        let options = Self::open_options(mode);

        self.open_resolved(guest_path, path, options)
    }

    /// As opções de abertura equivalentes ao modo do BREW.
    fn open_options(mode: u32) -> std::fs::OpenOptions {
        let mut options = std::fs::OpenOptions::new();
        if mode & OFM_CREATE != 0 {
            options.create(true).read(true).write(true);
        } else if mode & (OFM_READWRITE | OFM_APPEND) != 0 {
            options.read(true).write(true);
        } else {
            options.read(true);
        }
        if mode & OFM_APPEND != 0 {
            options.append(true);
        }
        options
    }

    /// Abre `path` já resolvido e registra o `IFile`.
    fn open_resolved(
        &mut self,
        guest_path: &str,
        path: std::path::PathBuf,
        options: std::fs::OpenOptions,
    ) -> Result<u32, CpuError> {
        let Ok(file) = options.open(&path) else {
            // O caminho do host e o erro do sistema entram no registro: "o jogo não conseguiu
            // abrir o save" não diz se o problema é o caminho resolvido, a permissão ou o modo.
            let erro = options.open(&path).err().map(|e| e.to_string());
            if self.fs_log.len() >= MAX_FS_LOG {
                self.fs_log.pop_front();
            }
            self.fs_log.push_back(format!(
                "open falhou: {} ({})",
                path.display(),
                erro.unwrap_or_else(|| "sem motivo".into())
            ));
            self.file_error = EFAILED;
            self.missing_files.insert(guest_path.to_string());
            return Ok(0);
        };
        let handle = self.new_object(Interface::File)?;
        if handle == 0 {
            return Ok(0);
        }
        self.open_files.insert(
            handle,
            OpenFile {
                file,
                guest_path: guest_path.to_string(),
                caminho: path,
            },
        );
        self.file_error = SUCCESS;
        Ok(handle)
    }

    /// Preenche um `FileInfo`: `char attrib` (com três bytes de alinhamento), `uint32
    /// dwCreationDate`, `uint32 dwSize` e `char szName[64]`.
    /// Os arquivos (ou os diretórios) de um diretório do guest, já com o caminho que o jogo
    /// entende.
    ///
    /// O nome devolvido é o caminho completo, com o mesmo prefixo que o jogo passou: é ele que
    /// volta para o `OpenFile` logo em seguida, e um nome solto não abriria nada.
    pub(super) fn list_dir(
        &self,
        guest_dir: &str,
        want_dirs: bool,
    ) -> std::collections::VecDeque<String> {
        let Some(dir) = self.vfs.resolve_dir(guest_dir) else {
            return Default::default();
        };
        // Conteúdo e overlay somam: o jogo precisa ver o que veio no pacote **e** o que ele
        // mesmo gravou. `read_dir` não promete ordem, e um jogo que monta menu com ela mudaria
        // de ordem entre duas aberturas; por isso a lista sai ordenada.
        let mut dirs = vec![dir];
        if let Some(overlay) = self.vfs.overlay_dir(guest_dir) {
            dirs.push(overlay);
        }
        let mut names: Vec<String> = Vec::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            names.extend(
                entries
                    .flatten()
                    .filter(|entry| entry.path().is_dir() == want_dirs)
                    // **O manifesto do pacote não é do jogo.** Ele é um arquivo *nosso*, escrito na
                    // pasta que o guest enxerga como raiz, e um jogo que enumere a própria pasta
                    // não pode ver nele um arquivo que o console não tem. Ver
                    // [`crate::loader::archive::MANIFESTO`].
                    .filter(|entry| entry.file_name() != crate::loader::archive::MANIFESTO)
                    .filter_map(|entry| entry.file_name().into_string().ok()),
            );
        }
        names.sort();
        names.dedup();
        let prefix = guest_dir.trim_end_matches('/');
        names
            .into_iter()
            .map(|name| match prefix.is_empty() {
                true => name,
                false => format!("{prefix}/{name}"),
            })
            .collect()
    }

    pub(super) fn write_file_info(
        &mut self,
        addr: u32,
        guest_path: &str,
        meta: &std::fs::Metadata,
    ) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let attrib = if meta.is_dir() { FA_DIR } else { FA_NORMAL };
        self.cpu.write_u32(addr, attrib)?;
        self.cpu.write_u32(addr + 4, 0)?;
        self.cpu.write_u32(addr + 8, meta.len() as u32)?;
        let mut name = guest_path.as_bytes().to_vec();
        name.truncate(MAX_FILE_NAME - 1);
        name.resize(MAX_FILE_NAME, 0);
        self.cpu.write_mem(addr + 12, &name)
    }
}

/// A `tectoy.cfg` com as trocas de [`Machine::configura_z_wheel`], o resto como veio.
fn ajusta_cfg(original: &str, opcoes: crate::config::ZWheel) -> String {
    original
        .split_inclusive('\n')
        .map(|linha| {
            let chave = linha.split('=').next().unwrap_or("").trim();
            let fim = &linha[linha.trim_end_matches(['\r', '\n']).len()..];
            let zera = match chave {
                "EOL" | "zeebomenu_hide" => !opcoes.fim_de_vida,
                "SlideOnceToForm" => opcoes.transicoes_sempre,
                _ => false,
            };
            match zera && linha.contains('=') {
                true => format!("{chave}=0{fim}"),
                false => linha.to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod testes_da_cfg {
    use super::ajusta_cfg;
    use crate::config::ZWheel;

    const ORIGINAL: &str = "; EOL\r\nEOL=1\r\n#zeebomenu_hide - x\r\nzeebomenu_hide=1\r\nEOLX=1\r\nSlideOnceToForm=31\n";

    #[test]
    fn sem_fim_de_vida_so_as_duas_chaves_mudam() {
        let opcoes = ZWheel { fim_de_vida: false, transicoes_sempre: false };
        assert_eq!(
            ajusta_cfg(ORIGINAL, opcoes),
            "; EOL\r\nEOL=0\r\n#zeebomenu_hide - x\r\nzeebomenu_hide=0\r\nEOLX=1\r\nSlideOnceToForm=31\n"
        );
    }

    #[test]
    fn transicoes_sempre_zeram_o_slide_uma_vez() {
        let opcoes = ZWheel { fim_de_vida: true, transicoes_sempre: true };
        assert_eq!(
            ajusta_cfg(ORIGINAL, opcoes),
            "; EOL\r\nEOL=1\r\n#zeebomenu_hide - x\r\nzeebomenu_hide=1\r\nEOLX=1\r\nSlideOnceToForm=0\n"
        );
    }
}
