//! A introspecção: o rastreio, o despejo de falha e os contadores que a interface mostra.

use super::*;

/// Quantos ponteiros ou pedidos estranhos do jogo o diagnóstico guarda antes de parar.
///
/// Ver [`Machine::anota_ponto_ruim`]: o texto vem do jogo, então o tamanho da lista não pode
/// depender do que ele pedir. 512 linhas são mais do que qualquer relatório já precisou, e o
/// excedente é descartado com aviso.
const TETO_DE_PONTEIROS_RUINS: usize = 512;

/// Quantas linhas diferentes do log do jogo ficam guardadas. Ver [`Machine::record_debug`].
const MAX_LINHAS_DO_JOGO: usize = 2000;

impl<C: CpuBackend> Machine<C> {
    /// Liga o registro de todas as chamadas, na ordem.
    pub fn set_tracing(&mut self, on: bool) {
        self.tracing = on;
    }

    /// Restringe o rastreamento às chamadas cujo nome contém `part`.
    ///
    /// Sem isso, um jogo que faz milhares de `strlen` por quadro empurra para fora da janela
    /// justamente as chamadas que se quer ver.
    pub fn set_trace_filter(&mut self, part: Option<String>) {
        self.trace_filter = part;
    }

    /// As chamadas registradas, na ordem.
    pub fn trace(&self) -> &[String] {
        &self.trace
    }

    /// Copia um trecho da memória do guest, para inspeção externa.
    ///
    /// Existe para depuração: quando o jogo quebra num ponteiro nulo, o que responde "por quê"
    /// é a struct que o levou até lá, e ela só existe na memória do guest.
    pub fn dump(&self, addr: u32, len: usize) -> Result<Vec<u8>, CpuError> {
        let mut bytes = vec![0u8; len];
        self.cpu.read_mem(addr, &mut bytes)?;
        Ok(bytes)
    }

    /// `r0..r11` no momento da última falha de memória.
    pub fn fault_regs(&self) -> [u32; 12] {
        self.fault_regs
    }

    /// Endereços de retorno encontrados na pilha da última falha.
    pub fn fault_stack(&self) -> &[u32] {
        &self.fault_stack
    }

    /// Varre a pilha à procura de endereços de retorno.
    ///
    /// Não é um backtrace exato — sem tabela de desenrolamento, o que dá para fazer é ler as
    /// palavras da pilha e ficar com as que apontam para logo depois de uma instrução de
    /// chamada. Falsos positivos aparecem, mas a cadeia real aparece junto, e é ela que
    /// responde "como cheguei aqui".
    pub(super) fn scan_stack(&self) -> Vec<u32> {
        let sp = self.cpu.read_reg(Reg::Sp);
        let top = loader::STACK_BASE + loader::STACK_SIZE as u32;
        let module = &self.module.mem.regions()[0];
        let (low, high) = (module.base, module.base + module.bytes.len() as u32);

        let mut found = Vec::new();
        let mut address = sp;
        while address < top && found.len() < STACK_DEPTH {
            let Ok(value) = self.cpu.read_u32(address) else {
                break;
            };
            address += 4;
            if value < low || value >= high || value % 4 != 0 {
                continue;
            }
            // Um endereço de retorno tem um `bl`/`blx` logo antes dele.
            let Ok(previous) = self.cpu.read_u32(value - 4) else {
                continue;
            };
            let is_call = (previous >> 24) & 0x0f == 0x0b || (previous >> 20) & 0xff == 0x12;
            if is_call {
                found.push(value);
            }
        }
        found
    }

    /// Chamadas que receberam ponteiro inválido do guest.
    pub fn bad_pointers(&self) -> Vec<String> {
        let linhas: Vec<String> = self.bad_pointers.iter().cloned().collect();
        // O relatório diz que a lista está cheia, em vez de parecer completa quando não está.
        if self.bad_pointers.len() >= TETO_DE_PONTEIROS_RUINS {
            let mut com_aviso = linhas;
            com_aviso.push(format!(
                "(lista no teto de {TETO_DE_PONTEIROS_RUINS}; o resto não foi guardado)"
            ));
            return com_aviso;
        }
        linhas
    }

    /// Registra um ponteiro ou pedido estranho do jogo, **com teto**.
    ///
    /// **O texto é escolhido pelo jogo**: o nome do arquivo pedido e o endereço entram na linha.
    /// Sem teto, esta lista cresce com o que o jogo quiser pedir — e ela existe para o relatório,
    /// não para virar memória. No teto, o excedente é descartado, e o relatório avisa.
    pub(super) fn anota_ponto_ruim(&mut self, texto: String) {
        if self.bad_pointers.len() < TETO_DE_PONTEIROS_RUINS {
            self.bad_pointers.insert(texto);
        }
    }

    /// Liga a medição de tempo por método de API. Ver [`Machine::api_profile`].
    ///
    /// A medida é **amostrada** — uma chamada em cada 64 —, e por necessidade: o relógio desta
    /// máquina é chamada de sistema, e ler o par por chamada fazia o instrumento cobrar mais que
    /// o método medido. A contagem de chamadas continua exata em [`Machine::call_log`].
    pub fn enable_api_profile(&mut self) {
        self.profiling_api = true;
        // **O preço do próprio instrumento, medido na máquina em que ele roda.** Ler o relógio
        // custa dezenas de nanossegundos onde o `vDSO` responde e mais de um microssegundo onde
        // ele não responde; sem descontar isso, um método que não faz nada aparece custando
        // 1,4 µs — e foi essa leitura que pôs o `GetClipRect` no topo de um relatório.
        self.clock_ns = Self::mede_o_relogio();
    }

    /// Quanto tempo real cada método de API custou, **estimado**, do mais caro para o mais barato.
    ///
    /// A estimativa usa a média das amostras daquele método, multiplicada pelo número de chamadas
    /// dele: nenhuma chamada é especial, então a média vale para todas. Um método com poucas
    /// amostras sai com margem larga — e é por isso que a contagem de amostras vai junto no
    /// relatório, em vez de o número aparecer sozinho.
    pub fn api_profile(&self) -> Vec<(String, u64)> {
        // O par de leituras que o perfil faz por chamada amostrada. Descontado da média antes de
        // estimar, senão o número publicado é o preço do instrumento.
        let instrumento = self.clock_ns.saturating_mul(2);
        let mut linhas: Vec<_> = self
            .api_time
            .iter()
            .map(|(&(iface, slot), &(ns, amostras))| {
                let nome = aee::Interface::from_index_public(iface)
                    .and_then(|i| i.method(slot).map(|m| format!("{}::{m}", i.name())))
                    .unwrap_or_else(|| format!("interface {iface} slot {slot}"));
                let chamadas = self
                    .call_log()
                    .iter()
                    .find(|(outro, _)| outro == &nome)
                    .map(|(_, vezes)| u64::from(*vezes))
                    .unwrap_or(amostras);
                let estimado = match amostras {
                    0 => 0,
                    // `saturating_sub`: método mais barato que o instrumento fica em zero, e zero
                    // é a resposta certa. Negativo não existe, e "quase zero" não se distingue do
                    // ruído desta medida.
                    n => (ns / n).saturating_sub(instrumento) * chamadas.max(n),
                };
                (nome, estimado)
            })
            .collect();
        linhas.sort_unstable_by_key(|linha| std::cmp::Reverse(linha.1));
        linhas
    }

    /// Põe um corpo de rede na captura de serial, em texto quando dá e em hexadecimal quando
    /// não dá — e, se for `deflate`, também o conteúdo inflado.
    ///
    /// Só a URL, o status e o tamanho iam para o relatório, e isso não basta para depurar um
    /// protocolo: o que estraga um registro é **o conteúdo**, campo a campo. Um Zeeboid chegou
    /// do servidor com os campos deslocados de uma casa e a senha truncada em dezesseis
    /// caracteres emendada no IMEI, e sem os bytes não há como dizer se quem errou foi o
    /// servidor, o transporte ou a leitura do jogo.
    ///
    /// Vai só para a serial, que é opcional: o corpo pode trazer IMEI e senha, e isso não entra
    /// num relatório que se manda por aí sem querer.
    pub(super) fn registra_corpo(&mut self, que: &str, bytes: &[u8]) {
        /// Quanto de um corpo cabe no registro.
        const TETO: usize = 4096;

        if self.serial.is_none() {
            return;
        }
        let mostrar = |dados: &[u8]| -> String {
            let corte = &dados[..dados.len().min(TETO)];
            match corte
                .iter()
                .all(|&b| b == b'\n' || (0x20..0x7f).contains(&b))
            {
                true => String::from_utf8_lossy(corte).into_owned(),
                false => corte.iter().map(|b| format!("{b:02x}")).collect(),
            }
        };
        let mut linha = format!("<{que} {} bytes: {}>", bytes.len(), mostrar(bytes));
        if let Some(inflado) = inflate(bytes) {
            linha.push_str(&format!("\n<{que} inflado: {}>", mostrar(&inflado)));
        }
        self.registra_serial(linha);
    }

    /// Liga a captura de serial: cada linha de log vai para este arquivo, na ordem e com o
    /// instante do relógio virtual.
    ///
    /// O Zeebo tem uma UART de depuração, e o que sai por ela é o `DBGPRINTF` — o módulo não
    /// fala com o hardware, quem roteia é o firmware. Então o fluxo que um cabo de serial
    /// veria é exatamente este.
    ///
    /// Existe separado do relatório porque o relatório **agrupa repetições**, e agrupar perde
    /// as duas coisas que uma análise precisa: a ordem em que as linhas saíram e o intervalo
    /// entre elas. `Couldn't create z-pad instruction form (6)   (2153x)` diz que aconteceu
    /// duas mil vezes; não diz que aconteceu a cada 70 ms, nem o que veio antes da primeira.
    pub fn liga_serial(&mut self, caminho: &std::path::Path) -> std::io::Result<()> {
        let arquivo = std::fs::File::create(caminho)?;
        self.serial = Some(std::io::BufWriter::new(arquivo));
        Ok(())
    }

    /// Escreve **só** na captura de serial, sem passar pelo relatório.
    ///
    /// A instrumentação — classes criadas, bancos abertos, SQL — é ferramenta, não coisa que o
    /// jogo disse. Mandá-la pelo `record_debug` a punha no "log do jogo" do relatório, onde ela
    /// se mistura com o que o jogo de fato imprimiu e atrapalha justamente quem está lendo para
    /// entender o jogo.
    pub(super) fn registra_serial(&mut self, message: String) {
        let agora = self.now_ms();
        if let Some(serial) = self.serial.as_mut() {
            use std::io::Write;
            let _ = writeln!(serial, "[{agora:>9} ms] {message}");
            let _ = serial.flush();
        }
    }

    /// Guarda uma linha de log, agrupando repetições em vez de encher o relatório.
    ///
    /// Com a serial ligada, a linha também vai crua para o arquivo. Ver [`Machine::liga_serial`].
    pub(super) fn record_debug(&mut self, message: String) {
        self.procura_calibracao(&message);
        // O mesmo relógio que o resto do emulador reporta: o `clock_us` sozinho ignora o
        // tempo que as instruções gastaram, e a serial ficaria atrasada em relação ao rastro.
        let agora = self.now_ms();
        if let Some(serial) = self.serial.as_mut() {
            use std::io::Write;
            // Falha de escrita não pode derrubar o jogo: a serial é instrumento, não emulação.
            let _ = writeln!(serial, "[{:>9} ms] {message}", agora);
            // **Descarrega a cada linha.** Sem isto o arquivo fica vazio enquanto a sessão corre
            // — o `BufWriter` só escreve quando enche ou quando é destruído —, e uma captura que
            // só aparece depois de fechar o jogo não serve para acompanhar o que está
            // acontecendo. É o uso inteiro da ferramenta.
            let _ = serial.flush();
        }
        if let Some(&indice) = self.debug_indice.get(&message) {
            self.debug_output[indice].1 += 1;
            return;
        }
        // **Com teto.** Uma linha que traz um número que muda — quadro, tempo, fps — é uma
        // linha nova a cada chamada, e a lista crescia pela sessão inteira. Passando do teto,
        // a metade mais antiga sai: o que interessa num log é o que aconteceu por último.
        if self.debug_output.len() >= MAX_LINHAS_DO_JOGO {
            self.debug_output.drain(..MAX_LINHAS_DO_JOGO / 2);
            self.debug_indice = self
                .debug_output
                .iter()
                .enumerate()
                .map(|(indice, (texto, _))| (texto.clone(), indice))
                .collect();
        }
        self.debug_indice
            .insert(message.clone(), self.debug_output.len());
        self.debug_output.push((message, 1));
    }

    /// Quantas vezes cada método foi chamado, em ordem — o backlog de APIs, medido.
    /// Quantas leituras de posição acharam algum eixo fora do centro. Ver o campo.
    pub fn leituras_com_eixo_deslocado(&self) -> u64 {
        self.eixos_deslocados
    }

    /// Quais eixos foram vistos fora do centro, pelos nomes do console: `X` e `Y` são os do
    /// manche esquerdo, `Z` e `RZ` os do direito.
    pub fn eixos_vistos_deslocados(&self) -> Vec<&'static str> {
        crate::input::AXIS_NAMES
            .iter()
            .enumerate()
            .filter(|(indice, _)| self.mascara_de_eixos_deslocados & (1 << indice) != 0)
            .map(|(_, nome)| *nome)
            .collect()
    }

    pub fn call_log(&self) -> Vec<(String, u64)> {
        self.calls
            .iter()
            .map(|(&(iface, slot), &count)| (aee::describe(aee::encode_raw(iface, slot)), count))
            .collect()
    }

    /// Acessos inválidos que a execução seguiu por cima. Ver [`Machine::execute`].
    pub fn swallowed_faults(&self) -> Vec<String> {
        self.falhas_engolidas.iter().cloned().collect()
    }

    /// As APIs que faltaram durante a execução, inclusive dentro de retornos de chamada.
    /// O que cada classe de widget recebeu no acessador, por seletor, em ordem de classe.
    ///
    /// Serve para responder "o que esta classe proprietária espera?" **pelo uso**: a família dos
    /// widgets é atendida em bloco (por vizinhança de numeração), e a diferença entre as classes
    /// dela não está em header nenhum — está no que cada uma recebe. A `0x01028e19` é o caso que
    /// motivou o instrumento.
    pub fn liga_censo_de_widgets(&mut self) {
        self.censo_de_widgets = true;
    }

    pub fn seletores_por_classe(&self) -> Vec<(u32, u32, u32)> {
        self.seletores_por_classe
            .iter()
            .map(|(&(classe, seletor), &vezes)| (classe, seletor, vezes))
            .collect()
    }

    pub fn missing_apis(&self) -> Vec<String> {
        self.missing_apis.iter().cloned().collect()
    }

    /// Quantos objetos vivos de cada interface. Ver [`crate::brew::objects::ObjectStore::live_by_kind`].
    pub fn live_objects_by_kind(&self) -> Vec<(&'static str, usize)> {
        self.objects
            .live_by_kind()
            .into_iter()
            .map(|(iface, quantos)| (iface.name(), quantos))
            .collect()
    }

    /// ClassIDs pedidos que ainda não sabemos instanciar.
    /// As chamadas ao `IFileMgr`, em ordem, com o caminho e o retorno.
    ///
    /// Existe porque uma tela de erro do jogo não diz **qual** verificação falhou: o Double Dragon
    /// mostra "Memory is insufficient" quando qualquer consulta de espaço ou de arquivo não
    /// responde o que ele espera, e sem esta lista não há como saber o que ele perguntou.
    pub fn fs_log(&self) -> impl Iterator<Item = &String> {
        self.fs_log.iter()
    }

    /// As classes que o jogo pediu, com quantas vezes — conhecidas e desconhecidas.
    ///
    /// A lista das **desconhecidas** não basta para achar um defeito de interface: o Double Dragon
    /// pedia `AEECLSID_FONT_STANDARD*`, nós respondíamos "classe desconhecida", e o sintoma era uma
    /// tela de falta de memória. Depois de atender as fontes, o pedido saiu da lista — e sem esta
    /// lista completa não dá para ver que a classe foi atendida com a **interface errada**.
    pub fn requested_classes(&self) -> Vec<(u32, u32)> {
        self.classes_pedidas.iter().map(|(c, n)| (*c, *n)).collect()
    }

    pub fn unknown_classes(&self) -> Vec<u32> {
        self.unknown_classes.iter().copied().collect()
    }

    /// Quantas instruções o guest executou.
    pub fn instructions(&self) -> u64 {
        self.cpu.instructions()
    }

    /// Entradas no JIT e tempo gasto dentro dele, quando o backend mede.
    ///
    /// Serve para separar guest de despacho: `chamadas` conta as APIs, e o tempo de relógio que
    /// **não** está aqui dentro é trampolim mais corpo do método. Ver [`Machine::api_profile`]
    /// para o que acontece do lado de fora, e o relatório da varredura para a razão entre os dois.
    pub fn relato_do_jit(&self) -> Option<(u64, u64, u64)> {
        self.cpu.relato_do_jit()
    }

    /// Quantos quadros o jogo apresentou pelo OpenGL.
    ///
    /// Só as trocas de buffer, porque é isso que o laço de quadros usa para saber que há coisa
    /// nova para mostrar. Para o indicador da janela, ver [`Machine::quadros`].
    pub fn gl_swaps(&self) -> u32 {
        self.egl_swaps
    }

    /// Quantos quadros o jogo desenhou, para quem quer mostrar uma taxa.
    ///
    /// **Nem todo jogo apresenta trocando buffer.** A Z-Wheel desenha o palco num pbuffer e
    /// pega o resultado pelo `eglGetColorBufferQUALCOMM` para compor com o 2D: ela nunca chama
    /// `eglSwapBuffers`, e enquanto a taxa contava só trocas o indicador da janela marcava zero
    /// com o palco girando na tela.
    ///
    /// Sem troca, o que delimita um quadro é o `glClear` da cor — o começo do desenho seguinte.
    /// Contar começos e contar fins dá a mesma taxa, que é o que o indicador mostra.
    pub fn quadros(&self) -> u32 {
        match self.egl_swaps {
            0 => self.gl_clears,
            trocas => trocas,
        }
    }

    /// O último quadro que o jogo apresentou pelo OpenGL, se houve algum.
    ///
    /// Sai separado da tela porque os dois desenhos convivem: o `IDisplay` continua pintando
    /// por cima entre um `eglSwapBuffers` e o seguinte, e no fim quem escreveu por último é
    /// quem aparece. Ver o quadro do OpenGL sozinho é o que diz se a renderização 3D está
    /// certa.
    pub fn gl_frame(&mut self) -> Option<Framebuffer> {
        // Quem pergunta pelo quadro do OpenGL quer **os pixels**, então aqui a leitura pendente
        // acontece: sem isto o diagnóstico mostraria o quadro da vez anterior.
        self.materializa_quadro_gl();
        if self.gl_last_frame_words.is_empty() {
            return None;
        }
        let (width, height) = {
            let target = self.screen();
            (target.width(), target.height())
        };
        let mut surface = Framebuffer::new(width, height);
        surface.load_rgb565_words(&self.gl_last_frame_words);
        Some(surface)
    }

    /// Quantas superfícies de desenho existem.
    pub fn bitmap_count(&self) -> usize {
        self.bitmaps.len()
    }

    /// Mensagens que o jogo mandou para o log, com a contagem de repetições.
    pub fn debug_output(&self) -> &[(String, u64)] {
        &self.debug_output
    }

    /// Palpites em uso nesta execução.
    pub fn assumptions(&self) -> Vec<&'static str> {
        self.assumptions.iter().copied().collect()
    }

    /// Ponteiros `this` que não batem com a interface chamada.
    pub fn suspicious_objects(&self) -> Vec<(u32, u32)> {
        self.suspicious_objects.iter().copied().collect()
    }

    pub fn live_objects(&self) -> usize {
        self.objects.live_count()
    }
}
