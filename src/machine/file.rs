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
    pub(super) fn file_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
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
                let path = if guest_path.eq_ignore_ascii_case("preloaded.cfg") {
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
                match self.vfs.resolve(&guest_path) {
                    Some(path) if std::fs::remove_file(&path).is_ok() => SUCCESS,
                    _ => EFAILED,
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
                match (self.vfs.resolve(&origem), self.vfs.resolve_new(&destino)) {
                    (Some(de), Some(para)) if std::fs::rename(&de, &para).is_ok() => SUCCESS,
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
            "GetFreeSpace" | "GetFreeSpaceEx" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, FS_TOTAL_BYTES)?;
                }
                FS_FREE_BYTES
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
            "Read" => {
                let count = a2 as usize;
                let mut buffer = vec![0u8; count];
                let read = match self.open_files.get_mut(&this) {
                    Some(open) => std::io::Read::read(&mut open.file, &mut buffer).unwrap_or(0),
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
        Ok(Some(result))
    }

    /// Abre um arquivo do jogo, devolvendo o `IFile*` ou zero se não deu.
    pub(super) fn open_file(&mut self, guest_path: &str, mode: u32) -> Result<u32, CpuError> {
        let preloaded = guest_path.eq_ignore_ascii_case("preloaded.cfg");
        let path = if preloaded {
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
            self.vfs.resolve(guest_path)
        };
        let Some(path) = path else {
            self.file_error = EFAILED;
            return Ok(0);
        };
        let mut options = std::fs::OpenOptions::new();
        if mode & OFM_CREATE != 0 {
            // No console o diretório do módulo já vem pronto do instalador; aqui ele só existe
            // se o jogo o criar, e vários jogos abrem `udata\algo` sem chamar `MkDir` antes.
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            options.create(true).read(true).write(true);
        } else if mode & (OFM_READWRITE | OFM_APPEND) != 0 {
            options.read(true).write(true);
        } else {
            options.read(true);
        }
        if mode & OFM_APPEND != 0 {
            options.append(true);
        }

        let Ok(file) = options.open(&path) else {
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
    pub(super) fn list_dir(&self, guest_dir: &str, want_dirs: bool) -> std::collections::VecDeque<String> {
        let Some(dir) = self.vfs.resolve_dir(guest_dir) else {
            return Default::default();
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Default::default();
        };
        // Ordenar deixa a listagem repetível: `read_dir` não promete ordem, e um jogo que
        // monta um menu com ela mudaria de ordem entre duas aberturas.
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir() == want_dirs)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();
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
