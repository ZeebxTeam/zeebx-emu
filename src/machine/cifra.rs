//! Criptografia: o ICipher e o IHash do BREW.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `ILicense` (`AEECLSID_LICENSE` = `0x0100100f`), do `QINTERFACE(ILicense)` em
    /// `sdk/inc/AEELicense.h`.
    ///
    /// Responde o que vale para uma ROM legitimamente comprada e instalada no console:
    /// licença sem expiração (`LT_NONE`) e compra definitiva (`PT_PURCHASE`). Não é
    /// contornar verificação nenhuma — é a resposta que o próprio aparelho daria para um
    /// jogo que o dono comprou.
    /// Rede e criptografia: `IWeb`, `IHash` (`AEECLSID_MD5`), `ICipherFactory` e `ICipher1`.
    ///
    /// O Boomerang Sports Dodgeball cria os três em sequência e **não confere o retorno**: se a
    /// primeira criação falha, ele pula as outras duas e usa o ponteiro que nunca foi escrito.
    /// Por isso os objetos precisam existir mesmo com o console offline — recusá-los derrubava
    /// o jogo antes da primeira tela.
    pub(super) fn crypto_call(
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
            "QueryInterface" => {
                let out = self.cpu.read_reg(Reg::R2);
                if out != 0 {
                    self.cpu.write_u32(out, this)?;
                }
                SUCCESS
            }
            // int CreateCipher(ICipherFactory *, AEECLSID cipher, int direction,
            //                  AEECLSID mode, int padding, ICipher1 **ppCipher)
            "CreateCipher" => {
                let out = self.stack_arg(1)?;
                let cipher = self.new_object(Interface::Cipher)?;
                if cipher == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                let padding = self.stack_arg(0)?;
                self.ciphers.insert(
                    cipher,
                    CipherState {
                        padding,
                        ..Default::default()
                    },
                );
                if out != 0 {
                    self.cpu.write_u32(out, cipher)?;
                }
                SUCCESS
            }
            // void IHASH_Reset(IHash *) — recomeça o resumo do zero.
            "Reset" => {
                self.hashes.insert(this, HashState::default());
                SUCCESS
            }
            // void IHASH_Update(IHash *, const byte *pData, int nLen)
            "Update" => {
                let (src, len) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let bytes = match len {
                    0 => Vec::new(),
                    _ => self.read_bytes(src, len)?,
                };
                self.hashes.entry(this).or_default().md5.update(&bytes);
                SUCCESS
            }
            // void GetDigest(IHash *, byte *pBuf, int *pnLen)
            //
            // O chamador zera um buffer de 33 bytes e põe 17 no tamanho — dezesseis bytes de
            // resumo e o terminador. Escrevemos os dezesseis e devolvemos dezesseis no tamanho,
            // respeitando o teto que ele pediu.
            "GetDigest" => {
                let (destino, tamanho) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let resumo = self.hashes.entry(this).or_default().md5.clone().finish();
                let cabe = match tamanho {
                    0 => resumo.len() as u32,
                    p => self
                        .cpu
                        .read_u32(p)
                        .unwrap_or(0)
                        .min(resumo.len() as u32 + 1),
                };
                let escrever = cabe.min(resumo.len() as u32) as usize;
                if destino != 0 {
                    self.cpu.write_mem(destino, &resumo[..escrever])?;
                }
                if tamanho != 0 {
                    self.cpu.write_u32(tamanho, escrever as u32)?;
                }
                SUCCESS
            }
            // int QueryCipher(ICipherFactory *, AEECLSID cipher, AEECLSID mode, int padding,
            //                 unsigned keysize)
            // void IWEB_GetResponse(IWeb *po, IWeb *po, IWebResp **ppResp, AEECallback *pcb,
            //                       const char *pszUrl, ...)
            //
            // A macro do SDK repete o `po` como primeiro vararg, então o `r1` é o próprio
            // objeto, o `r2` é o ponteiro de saída da resposta, o `r3` é o callback e a URL vem
            // da pilha. As opções seguem depois, terminadas por `WEBOPT_END`.
            //
            // Por ora isto **observa e recusa**. Observar primeiro é deliberado: o formato dos
            // varargs não está em header nenhum que tenhamos, e implementar HTTP contra um
            // palpite de layout daria um cliente que erra o endereço sem dizer. Registrado o
            // que o jogo pede, o passo seguinte é atender de verdade — e aí dar acesso à rede a
            // um binário de origem externa é decisão de projeto, com autorização explícita.
            "GetResponse" => {
                let (r1, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let url_ptr = self.stack_arg(0)?;
                let url = match url_ptr {
                    0 => String::new(),
                    p => self.cpu.read_cstring(p, MAX_STRING),
                };
                self.web_requests.insert(match url.is_empty() {
                    true => format!("(sem URL) r1={r1:#x} r2={saida:#x}"),
                    false => url,
                });
                // Ponteiro de saída zerado: o jogo precisa ver que não há resposta, em vez de
                // seguir com lixo.
                if saida != 0 {
                    self.cpu.write_u32(saida, 0)?;
                }
                SUCCESS
            }
            "QueryCipher" => SUCCESS,
            // int AddOpt(IWeb *, WebOpt *apWebOpt) — cabeçalhos, tempo limite e afins. Aceitar
            // não custa nada: quem decide o destino da requisição é o `GetResponse`.
            //
            // O `AddOptBuffer` é o slot 6, que no firmware monta um descritor na pilha e chama
            // esta mesma função. Aceitar os dois é a mesma decisão.
            "AddOpt" | "AddOptBuffer" => SUCCESS,
            // Os slots do `IWeb` que a vtable do firmware mostra existir e que ainda não
            // apareceram em uso. Responder sucesso os deixa aparecer no relatório de chamadas em
            // vez de derrubar o jogo — que foi como o slot 6 foi encontrado.
            "slot4" | "slot5" | "slot7" | "slot8" | "slot9" | "slot10" | "slot12"
                if iface == Interface::Web =>
            {
                SUCCESS
            }
            // int SetParam(ICipher1 *, int nId, const void *pParam, unsigned uParamLen)
            "SetParam" => {
                let (id, param, len) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3),
                );
                let Some(state) = self.ciphers.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                match id {
                    CIPHER_PARAM_KEY | CIPHER_PARAM_IV => {
                        if len as usize != AES_BLOCK {
                            return Ok(Some(EBADPARM));
                        }
                        let mut bytes = [0u8; AES_BLOCK];
                        self.cpu.read_mem(param, &mut bytes)?;
                        let state = self.ciphers.get_mut(&this).expect("acabou de ser achado");
                        if id == CIPHER_PARAM_KEY {
                            state.key = Some(bytes);
                        } else {
                            state.iv = bytes;
                        }
                        SUCCESS
                    }
                    CIPHER_PARAM_PADDING => {
                        state.padding = self.cpu.read_u32(param)?;
                        SUCCESS
                    }
                    // A direção e o modo já vieram no `CreateCipher`; repeti-los não muda nada,
                    // e recusar faria o jogo desistir.
                    CIPHER_PARAM_DIRECTION | CIPHER_PARAM_MODE => SUCCESS,
                    _ => EUNSUPPORTED,
                }
            }
            // int GetParam(ICipher1 *, int nId, void *pParam, unsigned *puParamLen)
            "GetParam" => {
                let (id, param, len) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3),
                );
                let value = match id {
                    CIPHER_PARAM_KEY_SIZE | CIPHER_PARAM_IV_SIZE | CIPHER_PARAM_BLOCKSIZE => {
                        AES_BLOCK as u32
                    }
                    _ => return Ok(Some(EUNSUPPORTED)),
                };
                if param != 0 {
                    self.cpu.write_u32(param, value)?;
                }
                if len != 0 {
                    self.cpu.write_u32(len, 4)?;
                }
                SUCCESS
            }
            // int Process(ICipher1 *, const byte *pbIn, unsigned cbIn, byte *pbOut,
            //             unsigned *pcbOut)
            "Process" | "ProcessLast" => self.cipher_process(this, name == "ProcessLast")?,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// `ICipher1_Process` e `ICipher1_ProcessLast`, que só diferem no fecho.
    ///
    /// O `ICipher1` é de fluxo: o jogo entrega quantos bytes quiser, e o que não completa um
    /// bloco fica guardado para a chamada seguinte. Só o `ProcessLast` preenche o bloco que
    /// faltou, do jeito que o `padding` pedir.
    pub(super) fn cipher_process(&mut self, this: u32, last: bool) -> Result<u32, CpuError> {
        // `Process` tem os cinco argumentos; `ProcessLast` só tem a saída e o tamanho dela.
        let (input, count, out, out_len) = match last {
            false => (
                self.cpu.read_reg(Reg::R1),
                self.cpu.read_reg(Reg::R2) as usize,
                self.cpu.read_reg(Reg::R3),
                self.stack_arg(0)?,
            ),
            true => (0, 0, self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2)),
        };
        let Some(state) = self.ciphers.get(&this) else {
            return Ok(EBADPARM);
        };
        let Some(key) = state.key else {
            return Ok(EBADPARM);
        };

        let mut data = state.pending.clone();
        if count > 0 {
            let mut bytes = vec![0u8; count];
            self.cpu.read_mem(input, &mut bytes)?;
            data.extend_from_slice(&bytes);
        }
        if last {
            match state.padding {
                CIPHER_PADDING_NONE if data.len() % AES_BLOCK != 0 => return Ok(EBADPARM),
                CIPHER_PADDING_NONE => {}
                // `CIPHER_PADDING_ZERO` e o que não conhecemos completam com zeros: é o
                // preenchimento que o BREW usa por omissão nos cifradores de bloco.
                _ => data.resize(data.len().div_ceil(AES_BLOCK) * AES_BLOCK, 0),
            }
        }
        // O que o jogo cifra é registrado em claro, e é a coisa mais útil que este método faz
        // para quem estuda protocolo: o corpo que sai pela rede vai cifrado, e decifrá-lo do
        // outro lado exige ter a chave e acertar o modo. Aqui ele passa por nós antes disso.
        if !data.is_empty() {
            if self.plaintexts.len() == PLAINTEXT_MAX {
                self.plaintexts.pop_front();
            }
            self.plaintexts
                .push_back(data[..data.len().min(PLAINTEXT_BYTES)].to_vec());
        }
        // O que não fecha um bloco espera a próxima chamada.
        let whole = data.len() / AES_BLOCK * AES_BLOCK;
        let leftover = data.split_off(whole);

        let mut iv = state.iv;
        crypto::cbc_encrypt(&crypto::Aes128::new(&key), &mut iv, &mut data);
        if let Some(state) = self.ciphers.get_mut(&this) {
            state.iv = iv;
            state.pending = leftover;
        }

        // A capacidade oferecida chega no mesmo ponteiro que devolve quanto foi escrito.
        let capacity = match out_len {
            0 => data.len(),
            addr => self.cpu.read_u32(addr)? as usize,
        };
        if capacity < data.len() {
            if out_len != 0 {
                self.cpu.write_u32(out_len, data.len() as u32)?;
            }
            return Ok(EBUFFERTOOSMALL);
        }
        if out != 0 && !data.is_empty() {
            self.cpu.write_mem(out, &data)?;
        }
        if out_len != 0 {
            self.cpu.write_u32(out_len, data.len() as u32)?;
        }
        Ok(SUCCESS)
    }

    /// O que o jogo cifrou, em claro, na ordem em que entregou.
    pub fn plaintexts(&self) -> Vec<&[u8]> {
        self.plaintexts.iter().map(Vec::as_slice).collect()
    }

    /// As chaves de cifra que os jogos configuraram, em hexadecimal.
    ///
    /// Existe por um motivo prático: o Zeeboids **cifra o corpo antes de enviar**, então o
    /// servidor recebe ruído. A chave é do próprio jogo e passa por nós no `ICipher1::SetParam`;
    /// sem ela, o registro do servidor não diz nada sobre o protocolo.
    pub fn cipher_keys(&self) -> Vec<String> {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        self.ciphers
            .values()
            .filter_map(|c| {
                c.key
                    .map(|k| format!("chave {} iv {}", hex(&k), hex(&c.iv)))
            })
            .collect()
    }
}
