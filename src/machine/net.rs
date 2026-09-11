//! IWeb e ISource: a rede, a requisição e a resposta devolvida ao jogo.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Atende o `ISource` e o `IPeek`. Ver [`Interface::Peek`].
    ///
    /// Do `IPeek` só o slot 8 tem corpo, e ele é a razão de tudo isto existir: a Z-Wheel lê o
    /// `tectoy.cfg` linha a linha por ele. A chamada é `slot8(this, &par, 3)`, com `par` sendo
    /// `{ponteiro, tamanho}` — o jogo lê os dois e **copia** o texto antes de pedir a próxima
    /// linha, o que é o que permite reaproveitar um buffer só.
    ///
    /// **O valor de retorno é uma hipótese, e ela está medida.** O laço em `0x884fc` continua
    /// enquanto `-retorno >= 2` e para em `-3`; devolver zero o encerraria na primeira linha,
    /// inclusive numa linha vazia. Então: `1` enquanto houver linha, `-3` no fim. Que o console
    /// devolva o mesmo `1` não se sabe — o que se sabe é a condição do laço.
    pub(super) fn source_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        /// "Acabaram as linhas": o único valor que o laço da `0x88338` aceita como fim.
        const FIM: u32 = (-3i32) as u32;
        /// "Veio linha". Ver a nota sobre o retorno, acima.
        const VEIO: u32 = 1;

        let Some(name) = iface.method(slot) else {
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
                    self.sources.remove(&this);
                    self.peeks.remove(&this);
                }
                restantes
            }
            "QueryInterface" => {
                let saida = self.cpu.read_reg(Reg::R2);
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // `int32 ISOURCE_Read(ISource *po, char *pcBuf, int32 cbBuf)`.
            "Read" => {
                let (destino, cabe) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2) as usize,
                );
                let Some(bytes) = self.sources.get(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let pedaco = bytes[..bytes.len().min(cabe)].to_vec();
                self.cpu.write_mem(destino, &pedaco)?;
                self.sources.insert(this, bytes[pedaco.len()..].to_vec());
                pedaco.len() as u32
            }
            "LerLinha" => {
                let par = self.cpu.read_reg(Reg::R1);
                let Some(leitor) = self.peeks.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let Some(linha) = leitor.proxima_linha() else {
                    return Ok(Some(FIM));
                };
                let (buffer, tamanho) = (leitor.buffer, linha.len() as u32);
                self.cpu.write_mem(buffer, &linha)?;
                self.cpu.write_mem(buffer + tamanho, &[0])?;
                if par != 0 {
                    self.cpu.write_u32(par, buffer)?;
                    self.cpu.write_u32(par + 4, tamanho)?;
                }
                VEIO
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende a `ISourceUtil`. Ver [`Interface::SourceUtil`].
    pub(super) fn source_util_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::SourceUtil.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "QueryInterface" => {
                let saida = self.cpu.read_reg(Reg::R2);
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // `int PeekSourceFromSource(ISourceUtil *po, ISource *ps, int nMax, IPeek **ppo)`.
            //
            // O `nMax` é o teto do que o leitor pode manter em memória; a Z-Wheel passa o
            // tamanho do arquivo mais um, ou seja, o arquivo inteiro. Como já temos os bytes
            // todos, ele não muda nada aqui — mas é o que diz que o jogo espera ler tudo.
            "PeekSourceFromSource" => {
                let (fonte, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R3));
                let Some(bytes) = self.sources.get(&fonte).cloned() else {
                    return Ok(Some(EBADPARM));
                };
                let leitor = self.new_object(Interface::Peek)?;
                if leitor == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                // O buffer de uma linha vive junto do leitor: o jogo recebe um ponteiro para
                // ele e **copia** o conteúdo antes de pedir a próxima, então um buffer só,
                // reaproveitado, basta. Reservar do tamanho da fonte garante que a maior linha
                // possível caiba.
                let buffer = self.heap.alloc(bytes.len() as u32 + 1).unwrap_or(0);
                if buffer == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.peeks.insert(
                    leitor,
                    Peek {
                        bytes,
                        posicao: 0,
                        buffer,
                    },
                );
                if saida != 0 {
                    self.cpu.write_u32(saida, leitor)?;
                }
                SUCCESS
            }
            // `int SourceFromFile(ISourceUtil *po, IFile *pf, ISource **ppo)`.
            //
            // Lemos o arquivo inteiro pelo caminho, e não pelo descritor aberto, para não mexer
            // na posição do `IFile` do jogo — ele continua sendo dele.
            "SourceFromFile" => {
                let (arquivo, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let Some(caminho) = self
                    .open_files
                    .get(&arquivo)
                    .map(|aberto| aberto.guest_path.clone())
                else {
                    return Ok(Some(EBADPARM));
                };
                let Some(bytes) = self
                    .vfs
                    .resolve(&caminho)
                    .and_then(|real| std::fs::read(real).ok())
                else {
                    return Ok(Some(EFAILED));
                };
                let fonte = self.new_object(Interface::Source)?;
                if fonte == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.sources.insert(fonte, bytes);
                if saida != 0 {
                    self.cpu.write_u32(saida, fonte)?;
                }
                SUCCESS
            }
            // `int SourceFromMemory(ISourceUtil *po, const void *pBuf, int nSize,
            //                       PFNNOTIFY pfn, void *pUser, ISource **ppo)`.
            //
            // É aqui que a ponte do Zeeboids entra, e vale explicar por quê. O método não
            // envia nada: ele embrulha um pedaço de memória num `ISource` para que a `IWeb`
            // possa lê-lo. Só que o pedaço de memória, no Zeeboids, **é o corpo do POST** — e
            // este é o único ponto em que ele existe inteiro e ainda em claro.
            //
            // No firmware ele registra o par guardado pelo `SetHandler` e enfileira o trabalho
            // no objeto interno. Aqui fazemos o trabalho na hora: `r1` e `r2` são o corpo e o
            // tamanho, e a resposta volta no ponteiro de saída.
            "SourceFromMemory" => {
                let (corpo, tamanho) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let saida = self.stack_arg(1)?;
                self.send_request(corpo, tamanho, saida)?
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Manda o corpo que o jogo entregou ao despachante e guarda a resposta.
    ///
    /// **Achar a URL é o passo delicado.** Ela não vem nos argumentos: fica no objeto do
    /// `ConnectionManager` do jogo, que é quem nos chamou. Em vez de fixar um deslocamento — o
    /// do Zeeboids é `+0x244`, e valeria só para ele —, procuramos no objeto o **trio**
    /// `{url, corpo, tamanho}` cujas duas últimas palavras são exatamente os argumentos que
    /// acabamos de receber. Isso se confere sozinho: um trio que case com o ponteiro e o
    /// tamanho recebidos não é coincidência, e um que não case é descartado.
    ///
    /// O `r4` é do chamador — em ARM ele é preservado pela função chamada, então na fronteira da
    /// chamada ainda guarda o objeto de quem chamou.
    pub(super) fn send_request(&mut self, corpo: u32, tamanho: u32, saida: u32) -> Result<u32, CpuError> {
        let objeto = self.cpu.read_reg(Reg::R4);
        let mut achado = None;
        for i in 0..MAX_CAMPOS_DO_OBJETO {
            let base = objeto + i * 4;
            if self.cpu.read_u32(base + 4) == Ok(corpo)
                && self.cpu.read_u32(base + 8) == Ok(tamanho)
                && let Ok(ponteiro) = self.cpu.read_u32(base)
            {
                let texto = self.cpu.read_cstring(ponteiro, MAX_STRING);
                if texto.starts_with("http") {
                    achado = Some((texto, base));
                    break;
                }
            }
        }
        let Some((url, base)) = achado else {
            self.assumptions
                .insert("um envio foi recusado: não achei a URL no objeto de quem chamou");
            return Ok(EFAILED);
        };

        let dados = self.read_bytes(corpo, tamanho.min(MAX_CORPO_ENVIADO))?;
        if !self.network {
            self.web_requests
                .insert(format!("{url} ({tamanho} bytes, rede desligada)"));
            return Ok(EFAILED);
        }
        // A área de saída ainda não tem formato conhecido: o que o console punha ali se descobre
        // vendo o jogo ler. Guardamos a resposta e deixamos o ponteiro como está, em vez de
        // escrever um palpite de struct sobre a memória do jogo.
        let _ = saida;
        self.registra_corpo("pedido", &dados);
        let (resultado, estado) = match rede::post(&url, &dados, self.network_to.as_deref()) {
            Ok(resposta) => {
                self.web_requests.insert(format!(
                    "{url} -> {} ({} bytes de resposta)",
                    resposta.status,
                    resposta.corpo.len()
                ));
                self.registra_corpo("resposta", &resposta.corpo);
                self.web_response = resposta.corpo;
                (SUCCESS, ESTADO_RECEBENDO)
            }
            Err(erro) => {
                self.web_requests.insert(format!("{url} -> falhou: {erro}"));
                (EFAILED, ESTADO_FALHOU)
            }
        };
        // Nada é entregue aqui. Estamos no meio do despacho de uma chamada, com o jogo dentro
        // da `ConnectionManager::init`, e foi assim que a ponte derrubou o jogo: chamar o
        // alocador dele nesse instante reentra num gerenciador que está no meio de uma operação.
        //
        // O emulador já tem a fronteira certa para isso — a mesma dos sinais, que roda fora do
        // despacho, quando o guest não está dentro de nada. A resposta espera na fila até lá.
        self.pending_response = Some((objeto, base, estado));
        Ok(resultado)
    }

    /// Deposita a resposta e avisa o estado, fora do despacho.
    ///
    /// A ordem importa: os campos primeiro, o estado depois. O jogo consulta o estado a cada
    /// volta do laço e, ao vê-lo em "recebendo", lê a contagem — se avisássemos antes de
    /// entregar, ele leria zero e concluiria que não veio nada.
    pub(super) fn flush_response(&mut self) -> Result<(), CpuError> {
        // O fim do fluxo anunciado na fronteira seguinte à entrega, para dar ao jogo uma volta
        // inteira em que ele consome os campos.
        if let Some(resposta) = self.pending_end.take() {
            self.cpu.write_mem(resposta + FIM_DO_FLUXO, &[1])?;
        }
        let Some((objeto, base, estado)) = self.pending_response.take() else {
            return Ok(());
        };
        if estado == ESTADO_RECEBENDO {
            self.deliver_response(objeto)?;
        }
        self.finish_request(base, estado)
    }

    /// Entrega a resposta ao objeto que o jogo preparou para recebê-la.
    ///
    /// O remetente guarda esse objeto em `+8` do próprio (`str r2, [r4, #8]` em `0x94eb8`), e o
    /// tratador do estado "recebendo" o lê assim (`0x85b44`): `[+8]` é a contagem de campos e
    /// zero quer dizer "não veio nada", `[+4]` é o vetor de ponteiros e `[+0xc]` a capacidade.
    /// É o `ttdArray` do próprio jogo.
    ///
    /// As strings **saem do alocador do jogo**, pela [`crate::ponte`], porque é dele que o
    /// gerenciador de memória espera recebê-las de volta. Sem ponte declarada para o módulo,
    /// não entregamos nada: melhor o jogo ver "não veio resposta" do que ver memória que ele vai
    /// recusar. O que trafega não muda em nenhum dos dois casos.
    pub(super) fn deliver_response(&mut self, objeto: u32) -> Result<(), CpuError> {
        // A ponte é opcional e vem desligada. Ela mexe na memória do jogo, e uma entrega errada
        // não falha na hora: ela corrompe e quebra adiante, como aconteceu — o vetor guarda
        // objetos `ttdString`, com o comprimento em `[0]` e o texto em `[4]`, e entregar texto
        // cru fez o destrutor liberar lixo. Enquanto isso não estiver certo, o padrão é não
        // entregar: o jogo vê "não veio resposta", que é um estado que ele sabe tratar.
        if !self.bridge {
            return Ok(());
        }
        let Some(ponte) = ponte::para(self.applet_class) else {
            self.delivered.push(format!(
                "sem ponte declarada para o módulo {:#010x}",
                self.applet_class
            ));
            return Ok(());
        };
        // Corpo vazio **é** resposta: o servidor pode não ter nada a devolver. O que não pode é
        // ficar em silêncio, senão o jogo espera para sempre pelo fim do fluxo que nunca vem.
        // Então o caminho é o mesmo, com texto vazio.
        // **O corpo do `import` vem comprimido, e só o dele.** No console quem o infla é o
        // próprio jogo, com um `AEECLSID_UNZIPSTREAM` que ele cria em `0x93454` e lê em blocos
        // de 499 bytes; a ponte pula o despachante do console, então pula essa etapa junto e
        // precisa inflar aqui.
        //
        // Sem isso, os bytes comprimidos passavam por `from_utf8_lossy`, cada byte inválido
        // virava um `U+FFFD` de três bytes — 416 bytes de resposta viravam 742 de "texto" — e
        // o parser do jogo, que fatia nos `;`, achava **um** campo. O boneco chegava, o jogo
        // dizia sucesso e não gravava nada.
        //
        // Tentar inflar o que não está comprimido não estraga nada: o `export` e o
        // `synchronize` respondem texto puro, e o inflador recusa texto puro.
        // Copia, e não consome: o mesmo corpo ainda alimenta a leitura normal do `IWeb`.
        let corpo = inflate(&self.web_response).unwrap_or_else(|| self.web_response.clone());
        let texto = String::from_utf8_lossy(&corpo)
            .trim_end_matches(['\r', '\n', '\0'])
            .to_string();

        // Quem fatia é o jogo. O parser dele lê o texto de `+0x20`, anexa o pedaço que recebe e
        // divide nos `;`, preenchendo o vetor com memória do próprio alocador e ligando as
        // marcas que o consumidor espera — inclusive a de "li o que precisava", que era o que
        // faltava. Reproduzir isso à mão foi o erro anterior: entregávamos campos que ele nunca
        // reconhecia como completos, e o jogo ficava esperando para sempre.
        //
        // Então damos só o texto, no lugar onde ele o procura, e mandamos fatiar.
        let resposta = self.cpu.read_u32(objeto + 8)?;
        if resposta == 0 {
            self.delivered
                .push("recusado: o objeto de resposta não existe".to_string());
            return Ok(());
        }
        let bytes = texto.as_bytes();
        let buffer = self.alocar_no_jogo(ponte, bytes.len() as u32 + 1)?;
        if buffer == 0 {
            return Ok(());
        }
        self.cpu.write_mem(buffer, bytes)?;
        self.cpu.write_mem(buffer + bytes.len() as u32, &[0])?;

        // O que já estivesse ali é devolvido ao alocador, senão vaza.
        let anterior = self.cpu.read_u32(resposta + TEXTO_DA_RESPOSTA)?;
        if anterior != 0 {
            self.call_guest_with_stack(ponte.liberador, [anterior, 0, 0, 0], &[], PONTE_BUDGET)?;
        }
        self.cpu.write_u32(resposta + TEXTO_DA_RESPOSTA, buffer)?;

        // O segundo argumento do parser é o pedaço a anexar. O texto inteiro já está no lugar,
        // então vai uma string vazia.
        let vazio = self.alocar_no_jogo(ponte, 1)?;
        if vazio == 0 {
            return Ok(());
        }
        self.cpu.write_mem(vazio, &[0])?;
        self.call_guest_with_stack(ponte.parser, [resposta, vazio, 0, 0], &[], PONTE_BUDGET)?;

        // A marca de "chegou dado novo". O tratador em `0x85b5c` só interpreta o campo 0 quando
        // ela está ligada, e a apaga logo depois — é bandeira de uma via, e no console quem a
        // ligava era o despachante.
        self.cpu.write_mem(resposta + 0x18, &[1])?;

        // A marca de "acabou o fluxo" fica para a **próxima** fronteira, e essa espera é o ponto.
        //
        // O `ConnectionManager` recebe em pedaços: a cada um chama o parser, e só quando chega
        // um de zero bytes ele liga a marca. Entre um pedaço e outro o jogo consulta a resposta
        // e **consome os campos**. Ligá-la junto com a entrega pulava esse consumo — o tratador
        // em `0x85c9c` desvia direto para o fim quando a marca já está lá, devolve sucesso e o
        // boneco fica sem os números.
        //
        // Entregamos tudo de uma vez, então imitamos o intervalo: o texto agora, o fim depois.
        self.pending_end = Some(resposta);

        let campos = self.cpu.read_u32(resposta + 8).unwrap_or(0);
        self.delivered.push(format!(
            "entregue ao parser do jogo: {} bytes, {campos} campo(s) reconhecido(s)",
            bytes.len()
        ));
        Ok(())
    }

    /// Pede memória ao alocador do próprio jogo, pela ponte.
    ///
    /// A assinatura observada é `(tamanho, pool, linha, arquivo, 1)`, com o pool zero — que ele
    /// exige menor que 32 — e os dois do meio servindo ao rastreio de origem dele.
    pub(super) fn alocar_no_jogo(&mut self, ponte: ponte::Ponte, tamanho: u32) -> Result<u32, CpuError> {
        let outcome = self.call_guest_with_stack(
            ponte.alocador,
            [tamanho, 0, LINHA_DE_ORIGEM, ponte.origem],
            &[1],
            PONTE_BUDGET,
        )?;
        match outcome {
            Outcome::Returned { code } => Ok(code),
            _ => {
                self.assumptions
                    .insert("a resposta não foi entregue: o alocador do jogo não retornou");
                Ok(0)
            }
        }
    }

    /// Avisa o jogo que a requisição terminou.
    ///
    /// Ele não espera evento nenhum: a tela de sync **consulta um campo de estado** do objeto de
    /// quem pediu, e enquanto ele valer 0 ou 2 continua mostrando "Connecting". Com 9 ela vai
    /// para "ReceivingData" e lê a resposta; com 11 vai para o ramo de falha.
    ///
    /// O campo fica doze bytes antes da URL, no mesmo objeto — então ele é achado pela mesma
    /// âncora que já se conferiu, e não por um deslocamento solto.
    ///
    /// **A trava está aqui**: só escrevemos se o campo tiver agora um dos valores que o próprio
    /// código do jogo trata como "em andamento". Se tiver qualquer outra coisa, a âncora não é o
    /// que pensamos e não mexemos na memória dele. Escrever um palpite sobre a memória do guest
    /// é o tipo de erro que se paga caro e tarde.
    pub(super) fn finish_request(&mut self, base: u32, estado: u32) -> Result<(), CpuError> {
        let campo = base - OFFSET_ESTADO_ANTES_DA_URL;
        match self.cpu.read_u32(campo) {
            Ok(ESTADO_TRABALHANDO | ESTADO_TRABALHANDO_2) => self.cpu.write_u32(campo, estado),
            _ => {
                self.assumptions
                    .insert("o estado da conexão não foi avisado: o campo não parecia o esperado");
                Ok(())
            }
        }
    }

    /// Liga ou desliga o acesso à rede.
    pub fn set_network(&mut self, ligada: bool) {
        self.network = ligada;
    }

    /// Liga a ponte do módulo, que entrega a resposta ao jogo. Desligada por padrão.
    pub fn set_bridge(&mut self, ligada: bool) {
        self.bridge = ligada;
    }

    /// Desvia as conexões para outra máquina ou porta, sem mexer no que o jogo pediu.
    pub fn set_network_to(&mut self, destino: Option<String>) {
        self.network_to = destino;
    }

    /// As respostas que a ponte entregou ao jogo.
    pub fn delivered(&self) -> &[String] {
        &self.delivered
    }

    /// O corpo da última resposta HTTP recebida.
    pub fn web_response(&self) -> &[u8] {
        &self.web_response
    }

    pub fn web_requests(&self) -> Vec<String> {
        self.web_requests.iter().cloned().collect()
    }
}
