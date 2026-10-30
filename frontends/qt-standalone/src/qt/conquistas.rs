//! As conquistas do RetroAchievements na interface: a conta e os avisos por cima do jogo. O objeto
//! existe com ou sem a feature, para o QML ser um só; sem ela, `disponivel` esconde a aba.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type Conquistas = super::ConquistasRust;

        #[qinvokable]
        fn disponivel(self: &Conquistas) -> bool;

        #[qinvokable]
        fn entrando(self: &Conquistas) -> bool;

        /// "Conectado como …"; vazio sem conta.
        #[qinvokable]
        fn conta(self: &Conquistas) -> QString;

        /// O erro do último login; vazio se deu certo.
        #[qinvokable]
        fn erro(self: &Conquistas) -> QString;

        /// A senha vai ao servidor e é esquecida: o que fica nas configurações é o token.
        #[qinvokable]
        fn entra(self: &Conquistas, usuario: &QString, senha: &QString);

        /// Sai da conta e esquece o token.
        #[qinvokable]
        fn sai(self: &Conquistas);
    }
}

use cxx_qt_lib::QString;

use super::nucleo;

#[derive(Default)]
pub struct ConquistasRust;

impl qobject::Conquistas {
    pub fn disponivel(&self) -> bool {
        cfg!(feature = "conquistas")
    }

    pub fn entrando(&self) -> bool {
        nucleo::com(|n| n.conquistas.entrando())
    }

    pub fn conta(&self) -> QString {
        QString::from(&nucleo::com(|n| n.conquistas.conta(&n.catalogo)))
    }

    pub fn erro(&self) -> QString {
        QString::from(&nucleo::com(|n| n.conquistas.erro()))
    }

    pub fn entra(&self, usuario: &QString, senha: &QString) {
        let (usuario, senha) = (String::from(usuario), String::from(senha));
        nucleo::com(|n| n.conquistas.entra(usuario.trim(), &senha));
    }

    pub fn sai(&self) {
        nucleo::com(|n| {
            n.conquistas.sai();
            n.settings.conquistas = Default::default();
            n.salva_configuracoes();
        });
    }
}

/// Um aviso por cima do jogo, já traduzido. `icone` é uma URL, ou vazio.
pub struct AvisoDeConquista {
    pub titulo: String,
    pub texto: String,
    pub icone: String,
}

#[cfg(feature = "conquistas")]
pub use com_a_feature::NoNucleo;
#[cfg(not(feature = "conquistas"))]
pub use sem_a_feature::NoNucleo;

#[cfg(not(feature = "conquistas"))]
mod sem_a_feature {
    use zeebx::session::Session;
    use zeebx::ui::i18n::Catalog;
    use zeebx::ui::settings::Settings;

    use super::AvisoDeConquista;

    #[derive(Default)]
    pub struct NoNucleo;

    impl NoNucleo {
        pub fn entrando(&self) -> bool {
            false
        }
        pub fn conta(&self, _: &Catalog) -> String {
            String::new()
        }
        pub fn erro(&self) -> String {
            String::new()
        }
        pub fn entra(&mut self, _: &str, _: &str) {}
        pub fn sai(&mut self) {}
        pub fn a_cada_quadro(&mut self, _: &mut Settings) -> bool {
            false
        }
        pub fn liga(&mut self, _: &mut Session) {}
        pub fn desliga(&mut self, _: &mut Session) {}
        pub fn depois_do_passo(
            &mut self,
            _: &mut Session,
            _: bool,
            _: &mut Settings,
            _: &Catalog,
        ) -> (Vec<AvisoDeConquista>, bool) {
            (Vec::new(), false)
        }
    }
}

#[cfg(feature = "conquistas")]
mod com_a_feature {
    use std::cell::RefCell;
    use std::rc::Rc;

    use zeebx::conquistas::http::ServidorHttp;
    use zeebx::conquistas::{Compartilhado, Conquistas, Evento};
    use zeebx::session::Session;
    use zeebx::ui::i18n::Catalog;
    use zeebx::ui::settings::Settings;

    use super::AvisoDeConquista;

    #[derive(Default)]
    pub struct NoNucleo {
        /// Criado no primeiro login, e não na abertura: sem conta, nada fala com o servidor.
        cliente: Option<Compartilhado>,
        /// O login a caminho, e se é com o token guardado.
        entrando: Option<bool>,
        erro: String,
        tentou_o_token: bool,
        /// Há um jogo aberto, e se a sessão dele já tem o cliente.
        jogo: Option<bool>,
    }

    impl NoNucleo {
        fn cliente(&mut self) -> Compartilhado {
            let cliente = self.cliente.get_or_insert_with(|| {
                Rc::new(RefCell::new(Conquistas::novo(Box::new(
                    ServidorHttp::default(),
                ))))
            });
            Rc::clone(cliente)
        }

        fn conectado(&self) -> bool {
            self.cliente
                .as_ref()
                .is_some_and(|c| c.borrow().usuario().is_some())
        }

        pub fn entrando(&self) -> bool {
            self.entrando.is_some()
        }

        pub fn conta(&self, catalogo: &Catalog) -> String {
            let Some(usuario) = self.cliente.as_ref().and_then(|c| c.borrow().usuario()) else {
                return String::new();
            };
            let pontos = usuario.pontos.to_string();
            let texto = [("user", usuario.nome.as_str()), ("points", &pontos)];
            catalogo.format("achievements.connected", &texto)
        }

        pub fn erro(&self) -> String {
            self.erro.clone()
        }

        pub fn entra(&mut self, usuario: &str, senha: &str) {
            if usuario.is_empty() || senha.is_empty() || self.entrando.is_some() {
                return;
            }
            self.erro.clear();
            self.entrando = Some(false);
            self.cliente().borrow_mut().entra_com_senha(usuario, senha);
        }

        pub fn sai(&mut self) {
            if let Some(cliente) = &self.cliente {
                cliente.borrow_mut().sai();
            }
            self.erro.clear();
        }

        /// Entra com o token guardado na primeira vez, e anda com os pedidos quando nenhuma sessão
        /// tem o cliente. Devolve se as configurações mudaram.
        pub fn a_cada_quadro(&mut self, settings: &mut Settings) -> bool {
            if !std::mem::replace(&mut self.tentou_o_token, true)
                && let (Some(usuario), Some(token)) =
                    (&settings.conquistas.usuario, &settings.conquistas.token)
            {
                self.entrando = Some(true);
                let (usuario, token) = (usuario.clone(), token.clone());
                self.cliente()
                    .borrow_mut()
                    .entra_com_token(&usuario, &token);
            }
            let Some(cliente) = self.cliente.clone() else {
                return false;
            };
            if self.jogo == Some(true) {
                return false;
            }
            cliente.borrow_mut().ocioso(None);
            let eventos = cliente.borrow_mut().eventos();
            eventos.iter().fold(false, |mudou, evento| {
                self.do_login(evento, settings) | mudou
            })
        }

        /// Guarda o token de um login, ou o erro. Devolve se as configurações mudaram.
        fn do_login(&mut self, evento: &Evento, settings: &mut Settings) -> bool {
            let Evento::Entrou(resultado) = evento else {
                return false;
            };
            let com_token = self.entrando.take().unwrap_or(false);
            match resultado {
                Ok(()) => {
                    let Some(usuario) = self.cliente.as_ref().and_then(|c| c.borrow().usuario())
                    else {
                        return false;
                    };
                    self.erro.clear();
                    settings.conquistas.usuario = Some(usuario.nome);
                    settings.conquistas.token = Some(usuario.token);
                    true
                }
                Err(motivo) => {
                    self.erro = motivo.clone();
                    // Um token recusado venceu: guardado, falharia em toda abertura.
                    com_token && settings.conquistas.token.take().is_some()
                }
            }
        }

        /// A sessão ganha o cliente se há conta ou login a caminho, que o rcheevos espera.
        pub fn liga(&mut self, sessao: &mut Session) {
            let ligar = self.conectado() || self.entrando.is_some();
            if let (true, Some(cliente)) = (ligar, &self.cliente) {
                sessao.liga_conquistas(Rc::clone(cliente));
            }
            self.jogo = Some(ligar && self.cliente.is_some());
        }

        pub fn desliga(&mut self, sessao: &mut Session) {
            sessao.desliga_conquistas();
            self.jogo = None;
        }

        /// Os pedidos andam com o jogo pausado, e os eventos viram avisos. Devolve também se as
        /// configurações mudaram.
        pub fn depois_do_passo(
            &mut self,
            sessao: &mut Session,
            pausada: bool,
            settings: &mut Settings,
            catalogo: &Catalog,
        ) -> (Vec<AvisoDeConquista>, bool) {
            // O login terminou com o jogo já aberto: a sessão ganha o cliente agora.
            if self.jogo == Some(false) && self.conectado() {
                self.liga(sessao);
            }
            let (Some(true), Some(cliente)) = (self.jogo, self.cliente.clone()) else {
                return (Vec::new(), false);
            };
            if pausada {
                sessao.conquistas_ociosas();
            }
            let eventos = cliente.borrow_mut().eventos();
            let mut avisos = Vec::new();
            let mut mudou = false;
            for evento in eventos {
                let (titulo, texto, icone) = match evento {
                    Evento::Entrou(_) => {
                        mudou |= self.do_login(&evento, settings);
                        continue;
                    }
                    Evento::JogoCarregado(Ok(Some(j))) => {
                        let (d, t) = (j.desbloqueadas.to_string(), j.total.to_string());
                        let texto = [("unlocked", d.as_str()), ("total", &t)];
                        (
                            j.titulo,
                            catalogo.format("achievements.game", &texto),
                            j.icone,
                        )
                    }
                    Evento::JogoCarregado(Ok(None)) => (
                        catalogo.get("achievements.game.none").into(),
                        String::new(),
                        None,
                    ),
                    Evento::JogoCarregado(Err(m)) => {
                        let titulo = catalogo.format("achievements.game.error", &[("reason", &m)]);
                        (titulo, String::new(), None)
                    }
                    Evento::Desbloqueou(c) => {
                        let pontos = c.pontos.to_string();
                        let texto =
                            catalogo.format("achievements.unlocked", &[("points", &pontos)]);
                        (c.titulo, texto, c.icone)
                    }
                };
                let icone = icone.unwrap_or_default();
                avisos.push(AvisoDeConquista {
                    titulo,
                    texto,
                    icone,
                });
            }
            (avisos, mudou)
        }
    }
}
