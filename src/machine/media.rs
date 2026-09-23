//! IMedia: a reprodução de som e vídeo pedida pelo jogo.

use super::*;

/// Quantos sons decodificados o cache guarda antes de esquecer os que ninguém usa. Ver
/// [`Machine::descarta_sons_sem_dono`].
const MAX_SONS_GUARDADOS: usize = 64;

impl<C: CpuBackend> Machine<C> {
    /// `ISound` (`AEECLSID_SOUND` = `0x01001056`), de `inc/AEEISound.h`.
    ///
    /// É a API de som *básica* do BREW: tons do gerador do aparelho, vibração e volume — não
    /// toca arquivos, isso é o `ISoundPlayer`. Aqui ela é silenciosa: aceita tudo, guarda o
    /// estado que o jogo pode ler de volta e avisa o callback de que a reprodução terminou.
    /// Sem isso os jogos que checam o retorno de `Set` desistem da inicialização.
    /// `IMediaUtil` e `IMedia`, de `sdk/inc/AEEMediaUtil.h` e `inc/AEEIMedia.h`.
    ///
    /// Mudos, como o `ISound`: guardam o estado de reprodução e respondem o que o jogo espera,
    /// mas não sai som. O que importa aqui é existir — o Quake cria o tocador de trilha na
    /// inicialização do áudio e, se ela falha, ele segue em frente e depois chama `Play` num
    /// ponteiro nulo, sem conferir.
    pub(super) fn media_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.arg(0);
        let chamada = self.descreve_chamada_de_midia(iface, name)?;
        let result = self.media_call_inner(iface, name, this)?;
        if let Some(result) = result {
            self.anota_midia(this, format!("{chamada} -> {result:#x}"));
        } else {
            self.anota_midia(this, format!("{chamada} -> NÃO IMPLEMENTADO"));
        }
        Ok(result)
    }

    /// A chamada em texto, com os argumentos que dizem alguma coisa sobre o som.
    fn descreve_chamada_de_midia(
        &mut self,
        iface: Interface,
        name: &str,
    ) -> Result<String, CpuError> {
        let (a1, a2, a3) = (self.arg(1), self.arg(2), self.arg(3));
        Ok(match (iface, name) {
            (Interface::Media, "SetMediaParm") => {
                let extra = match a1 {
                    MM_PARM_MEDIA_DATA if a2 != 0 => {
                        let classe = self.cpu.read_u32(a2)?;
                        let dados = self.cpu.read_u32(a2 + 4)?;
                        let tamanho = self.cpu.read_u32(a2 + 8)?;
                        let mut cabeca = [0u8; 4];
                        if classe == MMD_BUFFER && dados != 0 {
                            self.cpu.read_mem(dados, &mut cabeca).ok();
                        }
                        match classe {
                            MMD_FILE_NAME if dados != 0 => format!(
                                " arquivo{{{:?}}}",
                                self.cpu.read_cstring(dados, MAX_STRING)
                            ),
                            _ => format!(
                                " dados{{classe {classe:#x}, {tamanho} bytes em {dados:#x}, {:?}}}",
                                String::from_utf8_lossy(&cabeca)
                            ),
                        }
                    }
                    _ => String::new(),
                };
                format!("SetMediaParm(parm {a1}, {a2:#x}, {a3:#x}){extra}")
            }
            (Interface::MediaUtil, _) => format!("IMediaUtil::{name}({a1:#x}, {a2:#x})"),
            _ => format!("{name}({a1:#x}, {a2:#x})"),
        })
    }

    /// Guarda uma linha no [`Machine::media_log`], juntando repetições seguidas.
    fn anota_midia(&mut self, objeto: u32, linha: String) {
        let agora = self.elapsed_ms();
        if let Some(ultima) = self.media_log.back_mut() {
            if ultima.1 == objeto && ultima.2 == linha {
                ultima.3 += 1;
                return;
            }
        }
        if self.media_log.len() == MEDIA_LOG_MAX {
            self.media_log.pop_front();
        }
        self.media_log.push_back((agora, objeto, linha, 1));
    }

    /// O registro de mídia, para o relatório: `(instante, objeto, chamada, vezes seguidas)`.
    pub fn media_log(&self) -> Vec<(u32, u32, String, u32)> {
        self.media_log.iter().cloned().collect()
    }

    fn media_call_inner(
        &mut self,
        iface: Interface,
        name: &str,
        this: u32,
    ) -> Result<Option<u32>, CpuError> {
        let result = match (iface, name) {
            (_, "AddRef") => self.objects.add_ref(this),
            (_, "Release") => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.media.remove(&this);
                    self.fluxos_pcm.remove(&this);
                    // Uma música em repetição seguia tocando depois de o objeto sumir.
                    if let Some(mixer) = &self.audio {
                        mixer.stop(this);
                    }
                }
                remaining
            }
            (_, "QueryInterface") => {
                let out = self.arg(2);
                if out != 0 {
                    self.cpu.write_u32(out, this)?;
                }
                SUCCESS
            }
            // int CreateMedia(IMediaUtil *, AEEMediaData *, IMedia **ppm)
            (Interface::MediaUtil, "CreateMedia" | "CreateMediaEx") => {
                let out = self.arg(2);
                let media = self.new_object(Interface::Media)?;
                if media == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                let mut state = MediaState::default();
                match self.read_media_data(self.arg(1))? {
                    Entrega::Pronta(carga) => state.carga = carga,
                    Entrega::Buffer(onde, tamanho) => {
                        (state.pendente, state.buffer) = ((onde, tamanho), (onde, tamanho))
                    }
                    Entrega::Nada => {}
                }
                self.media.insert(media, state);
                if out != 0 {
                    self.cpu.write_u32(out, media)?;
                }
                SUCCESS
            }
            // int SetMediaParm(IMedia *, int16 nParmID, int32 p1, int32 p2)
            (Interface::Media, "SetMediaParm") => {
                let (parm, p1) = (self.arg(1), self.arg(2));
                let mut state = self
                    .media
                    .get(&this)
                    .copied()
                    .unwrap_or(MediaState::default());
                let mut resultado = SUCCESS;
                match parm {
                    MM_PARM_MEDIA_DATA if self.le_fluxo_pcm(this, p1)? => {}
                    MM_PARM_MEDIA_DATA => match self.read_media_data(p1)? {
                        Entrega::Pronta(carga) => {
                            (state.carga, state.pendente, state.buffer) = (carga, (0, 0), (0, 0))
                        }
                        Entrega::Buffer(onde, tamanho) => {
                            (state.pendente, state.buffer) = ((onde, tamanho), (onde, tamanho))
                        }
                        Entrega::Nada => resultado = EFAILED,
                    },
                    MM_PARM_VOLUME => state.volume = p1.min(MAX_VOLUME),
                    MM_PARM_MUTE => state.muted = p1 != 0,
                    MM_PARM_PLAY_REPEAT => state.repeat = p1,
                    // O resto do `MM_PARM_XXX` é aceito e ignorado: recusar faria jogos
                    // desistirem de tocar por causa de um ajuste que não muda o som.
                    _ => {}
                }
                self.media.insert(this, state);
                if let (Some(mixer), MM_PARM_VOLUME | MM_PARM_MUTE) = (&self.audio, parm) {
                    mixer.set_volume(this, state.gain());
                }
                resultado
            }
            // void RegisterNotify(IMedia *, PFNMEDIANOTIFY pfn, void *pUser)
            (Interface::Media, "RegisterNotify") => {
                let notify = Callback {
                    function: self.arg(1),
                    context: self.arg(2),
                };
                self.media.entry(this).or_default().notify = notify;
                SUCCESS
            }
            (Interface::Media, "Play") => self.media_play(this)?,
            (Interface::Media, "Resume") => {
                if let Some(state) = self.media.get_mut(&this) {
                    state.state = MM_STATE_PLAY;
                }
                if let Some(mixer) = &self.audio {
                    mixer.set_paused(this, false);
                }
                SUCCESS
            }
            (Interface::Media, "Pause") => {
                if let Some(state) = self.media.get_mut(&this) {
                    state.state = MM_STATE_PLAY_PAUSE;
                }
                if let Some(mixer) = &self.audio {
                    mixer.set_paused(this, true);
                }
                SUCCESS
            }
            // O `Stop` avisa o callback de que a reprodução terminou, **com `MM_STATUS_DONE`**. O
            // Crash Nitro Kart conta os sons ativos e só toca a música da pista quando a conta
            // zera; o tratador dele (`0x11a80`) desconta no `DONE` e no status 9 e ignora o
            // `ABORT`. Sem aviso — e depois com `ABORT` —, as músicas do menu paradas na entrada
            // da corrida nunca saíam da conta, e a corrida inteira ficava muda. O Double Dragon
            // trata os dois status do mesmo jeito.
            (Interface::Media, "Stop") => {
                let tocando = self.esta_tocando(this);
                if let Some(fluxo) = self.fluxos_pcm.get_mut(&this) {
                    fluxo.tocando = false;
                }
                if let Some(state) = self.media.get_mut(&this) {
                    state.state = MM_STATE_READY;
                    state.ends_us = 0;
                    state.tocar_ao_ler = false;
                }
                if let Some(mixer) = &self.audio {
                    mixer.stop(this);
                }
                if tocando {
                    self.notify_media(this, MM_CMD_PLAY, MM_STATUS_DONE)?;
                }
                SUCCESS
            }
            // int GetState(IMedia *, boolean *pbStateChanging) — o estado é o retorno, e o
            // ponteiro diz se ele está em transição. Aqui nunca está: a mudança é imediata.
            (Interface::Media, "GetState") => {
                let out = self.arg(1);
                if out != 0 {
                    self.cpu.write_mem(out, &[0])?;
                }
                // Um som que acabou volta o objeto para "pronto": é o que o jogo consulta
                // para saber que o som terminou. Quem diz que acabou é o **relógio virtual**,
                // não o mixer — pelo mesmo motivo que em `media_play`: o emulador roda mudo
                // sem deixar de contar o tempo, e há som que toca em silêncio porque não
                // sabemos decodificá-lo, só cronometrá-lo.
                let now = self.now_us();
                let state = self.media.entry(this).or_default();
                if state.state == MM_STATE_PLAY && now >= state.ends_us {
                    state.state = MM_STATE_READY;
                }
                state.state
            }
            // int32 GetTotalTime(IMedia *) — a duração do som, em milissegundos.
            //
            // O som que toca em silêncio responde a duração dele como qualquer outro. Zero aqui
            // é um divisor esperando acontecer: quem monta uma barra de progresso divide pelo
            // total, e um total zero derruba o jogo por uma resposta nossa.
            (Interface::Media, "GetTotalTime") => match {
                // A duração precisa do som: quem pergunta não pode esperar a próxima volta.
                self.resolve_midia(this)?;
                self.media_sound(this)?
            } {
                Some(sound) => (sound.frames() as u64 * 1000 / u64::from(sound.rate.max(1))) as u32,
                None => (self.media_silent_length(this)?.unwrap_or(0) / 1000) as u32,
            },
            // O volume e o mudo voltam o que foi guardado: um jogo que lê o volume e grava de
            // volta se emudecia com o zero de antes. O resto continua zerado.
            (Interface::Media, "GetMediaParm") => {
                let state = self.media.get(&this).copied().unwrap_or_default();
                let valor = match self.arg(1) {
                    MM_PARM_VOLUME => state.volume,
                    MM_PARM_MUTE => u32::from(state.muted),
                    _ => 0,
                };
                for (index, v) in [(2, valor), (3, 0)] {
                    let out = self.arg(index);
                    if out != 0 {
                        self.cpu.write_u32(out, v)?;
                    }
                }
                SUCCESS
            }
            // O resto do `IMedia` responde sucesso sem fazer nada — e agora aparece no registro
            // com o nome, que é o que faltava para saber se uma partida muda depende dele.
            (Interface::Media, _) => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Lê um `AEEMediaData` do guest e devolve a chave do som em [`Machine::cargas_de_midia`].
    ///
    /// A struct é `{ AEECLSID clsData; void *pData; uint32 dwSize; }`. `MMD_BUFFER` aponta para
    /// os bytes; `MMD_FILE_NAME` aponta para o nome de um arquivo do pacote. Os bytes são lidos e
    /// decodificados **agora** — ver [`CargaDeMidia`]. `None` quando não há som a guardar: um
    /// arquivo que não existe ou uma forma de entrega que não conhecemos.
    pub(super) fn read_media_data(&mut self, pointer: u32) -> Result<Entrega, CpuError> {
        if pointer == 0 {
            return Ok(Entrega::Nada);
        }
        let class = self.cpu.read_u32(pointer)?;
        let data = self.cpu.read_u32(pointer + 4)?;
        let size = self.cpu.read_u32(pointer + 8)?;
        let bytes = match class {
            MMD_BUFFER if data != 0 && size != 0 && size <= MAX_MEDIA_BUFFER => {
                return Ok(Entrega::Buffer(data, size));
            }
            MMD_BUFFER => return Ok(Entrega::Nada),
            MMD_FILE_NAME if data != 0 => {
                let nome = self.cpu.read_cstring(data, MAX_STRING);
                match self.vfs.resolve(&nome).and_then(|p| std::fs::read(p).ok()) {
                    Some(bytes) => bytes,
                    None => {
                        self.bad_pointers
                            .insert(format!("som pedido por nome, e o arquivo não existe: {nome}"));
                        return Ok(Entrega::Nada);
                    }
                }
            }
            _ => {
                self.bad_pointers.insert(format!(
                    "uma mídia foi entregue como {class:#010x}, e só sabemos ler memória e arquivo"
                ));
                return Ok(Entrega::Nada);
            }
        };
        Ok(Entrega::Pronta(self.guarda_som(bytes)))
    }

    /// Lê agora o buffer que um objeto recebeu e ainda não foi lido.
    ///
    /// **O buffer é lido na volta seguinte do laço, e não na entrega nem no `Play`.** Os dois
    /// extremos quebram jogos diferentes. Lido no `Play`, um jogo que carrega vários sons pelo
    /// mesmo buffer de rascunho já o reaproveitou quando toca, e sai o som errado. Lido na entrega,
    /// o Zeebo F.C. Super League entrega o som de navegação do menu com só o cabeçalho escrito —
    /// ele manda tocar e copia as amostras em seguida, no mesmo tratador —, e saía um chiado com
    /// o conteúdo antigo do buffer (havia até o cabeçalho de uma textura ATC lá dentro). No
    /// aparelho o `Play` também só lê o buffer depois, numa tarefa separada.
    pub(super) fn resolve_midia(&mut self, this: u32) -> Result<(), CpuError> {
        let Some(state) = self.media.get(&this).copied() else {
            return Ok(());
        };
        let (onde, tamanho) = state.pendente;
        if tamanho == 0 {
            return Ok(());
        }
        let bytes = self.read_bytes(onde, tamanho)?;
        let carga = self.guarda_som(bytes);
        let tocar = match self.media.get_mut(&this) {
            Some(state) => {
                state.carga = carga;
                state.pendente = (0, 0);
                std::mem::take(&mut state.tocar_ao_ler)
            }
            None => false,
        };
        if tocar {
            // O aviso de início já saiu no `Play`.
            self.inicia_reproducao(this, false)?;
        }
        Ok(())
    }

    /// Lê os buffers que ficaram para esta volta do laço. Ver [`Machine::resolve_midia`].
    pub(super) fn resolve_midias_pendentes(&mut self) -> Result<(), CpuError> {
        let pendentes: Vec<u32> = self
            .media
            .iter()
            .filter(|(_, state)| state.pendente.1 != 0)
            .map(|(this, _)| *this)
            .collect();
        for this in pendentes {
            self.resolve_midia(this)?;
        }
        Ok(())
    }

    /// Decodifica e guarda um som pelos bytes, e devolve a chave dele.
    fn guarda_som(&mut self, bytes: Vec<u8>) -> u64 {
        // O `sound.ggz` do Double Dragon guarda os sons comprimidos: com o envelope de gzip, o
        // formato de verdade está dentro dele.
        let bytes = match bytes.starts_with(&[0x1f, 0x8b]) {
            true => inflate(&bytes).unwrap_or(bytes),
            false => bytes,
        };
        let chave = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut hasher);
            // Zero é "nada entregue" no estado do objeto.
            hasher.finish().max(1)
        };
        if !self.cargas_de_midia.contains_key(&chave) {
            let carga = self.decodifica_som(&bytes);
            self.cargas_de_midia.insert(chave, carga);
            self.descarta_sons_sem_dono(chave);
        }
        chave
    }

    /// Esquece os sons decodificados que nenhum `IMedia` usa, passando de
    /// [`MAX_SONS_GUARDADOS`].
    ///
    /// O cache existe para não decodificar de novo o som que o jogo entrega outra vez, mas sem
    /// teto ele guardava **todo** conteúdo que já passou por um `Play`: o Zeebo F.C. escreve
    /// efeitos diferentes no mesmo buffer de 500 KB, e cada um virava uma entrada para sempre.
    /// Uma voz tocando não perde nada: o PCM dela está num `Arc` que o mixer também segura.
    fn descarta_sons_sem_dono(&mut self, nova: u64) {
        if self.cargas_de_midia.len() <= MAX_SONS_GUARDADOS {
            return;
        }
        let em_uso: std::collections::HashSet<u64> =
            self.media.values().map(|state| state.carga).collect();
        self.cargas_de_midia
            .retain(|chave, _| *chave == nova || em_uso.contains(chave));
    }


    /// Toca uma partitura com o banco de amostras, quando o aparelho tem um.
    ///
    /// Só tenta quando os bytes começam com `MThd`: reconhecer o formato **antes** de abrir o banco
    /// evita gastar 32 MB de carga por causa de um som que não é partitura, e evita que o banco
    /// mude o desfecho de um formato que a tabela já tratava.
    #[cfg(feature = "soundfont")]
    fn toca_com_banco(&self, bytes: &[u8]) -> Option<crate::audio::wav::Sound> {
        if !bytes.starts_with(b"MThd") {
            return None;
        }
        let banco = self.banco_de_som.as_ref()?;
        crate::audio::soundfont::toca(banco, bytes, crate::audio::midi::RATE)
    }

    /// Sem a feature, o caminho é sempre o da tabela de timbres.
    #[cfg(not(feature = "soundfont"))]
    fn toca_com_banco(&self, _bytes: &[u8]) -> Option<crate::audio::wav::Sound> {
        None
    }

    /// Decodifica um som pelo que ele é, e não pelo nome.
    fn decodifica_som(&mut self, bytes: &[u8]) -> CargaDeMidia {
        // RIFF/WAVE primeiro porque é o que quase todo som é, e é o mais barato de reconhecer.
        // MP3 depois: é o formato da **música**, e enquanto ele não existia aqui, efeito tocava
        // e trilha não tocava em jogo nenhum.
        let som = match crate::audio::wav::parse(bytes) {
            Ok(sound) => Some(sound),
            Err(sem_wav) => match crate::audio::mp3::decode_detalhado(bytes) {
                Ok(sound) => Some(sound),
                // **O banco de amostras vem antes da tabela de timbres.** Quando ele existe, a
                // partitura é tocada com as amostras de verdade, e o que a tabela não alcança (a
                // razão de harmônicos das cordas e da distorção, a ressonância da bateria) passa a
                // vir do banco. Sem banco, o caminho é o de sempre.
                Err(porque) => match self.toca_com_banco(bytes) {
                    Some(sound) => {
                        self.assumptions.insert(concat!(
                            "a música MIDI é tocada com o banco de amostras do aparelho, e não com ",
                            "a tabela de timbres; o banco do console está no firmware que ainda não lemos"
                        ));
                        Some(sound)
                    }
                    None => match crate::audio::midi::decode(bytes) {
                    Some(sound) => {
                        self.assumptions.insert(concat!(
                            "a música MIDI é sintetizada aqui, com timbre aproximado — ",
                            "o banco de instrumentos do console está no firmware que ainda não lemos"
                        ));
                        Some(sound)
                    }
                    None => {
                    // Dizer *qual* formato chegou é o que permite saber o que implementar
                    // depois — e "não é um RIFF/WAVE" não diz. O que diz é a assinatura do
                    // próprio bloco: é assim que se soube que a trilha do Tekken 2 é MP3 sem
                    // abrir o jogo, e que a dos ports de arcade é MIDI.
                    let formato = detect_mime(bytes, "").unwrap_or("formato desconhecido");
                    // Os primeiros bytes vão no relatório junto do nome do formato: é o que
                    // **identifica** o que chegou sem abrir o jogo. `FF FB` é quadro MP3, `ftyp`
                    // é caixa MP4, `OggS` é Ogg — e a diferença entre eles decide o que
                    // implementar. Sem isto, a linha dizia só "recusado", e a investigação
                    // seguinte começava do zero, dentro do jogo.
                    let assinatura: String = bytes
                        .iter()
                        .take(16)
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                        // **Os dois motivos**, e não só o do WAV: "não é um RIFF/WAVE" é
                        // verdade e não ajuda — a pergunta é o que o decodificador de música
                        // recusou. Foi vendo os dois que se descobriu o Ogg do Turma da Mônica.
                        self.bad_pointers.insert(format!(
                            "som recusado ({formato}, {} bytes, {assinatura}): {sem_wav} / {porque}",
                            bytes.len()
                        ));
                        None
                    }
                    },
                },
            },
        };
        let silencio_us = match som {
            Some(_) => None,
            None => crate::audio::mp3::probe(bytes).map(|mp3| mp3.duration_us()),
        };
        CargaDeMidia {
            som: som.map(std::sync::Arc::new),
            silencio_us,
        }
    }

    /// O som de um objeto `IMedia`, já lido na entrega.
    pub(super) fn media_sound(
        &mut self,
        this: u32,
    ) -> Result<Option<std::sync::Arc<crate::audio::wav::Sound>>, CpuError> {
        Ok(self.carga_do(this).and_then(|carga| carga.som.clone()))
    }

    /// Quanto dura um som que não sabemos decodificar, quando dá para descobrir sem decodificar.
    ///
    /// Hoje só o MP3 cai aqui, pelo cabeçalho do primeiro quadro e pela etiqueta do codificador
    /// — ver [`crate::audio::mp3`].
    pub(super) fn media_silent_length(&mut self, this: u32) -> Result<Option<u64>, CpuError> {
        Ok(self.carga_do(this).and_then(|carga| carga.silencio_us))
    }

    fn carga_do(&self, this: u32) -> Option<&CargaDeMidia> {
        let carga = self.media.get(&this)?.carga;
        self.cargas_de_midia.get(&carga)
    }

    /// Se o objeto tem um som tocando agora.
    fn esta_tocando(&self, this: u32) -> bool {
        self.media
            .get(&this)
            .is_some_and(|state| state.state == MM_STATE_PLAY && self.now_us() < state.ends_us)
    }

    /// `int Play(IMedia *)`.
    ///
    /// **Um som entregue por memória é relido a cada `Play`.** O `IMedia` do aparelho não copia
    /// o buffer: toca o que estiver lá. O Zeebo F.C. Super League tem um objeto só para os
    /// efeitos da partida, com um buffer de 500 KB; ele escreve o chute, ou o passo, e manda
    /// tocar de novo, sem outro `SetMediaParm`. Guardado da primeira leitura, todo efeito saía
    /// com o som de seleção do menu, que foi o primeiro a passar por ali.
    pub(super) fn media_play(&mut self, this: u32) -> Result<u32, CpuError> {
        if self.fluxos_pcm.contains_key(&this) {
            return self.inicia_fluxo(this);
        }
        // **Um `Play` sobre a música que já toca em laço não a recomeça.** O gerenciador de som
        // dos Zeebo Extreme manda tocar a trilha da pista de novo toda vez que um efeito acaba —
        // o turbo, a derrapagem —, e aqui a voz recomeçava do início: a música reiniciava a cada
        // efeito. Recusar com `EBADSTATE` também não serve: ele entende que a música parou e
        // repete o `Play` a cada quadro. O que ele espera é o `START`, que o passa a "tocando"; a
        // voz segue de onde está. A regra fica restrita ao laço infinito: um efeito tocado de
        // novo por cima de si mesmo é o que o Zeebo F.C. faz, e ele precisa recomeçar.
        if self.esta_tocando(this) && self.media.get(&this).is_some_and(|state| state.repeat == 0)
        {
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
            return Ok(SUCCESS);
        }
        if let Some(state) = self.media.get_mut(&this)
            && state.buffer.1 != 0
        {
            state.pendente = state.buffer;
        }
        // Um som ainda não lido começa quando for lido, na volta seguinte do laço. Para o jogo ele
        // já está tocando.
        if let Some(state) = self.media.get_mut(&this)
            && state.pendente.1 != 0
        {
            state.tocar_ao_ler = true;
            state.state = MM_STATE_PLAY;
            state.ends_us = u64::MAX;
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
            return Ok(SUCCESS);
        }
        self.inicia_reproducao(this, true)
    }

    /// Começa a tocar o som já lido de um objeto.
    fn inicia_reproducao(&mut self, this: u32, avisa: bool) -> Result<u32, CpuError> {
        // **Um `Play` sobre um som que ainda toca não avisa.** Avisar `DONE` aqui fazia um ciclo
        // nos jogos que tocam de novo dentro do tratador do aviso: o novo `Play` caía sobre o som
        // que acabara de começar, gerava outro aviso, e o som reiniciava a cada quadro — o áudio
        // do Zeebo F.C. Super League saía estourado e picotado. O aviso de fim fica só no `Stop`
        // e no fim natural.
        let Some(sound) = self.media_sound(this)? else {
            // Um som que não sabemos ler mas sabemos **cronometrar** toca em silêncio pelo
            // tempo certo. Sem isso o Tekken 2 ficava preso: a música dele é MP3, o `Play`
            // respondia "esse som já acabou", o jogo consultava o estado, via "pronto" e
            // mandava tocar de novo — 766 mil vezes em quatro segundos virtuais, o que o
            // deixava na lista de "lento demais" sem ter trabalho nenhum para fazer.
            if let Some(length_us) = self.media_silent_length(this)? {
                self.assumptions
                    .insert("um som em formato que não decodificamos toca em silêncio, só com a duração certa");
                let now = self.now_us();
                let state = self.media.entry(this).or_default();
                state.state = MM_STATE_PLAY;
                state.ends_us = match state.repeat {
                    0 => u64::MAX,
                    times => now + length_us * u64::from(times),
                };
                if avisa {
                    self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
                }
                return Ok(SUCCESS);
            }
            // Sem som legível não há o que tocar, mas recusar faria o jogo tratar como erro
            // grave; para ele, o som simplesmente acabou na hora.
            if let Some(state) = self.media.get_mut(&this) {
                state.state = MM_STATE_READY;
            }
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_DONE)?;
            return Ok(SUCCESS);
        };
        // Quando o som acaba sai do **relógio virtual**, e não do mixer: um jogo que espera o
        // aviso de fim para tocar o próximo precisa recebê-lo mesmo com o som desligado, ou
        // emudece de vez depois do primeiro efeito.
        let length_us = sound.frames() as u64 * 1_000_000 / u64::from(sound.rate.max(1));
        let now = self.now_us();
        let state = self.media.entry(this).or_default();
        state.state = MM_STATE_PLAY;
        state.ends_us = match state.repeat {
            0 => u64::MAX,
            times => now + length_us * u64::from(times),
        };
        let (gain, repeat) = (state.gain(), state.repeat);
        if let Some(mixer) = &self.audio {
            mixer.play(this, sound, gain, repeat);
        }
        if avisa {
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
        }
        Ok(SUCCESS)
    }

    /// Avisa o jogo de que um som começou ou chegou ao fim.
    ///
    /// O aviso é o que fecha o ciclo de quem toca uma coisa de cada vez: sem ele o jogo fica
    /// esperando para sempre o efeito anterior terminar, e o som para depois do primeiro.
    ///
    /// **Todo aviso sai na volta do laço de eventos, e não na saída da chamada** — como no BREW,
    /// que entrega pelo laço depois de o tratador do jogo devolver o controle. Três jogos
    /// mediram as duas escolhas:
    ///
    /// - Os **Zeebo Extreme** marcam o som como "pedido" (2) *depois* do `Play`, e o `START` o
    ///   passa a "tocando" (1). Entregue na saída do `Play`, o `START` chegava antes da marca e o
    ///   som ficava em 2 para sempre; o `Stop` deles só age em 1, a música do menu nunca parava, e
    ///   com a da pista na fila o tocador recusava todo efeito — o Bóia Cross corria só com a trilha.
    /// - O **Double Dragon** repete a música pelo `DONE`: se o objeto ainda está marcado, agenda
    ///   um `Play` para dali a 100 ms. Ele desmarca e solta o objeto logo depois do `Stop`. Com o
    ///   `DONE` na saída do `Stop`, o tratador via o objeto ainda marcado, e o `Play` agendado caía
    ///   num `IMedia` já liberado — endereço zero, ao apertar voltar.
    /// - O **Zeebo F.C. Super League** também para e solta, e espera o `DONE` desse som. Por isso o
    ///   aviso **não** some com o `Release`: o tratador é guardado quando o aviso nasce. Descartado
    ///   junto com o objeto, a abertura parava na tela de aviso.
    pub(super) fn notify_media(
        &mut self,
        this: u32,
        cmd: u32,
        status: u32,
    ) -> Result<(), CpuError> {
        if let Some(state) = self.media.get(&this)
            && state.notify.function != 0
        {
            self.avisos_de_midia.push((this, cmd, status, state.notify));
        }
        Ok(())
    }

    /// Entrega os avisos enfileirados, em ordem.
    ///
    /// O `AEEMediaCmdNotify` é escrito **na hora de cada chamada**, num bloco só: um `START` e um
    /// `DONE` na mesma volta, escritos na entrada, diriam os dois `DONE`.
    pub fn entrega_avisos_de_midia(&mut self, budget: u64) -> Result<(), CpuError> {
        if self.avisos_de_midia.is_empty() {
            return Ok(());
        }
        if self.bloco_de_aviso_de_midia == 0 {
            self.bloco_de_aviso_de_midia = self.heap.alloc(MEDIA_NOTIFY_LEN).unwrap_or(0);
        }
        let block = self.bloco_de_aviso_de_midia;
        if block == 0 {
            self.avisos_de_midia.clear();
            return Ok(());
        }
        for (this, cmd, status, notify) in std::mem::take(&mut self.avisos_de_midia) {
            // `AEEMediaCmdNotify`: clsMedia, pIMedia, nCmd, nSubCmd, nStatus, pCmdData, dwSize.
            // A classe vai zerada — o jogo identifica o som pelo ponteiro, não por ela.
            for (index, value) in [0, this, cmd, 0, status, 0, 0].into_iter().enumerate() {
                self.cpu.write_u32(block + index as u32 * 4, value)?;
            }
            self.call_guest(notify.function, [notify.context, block, 0, 0], budget)?;
        }
        Ok(())
    }

    /// Reconhece a entrega de um som que o jogo gera enquanto toca, e guarda o formato dele.
    ///
    /// O `AEEMediaDataEx` é `{ clsData, pData, dwSize, dwStructSize, dwCaps, bRaw, pSpec,
    /// dwSpecSize, dwBufferSize }`, e o `AEEMediaWaveSpec` apontado por `pSpec` é `{ uint16
    /// wSize; AEECLSID clsMedia; uint16 wChannels; uint32 dwSamplesPerSec; uint16
    /// wBitsPerSample; boolean bUnsigned; uint32 dwAvgBytesPerSec; uint16 wBlockAlign }`. O
    /// Caveman Ninja monta exatamente isso: 11025 Hz, mono, 16 bits.
    ///
    /// Devolve `false` quando a entrega não é essa, para seguir pelo caminho de memória e arquivo.
    pub(super) fn le_fluxo_pcm(&mut self, this: u32, pointer: u32) -> Result<bool, CpuError> {
        if pointer == 0 || self.cpu.read_u32(pointer)? != MMD_ISOURCE {
            return Ok(false);
        }
        let fonte = self.cpu.read_u32(pointer + 4)?;
        let bruto = self.cpu.read_u32(pointer + 20)? & 0xff != 0;
        let spec = self.cpu.read_u32(pointer + 24)?;
        if fonte == 0 || spec == 0 || !bruto {
            self.bad_pointers
                .insert("um som veio de um ISource sem ser PCM cru, e só sabemos tocar PCM".into());
            return Ok(false);
        }
        let mut bytes = [0u8; 24];
        self.cpu.read_mem(spec, &mut bytes)?;
        let u16_em = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let canais = u16_em(8);
        let taxa = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        let bits = u16_em(16);
        let sem_sinal = bytes[18] != 0;
        if canais == 0 || taxa == 0 || !matches!(bits, 8 | 16) {
            self.bad_pointers.insert(format!(
                "PCM de {canais} canal(is), {taxa} Hz e {bits} bits, que não sabemos tocar"
            ));
            return Ok(false);
        }
        self.fluxos_pcm.insert(
            this,
            FluxoPcm {
                fonte,
                taxa,
                canais,
                bits,
                sem_sinal,
                inicio_us: 0,
                quadros_lidos: 0,
                tocando: false,
            },
        );
        Ok(true)
    }

    /// `Play` de um fluxo: a partir daqui as amostras são pedidas ao jogo a cada volta do laço.
    fn inicia_fluxo(&mut self, this: u32) -> Result<u32, CpuError> {
        let now = self.now_us();
        let state = self.media.entry(this).or_default();
        state.state = MM_STATE_PLAY;
        // Um fluxo não tem fim conhecido: acaba no `Stop`.
        state.ends_us = u64::MAX;
        let gain = state.gain();
        let Some(fluxo) = self.fluxos_pcm.get_mut(&this) else {
            return Ok(SUCCESS);
        };
        fluxo.inicio_us = now;
        fluxo.quadros_lidos = 0;
        fluxo.tocando = true;
        let (taxa, canais) = (fluxo.taxa, fluxo.canais);
        if let Some(mixer) = &self.audio {
            mixer.open_stream(this, taxa, canais, gain);
        }
        self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
        Ok(SUCCESS)
    }

    /// Pede ao jogo as amostras que o relógio virtual já deve, e as manda para o mixer.
    ///
    /// Quem marca o ritmo é o **tempo virtual**, como no resto do som: sem placa o emulador
    /// continua pedindo, e o jogo, que em geral emula o chip de som dentro do `Read`, anda do mesmo
    /// jeito. Um décimo de segundo vai adiantado, para a placa não esvaziar entre duas voltas.
    pub(super) fn bombeia_fluxos_pcm(&mut self, budget: u64) -> Result<(), CpuError> {
        let tocando: Vec<u32> = self
            .fluxos_pcm
            .iter()
            .filter(|(this, fluxo)| {
                fluxo.tocando
                    && self
                        .media
                        .get(this)
                        .is_some_and(|state| state.state == MM_STATE_PLAY)
            })
            .map(|(this, _)| *this)
            .collect();
        if tocando.is_empty() {
            return Ok(());
        }
        if self.buffer_de_fluxo == 0 {
            self.buffer_de_fluxo = self.heap.alloc(MAX_LEITURA_PCM).unwrap_or(0);
            if self.buffer_de_fluxo == 0 {
                return Ok(());
            }
        }
        let buffer = self.buffer_de_fluxo;
        let now = self.now_us();
        for this in tocando {
            let Some(fluxo) = self.fluxos_pcm.get(&this).copied() else {
                continue;
            };
            let por_quadro = u32::from(fluxo.canais) * u32::from(fluxo.bits / 8);
            let devidos = (now.saturating_sub(fluxo.inicio_us) + 100_000) * u64::from(fluxo.taxa)
                / 1_000_000;
            let mut faltam = devidos.saturating_sub(fluxo.quadros_lidos);
            let Ok(vtable) = self.cpu.read_u32(fluxo.fonte) else {
                continue;
            };
            let read = self.cpu.read_u32(vtable + ISOURCE_READ_SLOT * 4)?;
            // Algumas leituras por volta bastam; um `Read` que devolve menos é o jogo sem
            // amostras prontas, e insistir na mesma volta só gasta.
            for _ in 0..8 {
                if faltam == 0 {
                    break;
                }
                let pedido = (faltam * u64::from(por_quadro)).min(u64::from(MAX_LEITURA_PCM)) as u32;
                let pedido = pedido - pedido % por_quadro;
                if pedido == 0 {
                    break;
                }
                let outcome = self.call_guest(read, [fluxo.fonte, buffer, pedido, 0], budget)?;
                let Outcome::Returned { code } = outcome else {
                    break;
                };
                let lidos = code as i32;
                if lidos <= 0 {
                    break;
                }
                let lidos = (lidos as u32).min(pedido);
                let bytes = self.read_bytes(buffer, lidos)?;
                let amostras = pcm_para_f32(&bytes, fluxo.bits, fluxo.sem_sinal);
                if let Some(mixer) = &self.audio {
                    mixer.feed_stream(this, &amostras);
                }
                let quadros = u64::from(lidos / por_quadro);
                if let Some(guardado) = self.fluxos_pcm.get_mut(&this) {
                    guardado.quadros_lidos += quadros;
                }
                faltam = faltam.saturating_sub(quadros);
                if lidos < pedido {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Enfileira o aviso de fim dos sons que já terminaram.
    pub(super) fn poll_media(&mut self) -> Result<(), CpuError> {
        let now = self.now_us();
        let finished: Vec<u32> = self
            .media
            .iter()
            .filter(|(_, state)| state.state == MM_STATE_PLAY && now >= state.ends_us)
            .map(|(this, _)| *this)
            .collect();
        for this in finished {
            if let Some(state) = self.media.get_mut(&this) {
                state.state = MM_STATE_READY;
                state.ends_us = 0;
            }
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_DONE)?;
        }
        Ok(())
    }
}

/// Amostras PCM de 8 ou 16 bits, little-endian, para `f32` em [-1, 1].
pub(super) fn pcm_para_f32(bytes: &[u8], bits: u16, sem_sinal: bool) -> Vec<f32> {
    match bits {
        8 => bytes
            .iter()
            .map(|&b| match sem_sinal {
                true => (f32::from(b) - 128.0) / 128.0,
                false => f32::from(b as i8) / 128.0,
            })
            .collect(),
        _ => bytes
            .chunks_exact(2)
            .map(|c| {
                let valor = u16::from_le_bytes([c[0], c[1]]);
                match sem_sinal {
                    true => (f32::from(valor) - 32768.0) / 32768.0,
                    false => f32::from(valor as i16) / 32768.0,
                }
            })
            .collect(),
    }
}
