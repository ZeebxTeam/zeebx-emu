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
            "Release" => self.objects.release(this),
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
                    UID_JOYSTICK_DEVICE => {
                        self.portas_com(crate::input::bindings::Aparelho::Controle)
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
            // GetDeviceInfo(AEEHIDDeviceInfo *pInfo): { int type; uint16 pid; uint16 vid;
            // boolean bluetooth }. Em IHID a struct vem no segundo argumento.
            "GetDeviceInfo" => {
                let out = if iface == Interface::Hid { a2 } else { a1 };
                // No `IHID` o identificador da porta vem em `r1`; no `IHIDDevice` é o próprio
                // objeto que diz de qual porta ele é.
                let porta = match iface == Interface::Hid {
                    true => (a1.saturating_sub(1) as usize).min(input::PORTAS - 1),
                    false => self.porta_do(this),
                };
                let tipo = match self.portas[porta] {
                    Some(crate::input::bindings::Aparelho::Teclado) => HID_TYPE_KEYBOARD,
                    _ => HID_TYPE_GAMEPAD,
                };
                if out != 0 {
                    self.cpu.write_u32(out, tipo)?;
                    self.cpu
                        .write_mem(out + 4, &GAMEPAD_PRODUCT_ID.to_le_bytes())?;
                    self.cpu
                        .write_mem(out + 6, &GAMEPAD_VENDOR_ID.to_le_bytes())?;
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
            "GetPositionState" => {
                let axes = self.pads[self.porta_do(this)].axes;
                self.write_position_info(a1, &axes)?;
                SUCCESS
            }
            // `GetAxesInfo` não devolve valores: devolve, em cada campo, o UID do eixo que
            // ocupa aquele campo. É assim que o `AEEHIDThumbsticks.c` do SDK descobre onde
            // está cada direção — e enquanto respondíamos zeros, ele não achava nenhuma.
            "GetAxesInfo" => {
                self.write_position_info(a1, &input::AXIS_UIDS.map(|uid| uid as i32))?;
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
            "GetNextConnectEvent" => {
                for out in [a1, a2, a3] {
                    if out != 0 {
                        self.cpu.write_u32(out, 0)?;
                    }
                }
                SUCCESS
            }
            // Os `RegisterFor*` recebem um `ISignal` que devemos disparar quando houver evento.
            // Guardamos qual é; disparar de fato depende de ligar a entrada do host.
            "RegisterForConnectEvents"
            | "RegisterForStatusChange"
            | "RegisterForButtonEvent"
            | "RegisterForPositionChange" => {
                if a1 != 0 {
                    self.input_signals.insert(name, a1);
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

    /// Preenche um `AEEHIDPositionInfo` com um valor por eixo.
    ///
    /// A struct é `boolean bRelativeAxes` seguido de vinte e quatro inteiros, um por eixo
    /// possível. O controle do Zeebo usa quatro deles — `X`, `Y`, `Z` e `RZ` —, e os outros
    /// ficam zerados: um eixo que não existe tem faixa zero, e é assim que o jogo sabe
    /// ignorá-lo. Os eixos são absolutos, então `bRelativeAxes` também fica zero.
    pub(super) fn write_position_info(
        &mut self,
        addr: u32,
        values: &[i32; 4],
    ) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let mut words = [0u32; input::POSITION_INFO_WORDS];
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
        let agora = self.elapsed_ms();
        for &(index, down) in &changes {
            if self.pad_log.len() == PAD_LOG_MAX {
                self.pad_log.pop_front();
            }
            self.pad_log.push_back((agora, porta, index, down));
        }

        if !changes.is_empty() {
            self.raise_input_signal("RegisterForButtonEvent");
        }
        if moved {
            self.raise_input_signal("RegisterForPositionChange");
        }
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
            let evento = match down {
                true => input::EVT_KEY,
                false => input::EVT_KEY + 1,
            };
            let mut tratado = false;
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
    pub(super) fn raise_input_signal(&mut self, register: &'static str) {
        let Some(&signal) = self.input_signals.get(register) else {
            return;
        };
        if let Some(&callback) = self.signals.get(&signal) {
            self.pending_signals.push(callback);
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
