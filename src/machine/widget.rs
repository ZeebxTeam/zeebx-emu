//! IWidget, IControl e IForm: a árvore de widgets e a pintura dela.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Compatibilidade com a abertura da Z-Wheel distribuída no pacote 274755.
    /// Usa o atalho do próprio formulário, sem alterar instruções ou dados da ROM.
    /// A assinatura evita aplicar os endereços medidos a outra versão do applet.
    pub(super) fn skip_wheel_instructions(&mut self) -> Result<(), CpuError> {
        if self.applet_class != 0x01070798 || self.wheel_boot_skipped {
            return Ok(());
        }
        let Some((function, context)) = self
            .widgets
            .values()
            .map(|widget| widget.tratador)
            .find(|(function, _)| *function == 0x11828)
        else {
            return Ok(());
        };
        let mut signature = [0; 16];
        self.cpu.read_mem(function, &mut signature)?;
        if signature
            != [
                0xf0, 0x41, 0x2d, 0xe9, 0x01, 0x0c, 0x51, 0xe3, 0x03, 0x70, 0xa0, 0xe1, 0x01, 0x60,
                0xa0, 0xe1,
            ]
        {
            return Ok(());
        }
        let app = self.cpu.read_u32(context)?;
        // Durante a transição o formulário ignora entrada; aguarda o próximo tique.
        if self.cpu.read_u32(app + 0x2138)? & 1 != 0 {
            return Ok(());
        }
        self.call_guest(
            function,
            [context, input::EVT_KEY, input::avk::CLR, 0],
            QSORT_BUDGET,
        )?;
        self.wheel_boot_skipped = self.cpu.read_u32(app + 0x3610)? & 0x10000000 != 0;
        if self.wheel_boot_skipped {
            self.assumptions.insert("a abertura conhecida da Z-Wheel recebe automaticamente o atalho de pular instruções");
        }
        Ok(())
    }

    /// Dá a partida na abertura, **uma vez** por tratador registrado.
    ///
    /// Daqui para a frente a `AnimationVideo_Form` anda sozinha, e a corrente inteira está
    /// lida: a `0x11528` arma um `ISHELL_SetTimer` de mil milissegundos com o retorno de chamada
    /// `0x114ac`, que é o próprio tique; cada tique olha `[formulário+0x2c]` e avança de estado;
    /// no estado três a `0x11610` registra um `ISHELL_Resume` para a `0x11750`, que fecha o
    /// formulário e chama a `0x82464` — e é ela que leva ao menu principal.
    ///
    /// O console dá **um** aviso e o resto é do jogo. O aviso é o par `(0x801, 0x5064)` no
    /// tratador que o slot 4 registrou, que é a forma que o `0x11828` desvia para o `0x114ac`.
    ///
    /// Entregar mais de um seria inventar cadência, e isso já custou uma travada.
    pub(super) fn parte_animacao(&mut self) -> Result<(), CpuError> {
        /// O par que a `0x11828` entende como "começou".
        const PARTIDA: (u32, u32) = (0x801, 0x5064);

        let novos: Vec<(u32, (u32, u32))> = self
            .widgets
            .iter()
            .filter(|(_, widget)| widget.tratador.0 != 0 && !widget.partiu)
            .map(|(&objeto, widget)| (objeto, widget.tratador))
            .collect();
        for (objeto, (funcao, contexto)) in novos {
            if let Some(widget) = self.widgets.get_mut(&objeto) {
                widget.partiu = true;
            }
            self.call_guest(funcao, [contexto, PARTIDA.0, PARTIDA.1, 1], QSORT_BUDGET)?;
        }
        Ok(())
    }

    /// Chama o retorno de desenho que o slot 16 registrou, um por widget do formulário atual.
    ///
    /// **Quem desenha um `OwnerDrawWidget` é o jogo, e quem manda desenhar é a extensão de
    /// widgets — nós.** O widget guarda `(função, contexto)`; a chamada é
    /// `função(contexto, tela, x, y)`, com o canto do widget já em coordenadas de tela.
    ///
    /// A forma dos argumentos foi lida na `0x7562c`, o desenho do palco: ela guarda `r2` e `r3`
    /// e os usa, mais adiante, como o par de coordenadas de um traço. O `r1` ela ignora — é o
    /// outro `OwnerDrawWidget` da tela, na `0x605e4`, que faz dele um `QueryInterface`. Daí
    /// passarmos um `IDisplay`: para o palco é indiferente, e para o outro a interface que
    /// faltar aparece no relatório em vez de sumir.
    ///
    /// Uma vez por quadro para cada widget visível que registrou desenho — e **não** só os do
    /// formulário atual, ao contrário do que o `pinta_widgets` faz.
    ///
    /// A restrição por árvore chegou a existir aqui e deixava o palco de fora: o formulário do
    /// menu fica com zero filho, porque não recebe o conteúdo pelo item `0x5000`, e o que ele
    /// mostra vira uma raiz solta. Desenho registrado é explícito — quem registrou quer ser
    /// chamado —, então ele não depende de acertarmos qual é a tela atual. Quando a ligação que
    /// falta chegar pelo evento `0x7b0a`, vale reconsiderar.
    pub(super) fn desenha_widgets(&mut self) -> Result<(), CpuError> {
        // Callbacks de desenho são a própria indicação de que o widget pertence ao palco. A
        // Z-Wheel mantém a barra inferior numa raiz separada do formulário atual; filtrá-la pela
        // árvore do formulário apaga a barra mesmo quando ela está visível.
        let mut chamar: Vec<(u32, (u32, u32))> = self
            .widgets
            .iter()
            .filter(|(_, no)| no.desenho.0 != 0 && no.visivel)
            .map(|(&endereco, no)| (endereco, no.desenho))
            .collect();
        // **Formulário coberto não desenha.** Com a tela de ajuda por cima, o palco e a roda do
        // menu — que moram no formulário de baixo — continuavam sendo chamados, e o palco caía
        // por cima do painel da ajuda. Só o que pertence a um formulário que não é o do topo sai;
        // o que não está sob formulário nenhum, como a barra de status, segue sendo desenhado.
        let topo = self.formulario_atual();
        chamar.retain(|(endereco, _)| match self.formulario_de(*endereco) {
            Some(formulario) => Some(formulario) == topo,
            None => true,
        });
        if chamar.is_empty() {
            return Ok(());
        }
        // A ordem é a de criação, como no `pinta_widgets`: não é profundidade de verdade, mas é
        // estável, e desenho instável é pior do que desenho na ordem errada.
        chamar.sort_by_key(|(endereco, _)| self.widgets[endereco].serial);
        let tela = self.target()?;
        for (endereco, (funcao, contexto)) in chamar {
            let (x, y) = self.posicao_na_tela(endereco);
            self.call_guest(funcao, [contexto, tela, x as u32, y as u32], QSORT_BUDGET)?;
        }
        Ok(())
    }

    /// O formulário a que um widget pertence, subindo pelos pais, quando há um.
    fn formulario_de(&self, widget: u32) -> Option<u32> {
        /// A árvore vem do jogo; um ciclo não pode prender o desenho.
        const TETO: usize = 64;
        let mut atual = widget;
        for _ in 0..TETO {
            let no = self.widgets.get(&atual)?;
            if no.classe == WIDGET_FORMULARIO {
                return Some(atual);
            }
            if no.pai == 0 || no.pai == atual {
                return None;
            }
            atual = no.pai;
        }
        None
    }

    /// Lê a posição que o `AdicionarFilho` traz em `r3` e guarda no filho.
    ///
    /// O terceiro argumento aponta para seis palavras — `{x, y, sinalizador, largura, altura,
    /// objeto}`. O jogo pendura a imagem de abertura com `{0, 0, 1, 640, 480, …}`, a tela
    /// inteira, e os pedaços do formulário do z-pad com coordenadas de verdade: `{100, 21}`,
    /// `{148, 20}`, `{365, 20}`. Enquanto isso era ignorado, tudo era pintado na origem e o
    /// que sobrava na tela era um amontoado no canto.
    ///
    /// Largura e altura vêm zeradas quando o widget se mede sozinho, e aí não são gravadas: um
    /// tamanho zero apagaria o que o slot 7 já disse.
    ///
    /// O mesmo slot atende cinco classes da família de widgets, e nem toda chamada tem esta
    /// forma — algumas passam uma função em `r2` e um objeto em `r3`. Por isso a posição só é
    /// lida quando `r2` é zero, que é a forma medida, e qualquer leitura que falhe é
    /// descartada em vez de virar coordenada.
    pub(super) fn anota_posicao(&mut self, filho: u32) -> Result<(), CpuError> {
        /// Deslocamentos dentro da estrutura, em palavras.
        const X: u32 = 0;
        const Y: u32 = 4;
        const LARGURA: u32 = 12;
        const ALTURA: u32 = 16;

        if self.cpu.read_reg(Reg::R2) != 0 {
            return Ok(());
        }
        let onde = self.cpu.read_reg(Reg::R3);
        let (Ok(x), Ok(y)) = (self.cpu.read_u32(onde + X), self.cpu.read_u32(onde + Y)) else {
            return Ok(());
        };
        let tamanho = match (
            self.cpu.read_u32(onde + LARGURA),
            self.cpu.read_u32(onde + ALTURA),
        ) {
            (Ok(largura), Ok(altura)) if largura != 0 && altura != 0 => Some((largura, altura)),
            _ => None,
        };
        if let Some(widget) = self.widgets.get_mut(&filho) {
            widget.posicao = (x as i32, y as i32);
            if let Some(tamanho) = tamanho {
                widget.tamanho = tamanho;
            }
        }
        Ok(())
    }

    /// Despeja a árvore de widgets na serial, um por linha, com pai, classe e o que carrega.
    ///
    /// Existe para o estudo do modelo de widgets: sem ver a árvore inteira não dá para dizer se
    /// ela é uma ou várias, e é justamente disso que depende saber para quem uma tecla vai.
    pub fn despeja_widgets(&mut self) {
        if self.serial.is_none() {
            return;
        }
        let mut linhas: Vec<(u64, String)> = self
            .widgets
            .iter()
            .map(|(&endereco, w)| {
                let filhos = w.filhos.len() + w.anexados.len();
                (
                    w.serial,
                    format!(
                        "<widget {endereco:#x} pai {:#x} classe {:#x} filhos {filhos} \
                         tratador {:#x} visível {} tamanho {:?} pos {:?} texto {:?}>",
                        w.pai, w.classe, w.tratador.0, w.visivel, w.tamanho, w.posicao, w.texto
                    ),
                )
            })
            .collect();
        linhas.sort_unstable();
        for (_, linha) in linhas {
            self.registra_serial(linha);
        }
    }

    /// Os tratadores que devem ver uma tecla, dizendo de cada um se ele está na tela atual.
    ///
    /// Primeiro os do formulário atual, do mais novo para o mais velho, e depois os de fora
    /// dele — onde mora o tratador da abertura, que devolve "tratei" para qualquer aperto e,
    /// vindo antes, decidia tudo.
    pub(super) fn tratadores_em_ordem(
        &self,
        dentro: &std::collections::HashSet<u32>,
    ) -> Vec<(bool, (u32, u32))> {
        let mut marcados = Vec::new();
        let mut vistos = std::collections::HashSet::new();
        for (&endereco, no) in &self.widgets {
            if no.tratador.0 == 0 || !dentro.contains(&endereco) {
                continue;
            }
            vistos.insert(endereco);
            // Um widget num ramo sem foco não vê a tecla. Na lista de jogos a barra de abas e as
            // capas moram no mesmo container, e as duas tratam esquerda e direita: sem olhar o
            // foco, a seta andava nas capas e trocava a aba ao mesmo tempo.
            if self.fora_do_foco(endereco) {
                continue;
            }
            marcados.push((true, no.tratador, no.serial));
        }
        marcados.sort_by_key(|item| std::cmp::Reverse(item.2));
        let mut fora: Vec<(bool, (u32, u32), u64)> = self
            .widgets
            .iter()
            .filter(|(endereco, no)| no.tratador.0 != 0 && !vistos.contains(*endereco))
            .map(|(_, no)| (false, no.tratador, no.serial))
            .collect();
        fora.sort_by_key(|item| std::cmp::Reverse(item.2));
        marcados.extend(fora);
        marcados.into_iter().map(|(d, par, _)| (d, par)).collect()
    }

    /// Se algum container acima do widget tem foco definido e o foco não está no caminho até ele.
    ///
    /// Onde nenhum container definiu foco, nada muda: a tecla continua indo a todos, que é a
    /// aproximação de antes para as telas cujo foco não passa pelo `EVT_WDG_MOVEFOCUS`.
    fn fora_do_foco(&self, widget: u32) -> bool {
        const MOVE_FOCO: u32 = 0x711;
        /// A árvore vem do jogo; um ciclo nela não pode prender a entrega de teclas.
        const TETO: usize = 64;
        let mut filho = widget;
        for _ in 0..TETO {
            let Some(pai) = self.widgets.get(&filho).map(|w| w.pai) else {
                return false;
            };
            if pai == 0 || pai == filho {
                return false;
            }
            if let Some(container) = self.widgets.get(&pai) {
                let foco = container.propriedades.get(&MOVE_FOCO).copied();
                if let Some(foco) = foco.filter(|f| container.anexados.contains(f)) {
                    if foco != filho && container.anexados.contains(&filho) {
                        return true;
                    }
                }
            }
            filho = pai;
        }
        false
    }

    /// Os widgets da árvore do formulário atual, incluindo a raiz.
    ///
    /// Desce da raiz em vez de subir de cada widget. Subir custava uma caminhada por widget
    /// vivo — e há doze mil deles, porque o jogo remonta o formulário a cada volta e não solta
    /// o anterior. A árvore do formulário tem uma dúzia de nós, então descer é o barato: com a
    /// subida, o laço caía de 896 voltas para 57.
    pub(super) fn arvore_do_formulario(&self) -> std::collections::HashSet<u32> {
        /// Teto de nós visitados. A árvore vem do jogo, e um ciclo nela não pode prender o
        /// desenho.
        const TETO: usize = 4096;

        let mut dentro = std::collections::HashSet::new();
        let Some(raiz) = self.formulario_atual() else {
            return dentro;
        };
        let mut fila = vec![raiz];
        dentro.insert(raiz);
        while let Some(atual) = fila.pop() {
            if dentro.len() >= TETO {
                break;
            }
            let Some(no) = self.widgets.get(&atual) else {
                continue;
            };
            for filho in no.filhos.values().chain(no.anexados.iter()) {
                if self.widgets.contains_key(filho) && dentro.insert(*filho) {
                    fila.push(*filho);
                }
            }
        }
        // A barra inferior da Z-Wheel não é filha do formulário selecionado. Ela vive no
        // container visual `0x01028e3f`; incluir somente a instância mais nova evita ressuscitar
        // todas as árvores antigas que o shell deixa alocadas durante o modo de atração.
        if let Some((&barra, _)) = self
            .widgets
            .iter()
            .filter(|(endereco, no)| {
                no.classe == 0x0102_8e3f
                    && (no.pai == 0 || no.pai == **endereco)
                    && (no.filhos.len() + no.anexados.len()) > 0
            })
            .max_by_key(|(_, no)| no.serial)
        {
            let mut fila = vec![barra];
            dentro.insert(barra);
            while let Some(atual) = fila.pop() {
                if dentro.len() >= TETO {
                    break;
                }
                let Some(no) = self.widgets.get(&atual) else {
                    continue;
                };
                for filho in no.filhos.values().chain(no.anexados.iter()) {
                    if self.widgets.contains_key(filho) && dentro.insert(*filho) {
                        fila.push(*filho);
                    }
                }
            }
        }
        dentro
    }

    /// Se `filho` está pendurado em `container` pelo slot 5.
    pub(super) fn e_filho_anexado(&self, container: u32, filho: u32) -> bool {
        filho != 0
            && self
                .widgets
                .get(&container)
                .is_some_and(|widget| widget.anexados.contains(&filho))
    }

    /// A raiz mais nova de todas — o formulário que o jogo acabou de montar.
    ///
    /// **A Z-Wheel nunca esconde nada.** O slot 6 só é chamado com verdadeiro, e o jogo monta um
    /// formulário novo a cada volta do ciclo em vez de reaproveitar: são 1722 raízes vivas numa
    /// execução de quinze segundos, cada uma um formulário do z-pad inteiro. No console a
    /// anterior seria destruída; aqui ela fica, porque o jogo não a solta e não temos por que
    /// soltar por conta.
    ///
    /// Pintar todas empilha a tela de boas-vindas de 640×480 — que é a raiz mais **velha** — por
    /// cima do formulário atual. O console mostra um formulário por vez, e o atual é o último
    /// montado: é o que esta função devolve.
    pub(super) fn formulario_atual(&self) -> Option<u32> {
        let com_filhos: std::collections::HashSet<u32> =
            self.widgets.values().map(|no| no.pai).collect();
        // **O formulário mais novo.** Desde que o item `0x5000` passou a pendurar o conteúdo, a
        // árvore é uma só: a raiz do applet tem os formulários por filhos, e cada formulário tem
        // o container do que ele mostra. Pintar a raiz inteira desenha a abertura e o z-pad um
        // por cima do outro — o que apareceu na tela assim que as árvores se juntaram.
        //
        // Quem está à mostra é o último empilhado, e é o que esta função devolve. Sem nenhum
        // formulário — antes de a interface existir —, vale a raiz mais nova que tenha filho,
        // que é o que sustentava a tela até aqui.
        let formulario = self
            .widgets
            .iter()
            .filter(|(endereco, no)| {
                no.classe == WIDGET_FORMULARIO && com_filhos.contains(endereco)
            })
            .max_by_key(|(_, no)| no.serial)
            .map(|(&endereco, _)| endereco);
        // **Nem todo formulário recebe o conteúdo pelo item `0x5000`.** O do menu principal não
        // recebe: ele fica com zero filho, e o que ele mostra vira uma raiz solta — o container
        // de `0x30000c10`, com a barra de status e o palco dentro. O evento `0x7b0a` já é
        // encaminhado pela raiz; a ligação de pai em si ainda não foi observada.
        //
        // Enquanto isso, escolher só entre formulários deixava o do z-pad como atual para
        // sempre, e ele traz a `0x77300` — um tratador que devolve "tratei" para qualquer
        // tecla. Com ela na frente, nenhuma tecla chegava ao menu: a roda não girava.
        //
        // Então a escolha é entre os dois candidatos, e vence o mais novo.
        let raiz = self.raiz_mais_nova(&com_filhos);
        match (formulario, raiz) {
            (Some(f), Some(r)) => {
                let idade = |quem: u32| self.widgets.get(&quem).map_or(0, |no| no.serial);
                return Some(if idade(r) > idade(f) { r } else { f });
            }
            (Some(f), None) => return Some(f),
            (None, _) => {}
        }
        raiz
    }

    /// A raiz mais nova que tenha filho, que é o que sustentava a tela antes dos formulários.
    pub(super) fn raiz_mais_nova(
        &self,
        com_filhos: &std::collections::HashSet<u32>,
    ) -> Option<u32> {
        self.widgets
            .iter()
            .filter(|(endereco, no)| {
                // Raiz é quem não tem pai. **E precisa ter filho**: o jogo cria widgets soltos
                // que nunca chegam a receber nada, e o mais novo de todos costuma ser um
                // desses — pintar por ele dava uma tela branca, que foi o que apareceu.
                (no.pai == 0 || no.pai == **endereco) && com_filhos.contains(endereco)
            })
            .max_by_key(|(_, no)| no.serial)
            .map(|(&endereco, _)| endereco)
    }

    /// A que altura da árvore um widget está — a raiz é zero.
    ///
    /// É o que dá a ordem de desenho: filho por cima de pai. Mesmo teto e mesmo motivo do
    /// [`Machine::posicao_na_tela`].
    pub(super) fn profundidade(&self, widget: u32) -> usize {
        /// Até onde subir na árvore antes de desistir.
        const FUNDO: usize = 32;

        let mut atual = widget;
        for altura in 0..FUNDO {
            let Some(no) = self.widgets.get(&atual) else {
                return altura;
            };
            if no.pai == 0 || no.pai == atual {
                return altura;
            }
            atual = no.pai;
        }
        FUNDO
    }

    /// Onde um widget cai na tela, somando a posição de cada pai até a raiz.
    ///
    /// O laço tem teto porque a árvore vem do jogo: um `pai` que aponte para trás travaria o
    /// desenho, e um quadro torto é melhor que um emulador preso.
    pub(super) fn posicao_na_tela(&self, widget: u32) -> (i32, i32) {
        /// Até onde subir na árvore antes de desistir.
        const FUNDO: usize = 32;

        let (mut x, mut y) = (0, 0);
        let mut atual = widget;
        for _ in 0..FUNDO {
            let Some(no) = self.widgets.get(&atual) else {
                break;
            };
            x += no.posicao.0;
            y += no.posicao.1;
            if no.pai == 0 || no.pai == atual {
                break;
            }
            atual = no.pai;
        }
        (x, y)
    }

    /// Pinta as imagens penduradas nos widgets.
    ///
    /// **Isto é um substituto declarado, não uma emulação.** No console quem desenha a
    /// interface é a extensão de widgets, que não temos: os nossos guardam a árvore — filhos,
    /// tamanho, tratador — e não rasterizam nada. Sem alguém pintando, a Z-Wheel monta a tela
    /// inteira e o quadro fica preto, que foi exatamente o que se via.
    ///
    /// O que dá para fazer com o que está guardado é isto: toda imagem pendurada num widget vai
    /// para a tela. Para a abertura da Z-Wheel basta, porque é uma imagem de 640×480 na origem
    /// — o mesmo tamanho que o jogo manda para o widget no slot 7.
    ///
    /// A ordem é a dos endereços dos objetos, que no nosso alocador é a de criação. Não é
    /// profundidade de verdade; é a única ordem estável que temos, e uma ordem estável ao menos
    /// faz o resultado ser o mesmo a cada execução.
    pub(super) fn pinta_widgets(&mut self) -> Result<(), CpuError> {
        let atual = self.formulario_atual();
        // **Formulário novo apaga o anterior.** A superfície é persistente: o que foi pintado
        // num quadro continua lá no seguinte. Sem limpar, a tela de boas-vindas de 640×480
        // ficava por cima de tudo o que veio depois, mesmo depois de deixarmos de desenhá-la.
        //
        // Limpar só na troca, e não a cada quadro, porque quem desenha pelo `IDisplay` — que é
        // a maioria dos jogos — não tem formulário nenhum e não pode ter a tela apagada por
        // baixo. Sem widget, `atual` é `None` e nada aqui acontece.
        if let Some(raiz) = atual {
            if raiz != self.formulario_pintado {
                self.formulario_pintado = raiz;
                let alvo = self.target()?;
                if let Some(surface) = self.bitmaps.get_mut(&alvo) {
                    let tela = Rect {
                        x: 0,
                        y: 0,
                        width: surface.width() as i16,
                        height: surface.height() as i16,
                    };
                    surface.fill_rect(tela, Rgb::WHITE);
                }
            }
        }
        let dentro = self.arvore_do_formulario();
        self.pinta_fundos(&dentro)?;
        let mut imagens: Vec<(u32, u32)> = self
            .widgets
            .iter()
            .filter(|(dono, widget)| widget.visivel && dentro.contains(*dono))
            .flat_map(|(dono, widget)| widget.anexados.iter().map(|&filho| (filho, *dono)))
            .filter(|(objeto, _)| self.images.contains_key(objeto))
            .collect();
        // **Filho por cima de pai.** A ordem era a dos endereços dos objetos, que já foi a de
        // criação e deixou de ser quando o alocador ganhou lista de livres. Com ela, o fundo
        // de 640×480 da tela de boas-vindas caía por cima do formulário inteiro, e o que
        // sobrava à vista era o pedaço de um widget que por acaso tinha endereço maior.
        //
        // A altura na árvore é a ordem que a interface quer dizer. O endereço fica como
        // desempate, para que duas execuções desenhem igual.
        imagens.sort_unstable_by_key(|&(imagem, dono)| (self.profundidade(dono), imagem));
        imagens.dedup_by_key(|(imagem, _)| *imagem);
        for (imagem, dono) in imagens {
            let (x, y) = self.posicao_na_tela(dono);
            self.draw_image(imagem, x, y, None)?;
        }
        self.pinta_textos()
    }

    /// Preenche, a cada quadro, o retângulo dos widgets da tela atual que têm cor de fundo.
    ///
    /// A cor é a [`PROP_COR_DE_FUNDO`], em `RRGGBBAA` como a do texto; alfa zero é "sem fundo",
    /// que é o que a Z-Wheel grava na maioria dos widgets. O container que cada formulário dela
    /// pendura em `0x5000` recebe `0xd0d0d0ff`: é o cinza claro atrás do palco, da roda e da
    /// lista de jogos. Sem ele o fundo ficava no branco da troca de formulário, e o que um quadro
    /// deixava — o quadrado da seta da grade, os tracinhos sob as abas — nunca era coberto.
    ///
    /// Widget sem tamanho medido ocupa a superfície inteira, que é o caso desse container.
    fn pinta_fundos(&mut self, dentro: &std::collections::HashSet<u32>) -> Result<(), CpuError> {
        let mut fundos: Vec<(usize, i32, i32, (u32, u32), u32)> = self
            .widgets
            .iter()
            .filter(|(dono, widget)| widget.visivel && dentro.contains(*dono))
            .filter_map(|(&dono, widget)| {
                let rgba = *widget.propriedades.get(&PROP_COR_DE_FUNDO)?;
                (rgba & 0xff != 0).then_some(dono)
            })
            .map(|dono| {
                let (x, y) = self.posicao_na_tela(dono);
                let no = &self.widgets[&dono];
                let rgba = no.propriedades[&PROP_COR_DE_FUNDO];
                (self.profundidade(dono), x, y, no.tamanho, rgba)
            })
            .collect();
        if fundos.is_empty() {
            return Ok(());
        }
        fundos.sort_unstable_by_key(|f| (f.0, f.2, f.1));
        let alvo = self.target()?;
        let Some(surface) = self.bitmaps.get_mut(&alvo) else {
            return Ok(());
        };
        for (_, x, y, (largura, altura), rgba) in fundos {
            let (largura, altura) = if largura == 0 || altura == 0 {
                (surface.width() as u32, surface.height() as u32)
            } else {
                (largura, altura)
            };
            let area = Rect {
                x: x as i16,
                y: y as i16,
                width: largura.min(i16::MAX as u32) as i16,
                height: altura.min(i16::MAX as u32) as i16,
            };
            let cor = Rgb {
                r: (rgba >> 24) as u8,
                g: (rgba >> 16) as u8,
                b: (rgba >> 8) as u8,
            };
            surface.fill_rect(area, cor);
        }
        Ok(())
    }

    /// Escreve o texto que os widgets guardam, na posição e na cor deles.
    ///
    /// Faz parte do mesmo substituto do [`Machine::pinta_widgets`]: o console desenharia isto
    /// pela extensão de widgets, que não está no dump. O que temos é o texto — que chega pelo
    /// slot 6 da [`WIDGET_DE_TEXTO`] — e a fonte do próprio pacote do jogo, que já carrega.
    ///
    /// A cor sai da propriedade [`PROP_COR`], que vem como `RRGGBBAA`; sem ela, preto. O alfa é
    /// descartado porque a superfície do console não tem canal para ele — mesma razão do
    /// meio-tom no [`Machine::escreve`].
    ///
    /// A ordem é a da árvore, como a das imagens: filho por cima de pai.
    pub(super) fn pinta_textos(&mut self) -> Result<(), CpuError> {
        let dentro = self.arvore_do_formulario();
        let mut escritas: Vec<(usize, i32, i32, u32, String)> = self
            .widgets
            .iter()
            .filter(|(dono, widget)| {
                widget.visivel && !widget.texto.is_empty() && dentro.contains(*dono)
            })
            .map(|(&dono, widget)| {
                let (x, y) = self.posicao_na_tela(dono);
                let cor = widget.propriedades.get(&PROP_COR).copied().unwrap_or(0);
                (self.profundidade(dono), x, y, cor, widget.texto.clone())
            })
            .collect();
        // **O mesmo texto, no mesmo lugar, na mesma cor, é um desenho só.** O formulário do
        // z-pad é remontado a cada volta do ciclo e cada remontagem deixa a árvore anterior
        // viva: são milhares de cópias do mesmo rótulo empilhadas na mesma coordenada. Pintar
        // todas dá exatamente o mesmo quadro e fazia um quadro levar mais de um minuto.
        escritas
            .sort_unstable_by(|a, b| (a.1, a.2, &a.4, a.3, a.0).cmp(&(b.1, b.2, &b.4, b.3, b.0)));
        escritas.dedup_by(|a, b| (a.1, a.2, &a.4, a.3) == (b.1, b.2, &b.4, b.3));
        // Já sem repetição, a ordem que vale é a da árvore: filho por cima de pai.
        escritas.sort_unstable_by(|a, b| (a.0, a.2, a.1).cmp(&(b.0, b.2, b.1)));
        self.pinta_html()?;
        for (_, x, y, rgba, texto) in escritas {
            // A cor vem como `RRGGBBAA`. O alfa é descartado porque a superfície do console não
            // tem canal para ele — mesma razão do meio-tom no `escreve`.
            let cor = Rgb {
                r: (rgba >> 24) as u8,
                g: (rgba >> 16) as u8,
                b: (rgba >> 8) as u8,
            };
            self.escreve(&texto, x, y, cor)?;
        }
        Ok(())
    }

    /// Pinta os widgets de HTML da tela atual com o texto do placeholder.
    ///
    /// **É um desvio, não um renderizador de HTML.** A tela de ajuda da Z-Wheel precisa do widget
    /// de HTML do firmware (`0x0102dd32`), que não temos. Em vez de interpretar a página que o jogo
    /// carregaria, cada widget dessa classe mostra o texto de [`caminho_do_html`], quebrado pela
    /// largura dele. O arquivo mora no aparelho emulado, fora da ROM, e pode ser editado à vontade.
    fn pinta_html(&mut self) -> Result<(), CpuError> {
        let dentro = self.arvore_do_formulario();
        let alvos: Vec<(i32, i32, u32, u32)> = self
            .widgets
            .iter()
            .filter(|(dono, widget)| {
                widget.classe == WIDGET_HTML && widget.visivel && dentro.contains(*dono)
            })
            .map(|(&dono, widget)| {
                let (x, y) = self.posicao_na_tela(dono);
                (x, y, widget.tamanho.0, widget.tamanho.1)
            })
            .collect();
        if alvos.is_empty() {
            return Ok(());
        }
        let paragrafos = texto_do_html(&le_placeholder_html());
        for (x, y, largura, altura) in alvos {
            let largura = match largura {
                0 => 600,
                l => l.saturating_sub(2 * MARGEM_DO_HTML as u32).max(1),
            };
            let Some(fonte) = self.font.as_ref() else {
                return Ok(());
            };
            let passo = (fonte.ascent(FONT_SIZE) + fonte.descent(FONT_SIZE)) as i32 + 2;
            let linhas: Vec<String> = paragrafos
                .iter()
                .flat_map(|paragrafo| quebra_linhas(paragrafo, largura, |t| fonte.width(t, FONT_SIZE)))
                .collect();
            let limite = match altura {
                0 => i32::MAX,
                a => y + a as i32 - passo,
            };
            let mut linha_y = y + MARGEM_DO_HTML;
            for linha in linhas {
                if linha_y > limite {
                    break;
                }
                self.escreve(&linha, x + MARGEM_DO_HTML, linha_y, Rgb::BLACK)?;
                linha_y += passo;
            }
        }
        Ok(())
    }

    /// O filho de um widget guardado sob `id`, criado na primeira vez que alguém o pede.
    ///
    /// Guardar é o que faz o acessador ser coerente consigo mesmo: a `0x78acc` pede o `0x5000`,
    /// configura, pede o `0x5002` e solta os dois no fim. Se cada pedido criasse um objeto
    /// novo, o jogo soltaria objetos que não são os que usou.
    pub(super) fn filho_do_widget(&mut self, this: u32, id: u32) -> Result<u32, CpuError> {
        if let Some(filho) = self
            .widgets
            .get(&this)
            .and_then(|widget| widget.filhos.get(&id).copied())
        {
            // Entregar é emprestar: quem recebe vai soltar.
            self.objects.add_ref(filho);
            return Ok(filho);
        }
        let filho = self.new_object(Interface::Widget)?;
        if filho == 0 {
            return Ok(0);
        }
        self.proximo_serial += 1;
        self.widgets.insert(
            filho,
            Widget {
                visivel: true,
                serial: self.proximo_serial,
                ..Widget::default()
            },
        );
        if let Some(widget) = self.widgets.get_mut(&this) {
            widget.filhos.insert(id, filho);
        }
        // Duas referências: **uma do mapa do pai** e uma de quem pediu. Sem a do pai o objeto
        // morria no primeiro `Release` do chamador e a entrada no mapa ficava apontando para um
        // endereço livre — que, com o alocador reusando endereço, é outro objeto.
        self.objects.add_ref(filho);
        Ok(filho)
    }

    /// Atende o widget da Z-Wheel (`0x01028e51`). Ver [`Interface::Widget`].
    ///
    /// **O retorno é invertido**: diferente de zero é sucesso. Os invólucros do jogo
    /// (`0x3f72c` e `0x403c8`) fazem `cmp r0,#0; moveq r0,#3`, transformando zero em erro. Isso
    /// vale só para o acessador; o `AddRef` e o `Release` continuam devolvendo a contagem, como
    /// em todo o BREW.
    ///
    /// O acessador tem dois seletores, e ambos foram lidos no código do jogo:
    ///
    /// - `0x800` **pega o filho** de número `id` e escreve o ponteiro no terceiro argumento. O
    ///   filho é criado na primeira vez e guardado: a `0x78acc` pede o `0x5000`, configura, e
    ///   depois pede o `0x5002`, e ela solta os dois no fim — se cada pedido criasse um objeto
    ///   novo, o jogo soltaria objetos que não são os que usou.
    /// - `0x801` **grava** a propriedade `id`. O que os números querem dizer ainda não sabemos;
    ///   guardá-los custa nada e é o que permitirá reconhecê-los quando a tela aparecer.
    ///
    /// Um seletor que não seja esses dois é recusado com zero em vez de aceito em silêncio: um
    /// terceiro seletor é coisa que precisamos ver, não esconder.
    /// Atende o controle leve usado pelo subsistema de texto do Zenonia. A implementação mantém
    /// a ABI e as respostas de estado que o jogo consulta, evitando que a criação caia em loop.
    pub(super) fn control_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Control.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "IsActive" => 1,
            // Eventos, ativação e reset são operações sem estado observável para o emulador.
            "HandleEvent" | "Redraw" | "SetActive" | "SetProperties" | "Reset" => SUCCESS,
            "SetRect" => SUCCESS,
            "GetRect" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    self.cpu.write_mem(out, &[0; 16])?;
                }
                SUCCESS
            }
            "GetProperties" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    self.cpu.write_u32(out, 0)?;
                }
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    pub(super) fn widget_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        /// Lê um item do widget. O que sai depende do número do item — ver abaixo.
        const LE: u32 = 0x800;
        /// A partir daqui, o item guarda um objeto; abaixo, um número.
        const PRIMEIRO_OBJETO: u32 = 0x5000;
        const GRAVA: u32 = 0x801;
        /// O item do widget de HTML que guarda um objeto abaixo da faixa `0x5000`.
        const PROP_HTML_OBJETO: u32 = 0x161;
        /// Sucesso para esta classe. Não é o `SUCCESS` do BREW — ver acima.
        const OK: u32 = 1;
        /// Define o filho em foco de um container. Ver o braço do acessador.
        const FOCO_DEFINE: u32 = 0x711;
        /// `WIDGET_FOCUS_PREV`, o maior dos valores especiais do [`FOCO_DEFINE`].
        const FOCO_ANTERIOR: u32 = 4;
        /// Lê o filho em foco definido pelo [`FOCO_DEFINE`].
        const FOCO_LE: u32 = 0x713;
        /// Marca o estado de foco do próprio widget (`id` = 1). Ver o braço do `0x702`.
        const MARCA_FOCO: u32 = 0x700;
        /// Pergunta, num byte, se o widget pode receber foco.
        const HABILITADO: u32 = 0x702;
        /// `FORM_LAST`, o ponteiro especial do `IROOTFORM_RemoveForm` que quer dizer "o do topo".
        const FORM_LAST: u32 = 1;

        let Some(name) = Interface::Widget.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.solta_widget(this)?,
            // Zero é sucesso aqui, ao contrário do acessador logo abaixo. Aceitamos qualquer
            // interface pedida porque, no nosso modelo, a família inteira de widgets **é** uma
            // interface só — a hipótese fica registrada, que é onde ela deve estar.
            // O slot 12 tem a mesma forma e a mesma convenção do slot 2. É por ele que a
            // Z-Wheel pega, de dentro do retorno de chamada da imagem em `0x4d4a4`, o objeto
            // em que vai pendurar o GIF de abertura: `slot12(IID, &saída)` e, em seguida,
            // `slot5(saída, imagem)`. Recusá-lo abortava o retorno de chamada inteiro, e a
            // animação nunca começava — sem erro nenhum no log, porque quem abortou fomos nós.
            "QueryInterface" | "PegarInterface" => {
                let saida = self.cpu.read_reg(Reg::R2);
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                self.assumptions
                    .insert("um widget aceitou toda interface que lhe pediram");
                SUCCESS
            }
            // O slot 6 quer dizer duas coisas, conforme a classe.
            //
            // Na maioria da família é `slot6(this, visível)` — a última coisa que a `0x11750`
            // faz antes de sair da abertura: esconde o formulário da animação e vai para a
            // transição. O retorno é ignorado.
            //
            // Na [`WIDGET_DE_TEXTO`] é `slot6(this, texto, tamanho, …)`, com o texto em
            // `AECHAR`. Ver a constante para como isso foi medido.
            // `slot14(this, objeto)`, com o retorno ignorado. Aceitar e não guardar nada é o
            // mínimo que deixa a montagem seguir; o que o objeto é, ainda não sabemos.
            // `Slot16(this, trio)` **registra quem desenha o widget**, e devolve o anterior.
            //
            // O `CreateOwnerDrawWidget` em `0x22cd0` monta o trio na própria estrutura antes de
            // chamar: `[r4+0x14] = 0x5250c`, `[r4+0x18] = r4`, `[r4+0x1c] = 0x52574`, e passa
            // `r4+0x14` em `r1`. Quem desenha é a `0x5250c`, e ela é um elo de corrente — chama
            // primeiro `[ctx+0x14]([ctx+0x18], r1, r2, r3)`, que é justamente onde o anterior
            // precisa ter sido escrito de volta, e depois o desenho próprio do roller, em
            // `[ctx+0x00]([ctx+0x08], …)`. Sem a devolução ela chamaria a si mesma.
            //
            // A terceira palavra do trio é a `0x52574`, o liberador: guardado, e chamado quando
            // o widget morre (ver o `Release`).
            "Slot13" => {
                // Leitura em 0x24100..0x24198 do tectoy.mod: (&bitmap, w, h),
                // seguida de QueryInterface no bitmap devolvido. O widget foi usado
                // como interface de bitmap pelo QueryInterface permissivo acima.
                let slot = crate::brew::aee_slots::BITMAP
                    .iter()
                    .position(|name| *name == "CreateCompatibleBitmap")
                    .expect("slot de bitmap");
                return self.bitmap_call(slot as u32);
            }
            "Slot16" => {
                let onde = self.cpu.read_reg(Reg::R1);
                let novo = match (self.cpu.read_u32(onde), self.cpu.read_u32(onde + 4)) {
                    (Ok(funcao), Ok(contexto)) => (funcao, contexto),
                    _ => return Ok(Some(EBADPARM)),
                };
                let liberador = self.cpu.read_u32(onde + 8)?;
                let (anterior, liberador_anterior) = self
                    .widgets
                    .get(&this)
                    .map_or(((0, 0), 0), |w| (w.desenho, w.liberadores.1));
                self.cpu.write_u32(onde, anterior.0)?;
                self.cpu.write_u32(onde + 4, anterior.1)?;
                self.cpu.write_u32(onde + 8, liberador_anterior)?;
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.desenho = novo;
                    widget.liberadores.1 = liberador;
                }
                if self.serial.is_some() {
                    let proprio = (self.cpu.read_u32(novo.1), self.cpu.read_u32(novo.1 + 8));
                    self.registra_serial(format!(
                        "<desenho {:#x} ctx {:#x} em {this:#x}; proprio {proprio:x?}>",
                        novo.0, novo.1
                    ));
                }
                SUCCESS
            }
            "Slot17" => {
                // `pWidget->Slot17(0x8000, pFont)` em `tectoy_rollerwidget.c` (0x23860,
                // 0x23d0c). Não é uma notificação: o módulo solta `pFont` após montar o
                // roller, então a associação precisa possuir uma referência própria.
                let (id, modelo) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                if modelo == 0 || self.objects.kind_of(modelo).is_none() {
                    return Ok(Some(EBADPARM));
                }
                let Some(anterior) = self
                    .widgets
                    .get_mut(&this)
                    .map(|widget| widget.modelos.insert(id, modelo))
                else {
                    return Ok(Some(EBADPARM));
                };
                if anterior != Some(modelo) {
                    self.objects.add_ref(modelo);
                    if let Some(anterior) = anterior {
                        self.objects.release(anterior);
                    }
                }
                SUCCESS
            }
            // `Slot16(this)`, chamado uma vez em `0x22d58`, logo depois de o palco existir.
            // Recusá-lo não devolvia a execução ao `0x22d5c`: a montagem do menu parava ali, e
            // o aplicativo ficava no pulso de dez segundos que lê pontos e fila de download sem
            // desenhar nada. Aceito com zero — o `SUCCESS` do BREW —, o jogo segue: cria a
            // `0x01028e3c`, gera vinte e cinco texturas e sobe dezenove delas comprimidas em
            // ATITC, monta o frustum e a matriz. É o palco carregando os próprios cenários.
            //
            // O que ele faz continua sem nome porque não foi lido: só se sabe que recebe o
            // widget e que o jogo não usa o retorno para nada além de seguir.
            "Anexar" => {
                // Registrar a ligação, e não só aceitar: sem ela a árvore ficava partida em
                // duas — os widgets que desenham numa metade e o tratador de tecla na outra —,
                // e qualquer regra sobre "o formulário atual" escolhia a metade errada.
                let filho = self.cpu.read_reg(Reg::R1);
                if self.widgets.contains_key(&filho) {
                    if let Some(widget) = self.widgets.get_mut(&this) {
                        if !widget.anexados.contains(&filho) && filho != this {
                            widget.anexados.push(filho);
                        }
                    }
                    if let Some(widget) = self.widgets.get_mut(&filho) {
                        widget.pai = this;
                    }
                }
                SUCCESS
            }
            // **Nos containers, os slots 6 e 7 são o `IContainer` do BREW**: `Remove(widget)` e
            // `GetWidget(pwRef, bNext, bWrap)`, com o `Insert` no slot 5 que já é o
            // `AdicionarFilho`. A família divide a tabela, e as duas leituras se separam pelos
            // argumentos: `Remove` recebe um filho **deste** container, e `GetWidget` recebe nulo
            // ou um filho seguido de dois booleanos. Nem um ponteiro de tamanho nem um booleano
            // de visibilidade é filho do container.
            //
            // Sem isto, a `0x80490` — que tira da raiz todos os formulários menos o que vai
            // abrir, antes de lançar um jogo — pedia o próximo filho, recebia lixo do "tamanho"
            // e rodava para sempre: dez mil voltas até o orçamento do callback acabar, e o jogo
            // escolhido na grade nunca abria.
            // **Na raiz, o slot 6 é o `IROOTFORM_RemoveForm`**, e `1` é o `FORM_LAST` do BREW:
            // `IROOTFORM_PopForm` é exatamente `RemoveForm(raiz, FORM_LAST)`. A Z-Wheel usa assim
            // duas vezes — em `0x11770`, para tirar o formulário da animação de abertura, e no
            // laço de `0x82580`, que tira todos os formulários antes de lançar um jogo. Lido
            // como "visível", o laço perguntava pelo topo, recebia sempre o mesmo formulário e
            // girava até o orçamento do callback acabar.
            "DefinirVisivel"
                if self.widgets.get(&this).map_or(0, |w| w.classe) == WIDGET_RAIZ
                    && self.cpu.read_reg(Reg::R1) == FORM_LAST =>
            {
                // A raiz solta a referência que o `AdicionarFilho` pôs no formulário. Sem isso a
                // tela removida nunca morria: antes de lançar um jogo, a Z-Wheel tira todos os
                // formulários e desmonta os próprios dados, e a barra de abas da lista seguia
                // viva com a animação de 40 ms armada sobre memória já solta. (Uma tentativa
                // antiga de soltar aqui punha ícones atrás do roller; a causa era o
                // `ObjectStore` ressuscitar endereços livres, já corrigida.) O `pai` fica.
                let topo = self.widgets.get_mut(&this).and_then(|widget| widget.anexados.pop());
                if let Some(formulario) = topo {
                    self.solta_widget(formulario)?;
                }
                SUCCESS
            }
            "DefinirVisivel" if self.e_filho_anexado(this, self.cpu.read_reg(Reg::R1)) => {
                let filho = self.cpu.read_reg(Reg::R1);
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.anexados.retain(|&w| w != filho);
                }
                if let Some(filho) = self.widgets.get_mut(&filho) {
                    if filho.pai == this {
                        filho.pai = 0;
                    }
                }
                self.solta_widget(filho)?;
                SUCCESS
            }
            "DefinirTamanho"
                if {
                    let (referencia, proximo, volta) = (
                        self.cpu.read_reg(Reg::R1),
                        self.cpu.read_reg(Reg::R2),
                        self.cpu.read_reg(Reg::R3),
                    );
                    (referencia == 0 || self.e_filho_anexado(this, referencia))
                        && proximo <= 1
                        && volta <= 1
                } =>
            {
                let (referencia, proximo, volta) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2) != 0,
                    self.cpu.read_reg(Reg::R3) != 0,
                );
                let filhos = self
                    .widgets
                    .get(&this)
                    .map(|widget| widget.anexados.clone())
                    .unwrap_or_default();
                // A pilha vai do primeiro inserido ao último. Sem referência, "próximo" começa
                // pelo de baixo e "anterior" pelo de cima. O `GetWidget` não dá referência a
                // quem recebe: é um ponteiro emprestado.
                vizinho_na_pilha(&filhos, referencia, proximo, volta)
            }
            "DefinirVisivel" => {
                let classe = self.widgets.get(&this).map_or(0, |widget| widget.classe);
                if classe == WIDGET_DE_TEXTO {
                    let texto = self.read_aechar(self.cpu.read_reg(Reg::R1))?;
                    if let Some(widget) = self.widgets.get_mut(&this) {
                        widget.texto = texto;
                    }
                    return Ok(Some(SUCCESS));
                }
                let visivel = self.cpu.read_reg(Reg::R1) != 0;
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.visivel = visivel;
                }
                SUCCESS
            }
            // `slot8(this, &saída)`, chamado a cada tique da abertura pela `0x11578`. O que
            // sai dali recebe em seguida um `slot3(widget, 0, 0)` e é solto — e slot 3 num
            // widget é o acessador. Um ponteiro de widget no lugar do seletor não é seletor
            // nenhum, então a leitura que resta é: **o pai é quem recebe o aviso de que o
            // filho mudou**, e o acessador recusa o que não conhece, que é o que ele já faz.
            //
            // **Leitura sem confirmação.** Enquanto ninguém dá a partida na animação, este
            // slot não é chamado, então não há execução que sustente ou derrube a hipótese.
            // Fica escrita para o próximo que passar aqui.
            //
            // Devolver com contagem, porque quem recebe solta logo depois.
            "PegarPai" => {
                let saida = self.cpu.read_reg(Reg::R1);
                let pai = self.widgets.get(&this).map_or(0, |widget| widget.pai);
                if pai != 0 {
                    self.objects.add_ref(pai);
                }
                if saida != 0 {
                    self.cpu.write_u32(saida, pai)?;
                }
                SUCCESS
            }
            // `slot4(this, &tratador)`, visto em `0x11a6c`. O que `r1` aponta é montado logo
            // acima, em `0x11a60`: o objeto do jogo se põe como contexto em `+0x1c` e o
            // endereço da função em `+0x20`. É um registro de tratador de eventos.
            //
            // Guardamos o endereço e não chamamos ninguém: quem dispararia estes eventos é a
            // interface que ainda não desenhamos. Quando ela existir, o tratador está aqui.
            // `slot4(this, &{função, contexto, liberador})`.
            //
            // **Ele devolve o tratador anterior**, escrevendo-o de volta na estrutura que
            // recebeu. É como o BREW encadeia: quem se registra guarda ali quem estava antes e
            // desvia para ele o que não tratar. O `ZPad_Keyboard_Instructions_Form.c` faz
            // exatamente isso — o `0x8f560` lê `[contexto+0x14]` e faz um salto de cauda para
            // lá quando não trata a tecla.
            //
            // Sem escrever nada de volta, a estrutura continuava descrevendo **o próprio**
            // tratador que acabara de se registrar, e o desvio virava recursão infinita: 250
            // milhões de instruções num quadro só, o laço caindo de 897 voltas para duas e a
            // janela congelando. Não havia erro no relatório porque não havia erro — havia um
            // laço.
            //
            // Sem tratador anterior, o que volta é zero, e o `0x8f564` reconhece isso.
            "DefinirTratador" => {
                let onde = self.cpu.read_reg(Reg::R1);
                let novo = match (self.cpu.read_u32(onde), self.cpu.read_u32(onde + 4)) {
                    (Ok(funcao), Ok(contexto)) => (funcao, contexto),
                    _ => return Ok(Some(EBADPARM)),
                };
                let liberador = self.cpu.read_u32(onde + 8)?;
                let (anterior, liberador_anterior) = self
                    .widgets
                    .get(&this)
                    .map_or(((0, 0), 0), |w| (w.tratador, w.liberadores.0));
                self.cpu.write_u32(onde, anterior.0)?;
                self.cpu.write_u32(onde + 4, anterior.1)?;
                self.cpu.write_u32(onde + 8, liberador_anterior)?;
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.tratador = novo;
                    widget.liberadores.0 = liberador;
                }
                SUCCESS
            }
            // `slot5(this, filho, 0, &posição, …)`, visto em `0x11cc0`: o jogo pendura um
            // widget no outro e solta a referência dele em seguida. O retorno é ignorado — a
            // instrução seguinte já sobrescreve `r0`.
            //
            // Aqui o filho é só registrado. Não há árvore de interface para montar enquanto
            // ninguém desenha por ela, e guardar a ligação é o que permite reconhecer, quando
            // isso mudar, que ela já existia.
            // O slot 5 quer dizer duas coisas, e o **terceiro argumento** separa.
            //
            // Com `r2 == 0` é `slot5(this, filho, 0, &posição, …)`: pendura um widget no outro.
            //
            // Com `r2 == 4` é um **getter**: `slot5(this, &saída, 4)`, e a saída são duas
            // meias-palavras. A `0x8fa04` faz essa chamada e em seguida soma as duas
            // (`0x8fa20`), divide por elas em `0x10a18` e multiplica de volta — é o passo de
            // uma lista, e a conta `(altura - 20) / passo * passo + 10` diz quantos itens
            // cabem. Tratando tudo como "pendurar filho", nada era escrito e a soma dava zero:
            // divisão por zero, que o runtime do jogo imprime por semihosting como
            // `Arithmetic exception: Divide By Zero`.
            //
            // O passo que devolvemos é a altura de linha da fonte, que é o que temos. É
            // hipótese, e está anotada: um passo errado erra o layout, um passo zero derruba.
            "AdicionarFilho" if self.cpu.read_reg(Reg::R2) == 4 => {
                /// Duas meias-palavras, somadas pelo chamador.
                const PASSO: i16 = FONT_SIZE as i16;

                let saida = self.cpu.read_reg(Reg::R1);
                if saida != 0 {
                    self.cpu.write_mem(saida, &PASSO.to_le_bytes())?;
                    self.cpu.write_mem(saida + 2, &0i16.to_le_bytes())?;
                }
                self.assumptions.insert(
                    "o passo de lista que o slot 5 pediu foi respondido com a altura da fonte",
                );
                SUCCESS
            }
            "AdicionarFilho"
                if self
                    .widgets
                    .get(&this)
                    .is_some_and(|w| matches!(w.classe, WIDGET_DE_TEXTO | 0x01028e19))
                    && !self.widgets.contains_key(&self.cpu.read_reg(Reg::R1))
                    && !self.images.contains_key(&self.cpu.read_reg(Reg::R1)) =>
            {
                // IWidget::GetExtent(&{cx, cy}) nas interfaces de texto/imagem.
                // A barra de status usa cx do rótulo para posicionar os créditos:
                // em 0x857ec lê cx e em 0x85828 soma 375. Deixar a saída zerada
                // colocava "10" em cima de "Meus Z-Credits".
                let widget = &self.widgets[&this];
                let size = if widget.classe == WIDGET_DE_TEXTO {
                    self.font
                        .as_ref()
                        .map(|font| {
                            (
                                font.width(&widget.texto, FONT_SIZE),
                                font.ascent(FONT_SIZE) + font.descent(FONT_SIZE),
                            )
                        })
                        .unwrap_or((0, 0))
                } else {
                    widget
                        .anexados
                        .iter()
                        .find_map(|id| self.images.get(id))
                        .map(|image| (image.width, image.height))
                        .unwrap_or(widget.tamanho)
                };
                let out = self.cpu.read_reg(Reg::R1);
                if out == 0 {
                    return Ok(Some(EBADPARM));
                }
                self.cpu.write_u32(out, size.0)?;
                self.cpu.write_u32(out + 4, size.1)?;
                SUCCESS
            }
            "AdicionarFilho" => {
                let filho = self.cpu.read_reg(Reg::R1);
                self.anota_posicao(filho)?;
                let ja_anexado = self
                    .widgets
                    .get(&this)
                    .is_some_and(|widget| widget.anexados.contains(&filho));
                if let Some(widget) = self.widgets.get_mut(&this) {
                    if filho != this && !ja_anexado {
                        widget.anexados.push(filho);
                    }
                }
                if let Some(filho) = self.widgets.get_mut(&filho) {
                    filho.pai = this;
                }
                // Quem guarda, segura. É a convenção do BREW inteiro, e aqui ela não é
                // teoria: logo depois de pendurar a imagem no widget, a Z-Wheel solta a
                // referência dela. Sem esta contagem, o objeto morria com a imagem
                // decodificada dentro — e o que sobrava para pintar era nada.
                if !ja_anexado && filho != this {
                    self.objects.add_ref(filho);
                }
                SUCCESS
            }
            // `slot7(this, &{largura, altura})`, visto em `0x11c90` com `640 × 480` — a tela
            // inteira. O jogo ignora o retorno: a instrução seguinte já sobrescreve `r0`.
            "DefinirTamanho" => {
                // O ponteiro nem sempre é ponteiro. Quando a árvore de widgets fica grande, o
                // jogo chama este slot com `r1` apontando para fora do mapa, e ler dali derruba
                // o núcleo ARM — apareceu como `READ_UNMAPPED` no relatório da interface.
                // Ignorar o que não dá para ler é o certo: um tamanho que não veio é um tamanho
                // que não muda.
                let par = self.cpu.read_reg(Reg::R1);
                let tamanho = match (self.cpu.read_u32(par), self.cpu.read_u32(par + 4)) {
                    (Ok(largura), Ok(altura)) => (largura, altura),
                    _ => return Ok(Some(EBADPARM)),
                };
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.tamanho = tamanho;
                }
                SUCCESS
            }
            "Acessador" => {
                let (seletor, id, terceiro) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3),
                );
                match seletor {
                    // Algumas classes usam o próprio endereço de um filho como seletor para
                    // consultar/ligar o estado visual. É uma operação sem valor de retorno;
                    // reconhecer o endereço mantém a árvore avançando e evita tratá-lo como
                    // um código de propriedade desconhecido.
                    _ if self.widgets.contains_key(&seletor) => OK,
                    // Eventos oferecidos primeiro ao root pelo applet (0x7b418).
                    // Um retorno verdadeiro interrompe o despacho em 0x7b420, antes dos
                    // handlers do próprio applet que preenchem as saídas de fonte/recurso.
                    // Sem handler aqui, devolver falso permite que o applet os resolva.
                    0x101 | 0x7b0a | 0x7b0e | 0x7b0f => FALSE,
                    LE => {
                        // **Nem todo `0x800` pede um filho.** O jogo lê e grava pelo mesmo
                        // seletor coisas de tipos diferentes, e o número do item é que diz
                        // qual: os do intervalo `0x5000` guardam **objetos** — a `0x88338` lê
                        // o `0x5000` e o `0x5002` e usa os dois como widgets —, e os de baixo
                        // são **números**.
                        //
                        // Isto não é dedução: o retorno de chamada da imagem lê o item `0x347`,
                        // soma dois e grava de volta. Enquanto a leitura criava um filho para
                        // qualquer item, o que ele somava dois era um **ponteiro nosso**, e o
                        // que ele gravava em `[formulário+0x24]` pelo item `0x414` era outro.
                        // Ponteiro onde o jogo espera número é o começo de uma sequência de
                        // sintomas que não se parecem com a causa.
                        // Os itens do widget são **tipados**, e o corte medido é `0x5000`: de
                        // lá para cima o item guarda objeto, abaixo guarda número.
                        //
                        // Cheguei a pôr o `0x414` como exceção, achando que ele guardava um
                        // widget: o retorno de chamada da imagem o lê para `[formulário+0x24]`,
                        // e a `0x11750` faz `[r0+0x24]->slot6(1)`. Eram campos **diferentes** —
                        // a `0x11750` recebe o **aplicativo**, não o formulário, e o objeto que
                        // ela solta é outro. O que o `0x414` guarda é número mesmo, e a
                        // `0x11668` prova: ela subtrai um e compara.
                        //
                        // A exceção custou caro enquanto durou. Com um ponteiro ali, a
                        // comparação da `0x11674` nunca era verdadeira e a abertura girava para
                        // sempre: doze milhões de idas ao acessador e seis milhões de
                        // temporizadores numa execução só.
                        // No widget de HTML, a `0x161` também é objeto: a tela de ajuda
                        // (`0x31c34`) lê o item e faz nele um `QueryInterface(0x102d691)` sem
                        // conferir o zero. Com um número ali a montagem abortava calada, e o que
                        // já estava criado ficava solto na tela, sem formulário.
                        let objeto = id >= PRIMEIRO_OBJETO
                            || (id == PROP_HTML_OBJETO
                                && self.widgets.get(&this).is_some_and(|w| w.classe == WIDGET_HTML));
                        let valor = match objeto {
                            true => self.filho_do_widget(this, id)?,
                            false => self
                                .widgets
                                .get(&this)
                                .and_then(|widget| widget.propriedades.get(&id).copied())
                                .unwrap_or(0),
                        };
                        if terceiro != 0 {
                            self.cpu.write_u32(terceiro, valor)?;
                        }
                        OK
                    }
                    GRAVA => {
                        // As propriedades vão para a serial, que é onde a instrumentação mora.
                        if self.serial.is_some() {
                            self.registra_serial(format!(
                                "<prop {id:#x}={terceiro:#x} em {this:#x}>"
                            ));
                        }
                        // **Gravar um objeto num item da faixa `0x5000` é pendurá-lo.** É por
                        // aqui que o formulário recebe o que ele mostra: a `0x8ed80` grava
                        // `0x5000` no formulário do z-pad com o container da barra de status e
                        // da foto do controle, e a abertura faz o mesmo com o dela.
                        //
                        // Sem tratar isso como ligação, a árvore ficava partida em três — os
                        // formulários numa, o que se desenha noutra — e nenhuma regra sobre "a
                        // tela atual" tinha como acertar, porque a tecla e o desenho moravam em
                        // árvores diferentes. Guardávamos o número e perdíamos o parentesco.
                        if id >= PRIMEIRO_OBJETO && self.widgets.contains_key(&terceiro) {
                            if let Some(widget) = self.widgets.get_mut(&this) {
                                widget.filhos.insert(id, terceiro);
                            }
                            if let Some(filho) = self.widgets.get_mut(&terceiro) {
                                filho.pai = this;
                            }
                        }
                        if let Some(widget) = self.widgets.get_mut(&this) {
                            widget.propriedades.insert(id, terceiro);
                        }
                        OK
                    }
                    // **`0x711` define o filho em foco de um container, e `0x713` o lê.**
                    //
                    // O par saiu do uso, não de header. A Z-Wheel grava pelo `0x711` um widget
                    // (`0x30000d90`) no container do roller, e é **nesse mesmo container** que ela
                    // pergunta pelo `0x713` quando o jogador confirma — por um invólucro em
                    // `0x3fe5c` que passa `&saída`. Em `0x4eb5c` ela compara a resposta com o
                    // widget do item que acha selecionado (`[[r4+0x24]+0xb0]`) e só executa a
                    // ação se os dois baterem. Enquanto o `0x713` era recusado, confirmar não
                    // fazia nada — nem "Jogar", nem "Ajuda".
                    //
                    // O `0x711` é o `EVT_WDG_MOVEFOCUS`, e o argumento pode não ser um widget: os
                    // valores de `1` a `4` são `WIDGET_FOCUS_FIRST`, `LAST`, `NEXT` e `PREV`, e o
                    // `0` é `NONE`. É com o `NEXT` e o `PREV` que a lista de jogos passa o foco da
                    // barra de abas para as capas (`0x4025c`, chamado com `3`); guardar o `3` como
                    // se fosse o filho em foco deixava o foco em lugar nenhum e nada se destacava.
                    FOCO_DEFINE if !self.widgets.contains_key(&terceiro) && terceiro <= FOCO_ANTERIOR => {
                        u32::from(self.move_foco(this, terceiro)?)
                    }
                    // Com um widget de verdade, quem sai e quem entra também são avisados. É assim
                    // que o menu principal dá foco à roda (`MOVEFOCUS(roda)` aos 7 s), e é o aviso
                    // que liga a moldura azul do item escolhido.
                    FOCO_DEFINE => {
                        let anterior = self
                            .widgets
                            .get(&this)
                            .and_then(|widget| widget.propriedades.get(&FOCO_DEFINE).copied())
                            .unwrap_or(0);
                        if let Some(widget) = self.widgets.get_mut(&this) {
                            widget.propriedades.insert(FOCO_DEFINE, terceiro);
                        }
                        if anterior != terceiro {
                            if self.widgets.contains_key(&anterior) {
                                self.avisa_widget(anterior, MARCA_FOCO, 0)?;
                            }
                            self.avisa_widget(terceiro, MARCA_FOCO, 1)?;
                        }
                        OK
                    }
                    // `0x702` responde **um byte** — o chamador lê com `ldrb` — e os dois usos o
                    // tratam como "pode": em `0x394b4` a ação do item só roda se ele for
                    // verdadeiro, e em `0x399a0` o widget que entra em foco só recebe o `0x700`
                    // (`acessador(widget, 0x700, 1, 0)`) se ele for verdadeiro. A leitura é
                    // "habilitado para foco", e todo widget nosso está habilitado; o `0x700`
                    // fica guardado para quando algo o ler.
                    HABILITADO => {
                        if terceiro != 0 {
                            self.cpu.write_mem(terceiro, &[1])?;
                        }
                        self.assumptions.insert(
                            "o seletor 0x702 do widget respondeu habilitado — a leitura é pelo uso, sem header",
                        );
                        OK
                    }
                    // Marcar foco num widget é o que dá foco a ele **dentro do pai**. A grade de
                    // jogos da Z-Wheel nunca grava o `0x711` no container dela: ao escolher um
                    // item, ela chama `acessador(item, 0x700, 1, 0)` no widget do item (`0x399d0`)
                    // e depois, ao confirmar, pergunta pelo `0x713` ao **pai** desse item. Sem
                    // levar o foco ao pai, a pergunta voltava nula e a confirmação não abria o
                    // jogo — medido: o item `0x30001250` tem pai `0x30000f10`, e é no `0x30000f10`
                    // que o `0x713` é feito.
                    //
                    // **O aviso passa primeiro pelo tratador que o jogo registrou**, como todo
                    // `HandleEvent` de widget no BREW. É ele que liga o estado de foco do widget
                    // do jogo: o roller da tela inicial só desenha a moldura azul do item escolhido
                    // quando o tratador dele (`0x6089c`) recebe o `EVT_WDG_SETFOCUS`. Anotar o
                    // foco sem avisá-lo deixava a roda sem moldura nenhuma.
                    MARCA_FOCO => {
                        self.avisa_widget(this, MARCA_FOCO, id)?;
                        let pai = self.widgets.get(&this).map_or(0, |widget| widget.pai);
                        if let Some(widget) = self.widgets.get_mut(&this) {
                            widget.propriedades.insert(MARCA_FOCO, id);
                        }
                        if pai != 0 && pai != this {
                            if let Some(container) = self.widgets.get_mut(&pai) {
                                match id {
                                    0 => {
                                        if container.propriedades.get(&FOCO_DEFINE) == Some(&this) {
                                            container.propriedades.remove(&FOCO_DEFINE);
                                        }
                                    }
                                    _ => {
                                        container.propriedades.insert(FOCO_DEFINE, this);
                                    }
                                }
                            }
                        }
                        OK
                    }
                    // Quem lê o foco **solta o que recebeu** — o tratador da grade faz o
                    // `Release` em `0x39504` —, então a resposta leva uma referência nossa, como
                    // todo getter de interface do BREW. Sem ela, cada consulta tirava uma
                    // contagem que ninguém tinha posto. E só sai objeto vivo: um endereço que já
                    // voltou para a lista de livres é pior que nulo.
                    FOCO_LE => {
                        let foco = self
                            .widgets
                            .get(&this)
                            .and_then(|widget| widget.propriedades.get(&FOCO_DEFINE).copied())
                            .filter(|&foco| self.widgets.contains_key(&foco))
                            .unwrap_or(0);
                        if foco != 0 {
                            self.objects.add_ref(foco);
                        }
                        if terceiro != 0 {
                            self.cpu.write_u32(terceiro, foco)?;
                        }
                        OK
                    }
                    outro => {
                        let quem = self.cpu.read_reg(Reg::Lr);
                        let classe = self.widgets.get(&this).map_or(0, |w| w.classe);
                        self.missing_apis.insert(format!(
                            "IWidget::Acessador seletor {outro:#x} (classe {classe:#x}, de {quem:#010x}, {id:#x}, {terceiro:#x})"
                        ));
                        0
                    }
                }
            }
            _ => OK,
        };
        Ok(Some(result))
    }

    /// Atende a fonte TrueType. Ver [`Interface::Typeface`].
    ///
    /// O único método com corpo é o slot 4, que a `0x7bfc8` chama assim:
    /// `slot4(this, a, b, c, &saída)`, com a saída no **primeiro argumento de pilha** — os
    /// quatro registradores já estão ocupados. Zero é sucesso, e o que sai é o objeto de fonte,
    /// que o jogo entrega ao slot 9 de um contêiner e depois usa.
    ///
    /// Damos um widget. Não é palpite de conveniência: na extensão de interface do console tudo
    /// que entra numa árvore de tela é widget, e o que o jogo faz com o objeto em seguida — um
    /// `AddRef` e um slot 6 — é vocabulário de widget. Se ele pedir algo que um widget não tem,
    /// o slot aparece no relatório, que é como o resto disto foi descoberto.
    pub(super) fn typeface_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Typeface.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "CriarFonte" => {
                let saida = self.stack_arg(0)?;
                let fonte = self.new_object(Interface::Widget)?;
                if fonte == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.widgets.insert(
                    fonte,
                    Widget {
                        visivel: true,
                        ..Widget::default()
                    },
                );
                if saida != 0 {
                    self.cpu.write_u32(saida, fonte)?;
                }
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// De onde saiu a fonte que está desenhando o texto, se houver uma.
    pub fn font_source(&self) -> Option<&str> {
        self.font.as_ref().map(|f| f.source.as_str())
    }

    #[cfg(debug_assertions)]
    pub fn retrato_widgets(&self) -> (usize, usize, usize) {
        let total = self.widgets.len();
        let maior = self
            .widgets
            .values()
            .map(|w| w.anexados.len())
            .max()
            .unwrap_or(0);
        let repetidos = self
            .widgets
            .values()
            .map(|w| {
                let unicos: std::collections::HashSet<_> = w.anexados.iter().collect();
                w.anexados.len() - unicos.len()
            })
            .sum();
        (total, maior, repetidos)
    }
}

impl<C: CpuBackend> Machine<C> {
    /// O `EVT_WDG_MOVEFOCUS` com um dos valores especiais: `0` tira o foco, `1` e `2` levam às
    /// pontas, `3` e `4` ao próximo e ao anterior que aceite foco. Devolve se o foco mudou.
    ///
    /// Quem sai recebe `EVT_WDG_SETFOCUS` com falso, quem entra com verdadeiro — é esse aviso
    /// que faz o widget do jogo se desenhar como selecionado. Passando da ponta o foco fica onde
    /// está e a resposta é falsa, para a tecla seguir adiante.
    fn move_foco(&mut self, container: u32, pedido: u32) -> Result<bool, CpuError> {
        const MOVE_FOCO: u32 = 0x711;
        const DEFINE_FOCO: u32 = 0x700;
        let Some(widget) = self.widgets.get(&container) else {
            return Ok(false);
        };
        let filhos = widget.anexados.clone();
        let atual = widget
            .propriedades
            .get(&MOVE_FOCO)
            .copied()
            .filter(|foco| filhos.contains(foco))
            .unwrap_or(0);
        let ordem: Vec<u32> = match pedido {
            0 => Vec::new(),
            1 => filhos.clone(),
            2 => filhos.iter().rev().copied().collect(),
            _ => {
                let posicao = filhos.iter().position(|&f| f == atual);
                match (pedido, posicao) {
                    (3, Some(i)) => filhos[i + 1..].to_vec(),
                    (3, None) => filhos.clone(),
                    (_, Some(i)) => filhos[..i].iter().rev().copied().collect(),
                    (_, None) => filhos.iter().rev().copied().collect(),
                }
            }
        };
        let mut novo = 0;
        for candidato in ordem {
            if self.aceita_foco(candidato)? {
                novo = candidato;
                break;
            }
        }
        if pedido != 0 && novo == 0 {
            return Ok(false);
        }
        if atual != 0 && atual != novo {
            self.avisa_widget(atual, DEFINE_FOCO, 0)?;
        }
        if let Some(widget) = self.widgets.get_mut(&container) {
            match novo {
                0 => widget.propriedades.remove(&MOVE_FOCO),
                novo => widget.propriedades.insert(MOVE_FOCO, novo),
            };
        }
        if novo != 0 && novo != atual {
            self.avisa_widget(novo, DEFINE_FOCO, 1)?;
        }
        Ok(true)
    }

    /// Se o filho aceita foco, perguntado ao tratador dele com o `EVT_WDG_CANTAKEFOCUS`.
    ///
    /// Só entra quem se desenha e tem tratador do jogo: o fundo da lista de jogos é um
    /// container sem desenho próprio, e o nosso `0x702` responde "pode" a todo mundo — deixá-lo
    /// candidato poria o foco nele ao voltar das capas.
    fn aceita_foco(&mut self, widget: u32) -> Result<bool, CpuError> {
        const PODE_TER_FOCO: u32 = 0x702;
        let Some(dados) = self.widgets.get(&widget) else {
            return Ok(false);
        };
        let ((funcao, contexto), desenho) = (dados.tratador, dados.desenho.0);
        if !dados.visivel || funcao == 0 || desenho == 0 {
            return Ok(false);
        }
        let resposta = self.malloc(4)?;
        if resposta == 0 {
            return Ok(false);
        }
        self.cpu.write_u32(resposta, 0)?;
        let _ = self.call_guest_aninhado(funcao, [contexto, PODE_TER_FOCO, 0, resposta], QSORT_BUDGET)?;
        let pode = self.cpu.read_u32(resposta)? & 0xff != 0;
        self.heap.free(resposta);
        Ok(pode)
    }

    /// Entrega um evento ao tratador que o jogo registrou no widget, se houver um.
    ///
    /// Um tratador costuma devolver ao widget o que não trata, e esse caminho volta para cá: a
    /// trava por widget é o que impede o mesmo evento de dar voltas entre os dois.
    fn avisa_widget(&mut self, widget: u32, evento: u32, parametro: u32) -> Result<(), CpuError> {
        let Some((funcao, contexto)) = self.widgets.get(&widget).map(|w| w.tratador) else {
            return Ok(());
        };
        if funcao == 0 || !self.widgets_avisando.insert(widget) {
            return Ok(());
        }
        let saida = self.call_guest_aninhado(funcao, [contexto, evento, parametro, 0], QSORT_BUDGET);
        self.widgets_avisando.remove(&widget);
        saida?;
        Ok(())
    }
}

impl<C: CpuBackend> Machine<C> {
    /// Solta uma referência de um widget e, se era a última, desmonta-o: chama os liberadores
    /// que o jogo registrou e solta, do mesmo jeito, tudo o que ele segurava. Devolve a contagem
    /// que sobrou.
    ///
    /// **Quem guarda, solta.** Os filhos que o acessador cria por conta própria e os que o
    /// `AdicionarFilho` pendura levaram uma contagem nossa; sumir com o widget sem devolvê-la
    /// vaza os dois — e a Z-Wheel em modo de atração monta e desmonta a abertura sem parar.
    ///
    /// **O liberador de cada trio é chamado quando o widget morre**, como o `HandlerDesc` do BREW
    /// manda. É ele que desmonta o que o jogo pendurou no widget: o do roller cancela o timer de
    /// animação de 40 ms (`0x2fc88`), que se rearma sozinho. E a descida vale para os filhos
    /// também: soltá-los só pela contagem deixava o roller fora do mapa de widgets, sem liberador
    /// chamado, com o timer armado sobre memória que a Z-Wheel já tinha soltado.
    pub(super) fn solta_widget(&mut self, alvo: u32) -> Result<u32, CpuError> {
        /// A árvore vem do jogo; um ciclo não pode prender a desmontagem.
        const TETO: usize = 4096;
        let restantes = self.objects.release(alvo);
        if restantes != 0 {
            return Ok(restantes);
        }
        let mut pendentes = vec![alvo];
        let mut visitados = 0;
        while let Some(morto) = pendentes.pop() {
            visitados += 1;
            if visitados > TETO {
                break;
            }
            let Some(widget) = self.widgets.remove(&morto) else {
                continue;
            };
            for (liberador, contexto) in [
                (widget.liberadores.0, widget.tratador.1),
                (widget.liberadores.1, widget.desenho.1),
            ] {
                if liberador != 0 {
                    let _ = self.call_guest_aninhado(liberador, [contexto, 0, 0, 0], QSORT_BUDGET)?;
                }
            }
            for filho in widget
                .filhos
                .into_values()
                .chain(widget.anexados)
                .chain(widget.modelos.into_values())
            {
                if self.objects.release(filho) == 0 {
                    pendentes.push(filho);
                }
            }
        }
        Ok(0)
    }
}

/// O `IContainer_GetWidget`: o vizinho de `referencia` na pilha, ou uma ponta dela.
///
/// A pilha vai do primeiro inserido (baixo) ao último (cima). Sem referência, `proximo` começa
/// por baixo e o contrário por cima. Passando da ponta, `volta` dá a volta; sem ele, zero.
fn vizinho_na_pilha(filhos: &[u32], referencia: u32, proximo: bool, volta: bool) -> u32 {
    if filhos.is_empty() {
        return 0;
    }
    let ultimo = filhos.len() - 1;
    let indice = match (filhos.iter().position(|&w| w == referencia), proximo) {
        (None, true) => Some(0),
        (None, false) => Some(ultimo),
        (Some(i), true) if i < ultimo => Some(i + 1),
        (Some(_), true) => volta.then_some(0),
        (Some(0), false) => volta.then_some(ultimo),
        (Some(i), false) => Some(i - 1),
    };
    indice.map_or(0, |i| filhos[i])
}

#[cfg(test)]
mod testes_do_container {
    use super::vizinho_na_pilha;

    #[test]
    fn percorre_a_pilha_nos_dois_sentidos_e_para_na_ponta() {
        let pilha = [10, 20, 30];
        assert_eq!(vizinho_na_pilha(&pilha, 0, false, false), 30, "sem referência, o de cima");
        assert_eq!(vizinho_na_pilha(&pilha, 0, true, false), 10, "sem referência, o de baixo");
        assert_eq!(vizinho_na_pilha(&pilha, 30, false, false), 20);
        assert_eq!(vizinho_na_pilha(&pilha, 10, false, false), 0, "a ponta termina o laço");
        assert_eq!(vizinho_na_pilha(&pilha, 10, false, true), 30, "com volta, dá a volta");
        assert_eq!(vizinho_na_pilha(&pilha, 20, true, false), 30);
        assert_eq!(vizinho_na_pilha(&[], 0, true, true), 0);
    }
}

/// Distância entre a borda do widget de HTML e o texto, em pixels.
const MARGEM_DO_HTML: i32 = 8;

/// Onde fica o HTML de placeholder da tela de ajuda: no aparelho emulado, ao lado dos outros
/// arquivos da Z-Wheel.
pub fn caminho_do_html() -> std::path::PathBuf {
    crate::loader::archive::device_dir()
        .join("z-wheel")
        .join("ajuda.html")
}

/// O conteúdo inicial do placeholder, gravado na primeira vez que a tela de ajuda aparece.
const HTML_PADRAO: &str = "<html>
<body>
<h1>Ajuda</h1>
<p>Esta página é um espaço reservado do Zeebx.</p>
<p>O console mostrava aqui as páginas de ajuda da Z-Wheel. Enquanto o visualizador de HTML
não existe no emulador, o texto vem deste arquivo, que pode ser editado livremente.</p>
</body>
</html>
";

/// O texto do placeholder, criando o arquivo com o conteúdo padrão quando ele não existe.
fn le_placeholder_html() -> String {
    let caminho = caminho_do_html();
    match std::fs::read_to_string(&caminho) {
        Ok(texto) => texto,
        Err(_) => {
            if let Some(pasta) = caminho.parent() {
                let _ = std::fs::create_dir_all(pasta);
            }
            let _ = std::fs::write(&caminho, HTML_PADRAO);
            HTML_PADRAO.to_string()
        }
    }
}

/// Os parágrafos de um HTML simples, como texto corrido.
///
/// Não é um analisador: blocos (`p`, `br`, `div`, `li`, títulos) viram quebra de parágrafo, o
/// resto das tags some, espaços se juntam e as entidades mais comuns são traduzidas. É o
/// suficiente para um texto de ajuda escrito à mão.
pub fn texto_do_html(html: &str) -> Vec<String> {
    const BLOCOS: [&str; 12] = ["p", "/p", "br", "br/", "div", "/div", "li", "h1", "/h1", "h2", "/h2", "/li"];
    let mut paragrafos = Vec::new();
    let mut atual = String::new();
    let mut resto = html;
    let fecha = |atual: &mut String, paragrafos: &mut Vec<String>| {
        let limpo = atual.split_whitespace().collect::<Vec<_>>().join(" ");
        if !limpo.is_empty() {
            paragrafos.push(limpo);
        }
        atual.clear();
    };
    while let Some(abre) = resto.find('<') {
        atual.push_str(&resto[..abre]);
        let Some(fim) = resto[abre..].find('>') else {
            resto = "";
            break;
        };
        let tag = resto[abre + 1..abre + fim].trim().to_ascii_lowercase();
        let nome = tag.split_whitespace().next().unwrap_or("").trim_end_matches('/');
        if BLOCOS.contains(&nome) || BLOCOS.contains(&tag.as_str()) {
            fecha(&mut atual, &mut paragrafos);
        }
        resto = &resto[abre + fim + 1..];
    }
    atual.push_str(resto);
    fecha(&mut atual, &mut paragrafos);
    paragrafos
        .into_iter()
        .map(|p| {
            p.replace("&nbsp;", " ")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&amp;", "&")
        })
        .collect()
}

/// Quebra um parágrafo em linhas que caibam em `largura`, pela medida da fonte.
fn quebra_linhas(paragrafo: &str, largura: u32, mede: impl Fn(&str) -> u32) -> Vec<String> {
    let mut linhas = Vec::new();
    let mut linha = String::new();
    for palavra in paragrafo.split_whitespace() {
        let candidata = match linha.is_empty() {
            true => palavra.to_string(),
            false => format!("{linha} {palavra}"),
        };
        if !linha.is_empty() && mede(&candidata) > largura {
            linhas.push(std::mem::take(&mut linha));
            linha = palavra.to_string();
        } else {
            linha = candidata;
        }
    }
    if !linha.is_empty() {
        linhas.push(linha);
    }
    linhas
}

#[cfg(test)]
mod testes_do_html {
    use super::{quebra_linhas, texto_do_html};

    #[test]
    fn blocos_viram_paragrafos_e_tags_somem() {
        let html = "<h1>Ajuda</h1><p>Um <b>texto</b>\n  com   espaços &amp; tags.</p>linha<br>outra";
        assert_eq!(
            texto_do_html(html),
            ["Ajuda", "Um texto com espaços & tags.", "linha", "outra"]
        );
    }

    #[test]
    fn a_linha_quebra_pela_largura() {
        let linhas = quebra_linhas("aa bb cc", 5, |t| t.len() as u32);
        assert_eq!(linhas, ["aa bb", "cc"]);
    }
}
