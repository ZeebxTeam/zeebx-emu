//! A tela do jogo, e a cola em C++ que ela chama.
//!
//! O quadro vai para o scene graph do Qt Quick pelo `ItemDoQuadro` (`cpp/quadro.h`), que é a base
//! da `TelaDoJogo`: com o rasterizador na placa, a textura dele entra embrulhada, sem voltar à CPU
//! e na resolução interna; no resto, a tela RGB565 sobe como imagem — o `QImage::Format_RGB16`
//! **é** RGB565 — e só quando mudou, a mesma regra do egui. O jogo em si — abrir, rodar, trocar —
//! mora no [`super::nucleo`].

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qsizef.h");
        type QSizeF = cxx_qt_lib::QSizeF;
        include!("cxx-qt-lib/qrectf.h");
        type QRectF = cxx_qt_lib::QRectF;
        include!("cxx-qt-lib/qimage.h");
        type QImage = cxx_qt_lib::QImage;
        include!("cxx-qt-lib/qlist.h");
        type QList_i32 = cxx_qt_lib::QList<i32>;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    unsafe extern "C++Qt" {
        include!("quadro.h");
        #[qobject]
        type ItemDoQuadro;
    }

    // Ver `cpp/gl_qt.h`.
    #[namespace = "zeebx"]
    unsafe extern "C++" {
        include!("gl_qt.h");
        fn prepara_gl();
        fn gl_cria() -> bool;
        fn gl_torna_corrente() -> bool;
        fn gl_solta();
        fn gl_destroi();
        fn aplica_icone();
        fn gl_funcao(nome: &str) -> usize;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = ItemDoQuadro]
        #[qproperty(QString, estado)]
        #[qproperty(QString, parou)]
        #[qproperty(QString, depuracao)]
        #[qproperty(QList_i32, linha_do_tempo, cxx_name = "linhaDoTempo")]
        #[qproperty(QString, aviso_titulo, cxx_name = "avisoTitulo")]
        #[qproperty(QString, aviso_estado, cxx_name = "avisoEstado")]
        #[qproperty(bool, aviso_parado, cxx_name = "avisoParado")]
        #[qproperty(f64, aviso_giro, cxx_name = "avisoGiro")]
        #[qproperty(QString, screenshot_texto, cxx_name = "screenshotTexto")]
        #[qproperty(bool, screenshot_falhou, cxx_name = "screenshotFalhou")]
        #[qproperty(QString, screenshot_pasta, cxx_name = "screenshotPasta")]
        #[qproperty(i32, screenshot_serie, cxx_name = "screenshotSerie")]
        #[qproperty(QString, estado_texto, cxx_name = "estadoTexto")]
        #[qproperty(bool, estado_falhou, cxx_name = "estadoFalhou")]
        #[qproperty(i32, estado_serie, cxx_name = "estadoSerie")]
        #[qproperty(QString, velocidade)]
        #[qproperty(QString, indicadores)]
        type TelaDoJogo = super::TelaDoJogoRust;

        /// Uma volta: entrada, emulação e, se a tela mudou, um quadro novo.
        #[qinvokable]
        fn passo(self: Pin<&mut Self>);

        /// Um `Qt::Key` apertado ou solto.
        #[qinvokable]
        fn tecla(self: Pin<&mut Self>, codigo: i32, apertada: bool);

        /// A janela perdeu o foco: nenhuma tecla continua apertada.
        #[qinvokable]
        fn solta(self: Pin<&mut Self>);

        /// Fecha o jogo aberto. A janela do jogo chama isto ao ser fechada.
        #[qinvokable]
        fn fecha(self: Pin<&mut Self>);

        /// Se o `Qt::Key` é o atalho do screenshot. Ver `atalhos.screenshot`.
        #[qinvokable]
        #[cxx_name = "ehAtalhoDeScreenshot"]
        fn eh_atalho_de_screenshot(self: &Self, codigo: i32) -> bool;

        /// Grava o quadro que está na tela. `carimbo` é a hora local, pronta para o nome do
        /// arquivo. O aviso chega depois, pela `screenshotSerie`.
        #[qinvokable]
        fn screenshot(self: Pin<&mut Self>, carimbo: &QString);

        /// Se o `Qt::Key`, com os `Qt::KeyboardModifiers` do evento, é um atalho do save state.
        /// Perguntado antes de tudo na janela do jogo: a combinação bate só com os mesmos
        /// modificadores, e o Shift+F9 de um save state não pode virar o F9 do screenshot.
        #[qinvokable]
        #[cxx_name = "ehAtalhoDeEstado"]
        fn eh_atalho_de_estado(self: &Self, codigo: i32, modificadores: i32) -> bool;

        /// Faz o que o atalho do save state pede. O aviso chega pela `estadoSerie`.
        #[qinvokable]
        #[cxx_name = "atalhoDeEstado"]
        fn atalho_de_estado(self: Pin<&mut Self>, codigo: i32, modificadores: i32);

        /// O save state pelo menu: `acao` 0 salva, 1 carrega e 2 desfaz o último carregar, no
        /// `slot` (de zero), que passa a ser o atual. O aviso chega pela `estadoSerie`.
        #[qinvokable]
        #[cxx_name = "estadoPeloMenu"]
        fn estado_pelo_menu(self: Pin<&mut Self>, acao: i32, slot: i32);

        /// Os slots do jogo aberto, um por linha, para o menu.
        #[qinvokable]
        #[cxx_name = "slotsDeEstado"]
        fn slots_de_estado(self: &Self) -> QStringList;

        /// Quais slots têm estado: 1 ou 0 por slot, na ordem. O Carregar do menu desabilita os
        /// vazios.
        #[qinvokable]
        #[cxx_name = "slotsOcupados"]
        fn slots_ocupados(self: &Self) -> QList_i32;

        #[qinvokable]
        #[cxx_name = "slotAtual"]
        fn slot_atual(self: &Self) -> i32;

        #[qinvokable]
        #[cxx_name = "podeDesfazerCarregar"]
        fn pode_desfazer_carregar(self: &Self) -> bool;

        /// Liga ou desliga o fast-forward pelo menu, e devolve como ficou.
        #[qinvokable]
        #[cxx_name = "alternaAvanco"]
        fn alterna_avanco(self: Pin<&mut Self>) -> bool;

        #[qinvokable]
        #[cxx_name = "avancoFixo"]
        fn avanco_fixo(self: &Self) -> bool;

        /// Pausa ou retoma o jogo. Devolve se ficou pausado.
        #[qinvokable]
        fn pausa(self: Pin<&mut Self>) -> bool;

        /// Se o `Qt::Key` é o atalho de reiniciar. Ver `atalhos.reiniciar`.
        #[qinvokable]
        #[cxx_name = "ehAtalhoDeReinicio"]
        fn eh_atalho_de_reinicio(self: &Self, codigo: i32) -> bool;

        /// Recomeça o jogo aberto. Devolve se recomeçou; falhando, o de antes continua, e a
        /// linha de estado diz por quê.
        #[qinvokable]
        fn reinicia(self: Pin<&mut Self>) -> bool;

        /// Relê o título do jogo aberto para a linha de estado. A janela chama isto ao aparecer.
        #[qinvokable]
        fn atualiza(self: Pin<&mut Self>);

        /// Como a janela do jogo abre: 0 em janela, 1 maximizada, 2 em tela cheia. Ver
        /// `graphics.janela_do_jogo`.
        #[qinvokable]
        #[cxx_name = "modoDaJanela"]
        fn modo_da_janela(self: &Self) -> i32;

        /// A largura em que a janela do jogo estava quando foi fechada, em pontos, ou zero
        /// sem tamanho guardado. Ver `graphics.tamanho_da_janela_do_jogo`.
        #[qinvokable]
        #[cxx_name = "larguraDaJanela"]
        fn largura_da_janela(self: &Self) -> f64;

        /// A altura em que a janela do jogo estava quando foi fechada, em pontos, ou zero
        /// sem tamanho guardado. Ver `graphics.tamanho_da_janela_do_jogo`.
        #[qinvokable]
        #[cxx_name = "alturaDaJanela"]
        fn altura_da_janela(self: &Self) -> f64;

        /// Guarda o tamanho atual da janela do jogo. Como na principal: depois de um tempo
        /// sem mudar de tamanho, e ao fechar. Ver `graphics.tamanho_da_janela_do_jogo`.
        #[qinvokable]
        #[cxx_name = "guardaTamanhoDaJanela"]
        fn guarda_tamanho_da_janela(self: &Self, largura: f64, altura: f64);

        /// O jogo saiu sozinho e não há para onde voltar: a janela fecha, como a do egui.
        #[qsignal]
        fn fechou(self: Pin<&mut Self>);

        #[inherit]
        fn size(self: &Self) -> QSizeF;

        #[inherit]
        #[cxx_name = "mostraTextura"]
        fn mostra_textura(
            self: Pin<&mut Self>,
            textura: u32,
            recorte_u: f32,
            recorte_v: f32,
            destino: QRectF,
            suave: bool,
        );

        #[inherit]
        #[cxx_name = "mostraImagem"]
        fn mostra_imagem(self: Pin<&mut Self>, imagem: &QImage, destino: QRectF, suave: bool);

        #[inherit]
        fn posiciona(self: Pin<&mut Self>, destino: QRectF, suave: bool);

        #[inherit]
        fn esvazia(self: Pin<&mut Self>);
    }

    // Sem isto o construtor gerado repassa o pai `QObject*` à base, que quer um `QQuickItem*`.
    impl cxx_qt::Initialize for TelaDoJogo {}
}

use std::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QImage, QImageFormat, QList, QRectF, QString, QStringList};

use zeebx::egui::Key;
use zeebx::ui::partida::enquadra;
use zeebx::ui::settings::{ModoDaJanela, Modificadores};

use super::nucleo::{self, Quadro};

#[derive(Default)]
pub struct TelaDoJogoRust {
    estado: QString,
    parou: QString,
    /// Os textos do painel de depuração numa linha; vazio com o painel desligado.
    depuracao: QString,
    /// A linha do tempo do painel, em pares `velocidade, quadros`, da amostra mais antiga para a
    /// mais nova.
    linha_do_tempo: QList<i32>,
    /// O aviso de calibração do Boomerang: o título vazio esconde o aviso. O giro é em graus,
    /// que é o que a `rotation` do QML usa.
    aviso_titulo: QString,
    aviso_estado: QString,
    aviso_parado: bool,
    aviso_giro: f64,
    /// O aviso do último screenshot. A série muda a cada um, para o QML mostrar de novo mesmo
    /// quando o texto é igual ao do anterior.
    screenshot_texto: QString,
    screenshot_falhou: bool,
    screenshot_pasta: QString,
    screenshot_serie: i32,
    /// O aviso do último atalho do save state, do mesmo jeito que o do screenshot.
    estado_texto: QString,
    estado_falhou: bool,
    estado_serie: i32,
    /// A velocidade e os quadros por segundo, para a barra de status da principal.
    velocidade: QString,
    /// O "▶▶ 3x" do fast-forward e o "Turbo"; vazio esconde.
    indicadores: QString,
    /// A tela que foi para o `ItemDoQuadro` como imagem: série e escritas. Igual, não há o que
    /// subir. `None` quando o que está lá é a textura da placa, ou nada.
    chave: Option<(u64, u64)>,
}

impl cxx_qt::Initialize for qobject::TelaDoJogo {
    fn initialize(self: Pin<&mut Self>) {}
}

/// O que a volta manda o `ItemDoQuadro` fazer. Sai do [`nucleo::com`] pronto, porque o item só
/// pode ser chamado depois: ver a regra no topo do `nucleo.rs`.
enum Mostra {
    Textura { textura: u32, recorte: [f32; 2] },
    Imagem { chave: (u64, u64), imagem: QImage },
    MesmaImagem,
    Nada,
}

/// O modo de uma janela como o QML o lê: 0 em janela, 1 maximizada, 2 em tela cheia.
pub fn modo(modo: ModoDaJanela) -> i32 {
    match modo {
        ModoDaJanela::Janela => 0,
        ModoDaJanela::Maximizada => 1,
        ModoDaJanela::TelaCheia => 2,
    }
}

/// O retângulo de tamanho `tamanho`, centrado na área.
fn centrado(area: [f32; 2], tamanho: [f32; 2]) -> QRectF {
    QRectF::new(
        f64::from((area[0] - tamanho[0]) / 2.0),
        f64::from((area[1] - tamanho[1]) / 2.0),
        f64::from(tamanho[0]),
        f64::from(tamanho[1]),
    )
}

impl qobject::TelaDoJogo {
    pub fn passo(mut self: Pin<&mut Self>) {
        let tamanho = self.size();
        let area = [tamanho.width() as f32, tamanho.height() as f32];
        let chave_atual = self.rust().chave;
        let (volta, mostra, destino, suave, depuracao) = nucleo::com(|nucleo| {
            let volta = nucleo.passo(area);
            let depuracao = nucleo.depuracao();
            // Copiados antes: o `quadro` empresta o núcleo inteiro para trazer o quadro da placa.
            let (scaling, keep_aspect, smooth) = {
                let graficos = &nucleo.valendo().graphics;
                (graficos.scaling, graficos.keep_aspect, graficos.smooth)
            };
            // O quadro largo experimental tem a proporção dele; o resto é o 4:3 do console.
            let (mostra, aspecto) = match nucleo.quadro() {
                Some(Quadro::Placa(quadro)) => (
                    Mostra::Textura {
                        textura: quadro.textura.0.get(),
                        recorte: quadro.recorte,
                    },
                    quadro.proporcao,
                ),
                Some(Quadro::Tela(tela)) => {
                    let chave = (tela.serie(), tela.escritas());
                    let mostra = match Some(chave) == chave_atual {
                        true => Mostra::MesmaImagem,
                        false => Mostra::Imagem {
                            chave,
                            imagem: imagem_rgb565(
                                &tela.to_rgb565_bytes(),
                                tela.width() as usize,
                                tela.height() as usize,
                            ),
                        },
                    };
                    (mostra, 4.0 / 3.0)
                }
                None => (Mostra::Nada, 4.0 / 3.0),
            };
            let enquadrado = enquadra(area, scaling, keep_aspect, aspecto);
            (volta, mostra, centrado(area, enquadrado), smooth, depuracao)
        });
        match mostra {
            Mostra::Textura { textura, recorte } => {
                self.as_mut().rust_mut().chave = None;
                self.as_mut().mostra_textura(textura, recorte[0], recorte[1], destino, suave);
            }
            Mostra::Imagem { chave, imagem } => {
                self.as_mut().rust_mut().chave = Some(chave);
                self.as_mut().mostra_imagem(&imagem, destino, suave);
            }
            Mostra::MesmaImagem => self.as_mut().posiciona(destino, suave),
            Mostra::Nada => {}
        }
        if let Some(estado) = volta.estado.map(|texto| QString::from(&texto)) {
            if self.estado() != &estado {
                self.as_mut().set_estado(estado);
            }
        }
        let (textos, historia) = depuracao.unwrap_or_default();
        let textos = QString::from(&textos.join("   ·   "));
        if self.depuracao() != &textos {
            self.as_mut().set_depuracao(textos);
        }
        let historia: Vec<i32> = historia
            .into_iter()
            .flat_map(|(velocidade, quadros)| [velocidade as i32, quadros as i32])
            .collect();
        if Vec::<i32>::from(self.linha_do_tempo()) != historia {
            self.as_mut().set_linha_do_tempo(QList::from(historia));
        }
        let aviso = volta.aviso.unwrap_or(nucleo::AvisoNaTela {
            titulo: String::new(),
            estado: String::new(),
            parado: false,
            giro: 0.0,
        });
        let titulo = QString::from(&aviso.titulo);
        if self.aviso_titulo() != &titulo {
            self.as_mut().set_aviso_titulo(titulo);
        }
        let estado = QString::from(&aviso.estado);
        if self.aviso_estado() != &estado {
            self.as_mut().set_aviso_estado(estado);
        }
        if *self.aviso_parado() != aviso.parado {
            self.as_mut().set_aviso_parado(aviso.parado);
        }
        let giro = f64::from(aviso.giro).to_degrees();
        if *self.aviso_giro() != giro {
            self.as_mut().set_aviso_giro(giro);
        }
        if let Some(aviso) = volta.screenshot {
            self.as_mut().set_screenshot_texto(QString::from(&aviso.texto));
            self.as_mut().set_screenshot_falhou(aviso.falhou);
            self.as_mut().set_screenshot_pasta(QString::from(&aviso.pasta));
            let serie = self.screenshot_serie().wrapping_add(1);
            self.as_mut().set_screenshot_serie(serie);
        }
        let velocidade = QString::from(
            &volta
                .ritmo
                .map(|(velocidade, quadros)| format!("{velocidade}% · {quadros} FPS"))
                .unwrap_or_default(),
        );
        if self.velocidade() != &velocidade {
            self.as_mut().set_velocidade(velocidade);
        }
        let indicadores = QString::from(&volta.indicadores);
        if self.indicadores() != &indicadores {
            self.as_mut().set_indicadores(indicadores);
        }
        let parou = QString::from(&volta.parou);
        if self.parou() != &parou {
            self.as_mut().set_parou(parou);
        }
        if volta.fechou {
            self.as_mut().esvazia();
            self.as_mut().rust_mut().chave = None;
            self.as_mut().fechou();
        }
    }

    pub fn tecla(self: Pin<&mut Self>, codigo: i32, apertada: bool) {
        if let Some(tecla) = tecla_do_qt(codigo) {
            nucleo::com(|nucleo| nucleo.tecla(tecla, apertada));
        }
    }

    pub fn eh_atalho_de_estado(&self, codigo: i32, modificadores: i32) -> bool {
        let modificadores = modificadores_do_qt(modificadores);
        tecla_do_qt(codigo).is_some_and(|tecla| {
            nucleo::com(|nucleo| nucleo.e_atalho_de_estado(tecla, modificadores))
        })
    }

    pub fn atalho_de_estado(self: Pin<&mut Self>, codigo: i32, modificadores: i32) {
        let Some(tecla) = tecla_do_qt(codigo) else {
            return;
        };
        let modificadores = modificadores_do_qt(modificadores);
        let resposta = nucleo::com(|nucleo| nucleo.atalho_de_estado(tecla, modificadores));
        self.mostra_aviso_de_estado(resposta);
    }

    pub fn estado_pelo_menu(self: Pin<&mut Self>, acao: i32, slot: i32) {
        let pedido = match acao {
            0 => nucleo::PedidoDeEstado::Salvar,
            1 => nucleo::PedidoDeEstado::Carregar,
            _ => nucleo::PedidoDeEstado::Desfazer,
        };
        let slot = usize::try_from(slot).ok();
        let resposta = nucleo::com(|nucleo| nucleo.faz_estado(pedido, slot));
        self.mostra_aviso_de_estado(resposta);
    }

    pub fn slots_de_estado(&self) -> QStringList {
        let slots = nucleo::com(|nucleo| nucleo.slots_de_estado());
        let mut lista = QStringList::default();
        for slot in &slots {
            lista.append(QString::from(slot));
        }
        lista
    }

    pub fn slots_ocupados(&self) -> QList<i32> {
        let ocupados = nucleo::com(|nucleo| nucleo.slots_ocupados());
        QList::from(ocupados.into_iter().map(i32::from).collect::<Vec<_>>())
    }

    pub fn slot_atual(&self) -> i32 {
        nucleo::com(|nucleo| nucleo.slot_atual() as i32)
    }

    pub fn pode_desfazer_carregar(&self) -> bool {
        nucleo::com(|nucleo| nucleo.pode_desfazer_carregar())
    }

    pub fn alterna_avanco(self: Pin<&mut Self>) -> bool {
        nucleo::com(|nucleo| nucleo.alterna_o_avanco_fixo())
    }

    pub fn avanco_fixo(&self) -> bool {
        nucleo::com(|nucleo| nucleo.avanco_fixo())
    }

    /// O aviso de um pedido do save state, pelo atalho ou pelo menu.
    fn mostra_aviso_de_estado(
        mut self: Pin<&mut Self>,
        resposta: Option<(nucleo::AvisoDeEstado, bool)>,
    ) {
        let Some((aviso, restaurou)) = resposta else {
            return;
        };
        // **O console voltou a outro instante**, e a tela restaurada pode ter a mesma série e as
        // mesmas escritas que a imagem que está no item: sem esquecer a chave, a próxima volta a
        // acharia igual e deixaria a de antes à mostra.
        if restaurou {
            self.as_mut().rust_mut().chave = None;
        }
        self.as_mut().set_estado_texto(QString::from(&aviso.texto));
        self.as_mut().set_estado_falhou(aviso.falhou);
        let serie = self.estado_serie().wrapping_add(1);
        self.as_mut().set_estado_serie(serie);
    }

    pub fn eh_atalho_de_screenshot(&self, codigo: i32) -> bool {
        tecla_do_qt(codigo).is_some_and(|tecla| nucleo::com(|nucleo| nucleo.e_atalho_de_screenshot(tecla)))
    }

    pub fn screenshot(self: Pin<&mut Self>, carimbo: &QString) {
        let carimbo = String::from(carimbo);
        nucleo::com(|nucleo| nucleo.screenshot(&carimbo));
    }

    pub fn solta(self: Pin<&mut Self>) {
        nucleo::com(|nucleo| nucleo.solta_teclas());
    }

    pub fn fecha(mut self: Pin<&mut Self>) {
        nucleo::com(|nucleo| nucleo.fecha());
        // A textura do jogo que fechou não existe mais: o nó não pode continuar apontando para ela.
        self.as_mut().esvazia();
        self.as_mut().rust_mut().chave = None;
        self.as_mut().set_parou(QString::default());
    }

    pub fn pausa(self: Pin<&mut Self>) -> bool {
        nucleo::com(|nucleo| {
            nucleo.alterna_pausa();
            nucleo.pausada()
        })
    }

    pub fn eh_atalho_de_reinicio(&self, codigo: i32) -> bool {
        tecla_do_qt(codigo).is_some_and(|tecla| nucleo::com(|nucleo| nucleo.e_atalho_de_reinicio(tecla)))
    }

    pub fn reinicia(mut self: Pin<&mut Self>) -> bool {
        let (resultado, estado) = nucleo::com(|nucleo| {
            let resultado = nucleo.reinicia();
            let estado = match &resultado {
                Ok(()) => nucleo.estado(),
                Err(erro) => format!("não reiniciou: {erro}"),
            };
            (resultado, estado)
        });
        self.as_mut().set_estado(QString::from(&estado));
        if resultado.is_err() {
            return false;
        }
        // **A textura da partida que acabou não existe mais**, como no `fecha`: o nó não pode
        // continuar apontando para ela. Apontando, o Qt desenhava um quadro com o número dela — e
        // a partida nova já tinha dado esse número a outra textura: por um quadro aparecia a
        // fonte do menu do jogo, em vermelho, no Resident Evil 4, no Ridge Racer e no Rolimã. A
        // próxima volta manda a imagem nova.
        self.as_mut().esvazia();
        self.as_mut().rust_mut().chave = None;
        self.as_mut().set_parou(QString::default());
        true
    }

    pub fn atualiza(mut self: Pin<&mut Self>) {
        let estado = nucleo::com(|nucleo| nucleo.estado());
        self.as_mut().set_estado(QString::from(&estado));
    }

    pub fn modo_da_janela(&self) -> i32 {
        nucleo::com(|nucleo| modo(nucleo.settings.graphics.janela_do_jogo))
    }

    pub fn largura_da_janela(&self) -> f64 {
        nucleo::com(|nucleo| {
            nucleo
                .settings
                .graphics
                .tamanho_da_janela_do_jogo
                .map_or(0.0, |[largura, _]| largura as f64)
        })
    }

    pub fn altura_da_janela(&self) -> f64 {
        nucleo::com(|nucleo| {
            nucleo
                .settings
                .graphics
                .tamanho_da_janela_do_jogo
                .map_or(0.0, |[_, altura]| altura as f64)
        })
    }

    pub fn guarda_tamanho_da_janela(&self, largura: f64, altura: f64) {
        nucleo::com(|nucleo| {
            nucleo.settings.graphics.tamanho_da_janela_do_jogo =
                (largura > 0.0 && altura > 0.0).then_some([largura as f32, altura as f32]);
            if let Err(erro) = nucleo.settings.save() {
                eprintln!("não deu para guardar as configurações: {erro}");
            }
        })
    }
}

/// O `QImage` construído sobre bytes próprios exige linhas alinhadas a quatro bytes. Uma
/// largura ímpar em RGB565 não é: a linha ganha dois bytes de enchimento.
fn imagem_rgb565(bytes: &[u8], largura: usize, altura: usize) -> QImage {
    let linha = largura * 2;
    let passo = (linha + 3) & !3;
    let dados = match passo == linha {
        true => bytes.to_vec(),
        false => {
            let mut dados = vec![0u8; passo * altura];
            for (destino, origem) in dados.chunks_exact_mut(passo).zip(bytes.chunks_exact(linha)) {
                destino[..linha].copy_from_slice(origem);
            }
            dados
        }
    };
    // O `Format_RGB16` é o `u16` na ordem da máquina; o framebuffer entrega little-endian, que
    // é a ordem dos três sistemas suportados.
    unsafe { QImage::from_raw_bytes(dados, largura as i32, altura as i32, QImageFormat::Format_RGB16) }
}

/// Os modificadores de um evento do Qt (`Qt::KeyboardModifiers`), como o [`Modificadores`] os
/// guarda.
///
/// **No Mac o Qt troca os nomes**: o que ele chama de `ControlModifier` é o ⌘, e o
/// `MetaModifier` é o Control. Fora dele o Meta é a tecla do sistema (a do Windows, o Super), que
/// o [`Modificadores`] também guarda no `cmd`. O do teclado numérico não conta: o Enter de lá é o
/// mesmo Enter.
pub(super) fn modificadores_do_qt(bits: i32) -> Modificadores {
    const SHIFT: i32 = 0x0200_0000;
    const CONTROL: i32 = 0x0400_0000;
    const ALT: i32 = 0x0800_0000;
    const META: i32 = 0x1000_0000;
    let (ctrl, cmd) = match cfg!(target_os = "macos") {
        true => (bits & META != 0, bits & CONTROL != 0),
        false => (bits & CONTROL != 0, bits & META != 0),
    };
    Modificadores {
        ctrl,
        alt: bits & ALT != 0,
        shift: bits & SHIFT != 0,
        cmd,
    }
}

/// O nome que o egui dá à tecla, a partir do `Qt::Key`. É o nome que está gravado no
/// `settings.json`: mudar de interface não pode desfazer o mapeamento de ninguém.
/// O `egui::Key` de um `Qt::Key`, passando pelo nome.
pub(super) fn tecla_do_qt(codigo: i32) -> Option<Key> {
    nome_da_tecla(codigo).and_then(|nome| Key::from_name(&nome))
}

fn nome_da_tecla(codigo: i32) -> Option<String> {
    let nome = match codigo {
        0x0100_0000 => "Escape",
        0x0100_0001 => "Tab",
        0x0100_0003 => "Backspace",
        // `Return` e o `Enter` do teclado numérico são uma tecla só para o egui.
        0x0100_0004 | 0x0100_0005 => "Enter",
        0x0100_0006 => "Insert",
        0x0100_0007 => "Delete",
        0x0100_0010 => "Home",
        0x0100_0011 => "End",
        0x0100_0012 => "Left",
        0x0100_0013 => "Up",
        0x0100_0014 => "Right",
        0x0100_0015 => "Down",
        0x0100_0016 => "PageUp",
        0x0100_0017 => "PageDown",
        0x20 => "Space",
        0x2c => "Comma",
        0x2d => "Minus",
        0x2e => "Period",
        0x2f => "Slash",
        0x3b => "Semicolon",
        0x3d => "Equals",
        0x5b => "OpenBracket",
        0x5c => "Backslash",
        0x5d => "CloseBracket",
        0x60 => "Backtick",
        0x27 => "Quote",
        // Dígitos e letras: o `Qt::Key` é o próprio ASCII maiúsculo, que é o nome no egui.
        0x30..=0x39 | 0x41..=0x5a => return char::from_u32(codigo as u32).map(String::from),
        // F1 a F35.
        0x0100_0030..=0x0100_0052 => return Some(format!("F{}", codigo - 0x0100_0030 + 1)),
        _ => return None,
    };
    Some(nome.to_string())
}

#[cfg(test)]
mod tests {
    use super::{Key, Modificadores, modificadores_do_qt, tecla_do_qt};

    /// O ⌘ do Mac chega ao Qt como Control; o Control, como Meta.
    #[test]
    fn os_modificadores_do_qt() {
        assert!(modificadores_do_qt(0).nenhum());
        // O teclado numérico não é modificador de ninguém.
        assert!(modificadores_do_qt(0x2000_0000).nenhum());
        let shift = modificadores_do_qt(0x0200_0000);
        assert_eq!(
            shift,
            Modificadores {
                shift: true,
                ..Modificadores::default()
            }
        );
        let control = modificadores_do_qt(0x0400_0000);
        match cfg!(target_os = "macos") {
            true => assert!(control.cmd && !control.ctrl),
            false => assert!(control.ctrl && !control.cmd),
        }
    }

    /// A seta do Qt é a seta do egui: daí em diante vale [`zeebx::ui::entrada::tecla_apertada`],
    /// que aceita as duas grafias do `settings.json`.
    #[test]
    fn a_seta_do_qt_e_a_do_egui() {
        assert_eq!(tecla_do_qt(0x0100_0013), Some(Key::ArrowUp));
    }

    #[test]
    fn as_teclas_do_qt_tem_o_nome_do_egui() {
        // O `settings.json` de quem já usa o emulador guarda estes nomes.
        for (codigo, nome) in [
            (0x0100_0013, "Up"),
            (0x0100_0004, "Enter"),
            (0x20, "Space"),
            (0x5a, "Z"),
            (0x34, "4"),
            (0x0100_0003, "Backspace"),
            (0x0100_0039, "F10"),
        ] {
            assert_eq!(tecla_do_qt(codigo), Key::from_name(nome), "{nome}");
            assert!(Key::from_name(nome).is_some(), "{nome} não é tecla do egui");
        }
    }
}
