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

        /// Pausa ou retoma o jogo. Devolve se ficou pausado.
        #[qinvokable]
        fn pausa(self: Pin<&mut Self>) -> bool;

        /// Relê o título do jogo aberto para a linha de estado. A janela chama isto ao aparecer.
        #[qinvokable]
        fn atualiza(self: Pin<&mut Self>);

        /// Como a janela do jogo abre: 0 em janela, 1 maximizada, 2 em tela cheia. Ver
        /// `graphics.janela_do_jogo`.
        #[qinvokable]
        #[cxx_name = "modoDaJanela"]
        fn modo_da_janela(self: &Self) -> i32;


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
use cxx_qt_lib::{QImage, QImageFormat, QList, QRectF, QString};

use zeebx::eframe::egui::Key;
use zeebx::ui::partida::enquadra;
use zeebx::ui::settings::ModoDaJanela;

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
            let graficos = &nucleo.settings.graphics;
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
            let enquadrado = enquadra(area, graficos.scaling, graficos.keep_aspect, aspecto);
            (volta, mostra, centrado(area, enquadrado), graficos.smooth, depuracao)
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

    pub fn atualiza(mut self: Pin<&mut Self>) {
        let estado = nucleo::com(|nucleo| nucleo.estado());
        self.as_mut().set_estado(QString::from(&estado));
    }

    pub fn modo_da_janela(&self) -> i32 {
        nucleo::com(|nucleo| modo(nucleo.settings.graphics.janela_do_jogo))
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
    use super::{Key, tecla_do_qt};

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
