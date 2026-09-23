//! IHID e o teclado: as portas, os eixos e a entrega das teclas.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `IHID` e `IHIDDevice` — o gamepad do Zeebo.
    ///
    /// Apresentamos um controle sempre conectado, com os doze botões e os quatro eixos que o
    /// `hid_devices.cfg` do console descreve. O que o jogador aperta chega por
    /// [`Machine::set_pad`].
    pub(super) fn hid_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            // `IHID` herda de `IQI`, então AddRef e Release ficam nos mesmos slots 0 e 1.
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.portas_de_aparelho.remove(&this);
                }
                restantes
            }
            // QueryInterface: devolvemos o próprio objeto, já que cada objeto nosso tem uma
            // interface só.
            "QueryInterface" => {
                if a2 != 0 {
                    self.cpu.write_u32(a2, this)?;
                }
                SUCCESS
            }
            // GetConnectedDevices(int nDeviceType, int *pnHandles, int nLen, int *pnLenReq).
            // O quarto argumento não cabe nos registradores e vem da pilha.
            "GetConnectedDevices" => {
                let wanted = a1;
                let handles = a2;
                let capacity = a3;
                let out_needed = self.stack_arg(0)?;
                // O `nLenReq` é o que o aparelho **tem**, e o vetor recebe o que couber. A
                // Z-Wheel passa capacidade dois nas duas chamadas, que é o número de USB do
                // console.
                let quais = match wanted {
                    // Um receptor atende os dois Boomerangs, então ele entra uma vez só, na
                    // primeira porta que tem um.
                    UID_JOYSTICK_DEVICE => {
                        let mut portas =
                            self.portas_com(crate::input::bindings::Aparelho::Controle);
                        portas.extend(self.portas_com(crate::input::bindings::Aparelho::ZPad));
                        portas.extend(
                            self.portas_com(crate::input::bindings::Aparelho::Boomerang)
                                .first(),
                        );
                        portas.sort_unstable();
                        portas
                    }
                    UID_KEYBOARD_DEVICE => {
                        self.portas_com(crate::input::bindings::Aparelho::Teclado)
                    }
                    _ => Vec::new(),
                };
                if out_needed != 0 {
                    self.cpu.write_u32(out_needed, quais.len() as u32)?;
                }
                if handles != 0 {
                    for (i, &porta) in quais.iter().take(capacity as usize).enumerate() {
                        self.cpu
                            .write_u32(handles + i as u32 * 4, handle_da_porta(porta))?;
                    }
                }
                SUCCESS
            }
            // CreateDevice(int nDevHandle, IHIDDevice **ppDevice)
            //
            // O identificador é o que saiu do `GetConnectedDevices`, e é aqui que ele vira
            // porta: sem guardar essa ligação, os dois aparelhos leriam o mesmo controle.
            "CreateDevice" => {
                let device = self.new_object(Interface::HidDevice)?;
                if device == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                let porta = (a1.saturating_sub(1) as usize).min(input::PORTAS - 1);
                self.portas_de_aparelho.insert(device, porta);
                if a2 != 0 {
                    self.cpu.write_u32(a2, device)?;
                }
                SUCCESS
            }
            // GetDeviceInfo(AEEHIDDeviceInfo *pInfo): { AEEUID type; uint16 pid; uint16 vid;
            // boolean bluetooth }. Em IHID a struct vem no segundo argumento.
            //
            // O `type` é o **UID do tipo de dispositivo**, o mesmo que o jogo passa ao
            // `GetConnectedDevices`, e não um número pequeno. O Bad Dudes vs. DragonNinja
            // compara o campo com `0x0106c3fd` antes de criar o aparelho: com o `1` que
            // respondíamos ele nunca chamava o `CreateDevice`, e o menu não via botão nenhum.
            "GetDeviceInfo" => {
                let out = if iface == Interface::Hid { a2 } else { a1 };
                // No `IHID` o identificador da porta vem em `r1`; no `IHIDDevice` é o próprio
                // objeto que diz de qual porta ele é.
                let porta = match iface == Interface::Hid {
                    true => (a1.saturating_sub(1) as usize).min(input::PORTAS - 1),
                    false => self.porta_do(this),
                };
                let tipo = match self.portas[porta] {
                    Some(crate::input::bindings::Aparelho::Teclado) => UID_KEYBOARD_DEVICE,
                    _ => UID_JOYSTICK_DEVICE,
                };
                let (vendedor, produto) = match self.portas[porta] {
                    Some(crate::input::bindings::Aparelho::Boomerang) => {
                        (BOOMERANG_VENDOR_ID, BOOMERANG_PRODUCT_ID)
                    }
                    Some(crate::input::bindings::Aparelho::ZPad) => (ZPAD_VENDOR_ID, ZPAD_PRODUCT_ID),
                    _ => (GAMEPAD_VENDOR_ID, GAMEPAD_PRODUCT_ID),
                };
                if out != 0 {
                    self.cpu.write_u32(out, tipo)?;
                    self.cpu.write_mem(out + 4, &produto.to_le_bytes())?;
                    self.cpu.write_mem(out + 6, &vendedor.to_le_bytes())?;
                    self.cpu.write_u32(out + 8, 0)?;
                }
                SUCCESS
            }
            "GetNumberOfButtons" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, input::BUTTON_UIDS.len() as u32)?;
                }
                SUCCESS
            }
            // GetButtonInfo(int nButtonID, AEEHIDButtonInfo *pInfo):
            // { id, estado, UID, mínimo, máximo }
            "GetButtonInfo" => {
                // O jogo passa um UID, não um índice — aceitamos as duas formas.
                let (index, uid) = match input::BUTTON_UIDS.iter().position(|&u| u == a1) {
                    Some(i) => (i as u32, a1),
                    None => match input::BUTTON_UIDS.get(a1 as usize) {
                        Some(&uid) => (a1, uid),
                        None => return Ok(Some(EBADPARM)),
                    },
                };
                // O estado tem de ser o de agora: um jogo que consulta em vez de esperar o
                // evento só enxerga a tecla por aqui.
                let state = u32::from(self.pads[self.porta_do(this)].is_down(index as usize));
                if a2 != 0 {
                    for (i, value) in [index, state, uid, 0, 1].iter().enumerate() {
                        self.cpu.write_u32(a2 + i as u32 * 4, *value)?;
                    }
                }
                SUCCESS
            }
            "GetDeviceStatus" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, HID_STATUS_CONNECTED)?;
                }
                SUCCESS
            }
            // A posição corrente de cada eixo.
            "GetPositionState" if self.e_boomerang(self.porta_do(this)) => {
                self.pacote_do_boomerang(a1)?;
                SUCCESS
            }
            "GetPositionState" => {
                let pad = self.pads[self.porta_do(this)];
                let axes = [0, 1, 2, 3].map(|i| pad.eixo_do_console(i));
                self.write_position_info(a1, &axes)?;
                SUCCESS
            }
            // `GetAxesInfo` não devolve valores: devolve, em cada campo, o UID do eixo que
            // ocupa aquele campo. É assim que o `AEEHIDThumbsticks.c` do SDK descobre onde
            // está cada direção — e enquanto respondíamos zeros, ele não achava nenhuma.
            "GetAxesInfo" => {
                // O receptor do Boomerang troca Z e RZ em relação ao controle, como diz a entrada
                // dele no `hid_devices.cfg`. O Crash Nitro Kart lê o acelerômetro pelos analógicos
                // e acha a gravidade pelo UID: com a tabela do controle, ela caía no campo errado e
                // a calibração nunca aceitava a leitura.
                let uids = match self.e_boomerang(self.porta_do(this)) {
                    true => [0x0106_c4d0, 0x0106_c4d1, 0x0106_c4cf, 0x0106_c4ce],
                    false => input::AXIS_UIDS,
                };
                self.write_axes_info(a1, &uids.map(|uid| uid as i32))?;
                SUCCESS
            }
            // Os limites valem para **todos** os eixos da struct, não só os quatro que o
            // console usa. Preenchendo apenas quatro, os outros vinte ficavam com mínimo e
            // máximo iguais a zero, e um jogo que normalize um eixo desses — `(valor - min) /
            // (max - min)` — divide por zero e trava a direção num extremo. Era o que prendia
            // o carro do Crash virando para a esquerda.
            "GetMinPositionInfo" => {
                self.write_axis_range(a1, input::AXIS_MIN)?;
                SUCCESS
            }
            "GetMaxPositionInfo" => {
                self.write_axis_range(a1, input::AXIS_MAX)?;
                SUCCESS
            }
            // GetNextButtonEvent(AEEHIDButtonInfo *, uint32 *pdwTimestamp, boolean *pbDropped).
            //
            // A fila é de eventos, não de estado: cada aperto e cada soltura vira uma entrada,
            // e o jogo lê até a fila esvaziar. `EFAILED` é o "não há mais nada".
            "GetNextButtonEvent" => {
                let Some((index, down)) = self.pad_events[self.porta_do(this)].pop_front() else {
                    if a1 != 0 {
                        self.cpu.write_mem(a1, &[0u8; 20])?;
                    }
                    if a2 != 0 {
                        self.cpu.write_u32(a2, self.elapsed_ms())?;
                    }
                    if a3 != 0 {
                        self.cpu.write_u32(a3, 0)?;
                    }
                    return Ok(Some(EFAILED));
                };
                let uid = input::BUTTON_UIDS.get(index).copied().unwrap_or(0);
                if a1 != 0 {
                    // `AEEHIDButtonInfo`: id, estado, UID, mínimo, máximo.
                    let info = [index as u32, u32::from(down), uid, 0, 1];
                    for (i, value) in info.iter().enumerate() {
                        self.cpu.write_u32(a1 + i as u32 * 4, *value)?;
                    }
                }
                if a2 != 0 {
                    self.cpu.write_u32(a2, self.elapsed_ms())?;
                }
                if a3 != 0 {
                    self.cpu.write_u32(a3, 0)?;
                }
                SUCCESS
            }
            // GetNextConnectEvent(int *pnDevHandle, int *pnStatus, boolean *pbDropped).
            // GetNextConnectEvent(uint32 *pdwHandle, boolean *pbConnected, uint32 *pdwTimestamp)
            //
            // **A fila está sempre vazia, e vazia responde `EFAILED`** — é o "não há mais evento"
            // do BREW, o mesmo do `GetNextButtonEvent`. Respondendo sucesso com os campos
            // zerados, o jogo entendia que havia um evento de conexão a cada pergunta: os Zeebo
            // Extreme ficavam em `GetNextConnectEvent` e `GetDeviceInfo` para sempre, sem armar
            // timer nem desenhar, e a sessão terminava por falta do que fazer.
            "GetNextConnectEvent" => {
                for out in [a1, a2, a3] {
                    if out != 0 {
                        self.cpu.write_u32(out, 0)?;
                    }
                }
                EFAILED
            }
            // Os `RegisterFor*` recebem um `ISignal` que devemos disparar quando houver evento.
            // Guardamos qual é; disparar de fato depende de ligar a entrada do host.
            "RegisterForConnectEvents"
            | "RegisterForStatusChange"
            | "RegisterForButtonEvent"
            | "RegisterForPositionChange" => {
                if a1 != 0 {
                    self.input_signals.insert((name, self.porta_do(this)), a1);
                }
                // **O registro de posição já vale um aviso.** No console o controle está
                // conectado e parado quando o jogo registra, e o driver entrega logo a
                // primeira posição; o jogo usa esse aviso para perguntar a faixa dos eixos
                // (`GetMinPositionInfo`, `GetMaxPositionInfo`) e onde eles estão. Enquanto o
                // aviso só saía no primeiro movimento, quem nunca encostasse no analógico
                // jogava com os eixos sem calibrar — o Ridge Racer chega ao título e não
                // pergunta nada antes disso.
                if name == "RegisterForPositionChange" {
                    self.raise_input_signal(name, self.porta_do(this));
                }
                SUCCESS
            }
            "SetExclusiveLevel" | "Rumble" => SUCCESS,
            "GetExclusiveLevel" | "GetRumbleStatus" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, 0)?;
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Preenche um `AEEHIDPositionInfo` com a posição de cada eixo.
    ///
    /// A struct é `boolean bRelativeAxes` seguido de vinte e quatro inteiros, um por eixo
    /// possível. O controle do Zeebo usa quatro deles — `X`, `Y`, `Z` e `RZ`.
    ///
    /// **Os outros vinte ficam no centro, não em zero.** Zero é o mínimo da faixa do aparelho,
    /// e um jogo que leia um eixo que não usamos o encontraria encostado no batente em vez de
    /// parado. Os eixos são absolutos, então `bRelativeAxes` continua zero.
    pub(super) fn write_position_info(
        &mut self,
        addr: u32,
        values: &[i32; 4],
    ) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let mut words = [input::AXIS_CENTRO as u32; input::POSITION_INFO_WORDS];
        words[0] = 0;
        for (slot, value) in input::AXIS_SLOTS.iter().zip(values) {
            words[*slot] = *value as u32;
        }
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes).map_err(|e| {
            CpuError(format!(
                "{e} ao escrever {} bytes em {addr:#010x}",
                bytes.len()
            ))
        })
    }

    /// Escreve a tabela do `GetAxesInfo`: em cada campo, o UID do eixo que o ocupa.
    ///
    /// Aqui os campos que sobram ficam **zerados**, e é o contrário do que faz a posição: zero
    /// não é um valor de eixo, é "não há eixo neste campo", e é assim que o jogo para de
    /// procurar.
    pub(super) fn write_axes_info(
        &mut self,
        addr: u32,
        uids: &[i32; 4],
    ) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let mut words = [0u32; input::POSITION_INFO_WORDS];
        for (slot, uid) in input::AXIS_SLOTS.iter().zip(uids) {
            words[*slot] = *uid as u32;
        }
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes)
    }

    /// Escreve `value` em todos os campos de eixo do `AEEHIDPositionInfo`.
    ///
    /// O primeiro campo da struct não é eixo: é o `boolean bRelativeAxes`, e ele fica em zero —
    /// os eixos do controle são absolutos, e dizer o contrário faria o jogo tratar cada leitura
    /// como um deslocamento e acumular.
    pub(super) fn write_axis_range(&mut self, addr: u32, value: i32) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let mut words = [value as u32; input::POSITION_INFO_WORDS];
        words[0] = 0;
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes)
    }

    /// Entrega um novo estado do controle.
    ///
    /// Cada botão que mudou vira um evento na fila, e a mudança acorda o jogo pelos `ISignal`
    /// que ele registrou — sem isso o jogo só veria a tecla na próxima vez que resolvesse
    /// perguntar, e alguns nunca perguntam.
    pub fn set_pad(&mut self, pad: Pad) {
        self.set_port_pad(0, pad);
    }

    /// O mesmo, para uma porta escolhida.
    pub fn set_port_pad(&mut self, porta: usize, pad: Pad) {
        if porta >= input::PORTAS || pad == self.pads[porta] {
            return;
        }
        let changes = self.pads[porta].changes(&pad);
        let moved = pad.axes != self.pads[porta].axes;
        self.pads[porta] = pad;
        self.pad_events[porta].extend(changes.iter().copied());
        // Só o `GetNextButtonEvent` esvazia esta fila, e o jogo que lê o controle por
        // `GetState` ou por `EVT_KEY` nunca o chama: ela crescia a cada toque pela sessão
        // inteira. Os mais antigos saem primeiro, que é o que uma fila de hardware faz ao
        // transbordar.
        let excesso = self.pad_events[porta].len().saturating_sub(PAD_EVENTS_MAX);
        self.pad_events[porta].drain(..excesso);
        let agora = self.elapsed_ms();
        for &(index, down) in &changes {
            if self.pad_log.len() == PAD_LOG_MAX {
                self.pad_log.pop_front();
            }
            self.pad_log.push_back((agora, porta, index, down));
        }

        if !changes.is_empty() {
            self.raise_input_signal("RegisterForButtonEvent", porta);
        }
        // **O retorno ao centro também é mudança de posição.** Soltar o manche precisa acordar
        // o callback como empurrá-lo: quem lê o eixo só de dentro do callback — e é como um
        // menu orientado a evento se escreve — guarda a última direção e nunca descobre que o
        // jogador soltou. O sintoma é o manche "preso" no último sentido para sempre. A
        // navegação dobrada que isto parecia causar era outra coisa, e está resolvida em
        // [`Machine::raise_input_signal`].
        if moved {
            self.raise_input_signal("RegisterForPositionChange", porta);
        }
    }

    /// A aceleração de uma porta com Boomerang, em g no referencial dele: `x` ao longo do
    /// controle, `y` para a frente e `z` saindo da face dos botões. Parado com a face para cima,
    /// `[0, 0, 1]`.
    pub fn set_port_motion(&mut self, porta: usize, aceleracao: [f32; 3]) {
        if let Some(movimento) = self.movimento.get_mut(porta) {
            *movimento = aceleracao;
        }
    }

    /// O receptor do Boomerang manda relatório sem parar, e cada um acorda quem registrou
    /// `RegisterForPositionChange`, mesmo com a aceleração igual. O Crash Nitro Kart só lê a
    /// posição dentro desse aviso: sem ele, a calibração recebia uma leitura e esperava as outras
    /// para sempre. O ritmo é o do tempo virtual, e não o de quem entrega o movimento.
    pub(super) fn relatorio_do_boomerang(&mut self) {
        let agora = self.now_us();
        if agora.saturating_sub(self.ultimo_relatorio_boomerang_us) < BOOMERANG_PERIODO_US {
            return;
        }
        self.ultimo_relatorio_boomerang_us = agora;
        for porta in 0..input::PORTAS {
            if self.e_boomerang(porta) {
                self.raise_input_signal("RegisterForPositionChange", porta);
            }
        }
    }

    /// `(começadas, terminadas)`: as calibrações do movimento que o jogo fez até agora.
    ///
    /// Nenhum jogo avisa o sistema, então isto é o que dá para ver de fora: as mensagens de
    /// depuração do jogo ("calib" ou o `ACCEL 1` do Crash Nitro Kart começam; o `ACCEL…CENTER`
    /// dele termina). A
    /// interface completa o resto olhando se o controle está parado.
    ///
    /// **A primeira leitura do Boomerang não serve de sinal.** Todo jogo lê o controle na
    /// abertura, e os da Boomerang Sports só calibram depois da escolha de personagem: o aviso
    /// abria no lugar errado. Eles também não deixam rastro de texto na hora — o nome das telas
    /// de calibração só passa pelas funções do BREW na pré-carga —, e o estado da calibração fica
    /// num objeto cujo layout muda de jogo para jogo.
    pub fn calibracao(&self) -> (u32, u32) {
        self.calibracoes
    }

    /// Lê uma mensagem de depuração do jogo à procura de calibração.
    pub(super) fn procura_calibracao(&mut self, mensagem: &str) {
        let minuscula = mensagem.to_ascii_lowercase();
        // O Crash Nitro Kart escreve "ACCEL 1" ao reconhecer o Boomerang, e abre a calibração em
        // seguida.
        if minuscula.contains("calib") || matches!(mensagem.trim(), "ACCEL 1" | "ACCEL 2") {
            self.calibracoes.0 += 1;
        }
        if minuscula.contains("center =") {
            self.calibracoes.1 += 1;
        }
    }

    pub(super) fn e_boomerang(&self, porta: usize) -> bool {
        self.portas.get(porta).copied().flatten() == Some(crate::input::bindings::Aparelho::Boomerang)
    }

    /// Escreve no `AEEHIDPositionInfo` um pacote do receptor do Boomerang.
    ///
    /// O Boomerang não usa a estrutura como eixos: ela é o relatório bruto do receptor, e o
    /// jogo o desmonta. Lido no Zeebo Sports Queimada (`0x4231c`, `0x42458`, `0x40b0c`,
    /// `0x413d0`, `0x412f4`), com os campos contados a partir do primeiro inteiro depois do
    /// `bRelativeAxes`:
    ///
    /// - **Campos 1, 2 e 3: o acelerômetro**, um byte por eixo centrado em `0x80`. O jogo gira X
    ///   e Y em 20° (`0,94` e `0,342`) antes de usar — o sensor fica torto dentro do braço — e
    ///   calibra zero e escala com o controle parado de face para cima e depois para baixo.
    /// - **Campo 4: os botões**, em lógica invertida (bit em zero é apertado): bit 0 o botão 1,
    ///   1 o botão 2, 2 o HOME, 3 esquerda, 4 baixo, 5 direita e 6 cima.
    /// - **Campo 5: o jogador e o tipo.** O bit 0 diz de qual dos dois Boomerangs é o pacote; o
    ///   resto é o tipo. Tipo 0 é "desconectado"; de 1 a 4 conecta, e o campo 6 leva um dado do
    ///   aparelho que muda com o tipo.
    /// - **Campo 8: o contador**, de 8 bits. O jogo espera que ele ande de um em um; um pulo
    ///   marca pacote perdido, e na partida ele espera o contador mudar para dar o receptor como
    ///   vivo. Parado, o jogo fica preso nesse laço.
    ///
    /// Um pacote novo a cada [`BOOMERANG_PERIODO_US`], alternando os dois jogadores.
    pub(super) fn pacote_do_boomerang(&mut self, addr: u32) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        // 1 g em unidades do sensor. A calibração do Crash Nitro Kart só aceita a gravidade entre
        // 162 e 220, o que centra 1 g perto de 63; o Queimada normaliza pela leitura parada.
        const ESCALA: f32 = 60.0;
        const COS: f32 = 0.94;
        const SEN: f32 = 0.342;
        // **Um pacote novo por período do receptor, não por leitura.** O Queimada lê três vezes por
        // quadro e só processa os botões de um jogador numa leitura sem pacote novo dele: com um
        // pacote a cada leitura, nenhum aperto chegava. Dentro do período, a leitura repete o
        // último pacote.
        let agora = self.now_us();
        if agora.saturating_sub(self.ultimo_pacote_boomerang_us) >= BOOMERANG_PERIODO_US {
            self.ultimo_pacote_boomerang_us = agora;
            self.boomerang_sequencia = self.boomerang_sequencia.wrapping_add(1);
        }
        let sequencia = self.boomerang_sequencia;
        // Os dois jogadores se alternam sempre; o que não tem controle vai como "desconectado".
        // O Queimada precisa dessa alternância para processar os botões.
        let boomerangs = self.portas_com(crate::input::bindings::Aparelho::Boomerang);
        let jogador = usize::from(sequencia & 1);
        let porta = boomerangs.get(jogador).copied();
        let mut campos = [0u32; input::POSITION_INFO_WORDS];
        // **O jogador desconectado leva a aceleração do primeiro.** O Crash Nitro Kart lê o
        // acelerômetro sem olhar de quem é o pacote: com o segundo jogador entrando parado a cada
        // relatório, a direção pulava entre o movimento e o repouso, o kart virava a esmo e a
        // calibração passava sem esperar. O Queimada ignora a aceleração de quem está desconectado.
        let [x, y, z] = porta
            .or(boomerangs.first().copied())
            .map_or([0.0, 0.0, 1.0], |p| self.movimento[p]);
        // O inverso do giro que o jogo aplica: assim ele chega à aceleração que entregamos.
        let bruto = [COS * x - SEN * y, SEN * x + COS * y, z];
        for (campo, valor) in campos[1..4].iter_mut().zip(bruto) {
            *campo = (0x80 as f32 + valor * ESCALA).round().clamp(0.0, 255.0) as u32;
        }
        let mut soltos = 0x7fu32;
        if let Some(porta) = porta {
            let pad = self.pads[porta];
            const BITS: [(&str, u32); 7] = [
                ("b1", 0),
                ("b2", 1),
                ("back", 2),
                ("left", 3),
                ("down", 4),
                ("right", 5),
                ("up", 6),
            ];
            for (nome, bit) in BITS {
                if Pad::button_by_name(nome).is_some_and(|i| pad.is_down(i)) {
                    soltos &= !(1 << bit);
                }
            }
        }
        campos[4] = soltos;
        let tipo = match porta {
            Some(_) => 1,
            None => 0,
        };
        campos[5] = (tipo << 1) | jogador as u32;
        campos[8] = u32::from(sequencia);
        let bytes: Vec<u8> = campos.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes)
    }

    /// A porta de um `IHIDDevice`, ou a primeira quando o objeto não foi registrado.
    ///
    /// O caminho sem janela e os testes criam aparelho sem passar pelo `CreateDevice` com
    /// identificador; para eles a porta um é a resposta certa, porque é a única que existe.
    pub(super) fn porta_do(&self, aparelho: u32) -> usize {
        self.portas_de_aparelho
            .get(&aparelho)
            .copied()
            .unwrap_or(0)
            .min(input::PORTAS - 1)
    }

    /// Quais portas estão ligadas com o aparelho pedido, na ordem.
    pub(super) fn portas_com(&self, aparelho: crate::input::bindings::Aparelho) -> Vec<usize> {
        (0..input::PORTAS)
            .filter(|&n| self.portas[n] == Some(aparelho))
            .collect()
    }

    pub fn set_key(&mut self, avk: u32, down: bool) {
        self.teclas.push_back((avk, down));
    }

    /// Entrega as teclas enfileiradas.
    ///
    /// O evento é o `EVT_KEY` do BREW, com o código virtual no `wParam`. A soltura vai como
    /// `EVT_KEY + 1`, que é o `EVT_KEY_RELEASE`: um jogo que só olhe o aperto ignora a segunda
    /// sem prejuízo, e um que conte as duas precisa das duas.
    ///
    /// **Quem recebe tecla primeiro é o widget, não o aplicativo.** No BREW é a extensão de
    /// interface que roteia a entrada para quem está em foco, e o tratador do formulário de
    /// abertura da Z-Wheel prova: ele testa `evt == 0x100` e compara o `wParam` com códigos
    /// `AVK_`. Mandar direto ao aplicativo devolve zero — medido.
    ///
    /// A ordem é a do BREW: o widget tem a primeira chance e, se ninguém tratou, o evento sobe
    /// para o aplicativo. Um evento que ninguém trata não faz nada, e é por isso que entregar
    /// tecla é seguro de um jeito que inventar evento de propriedade não era.
    pub(super) fn flush_keys(&mut self) -> Result<(), CpuError> {
        // A árvore inteira vai para a serial na primeira tecla: é o retrato do modelo no momento
        // em que a pergunta "para quem isto vai?" aparece.
        if !self.teclas.is_empty() && !self.despejou {
            self.despejou = true;
            self.despeja_widgets();
        }
        while let Some((avk, down)) = self.teclas.pop_front() {
            // **A saída da fila, antes de qualquer atalho.** A linha da entrega, mais abaixo, só
            // aparece para quem chega aos tratadores: uma seta consumida pelo rolamento do HTML sai
            // do laço sem deixar rastro, e "não chegou" fica indistinguível de "foi consumida".
            if self.serial.is_some() {
                self.registra_serial(format!(
                    "<fila {avk:#x} {} sai do despacho, {} na fila>",
                    match down {
                        true => "aperta",
                        false => "solta",
                    },
                    self.teclas.len()
                ));
            }
            let evento = match down {
                true => input::EVT_KEY,
                false => input::EVT_KEY + 1,
            };
            let mut tratado = false;
            // Com o painel de HTML em foco, as setas verticais são dele enquanto houver texto
            // para rolar. Ver [`Machine::rola_html_em_foco`].
            if matches!(avk, input::avk::UP | input::avk::DOWN) {
                let para_baixo = avk == input::avk::DOWN;
                if down && self.rola_html_em_foco(para_baixo) {
                    self.teclas_da_rolagem.insert(avk);
                    continue;
                }
                if !down && self.teclas_da_rolagem.remove(&avk) {
                    continue;
                }
            }
            // **O mais novo primeiro.** Os formulários se empilham e nenhum é destruído aqui,
            // então a lista tem o tratador da abertura ao lado do da tela atual. O da abertura
            // devolve 1 para qualquer aperto — ele trata a tecla 0 e a CLR —, e vindo antes
            // ele decidia tudo: o formulário do z-pad registrava o dele e nunca via uma tecla.
            //
            // Ordenar pela ordem de criação é o que um empilhamento de formulários faz: quem
            // está por cima tem a primeira chance, e o que ele não consumir desce.
            // A ordem aqui ainda não tem modelo, e três heurísticas já falharam: todos os
            // widgets, só a árvore do formulário atual, do mais novo para o mais velho e o
            // contrário. Em todas quem responde é um tratador que é literalmente
            // `mov r0,#1; bx lr` — a `0x77300` do jogo, que devolve "tratei" para qualquer
            // tecla. No console quem recebe é o widget **com foco**, e foco é coisa que ainda
            // não sabemos ler. Enquanto não soubermos, fica a ordem do mapa, que é a que
            // estava aqui antes de eu começar a mexer.
            // **Dentro do formulário atual, todo tratador vê a tecla.** Parar no primeiro que
            // devolve não-zero parecia o certo — é o que um container do BREW faz —, mas a
            // `0x77300` é `mov r0,#1; bx lr`: ela diz "tratei" para qualquer coisa que lhe
            // cheguem, e está registrada na **barra de status**, um container de 640×50 que no
            // console não recebe tecla nenhuma. Com ela na frente, o palco nunca via um aperto
            // e a roda não girava.
            //
            // Quem recebe no console é o widget **com foco**, e foco continua sendo coisa que
            // não sabemos ler. Entregar a todos os da tela atual é a aproximação que não
            // depende de saber: um tratador que não reconhece o evento não faz nada, e o que
            // reconhece age. Os de fora da tela seguem valendo só quando ninguém de dentro
            // respondeu, que é o que impede a abertura de responder por cima do menu.
            let dentro = self.arvore_do_formulario();
            let (na_tela, fora): (Vec<_>, Vec<_>) = self
                .tratadores_em_ordem(&dentro)
                .into_iter()
                .partition(|(esta_dentro, _)| *esta_dentro);
            // As contagens e os endereços saem antes dos laços: depois deles as listas foram
            // consumidas. **Os endereços são o que responde "a tecla chegou ao tratador da lista?"**
            // — a lista de jogos da Z-Wheel é o roller `0x30000ad0`, tratador `0x75724`.
            let (quantos_na_tela, quantos_fora) = (na_tela.len(), fora.len());
            let quem: Vec<String> = na_tela
                .iter()
                .chain(fora.iter())
                .take(4)
                .map(|(_, (funcao, _))| format!("{funcao:#x}"))
                .collect();
            for (_, (funcao, contexto)) in na_tela {
                let saida = self.call_guest(funcao, [contexto, evento, avk, 0], QSORT_BUDGET)?;
                if matches!(saida, Outcome::Returned { code } if code != 0) {
                    tratado = true;
                }
            }
            if !tratado {
                for (_, (funcao, contexto)) in fora {
                    let saida =
                        self.call_guest(funcao, [contexto, evento, avk, 0], QSORT_BUDGET)?;
                    if matches!(saida, Outcome::Returned { code } if code != 0) {
                        tratado = true;
                        break;
                    }
                }
            }
            // **A entrega da tecla vai para a captura de serial.** É o instrumento que responde
            // as duas perguntas que sobram quando um applet reage à primeira tecla e ignora as
            // seguintes: **a quem** ela foi entregue, e se alguém a tratou. Medido: no caminho do
            // core a segunda tecla não produz efeito nenhum, com a árvore de widgets idêntica à da
            // varredura — sem esta linha, "a tecla não chegou" e "chegou e ninguém tratou" são
            // indistinguíveis.
            if self.serial.is_some() {
                self.registra_serial(format!(
                    "<tecla {avk:#x} {} para {} tratador(es) na tela e {} fora ({}){}>",
                    match down {
                        true => "aperta",
                        false => "solta",
                    },
                    quantos_na_tela,
                    quantos_fora,
                    quem.join(" "),
                    match tratado {
                        true => ", tratada",
                        false => ", ninguém tratou",
                    }
                ));
            }
            if !tratado {
                let _ = self.send_applet_event(self.applet_class, evento, avk as u16, 0)?;
            }
        }
        Ok(())
    }

    /// Diz que aparelho o console vê em cada porta. `None` desliga a porta.
    pub fn set_portas(
        &mut self,
        portas: [Option<crate::input::bindings::Aparelho>; input::PORTAS],
    ) {
        self.portas = portas;
    }

    /// Dispara o sinal registrado num dos `RegisterFor*` do `IHIDDevice`.
    /// O `porta` é parte da chave porque o registro é feito **no objeto do aparelho**, um por
    /// controle: com dois ligados, guardar só pelo nome do registro fazia o segundo apagar o
    /// primeiro e todo evento acordar o callback do controle dois.
    pub(super) fn raise_input_signal(&mut self, register: &'static str, porta: usize) {
        let Some(&signal) = self.input_signals.get(&(register, porta)) else {
            return;
        };
        if let Some(&callback) = self.signals.get(&signal) {
            // Um sinal BREW é um aviso de "há trabalho", não um evento contado. O host pode
            // atualizar mais de uma porta (ou mover o eixo e logo voltar ao centro) antes de o
            // scheduler entregar o callback. Enfileirar a mesma função duas vezes faz muitos
            // menus processarem a mesma navegação em sentidos opostos. Coalescer aqui preserva
            // todas as mudanças no estado consultável pelo jogo e elimina a reentrada espúria.
            if !self.pending_signals.contains(&callback) {
                self.pending_signals.push(callback);
            }
        }
    }

    /// As URLs que o jogo tentou buscar pelo `IWeb`.
    /// Os últimos toques entregues ao jogo, como `(instante, nome do botão, apertado)`.
    pub fn pad_log(&self) -> Vec<(u32, usize, &'static str, bool)> {
        self.pad_log
            .iter()
            .map(|&(ms, porta, index, down)| {
                (
                    ms,
                    porta,
                    input::BUTTON_NAMES.get(index).copied().unwrap_or("?"),
                    down,
                )
            })
            .collect()
    }
}
