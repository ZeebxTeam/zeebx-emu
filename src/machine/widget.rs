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
            [context, input::EVT_KEY, input::avk::ZERO, 0],
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
            .filter(|(_, no)| {
                no.desenho.0 != 0 && no.visivel
            })
            .map(|(&endereco, no)| (endereco, no.desenho))
            .collect();
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
    pub(super) fn raiz_mais_nova(&self, com_filhos: &std::collections::HashSet<u32>) -> Option<u32> {
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
        /// Sucesso para esta classe. Não é o `SUCCESS` do BREW — ver acima.
        const OK: u32 = 1;
        /// O terceiro seletor, que grava sem número de item. Ver o ramo dele abaixo.
        const ELEVEN: u32 = 0x711;

        let Some(name) = Interface::Widget.method(slot) else {
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
                    // **Quem guarda, solta.** Os filhos que o acessador cria por conta própria
                    // e os que o `AdicionarFilho` pendura levaram uma contagem nossa; sumir com
                    // o widget sem devolvê-la vaza os dois. E vaza rápido: a Z-Wheel em modo de
                    // atração monta e desmonta a abertura sem parar, e o mil e vinte e quatro
                    // objetos da região acabavam numa volta só do laço.
                    if let Some(widget) = self.widgets.remove(&this) {
                        for filho in widget
                            .filhos
                            .into_values()
                            .chain(widget.anexados)
                            .chain(widget.modelos.into_values())
                        {
                            self.objects.release(filho);
                        }
                    }
                }
                restantes
            }
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
            // A terceira palavra do trio é a `0x52574`, o liberador. Não guardamos: nada aqui
            // destrói um registro de desenho.
            "Slot13" => {
                // Leitura em 0x24100..0x24198 do tectoy.mod: (&bitmap, w, h),
                // seguida de QueryInterface no bitmap devolvido. O widget foi usado
                // como interface de bitmap pelo QueryInterface permissivo acima.
                let slot = crate::aee_slots::BITMAP
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
                let anterior = self.widgets.get(&this).map_or((0, 0), |w| w.desenho);
                self.cpu.write_u32(onde, anterior.0)?;
                self.cpu.write_u32(onde + 4, anterior.1)?;
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.desenho = novo;
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
                let anterior = self.widgets.get(&this).map_or((0, 0), |w| w.tratador);
                self.cpu.write_u32(onde, anterior.0)?;
                self.cpu.write_u32(onde + 4, anterior.1)?;
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.tratador = novo;
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
                        let valor = match id >= PRIMEIRO_OBJETO {
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
                    // O terceiro seletor tem número: é o `0x711`, e o firmware o usa por um
                    // invólucro em `0x1035f23c` irmão do de gravar — `acessador(this, 0x711, 0,
                    // valor)`, mesma convenção invertida. O que ele quer dizer ainda não
                    // sabemos, e recusar continua sendo o certo; o que muda é o relatório
                    // dizer **qual**, em vez de "um seletor".
                    // **Aceitar, e não recusar.** O que o `0x711` quer dizer continua sem
                    // resposta, mas a convenção aqui é invertida: recusar é dizer ao jogo que a
                    // chamada falhou, e isso é uma afirmação mais forte do que "não sei".
                    // Guardamos o valor num item próprio e seguimos.
                    ELEVEN => {
                        if let Some(widget) = self.widgets.get_mut(&this) {
                            widget.propriedades.insert(ELEVEN, terceiro);
                        }
                        self.assumptions
                            .insert("o seletor 0x711 do widget foi aceito sem saber o que ele faz");
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
