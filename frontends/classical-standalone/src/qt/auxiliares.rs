//! As janelas auxiliares: o gerenciador de saves, os avisos de abertura e de versão nova, e o log
//! da execução. A regra de cada uma está no núcleo (`ui::saves`, `ui::partida`); aqui fica o que o
//! QML lê.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        /// Os saves: o que cada jogo escreveu na pasta dele, e o que está no sistema de arquivos
        /// do aparelho, que é de todos.
        #[qobject]
        #[qml_element]
        #[qproperty(i32, versao)]
        type Saves = super::SavesRust;

        /// Relê a lista. Toca o disco: ao abrir a janela e depois de cada exclusão.
        #[qinvokable]
        fn recarrega(self: Pin<&mut Saves>);

        #[qinvokable]
        fn quantos(self: &Saves) -> i32;

        #[qinvokable]
        fn titulo(self: &Saves, indice: i32) -> QString;

        /// "3 arquivo(s), 12 KB".
        #[qinvokable]
        fn resumo(self: &Saves, indice: i32) -> QString;

        /// "Apagar o save de …? Não tem volta."
        #[qinvokable]
        fn pergunta(self: &Saves, indice: i32) -> QString;

        /// Se o save é do aparelho, e não de um jogo.
        #[qinvokable]
        #[cxx_name = "doAparelho"]
        fn do_aparelho(self: &Saves, indice: i32) -> bool;

        /// Apaga o save e devolve o que dizer. Não tem volta: a janela confirma antes.
        #[qinvokable]
        fn apaga(self: Pin<&mut Saves>, indice: i32) -> QString;
    }

    extern "RustQt" {
        /// Os avisos da janela principal: o de abertura e o de versão nova.
        #[qobject]
        #[qml_element]
        type Avisos = super::AvisosRust;

        /// O aviso de abertura ainda não foi dispensado nesta versão.
        #[qinvokable]
        #[cxx_name = "deAbertura"]
        fn de_abertura(self: &Avisos) -> bool;

        /// O aviso de abertura saiu da frente; `de_vez` é o "não mostrar de novo".
        #[qinvokable]
        #[cxx_name = "dispensaAbertura"]
        fn dispensa_abertura(self: &Avisos, de_vez: bool);

        /// "O Zeebx 0.4.0 já está disponível…", se há versão nova a avisar; vazio se não.
        #[qinvokable]
        #[cxx_name = "deAtualizacao"]
        fn de_atualizacao(self: &Avisos) -> QString;

        #[qinvokable]
        #[cxx_name = "paginaDaAtualizacao"]
        fn pagina_da_atualizacao(self: &Avisos) -> QString;

        #[qinvokable]
        #[cxx_name = "dispensaAtualizacao"]
        fn dispensa_atualizacao(self: &Avisos);
    }

    extern "RustQt" {
        /// O log da execução: o que o jogo escreveu, e as queixas do emulador sobre ele.
        #[qobject]
        #[qml_element]
        type Log = super::LogRust;

        /// A janela de log está pedida — a opção ligada, e não fechada nesta execução.
        #[qinvokable]
        fn ativo(self: &Log) -> bool;

        /// Fechar a janela dispensa o log desta execução, e não a preferência.
        #[qinvokable]
        fn dispensa(self: &Log);

        #[qinvokable]
        fn linhas(self: &Log) -> QStringList;

        /// Onde o relatório desta execução é gravado sozinho.
        #[qinvokable]
        fn caminho(self: &Log) -> QString;

        /// Grava o log num arquivo escolhido e devolve o que dizer; vazio se desistiu.
        #[qinvokable]
        fn exporta(self: &Log) -> QString;
    }
}

use std::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QList, QString, QStringList};

use zeebx::ui::atualizacao;
use zeebx::ui::saves::{self, Save};

use super::nucleo;

#[derive(Default)]
pub struct SavesRust {
    versao: i32,
    /// Os saves listados, `true` nos do aparelho.
    lista: Vec<(bool, Save)>,
}

impl qobject::Saves {
    fn save(&self, indice: i32) -> Option<&(bool, Save)> {
        usize::try_from(indice).ok().and_then(|i| self.rust().lista.get(i))
    }

    pub fn recarrega(mut self: Pin<&mut Self>) {
        let roms = nucleo::com(|nucleo| nucleo.settings.roms_dir.clone());
        self.as_mut().rust_mut().lista = saves::todos(roms.as_deref());
        let versao = self.versao().wrapping_add(1);
        self.as_mut().set_versao(versao);
    }

    pub fn quantos(&self) -> i32 {
        self.rust().lista.len() as i32
    }

    pub fn titulo(&self, indice: i32) -> QString {
        QString::from(self.save(indice).map(|(_, save)| save.titulo.as_str()).unwrap_or_default())
    }

    pub fn resumo(&self, indice: i32) -> QString {
        let Some((_, save)) = self.save(indice) else {
            return QString::default();
        };
        nucleo::com(|nucleo| {
            QString::from(&nucleo.catalogo.format(
                "saves.files",
                &[
                    ("count", &save.arquivos.to_string()),
                    ("size", &saves::tamanho(save.bytes)),
                ],
            ))
        })
    }

    pub fn pergunta(&self, indice: i32) -> QString {
        let Some((_, save)) = self.save(indice) else {
            return QString::default();
        };
        nucleo::com(|nucleo| {
            QString::from(&nucleo.catalogo.format("saves.confirm", &[("name", &save.titulo)]))
        })
    }

    pub fn do_aparelho(&self, indice: i32) -> bool {
        self.save(indice).is_some_and(|(aparelho, _)| *aparelho)
    }

    pub fn apaga(mut self: Pin<&mut Self>, indice: i32) -> QString {
        let Some((_, save)) = self.save(indice) else {
            return QString::default();
        };
        let titulo = save.titulo.clone();
        let resultado = saves::apagar(save);
        let recado = nucleo::com(|nucleo| match resultado {
            Ok(()) => nucleo.catalogo.format("saves.deleted", &[("name", &titulo)]),
            Err(erro) => nucleo
                .catalogo
                .format("saves.failed", &[("name", &titulo), ("reason", &erro.to_string())]),
        });
        self.as_mut().recarrega();
        QString::from(&recado)
    }
}

#[derive(Default)]
pub struct AvisosRust;

impl qobject::Avisos {
    pub fn de_abertura(&self) -> bool {
        nucleo::com(|nucleo| nucleo.aviso_de_abertura_pendente())
    }

    pub fn dispensa_abertura(&self, de_vez: bool) {
        nucleo::com(|nucleo| nucleo.dispensa_aviso_de_abertura(de_vez));
    }

    pub fn de_atualizacao(&self) -> QString {
        nucleo::com(|nucleo| {
            let Some(lancamento) = nucleo.aviso_de_atualizacao() else {
                return QString::default();
            };
            QString::from(&nucleo.catalogo.format(
                "update.available",
                &[("new", &lancamento.versao), ("current", atualizacao::VERSAO_ATUAL)],
            ))
        })
    }

    pub fn pagina_da_atualizacao(&self) -> QString {
        nucleo::com(|nucleo| {
            QString::from(nucleo.aviso_de_atualizacao().map(|l| l.pagina.as_str()).unwrap_or_default())
        })
    }

    pub fn dispensa_atualizacao(&self) {
        nucleo::com(|nucleo| nucleo.dispensa_aviso_de_atualizacao());
    }
}

#[derive(Default)]
pub struct LogRust;

impl qobject::Log {
    pub fn ativo(&self) -> bool {
        nucleo::com(|nucleo| nucleo.settings.debug.log && !nucleo.log_dispensado)
    }

    pub fn dispensa(&self) {
        nucleo::com(|nucleo| nucleo.log_dispensado = true);
    }

    pub fn linhas(&self) -> QStringList {
        let mut lista = QList::<QString>::default();
        for linha in nucleo::com(|nucleo| nucleo.log()) {
            lista.append(QString::from(&linha));
        }
        QStringList::from(&lista)
    }

    pub fn caminho(&self) -> QString {
        let caminho = nucleo::com(|nucleo| nucleo.caminho_do_relatorio());
        QString::from(&caminho.map(|c| c.display().to_string()).unwrap_or_default())
    }

    pub fn exporta(&self) -> QString {
        let (linhas, sugestao) = nucleo::com(|nucleo| {
            let sugestao = nucleo
                .caminho_do_relatorio()
                .and_then(|c| c.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "zeebx.log".to_string());
            (nucleo.log(), sugestao)
        });
        let Some(destino) = rfd::FileDialog::new()
            .set_file_name(&sugestao)
            .add_filter("log", &["log"])
            .save_file()
        else {
            return QString::default();
        };
        let resultado = std::fs::write(&destino, linhas.join("\n") + "\n");
        nucleo::com(|nucleo| {
            QString::from(&match resultado {
                Ok(()) => nucleo
                    .catalogo
                    .format("debug.log.saved", &[("path", &destino.display().to_string())]),
                Err(erro) => nucleo
                    .catalogo
                    .format("debug.log.failed", &[("reason", &erro.to_string())]),
            })
        })
    }
}
