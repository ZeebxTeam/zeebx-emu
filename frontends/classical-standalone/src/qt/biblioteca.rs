//! A biblioteca de jogos, como modelo de lista para o QML, e as imagens dela.
//!
//! A lista é a do egui: em ordem de título, sem a Z-Wheel, com o nome oficial que ela traz, e
//! filtrada pela busca. As imagens chegam ao QML por `image://zeebx/…` — ver `cpp/imagens.h`.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++Qt" {
        include!(<QtCore/QAbstractListModel>);
        #[qobject]
        type QAbstractListModel;
    }

    unsafe extern "C++" {
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qlist.h");
        type QList_i32 = cxx_qt_lib::QList<i32>;
        include!("cxx-qt-lib/qimage.h");
        type QImage = cxx_qt_lib::QImage;
        include!("cxx-qt-lib/qqmlengine.h");
        type QQmlEngine = cxx_qt_lib::QQmlEngine;
        include!("cxx-qt-lib/qmap.h");
        type QMap_QString_QVariant = cxx_qt_lib::QMap<cxx_qt_lib::QMapPair_QString_QVariant>;
    }

    // Ver `cpp/imagens.h`.
    #[namespace = "zeebx"]
    unsafe extern "C++" {
        include!("imagens.h");
        fn registra_imagens(engine: Pin<&mut QQmlEngine>, fonte: fn(&str) -> QImage);
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QAbstractListModel]
        #[qproperty(QString, contagem)]
        #[qproperty(QString, vazio)]
        type Biblioteca = super::BibliotecaRust;

        #[qinvokable]
        #[cxx_override]
        fn data(self: &Biblioteca, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &Biblioteca, parent: &QModelIndex) -> i32;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &Biblioteca) -> QHash_i32_QByteArray;

        /// Quantas linhas a lista tem. O `rowCount` pede o índice pai, que o QML não passa.
        #[qinvokable]
        fn linhas(self: &Biblioteca) -> i32;

        /// Os papéis de uma linha qualquer, como objeto: o slider mostra os jogos em volta da
        /// escolha, dando a volta na lista, e não uma linha por delegado.
        #[qinvokable]
        fn item(self: &Biblioteca, linha: i32) -> QMap_QString_QVariant;

        /// Abre o jogo da linha dada. Devolve vazio, ou o motivo de não ter aberto.
        #[qinvokable]
        fn abre(self: &Biblioteca, linha: i32) -> QString;

        /// Abre a Z-Wheel. Devolve vazio, ou o motivo de não ter aberta.
        #[qinvokable]
        #[cxx_name = "abreZWheel"]
        fn abre_z_wheel(self: &Biblioteca) -> QString;

        /// Abre o jogo de um caminho — o que veio na linha de comando.
        #[qinvokable]
        #[cxx_name = "abreCaminho"]
        fn abre_caminho(self: &Biblioteca, caminho: &QString) -> QString;

        /// Se há Z-Wheel para o botão abrir.
        #[qinvokable]
        #[cxx_name = "temZWheel"]
        fn tem_z_wheel(self: &Biblioteca) -> bool;

        /// Como a janela principal abre: 0 em janela, 1 maximizada, 2 em tela cheia. Ver
        /// `graphics.janela`.
        #[qinvokable]
        #[cxx_name = "modoDaJanela"]
        fn modo_da_janela(self: &Biblioteca) -> i32;

        /// Como a biblioteca aparece: 0 em grade, 1 no slider. Ver `biblioteca` nas
        /// configurações.
        #[qinvokable]
        #[cxx_name = "modoDaBiblioteca"]
        fn modo_da_biblioteca(self: &Biblioteca) -> i32;

        /// A busca mudou: a lista passa a ter só o que casa com ela.
        #[qinvokable]
        fn busca(self: Pin<&mut Biblioteca>, texto: &QString);

        /// Refaz a lista sem varrer a pasta: a Z-Wheel ou o idioma mudaram, e com eles os nomes.
        #[qinvokable]
        fn refaz(self: Pin<&mut Biblioteca>);

        /// Varre a pasta de ROMs de novo, e relê a Z-Wheel e o acervo dela.
        #[qinvokable]
        #[cxx_name = "procuraDeNovo"]
        fn procura_de_novo(self: Pin<&mut Biblioteca>);

        /// O que o controle pede à biblioteca nesta leitura: 0 cima, 1 baixo, 2 esquerda,
        /// 3 direita, 4 abrir, 5 Z-Wheel. `escutando` falso é a biblioteca sem o foco.
        #[qinvokable]
        fn comandos(self: &Biblioteca, escutando: bool) -> QList_i32;

        /// Recua em um dia a trava de sincronização do Zeeboids. Devolve o que dizer.
        #[qinvokable]
        #[cxx_name = "liberaSincronizacao"]
        fn libera_sincronizacao(self: &Biblioteca) -> QString;


        #[inherit]
        fn index(self: &Biblioteca, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;

        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut Biblioteca>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut Biblioteca>);
    }

    impl cxx_qt::Initialize for Biblioteca {}
}

use std::path::{Path, PathBuf};
use std::pin::Pin;

use cxx_qt_lib::{
    QByteArray, QHash, QHashPair_i32_QByteArray, QImage, QImageFormat, QList, QMap,
    QMapPair_QString_QVariant, QModelIndex, QString, QVariant,
};

use zeebx::loader::archive;
use zeebx::ponte;
use zeebx::ui::acervo;
use zeebx::ui::navegacao::Comando;
use zeebx::ui::settings::ModoDaBiblioteca;

use super::nucleo;

/// O primeiro papel livre para o modelo: abaixo dele ficam os do Qt, como o `DisplayRole`.
const USER_ROLE: i32 = 0x0100;
const TITULO: i32 = USER_ROLE;
const CAMINHO: i32 = USER_ROLE + 1;
const DESCRICAO: i32 = USER_ROLE + 2;
const CLASSE: i32 = USER_ROLE + 3;
const CAPA: i32 = USER_ROLE + 4;
const CAPA_NITIDA: i32 = USER_ROLE + 5;
const LOGO: i32 = USER_ROLE + 6;
const CLASSIFICACAO: i32 = USER_ROLE + 7;

/// O quadro da imagem num cartão da grade, em pé, na proporção das caixas que a Z-Wheel traz
/// (170×220). O `Principal.qml` desenha o cartão com estas medidas.
const QUADRO_DA_CAPA: [f32; 2] = [112.0, 145.0];

#[derive(Default)]
pub struct BibliotecaRust {
    contagem: QString,
    vazio: QString,
}

/// O resultado de uma abertura, como o QML o lê: vazio é sucesso.
fn resposta(aberta: Result<(), String>) -> QString {
    match aberta {
        Ok(()) => QString::default(),
        Err(erro) => QString::from(&erro),
    }
}

/// A imagem pedida pelo QML, pelo endereço sem o esquema. É a função que o provedor chama.
fn imagem(endereco: &str) -> QImage {
    nucleo::com(|nucleo| {
        // O desenho do controle é feito na hora, na cor pedida; o resto já está na memória.
        let tingida;
        let imagem = match endereco.starts_with("controle/") {
            true => {
                tingida = nucleo.imagem_do_controle(endereco);
                tingida.as_ref()
            }
            false => nucleo.imagem(endereco),
        };
        let Some(imagem) = imagem else {
            return QImage::default();
        };
        // SAFETY: `rgba` tem `largura × altura × 4` bytes, que é o `Format_RGBA8888`, e uma linha
        // de pixels de quatro bytes sempre tem o alinhamento que o `QImage` exige.
        unsafe {
            QImage::from_raw_bytes(
                imagem.rgba.clone(),
                imagem.width as i32,
                imagem.height as i32,
                QImageFormat::Format_RGBA8888,
            )
        }
    })
}

/// Registra o provedor de imagens no engine: `image://zeebx/…`.
pub fn registra_imagens(engine: Pin<&mut qobject::QQmlEngine>) {
    qobject::registra_imagens(engine, imagem);
}

impl cxx_qt::Initialize for qobject::Biblioteca {
    fn initialize(self: Pin<&mut Self>) {
        self.atualiza_textos();
    }
}

impl qobject::Biblioteca {
    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Ok(linha) = usize::try_from(index.row()) else {
            return QVariant::default();
        };
        let texto = |texto: &str| QVariant::from(&QString::from(texto));
        nucleo::com(|nucleo| {
            let Some((titulo, indice)) = nucleo.lista.get(linha).cloned() else {
                return QVariant::default();
            };
            let geracao = nucleo.geracao;
            let classe = nucleo.jogos[indice].clsid;
            let ficha = nucleo.ficha(linha);
            match role {
                TITULO => texto(&titulo),
                CAMINHO => texto(&nucleo.jogos[indice].path.display().to_string()),
                DESCRICAO => {
                    let idioma = nucleo.catalogo.current();
                    texto(ficha.and_then(|ficha| ficha.descricao(idioma)).unwrap_or_default())
                }
                CLASSE => texto(&classe.map(|c| format!("{c:#010x}")).unwrap_or_default()),
                CAPA => texto(&format!("image://zeebx/capa/{geracao}/{indice}")),
                CAPA_NITIDA => {
                    let nitida = nucleo.capa(indice).is_some_and(|imagem| {
                        acervo::amplia_sem_interpolar(imagem, QUADRO_DA_CAPA)
                    });
                    QVariant::from(&nitida)
                }
                LOGO => match ficha.is_some_and(|ficha| ficha.logo.is_some()) {
                    true => texto(&format!("image://zeebx/logo/{geracao}/{}", classe.unwrap_or(0))),
                    false => texto(""),
                },
                CLASSIFICACAO => match ficha.is_some_and(|ficha| ficha.classificacao.is_some()) {
                    true => texto(&format!(
                        "image://zeebx/classificacao/{geracao}/{}",
                        classe.unwrap_or(0)
                    )),
                    false => texto(""),
                },
                _ => QVariant::default(),
            }
        })
    }

    pub fn row_count(&self, _parent: &QModelIndex) -> i32 {
        nucleo::com(|nucleo| nucleo.lista.len() as i32)
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut papeis = QHash::<QHashPair_i32_QByteArray>::default();
        for (papel, nome) in [
            (TITULO, "titulo"),
            (CAMINHO, "caminho"),
            (DESCRICAO, "descricao"),
            (CLASSE, "classe"),
            (CAPA, "capa"),
            (CAPA_NITIDA, "capaNitida"),
            (LOGO, "logo"),
            (CLASSIFICACAO, "classificacao"),
        ] {
            papeis.insert(papel, QByteArray::from(nome));
        }
        papeis
    }

    pub fn linhas(&self) -> i32 {
        nucleo::com(|nucleo| nucleo.lista.len() as i32)
    }

    pub fn item(&self, linha: i32) -> QMap<QMapPair_QString_QVariant> {
        let mut item = QMap::<QMapPair_QString_QVariant>::default();
        let indice = self.index(linha, 0, &QModelIndex::default());
        for (papel, nome) in [
            (TITULO, "titulo"),
            (DESCRICAO, "descricao"),
            (CAPA, "capa"),
            (CAPA_NITIDA, "capaNitida"),
            (LOGO, "logo"),
            (CLASSIFICACAO, "classificacao"),
        ] {
            item.insert(QString::from(nome), self.data(&indice, papel));
        }
        item
    }

    pub fn abre(&self, linha: i32) -> QString {
        resposta(nucleo::com(|nucleo| {
            let caminho = usize::try_from(linha)
                .ok()
                .and_then(|linha| nucleo.lista.get(linha))
                .map(|&(_, indice)| nucleo.jogos[indice].path.clone())
                .ok_or_else(|| "esse jogo não está na lista".to_string())?;
            nucleo.abre_pela_biblioteca(&caminho)
        }))
    }

    pub fn abre_z_wheel(&self) -> QString {
        resposta(nucleo::com(|nucleo| {
            let caminho = nucleo
                .z_wheel
                .clone()
                .ok_or_else(|| nucleo.catalogo.get("nav.z_wheel.missing").to_string())?;
            nucleo.abre_pela_biblioteca(&caminho)
        }))
    }

    pub fn abre_caminho(&self, caminho: &QString) -> QString {
        let caminho = PathBuf::from(String::from(caminho));
        let caminho = std::fs::canonicalize(&caminho).unwrap_or(caminho);
        resposta(nucleo::com(|nucleo| nucleo.abre_pela_biblioteca(Path::new(&caminho))))
    }

    pub fn tem_z_wheel(&self) -> bool {
        nucleo::com(|nucleo| nucleo.z_wheel.is_some())
    }

    pub fn modo_da_janela(&self) -> i32 {
        nucleo::com(|nucleo| super::ponte::modo(nucleo.settings.graphics.janela))
    }

    pub fn modo_da_biblioteca(&self) -> i32 {
        nucleo::com(|nucleo| match nucleo.settings.biblioteca {
            ModoDaBiblioteca::Grade => 0,
            ModoDaBiblioteca::Slider => 1,
        })
    }

    pub fn busca(mut self: Pin<&mut Self>, texto: &QString) {
        let texto = String::from(texto);
        // O reset fica fora do `nucleo::com`: ele avisa o QML, que pede as linhas de volta.
        unsafe { self.as_mut().begin_reset_model() };
        nucleo::com(|nucleo| nucleo.define_busca(&texto));
        unsafe { self.as_mut().end_reset_model() };
        self.atualiza_textos();
    }

    pub fn refaz(mut self: Pin<&mut Self>) {
        unsafe { self.as_mut().begin_reset_model() };
        nucleo::com(|nucleo| nucleo.refaz_lista());
        unsafe { self.as_mut().end_reset_model() };
        self.atualiza_textos();
    }

    pub fn procura_de_novo(mut self: Pin<&mut Self>) {
        unsafe { self.as_mut().begin_reset_model() };
        nucleo::com(|nucleo| nucleo.procura_de_novo());
        unsafe { self.as_mut().end_reset_model() };
        self.atualiza_textos();
    }

    pub fn comandos(&self, escutando: bool) -> QList<i32> {
        let comandos = nucleo::com(|nucleo| {
            // O mesmo relógio lê o controle e mantém o resto em dia: a presença no Discord e a
            // resposta da procura por versão nova.
            nucleo.a_cada_quadro();
            nucleo.comandos(escutando)
        });
        let codigos: Vec<i32> = comandos
            .into_iter()
            .map(|comando| match comando {
                Comando::Cima => 0,
                Comando::Baixo => 1,
                Comando::Esquerda => 2,
                Comando::Direita => 3,
                Comando::Abrir => 4,
                Comando::ZWheel => 5,
            })
            .collect();
        QList::from(codigos)
    }

    pub fn libera_sincronizacao(&self) -> QString {
        let recado = match ponte::liberar_sincronizacao(&archive::device_dir()) {
            Ok(_) => nucleo::com(|nucleo| nucleo.catalogo.get("nav.unlock_sync.done").to_string()),
            Err(erro) => erro,
        };
        QString::from(&recado)
    }

    /// A contagem e o recado de lista vazia, que mudam com a busca e com a varredura.
    fn atualiza_textos(mut self: Pin<&mut Self>) {
        let (contagem, vazio) = nucleo::com(|nucleo| {
            let catalogo = &nucleo.catalogo;
            let mostrados = nucleo.lista.len().to_string();
            let contagem = match nucleo.buscando() {
                true => catalogo.format(
                    "library.count_filtered",
                    &[("shown", &mostrados), ("count", &nucleo.total().to_string())],
                ),
                false => catalogo.format("library.count", &[("count", &mostrados)]),
            };
            let vazio = match (&nucleo.settings.roms_dir, nucleo.lista.is_empty()) {
                (_, false) => String::new(),
                (None, true) => catalogo.get("library.no_folder").to_string(),
                (Some(_), true) if nucleo.buscando() => catalogo.format(
                    "library.no_match",
                    &[("query", nucleo.busca_atual().trim())],
                ),
                (Some(pasta), true) => catalogo.format(
                    "library.empty",
                    &[("folder", &pasta.display().to_string())],
                ),
            };
            (contagem, vazio)
        });
        self.as_mut().set_contagem(QString::from(&contagem));
        self.as_mut().set_vazio(QString::from(&vazio));
    }
}
