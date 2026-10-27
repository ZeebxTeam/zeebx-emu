//! O tema da interface: a paleta, que o C++ aplica a todas as janelas, e as duas cores que o QML
//! escrevia à mão. Ver `zeebx::ui::temas` e `docs/implementacao/26-temas.md`.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    // Ver `cpp/paleta.h`.
    #[namespace = "zeebx"]
    unsafe extern "C++" {
        include!("paleta.h");
        fn paleta_cor(desabilitada: bool, papel: &str, rgb: u32) -> bool;
        fn paleta_aplica(escuro: bool);
        fn paleta_do_sistema();
    }

    extern "RustQt" {
        /// As cores que não são da paleta do Qt: o texto de aviso (uma tecla recusada, uma pasta
        /// que falta) e o de erro (o jogo que não abriu). Mudam com o tema.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, aviso)]
        #[qproperty(QString, erro)]
        type Tema = super::TemaRust;

        /// O tema mudou: as duas cores são relidas.
        #[qinvokable]
        fn recarrega(self: Pin<&mut Tema>);
    }

    impl cxx_qt::Initialize for Tema {}
}

use std::pin::Pin;

use cxx_qt_lib::QString;

use super::nucleo;

/// As de antes dos temas, que a paleta do sistema continua usando.
const AVISO_DO_SISTEMA: &str = "#e0a030";
const ERRO_DO_SISTEMA: &str = "#d04040";

#[derive(Default)]
pub struct TemaRust {
    aviso: QString,
    erro: QString,
}

impl cxx_qt::Initialize for qobject::Tema {
    fn initialize(self: Pin<&mut Self>) {
        self.recarrega();
    }
}

impl qobject::Tema {
    pub fn recarrega(mut self: Pin<&mut Self>) {
        let (aviso, erro) = nucleo::com(|nucleo| match nucleo.tema() {
            Some(tema) => (tema.aviso.hexa(), tema.erro.hexa()),
            None => (AVISO_DO_SISTEMA.to_string(), ERRO_DO_SISTEMA.to_string()),
        });
        self.as_mut().set_aviso(QString::from(&aviso));
        self.as_mut().set_erro(QString::from(&erro));
    }
}

/// Aplica o tema escolhido a todas as janelas, ou a paleta do sistema sem tema.
///
/// **Fora do [`nucleo::com`]**: o `setPalette` avisa as janelas na hora, as ligações do QML se
/// refazem ali dentro, e uma delas que chame o núcleo encontraria o `RefCell` emprestado. Por isso
/// o tema é copiado lá dentro e aplicado aqui fora.
pub fn aplica() {
    let Some(tema) = nucleo::com(|nucleo| nucleo.tema().cloned()) else {
        qobject::paleta_do_sistema();
        return;
    };
    let desabilitadas = tema.desabilitado.iter().map(|cor| (true, cor));
    for (desabilitada, (papel, cor)) in tema
        .cores
        .iter()
        .map(|cor| (false, cor))
        .chain(desabilitadas)
    {
        if !qobject::paleta_cor(desabilitada, papel, cor.0) {
            eprintln!("tema {}: o Qt não tem o papel {papel}", tema.chave);
        }
    }
    qobject::paleta_aplica(tema.escuro);
}
