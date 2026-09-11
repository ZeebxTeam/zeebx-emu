//! IMedia: a reprodução de som e vídeo pedida pelo jogo.

use super::*;

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
        let result = match (iface, name) {
            (_, "AddRef") => self.objects.add_ref(this),
            (_, "Release") => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.media.remove(&this);
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
                self.read_media_data(self.arg(1), &mut state)?;
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
                match parm {
                    MM_PARM_MEDIA_DATA => self.read_media_data(p1, &mut state)?,
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
                SUCCESS
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
            (Interface::Media, "Stop") => {
                if let Some(state) = self.media.get_mut(&this) {
                    state.state = MM_STATE_READY;
                }
                if let Some(mixer) = &self.audio {
                    mixer.stop(this);
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
            (Interface::Media, "GetTotalTime") => match self.media_sound(this)? {
                Some(sound) => (sound.frames() as u64 * 1000 / u64::from(sound.rate.max(1))) as u32,
                None => (self.media_silent_length(this)?.unwrap_or(0) / 1000) as u32,
            },
            (Interface::Media, "GetMediaParm") => {
                for index in [2, 3] {
                    let out = self.arg(index);
                    if out != 0 {
                        self.cpu.write_u32(out, 0)?;
                    }
                }
                SUCCESS
            }
            (Interface::Media, _) => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Lê um `AEEMediaData` do guest para o estado do objeto.
    ///
    /// A struct é `{ AEECLSID clsData; void *pData; uint32 dwSize; }`. Só a variante de memória
    /// interessa: os três jogos que tocam som passam `MMD_BUFFER` com um RIFF já carregado.
    pub(super) fn read_media_data(
        &mut self,
        pointer: u32,
        state: &mut MediaState,
    ) -> Result<(), CpuError> {
        if pointer == 0 {
            return Ok(());
        }
        let class = self.cpu.read_u32(pointer)?;
        let data = self.cpu.read_u32(pointer + 4)?;
        let size = self.cpu.read_u32(pointer + 8)?;
        if class != MMD_BUFFER {
            self.bad_pointers.insert(format!(
                "uma mídia foi entregue como {class:#010x}, e só sabemos ler buffer de memória"
            ));
            return Ok(());
        }
        (state.buffer, state.size) = (data, size);
        Ok(())
    }

    /// O som de um objeto `IMedia`, lido do buffer do guest e guardado.
    ///
    /// Um RIFF pode ter megabytes — o do Quake tem 1,4 —, e reinterpretá-lo a cada `Play`
    /// seria trabalho repetido a cada tiro disparado.
    pub(super) fn media_sound(
        &mut self,
        this: u32,
    ) -> Result<Option<std::sync::Arc<crate::audio::wav::Sound>>, CpuError> {
        let Some(state) = self.media.get(&this).copied() else {
            return Ok(None);
        };
        if state.buffer == 0 || state.size == 0 {
            return Ok(None);
        }
        let key = (state.buffer, state.size);
        if let Some(sound) = self.waves.get(&key) {
            return Ok(Some(sound.clone()));
        }
        let bytes = self.read_bytes(state.buffer, state.size)?;
        // RIFF/WAVE primeiro porque é o que quase todo som é, e é o mais barato de reconhecer.
        // MP3 depois: é o formato da **música**, e enquanto ele não existia aqui, efeito tocava
        // e trilha não tocava em jogo nenhum.
        let som = match crate::audio::wav::parse(&bytes) {
            Ok(sound) => Some(sound),
            Err(err) => match crate::audio::mp3::decode(&bytes).or_else(|| {
                crate::audio::midi::decode(&bytes).inspect(|_| {
                    self.assumptions.insert(concat!(
                        "a música MIDI é sintetizada aqui, com timbre aproximado — ",
                        "o banco de instrumentos do console está no firmware que ainda não lemos"
                    ));
                })
            }) {
                Some(sound) => Some(sound),
                None => {
                    // Dizer *qual* formato chegou é o que permite saber o que implementar
                    // depois — e "não é um RIFF/WAVE" não diz. O que diz é a assinatura do
                    // próprio bloco: é assim que se soube que a trilha do Tekken 2 é MP3 sem
                    // abrir o jogo, e que a dos ports de arcade é MIDI.
                    let formato = detect_mime(&bytes, "").unwrap_or("formato desconhecido");
                    self.bad_pointers
                        .insert(format!("som recusado ({formato}): {err}"));
                    None
                }
            },
        };
        let Some(sound) = som else { return Ok(None) };
        let sound = std::sync::Arc::new(sound);
        self.waves.insert(key, sound.clone());
        Ok(Some(sound))
    }

    /// Quanto dura um som que não sabemos decodificar, quando dá para descobrir sem decodificar.
    ///
    /// Hoje só o MP3 cai aqui, pelo cabeçalho do primeiro quadro e pela etiqueta do codificador
    /// — ver [`crate::audio::mp3`].
    pub(super) fn media_silent_length(&mut self, this: u32) -> Result<Option<u64>, CpuError> {
        let Some(state) = self.media.get(&this).copied() else {
            return Ok(None);
        };
        if state.buffer == 0 || state.size == 0 {
            return Ok(None);
        }
        let bytes = self.read_bytes(state.buffer, state.size)?;
        Ok(crate::audio::mp3::probe(&bytes).map(|mp3| mp3.duration_us()))
    }

    /// `int Play(IMedia *)`.
    pub(super) fn media_play(&mut self, this: u32) -> Result<u32, CpuError> {
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
                self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
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
        self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
        Ok(SUCCESS)
    }

    /// Avisa o jogo de que um som chegou ao fim.
    ///
    /// O aviso é o que fecha o ciclo de quem toca uma coisa de cada vez: sem ele o jogo fica
    /// esperando para sempre o efeito anterior terminar, e o som para depois do primeiro.
    pub(super) fn notify_media(
        &mut self,
        this: u32,
        cmd: u32,
        status: u32,
    ) -> Result<(), CpuError> {
        let Some(state) = self.media.get(&this).copied() else {
            return Ok(());
        };
        if state.notify.function == 0 {
            return Ok(());
        }
        let block = match state.notify_block {
            0 => {
                let block = self.heap.alloc(MEDIA_NOTIFY_LEN).unwrap_or(0);
                if let Some(state) = self.media.get_mut(&this) {
                    state.notify_block = block;
                }
                block
            }
            block => block,
        };
        if block == 0 {
            return Ok(());
        }
        // `AEEMediaCmdNotify`: clsMedia, pIMedia, nCmd, nSubCmd, nStatus, pCmdData, dwSize.
        // A classe vai zerada — o jogo identifica o som pelo ponteiro, não por ela.
        for (index, value) in [0, this, cmd, 0, status, 0, 0].into_iter().enumerate() {
            self.cpu.write_u32(block + index as u32 * 4, value)?;
        }
        self.pending_calls.push(GuestCall {
            function: state.notify.function,
            args: [state.notify.context, block, 0, 0],
        });
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
