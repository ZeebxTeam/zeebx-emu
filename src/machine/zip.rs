//! IUnzipAStream e os fluxos: o inflate e a leitura em pedaços.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `IUnzipAStream` (`AEECLSID_UNZIPSTREAM` = `0x01001014`), de `sdk/inc/AEEUnzipStream.h`.
    ///
    /// Estende o `IAStream` para ler um stream comprimido como se fosse texto claro: o jogo
    /// monta um stream sobre os bytes comprimidos, entrega em `SetStream`, e lê daqui. É o que
    /// o Double Dragon usa para abrir o `data.ggz` e o `sound.ggz`.
    ///
    /// A descompressão é feita **de uma vez**, na primeira leitura, e o resultado fica guardado.
    /// O BREW descomprime conforme se lê, mas o efeito visível é o mesmo, e fazer de uma vez
    /// dispensa manter estado de inflate parcial entre chamadas.
    pub(super) fn unzip_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::UnzipStream.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.unzips.remove(&this);
                }
                remaining
            }
            // void SetStream(IUnzipAStream *, IAStream *pInIAStream)
            "SetStream" => {
                self.unzips.insert(
                    this,
                    UnzipState {
                        source: a1,
                        ..UnzipState::default()
                    },
                );
                SUCCESS
            }
            // O conteúdo já está inteiro na memória, então há sempre o que ler.
            "Readable" => {
                if a1 != 0 {
                    self.pending_signals.push(Callback {
                        function: a1,
                        context: a2,
                    });
                }
                SUCCESS
            }
            // int32 Read(IAStream *, void *pDest, uint32 nWant)
            "Read" => {
                self.expand_unzip(this)?;
                let Some(state) = self.unzips.get(&this) else {
                    return Ok(Some(EFAILED));
                };
                let want = (a2 as usize).min(state.output.len().saturating_sub(state.position));
                if want > 0 {
                    let at = state.position;
                    let bytes = state.output[at..at + want].to_vec();
                    self.cpu.write_mem(a1, &bytes)?;
                    if let Some(state) = self.unzips.get_mut(&this) {
                        state.position += want;
                    }
                }
                want as u32
            }
            "Cancel" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Lê **tudo** o que resta de um `IAStream` do guest.
    ///
    /// `None` quando o objeto não é um stream que saibamos ler por dentro — o que é diferente
    /// de um erro de leitura, que vem como `Some(Err(..))`.
    pub(super) fn drain_stream(&mut self, source: u32) -> Option<Result<Vec<u8>, CpuError>> {
        if let Some(stream) = self.streams.get(&source).copied() {
            let available = stream.size.saturating_sub(stream.position);
            return Some(self.read_bytes(stream.buffer + stream.position, available));
        }
        // Um `IFile` aberto também é um `IAStream`, e é assim que o Double Dragon entrega o
        // `data.ggz`: lemos da posição corrente até o fim, como faria uma sequência de `Read`.
        let open = self.open_files.get_mut(&source)?;
        let mut bytes = Vec::new();
        Some(
            match std::io::Read::read_to_end(&mut open.file, &mut bytes) {
                Ok(_) => Ok(bytes),
                Err(_) => Ok(Vec::new()),
            },
        )
    }

    /// Descomprime a entrada de um `IUnzipAStream`, uma vez só.
    pub(super) fn expand_unzip(&mut self, this: u32) -> Result<(), CpuError> {
        let Some(state) = self.unzips.get(&this) else {
            return Ok(());
        };
        if state.expanded {
            return Ok(());
        }
        let source = state.source;
        if let Some(state) = self.unzips.get_mut(&this) {
            state.expanded = true;
        }

        // A entrada é um `IAStream`, e no BREW tanto um bloco de memória quanto um arquivo
        // aberto servem como um. O Double Dragon entrega o `IFile` do `data.ggz` direto.
        let Some(compressed) = self.drain_stream(source) else {
            // Dizer *qual* interface chegou é o que transforma isto de "não funcionou" em uma
            // pista: a próxima origem a suportar é a que aparecer aqui.
            let kind = self
                .objects
                .kind_of(source)
                .map(Interface::name)
                .unwrap_or("desconhecida");
            self.bad_pointers.insert(format!(
                "um IUnzipAStream recebeu como entrada um objeto de {kind}, que ainda não sabemos ler"
            ));
            return Ok(());
        };
        let compressed = compressed?;
        match inflate(&compressed) {
            Some(output) => {
                if let Some(state) = self.unzips.get_mut(&this) {
                    state.output = output;
                }
                // O stream de entrada foi consumido inteiro.
                if let Some(stream) = self.streams.get_mut(&source) {
                    stream.position = stream.size;
                }
            }
            None => {
                self.assumptions
                    .insert("um IUnzipAStream recebeu dados que não descomprimem");
            }
        }
        Ok(())
    }

    /// `IMemAStream` (`AEECLSID_MEMASTREAM` = `0x0100100c`), de `sdk/inc/AEE.h`.
    ///
    /// Apresenta um bloco de memória como stream: é assim que o BREW entrega dados a um
    /// decodificador de imagem. Aqui o "assíncrono" é trivial — os bytes já estão todos na
    /// memória, então `Readable` pode avisar o interessado na mesma hora.
    pub(super) fn stream_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::MemAStream.method(slot) else {
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
                    self.streams.remove(&this);
                }
                remaining
            }
            // void Set(IMemAStream *, byte *pBuff, uint32 dwSize, uint32 dwOffset,
            //          boolean bSysMem)
            "Set" | "SetEx" => {
                self.streams.insert(
                    this,
                    MemStream {
                        buffer: a1,
                        size: a2,
                        position: a3.min(a2),
                    },
                );
                SUCCESS
            }
            // void Readable(IAStream *, void (*pfnNotify)(void *), void *pUser)
            //
            // O stream está sempre pronto, então o callback vai direto para a fila — entregá-lo
            // aqui seria reentrar no guest no meio do despacho.
            "Readable" => {
                if a1 != 0 {
                    self.pending_signals.push(Callback {
                        function: a1,
                        context: a2,
                    });
                }
                SUCCESS
            }
            // int32 Read(IAStream *, void *pDest, uint32 nWant)
            "Read" => {
                let Some(stream) = self.streams.get(&this).copied() else {
                    return Ok(Some(EFAILED));
                };
                let want = a2.min(stream.size.saturating_sub(stream.position));
                if want > 0 {
                    let bytes = self.read_bytes(stream.buffer + stream.position, want)?;
                    self.cpu.write_mem(a1, &bytes)?;
                    if let Some(stream) = self.streams.get_mut(&this) {
                        stream.position += want;
                    }
                }
                want
            }
            "Cancel" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }
}
