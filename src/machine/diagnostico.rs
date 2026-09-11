//! A introspecção: o rastreio, o despejo de falha e os contadores que a interface mostra.

use super::*;

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
        self.bad_pointers.iter().cloned().collect()
    }

    /// Liga a medição de tempo real por método de API. Ver [`Machine::api_profile`].
    pub fn enable_api_profile(&mut self) {
        self.profiling_api = true;
    }

    /// Quanto tempo real cada método de API custou, do mais caro para o mais barato.
    pub fn api_profile(&self) -> Vec<(String, u64)> {
        let mut linhas: Vec<_> = self
            .api_time
            .iter()
            .map(|(&(iface, slot), &ns)| {
                let nome = aee::Interface::from_index_public(iface)
                    .and_then(|i| i.method(slot).map(|m| format!("{}::{m}", i.name())))
                    .unwrap_or_else(|| format!("interface {iface} slot {slot}"));
                (nome, ns)
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
        match self
            .debug_output
            .iter_mut()
            .find(|(text, _)| *text == message)
        {
            Some((_, count)) => *count += 1,
            None => self.debug_output.push((message, 1)),
        }
    }

    /// Quantas vezes cada método foi chamado, em ordem — o backlog de APIs, medido.
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
    pub fn unknown_classes(&self) -> Vec<u32> {
        self.unknown_classes.iter().copied().collect()
    }

    /// Quantas instruções o guest executou.
    pub fn instructions(&self) -> u64 {
        self.cpu.instructions()
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
    pub fn gl_frame(&self) -> Option<Framebuffer> {
        if self.gl_last_frame.is_empty() {
            return None;
        }
        let (width, height) = {
            let target = self.screen();
            (target.width(), target.height())
        };
        let mut surface = Framebuffer::new(width, height);
        surface.load_rgb565_bytes(&self.gl_last_frame);
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
