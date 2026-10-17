//! As conquistas do RetroAchievements na interface: a conta, o hardcore, os avisos e a lista. Ver
//! `docs/implementacao/25-conquistas.md`.
//!
//! **O objeto existe com ou sem a feature `conquistas`.** O cxx-qt gera a ponte a partir deste
//! arquivo, e uma ponte que só existisse com a feature pediria um `build.rs` e um QML para cada
//! caso. Sem a feature, o [`qobject::Conquistas::disponivel`] responde falso, o QML esconde a aba
//! e os avisos, e o [`NoNucleo`] não faz nada.

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

        /// Se esta compilação traz as conquistas.
        #[qinvokable]
        fn disponivel(self: &Conquistas) -> bool;

        #[qinvokable]
        fn conectado(self: &Conquistas) -> bool;

        /// O login está a caminho do servidor.
        #[qinvokable]
        fn entrando(self: &Conquistas) -> bool;

        /// "Conectado como …", já traduzido; vazio sem conta.
        #[qinvokable]
        fn conta(self: &Conquistas) -> QString;

        /// Por que o último login não deu certo, com o texto do servidor; vazio se deu.
        #[qinvokable]
        fn erro(self: &Conquistas) -> QString;

        /// Entra com usuário e senha. A senha vai ao servidor e é esquecida: o que fica nas
        /// configurações é o token.
        #[qinvokable]
        fn entra(self: &Conquistas, usuario: &QString, senha: &QString);

        /// Sai da conta e esquece o token.
        #[qinvokable]
        fn sai(self: &Conquistas);

        #[qinvokable]
        fn hardcore(self: &Conquistas) -> bool;

        /// Liga ou desliga o hardcore. Ligar com um jogo aberto o reinicia: a janela pergunta
        /// antes, pelo [`qobject::Conquistas::jogo_aberto`].
        #[qinvokable]
        #[cxx_name = "defineHardcore"]
        fn define_hardcore(self: &Conquistas, ligado: bool);

        /// Se há um jogo aberto que ligar o hardcore reiniciaria.
        #[qinvokable]
        #[cxx_name = "jogoAberto"]
        fn jogo_aberto(self: &Conquistas) -> bool;

        /// Um aviso ligado ou desligado: `desbloqueio` ou `placar`.
        #[qinvokable]
        fn aviso(self: &Conquistas, nome: &QString) -> bool;

        #[qinvokable]
        #[cxx_name = "defineAviso"]
        fn define_aviso(self: &Conquistas, nome: &QString, ligado: bool);

        /// O cabeçalho da lista: o jogo, quantas, os pontos e o modo. Vazio sem jogo com
        /// conquistas.
        #[qinvokable]
        fn cabecalho(self: &Conquistas) -> QString;

        /// A lista do jogo aberto, em JSON: `[{grupo, itens: [{titulo, descricao, pontos, icone,
        /// desbloqueada, progresso, percentual, semSuporte}]}]`. O QML a relê de tempos em tempos
        /// enquanto ela está aberta: os ícones chegam aos poucos.
        #[qinvokable]
        fn lista(self: &Conquistas) -> QString;
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

    pub fn conectado(&self) -> bool {
        nucleo::com(|n| n.conquistas.conectado())
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
        nucleo::com(|n| n.conquistas.entra(&n.settings, usuario.trim(), &senha));
    }

    pub fn sai(&self) {
        nucleo::com(|n| n.sai_das_conquistas());
    }

    pub fn hardcore(&self) -> bool {
        nucleo::com(|n| n.settings.conquistas.hardcore)
    }

    pub fn define_hardcore(&self, ligado: bool) {
        nucleo::com(|n| {
            n.settings.conquistas.hardcore = ligado;
            n.conquistas.define_hardcore(ligado);
            n.salva_configuracoes();
        });
    }

    pub fn jogo_aberto(&self) -> bool {
        nucleo::com(|n| n.conquistas.tem_jogo())
    }

    pub fn aviso(&self, nome: &QString) -> bool {
        let nome = String::from(nome);
        nucleo::com(|n| {
            let avisos = &n.settings.conquistas;
            match nome.as_str() {
                "desbloqueio" => avisos.aviso_de_desbloqueio,
                "placar" => avisos.aviso_de_placar,
                _ => false,
            }
        })
    }

    pub fn define_aviso(&self, nome: &QString, ligado: bool) {
        let nome = String::from(nome);
        nucleo::com(|n| {
            let avisos = &mut n.settings.conquistas;
            match nome.as_str() {
                "desbloqueio" => avisos.aviso_de_desbloqueio = ligado,
                "placar" => avisos.aviso_de_placar = ligado,
                _ => return,
            }
            n.salva_configuracoes();
        });
    }

    pub fn cabecalho(&self) -> QString {
        QString::from(&nucleo::com(|n| {
            let titulo = n.titulo().unwrap_or_default();
            let hardcore = n.settings.conquistas.hardcore;
            n.conquistas.cabecalho(&n.catalogo, &titulo, hardcore)
        }))
    }

    pub fn lista(&self) -> QString {
        QString::from(&nucleo::com(|n| n.conquistas.lista_em_json(&n.catalogo)))
    }
}

/// Um aviso por cima do jogo, já traduzido. Ver [`super::nucleo::Volta::conquistas`].
pub struct AvisoDeConquista {
    pub titulo: String,
    pub texto: String,
    /// O ícone em disco, como `file://`; vazio enquanto ele não chegou, ou sem ícone.
    pub icone: String,
}

/// O que uma volta do jogo traz das conquistas.
#[derive(Default)]
pub struct DaVolta {
    pub avisos: Vec<AvisoDeConquista>,
    /// Os contadores dos placares em andamento, numa linha; vazio esconde.
    pub contador: String,
    /// O hardcore foi ligado com o jogo aberto, e ele tem de recomeçar do zero.
    pub reiniciar: bool,
}

#[cfg(feature = "conquistas")]
pub use com_a_feature::NoNucleo;
#[cfg(not(feature = "conquistas"))]
pub use sem_a_feature::NoNucleo;

/// Sem a feature: tudo responde "nada", e nada acontece.
#[cfg(not(feature = "conquistas"))]
mod sem_a_feature {
    use std::path::Path;

    use zeebx::session::Session;
    use zeebx::ui::i18n::Catalog;
    use zeebx::ui::settings::Settings;

    use super::DaVolta;

    #[derive(Default)]
    pub struct NoNucleo;

    impl NoNucleo {
        pub fn conectado(&self) -> bool {
            false
        }
        pub fn entrando(&self) -> bool {
            false
        }
        pub fn conta(&self, _: &Catalog) -> String {
            String::new()
        }
        pub fn erro(&self) -> String {
            String::new()
        }
        pub fn entra(&mut self, _: &Settings, _: &str, _: &str) {}
        pub fn sai(&mut self) {}
        pub fn define_hardcore(&mut self, _: bool) {}
        pub fn tem_jogo(&self) -> bool {
            false
        }
        pub fn cabecalho(&self, _: &Catalog, _: &str, _: bool) -> String {
            String::new()
        }
        pub fn lista_em_json(&mut self, _: &Catalog) -> String {
            "[]".to_string()
        }
        pub fn a_cada_quadro(&mut self, _: &mut Settings) -> bool {
            false
        }
        pub fn liga(&mut self, _: &mut Session, _: &Path) {}
        pub fn desliga(&mut self, _: &mut Session) {}
        pub fn depois_do_passo(
            &mut self,
            _: &mut Session,
            _: bool,
            _: bool,
            _: &mut Settings,
            _: &Catalog,
        ) -> DaVolta {
            DaVolta::default()
        }
        pub fn resumo_guardado(&self, _: &Settings, _: &str, _: &Catalog) -> String {
            String::new()
        }
    }
}

#[cfg(feature = "conquistas")]
mod com_a_feature {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use zeebx::conquistas::http::ServidorHttp;
    use zeebx::conquistas::icones::{self, Icones};
    use zeebx::conquistas::{Compartilhado, Conquistas, ErroAoCarregar, Evento, Grupo, resumos};
    use zeebx::session::Session;
    use zeebx::ui::i18n::Catalog;
    use zeebx::ui::settings::Settings;

    use super::{AvisoDeConquista, DaVolta};

    /// O estado das conquistas que o [`super::super::nucleo::Nucleo`] guarda.
    #[derive(Default)]
    pub struct NoNucleo {
        /// Criado no primeiro login, e não na abertura: sem conta, nada fala com o servidor.
        cliente: Option<Compartilhado>,
        icones: Option<Icones>,
        /// O login a caminho, e se ele é com o token guardado (que, recusado, é esquecido).
        entrando: Option<bool>,
        erro: String,
        /// O jogo aberto: o caminho, para reabrir quando o hardcore liga, e a chave, para o
        /// resumo dos detalhes.
        jogo: Option<(PathBuf, Option<String>)>,
        /// O token guardado já foi tentado nesta execução.
        tentou_o_token: bool,
        contadores: BTreeMap<u32, String>,
        /// A sessão do jogo aberto já tem o cliente. Um jogo aberto antes de o login terminar —
        /// o `zeebx-qt <jogo>` abre antes do primeiro login com o token — ganha o cliente depois:
        /// ver [`NoNucleo::depois_do_passo`].
        ligado: bool,
    }

    impl NoNucleo {
        fn icones(&mut self) -> &mut Icones {
            self.icones
                .get_or_insert_with(|| Icones::novo(icones::pasta_padrao()))
        }

        /// O ícone de `url` como `file://`, se já está em disco; vazio enquanto baixa.
        fn icone(&mut self, url: Option<&str>) -> String {
            url.and_then(|url| self.icones().caminho(url))
                .map(|caminho| icones::endereco(&caminho))
                .unwrap_or_default()
        }

        fn cliente(&mut self, settings: &Settings) -> Compartilhado {
            let cliente = self.cliente.get_or_insert_with(|| {
                Rc::new(RefCell::new(Conquistas::novo(Box::new(
                    ServidorHttp::novo(),
                ))))
            });
            cliente
                .borrow_mut()
                .define_hardcore(settings.conquistas.hardcore);
            Rc::clone(cliente)
        }

        pub fn conectado(&self) -> bool {
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
            catalogo.format(
                "achievements.connected",
                &[
                    ("user", &usuario.nome_de_exibicao),
                    ("points", &usuario.pontos.to_string()),
                    ("casual", &usuario.pontos_casual.to_string()),
                ],
            )
        }

        pub fn erro(&self) -> String {
            self.erro.clone()
        }

        pub fn entra(&mut self, settings: &Settings, usuario: &str, senha: &str) {
            if usuario.is_empty() || senha.is_empty() || self.entrando.is_some() {
                return;
            }
            self.erro.clear();
            self.entrando = Some(false);
            self.cliente(settings)
                .borrow_mut()
                .entra_com_senha(usuario, senha);
        }

        pub fn sai(&mut self) {
            if let Some(cliente) = &self.cliente {
                cliente.borrow_mut().sai();
            }
            self.erro.clear();
            self.contadores.clear();
        }

        pub fn define_hardcore(&mut self, ligado: bool) {
            if let Some(cliente) = &self.cliente {
                cliente.borrow_mut().define_hardcore(ligado);
            }
        }

        pub fn tem_jogo(&self) -> bool {
            self.jogo.is_some()
                && self
                    .cliente
                    .as_ref()
                    .is_some_and(|c| c.borrow().jogo().is_some())
        }

        pub fn cabecalho(&self, catalogo: &Catalog, titulo: &str, hardcore: bool) -> String {
            let Some(cliente) = &self.cliente else {
                return String::new();
            };
            let Some(resumo) = cliente.borrow().resumo() else {
                return String::new();
            };
            let modo = match hardcore {
                true => catalogo.get("achievements.mode.hardcore"),
                false => catalogo.get("achievements.mode.casual"),
            };
            catalogo.format(
                "achievements.list.header",
                &[
                    ("game", titulo),
                    ("unlocked", &resumo.desbloqueadas.to_string()),
                    ("total", &resumo.total.to_string()),
                    ("points", &resumo.pontos_desbloqueados.to_string()),
                    ("max", &resumo.pontos.to_string()),
                    ("mode", modo),
                ],
            )
        }

        pub fn lista_em_json(&mut self, catalogo: &Catalog) -> String {
            let Some(cliente) = self.cliente.clone() else {
                return "[]".to_string();
            };
            let grupos = cliente.borrow_mut().lista();
            let grupos: Vec<serde_json::Value> = grupos
                .into_iter()
                .filter(|(_, itens)| !itens.is_empty())
                .map(|(grupo, itens)| {
                    let itens: Vec<serde_json::Value> = itens
                        .into_iter()
                        .map(|c| {
                            let url = match c.desbloqueada {
                                true => c.icone.as_deref(),
                                false => c.icone_bloqueado.as_deref().or(c.icone.as_deref()),
                            };
                            let icone = self.icone(url);
                            serde_json::json!({
                                "titulo": c.titulo,
                                "descricao": c.descricao,
                                "pontos": catalogo.format("achievements.points", &[("points", &c.pontos.to_string())]),
                                "icone": icone,
                                "desbloqueada": c.desbloqueada,
                                "progresso": c.progresso,
                                "percentual": c.percentual,
                                "semSuporte": c.sem_suporte,
                            })
                        })
                        .collect();
                    serde_json::json!({ "grupo": catalogo.get(chave_do_grupo(grupo)), "itens": itens })
                })
                .collect();
            serde_json::Value::Array(grupos).to_string()
        }

        /// O que a interface faz a cada leitura: entra com o token guardado na primeira vez, e
        /// anda com os pedidos quando nenhuma sessão tem o cliente (com uma, quem anda é o passo).
        /// Devolve se as configurações mudaram e precisam ser gravadas.
        pub fn a_cada_quadro(&mut self, settings: &mut Settings) -> bool {
            if !self.tentou_o_token {
                self.tentou_o_token = true;
                if let (Some(usuario), Some(token)) = (
                    settings.conquistas.usuario.clone(),
                    settings.conquistas.token.clone(),
                ) {
                    self.entrando = Some(true);
                    self.cliente(settings)
                        .borrow_mut()
                        .entra_com_token(&usuario, &token);
                }
            }
            if let Some(icones) = &mut self.icones {
                icones.recolhe();
            }
            let Some(cliente) = self.cliente.clone() else {
                return false;
            };
            // **Sem sessão ligada, é aqui que o login anda.** Antes, com um jogo aberto este passo
            // não fazia nada, e um jogo aberto antes de o login terminar travava os dois: o login
            // esperava um quadro que nenhuma sessão rodava.
            if self.ligado {
                return false;
            }
            cliente.borrow_mut().ocioso(None);
            let eventos = cliente.borrow_mut().eventos();
            let mut mudou = false;
            for evento in eventos {
                mudou |= self.do_login(&evento, settings);
            }
            mudou
        }

        /// O resultado de um login: o token guardado, ou o erro. Devolve se as configurações
        /// mudaram.
        fn do_login(&mut self, evento: &Evento, settings: &mut Settings) -> bool {
            let Evento::Entrou(resultado) = evento else {
                return false;
            };
            let com_token = self.entrando.take().unwrap_or(false);
            match resultado {
                Ok(()) => {
                    let usuario = self.cliente.as_ref().and_then(|c| c.borrow().usuario());
                    let Some(usuario) = usuario else {
                        return false;
                    };
                    self.erro.clear();
                    settings.conquistas.usuario = Some(usuario.nome);
                    settings.conquistas.token = Some(usuario.token);
                    true
                }
                Err(motivo) => {
                    self.erro = motivo.clone();
                    // Um token recusado está vencido ou foi revogado: guardá-lo faria cada
                    // abertura tentar de novo e falhar do mesmo jeito.
                    if com_token {
                        settings.conquistas.token = None;
                        return true;
                    }
                    false
                }
            }
        }

        /// O jogo abriu: a sessão ganha o cliente, se há conta.
        pub fn liga(&mut self, sessao: &mut Session, caminho: &Path) {
            self.contadores.clear();
            self.jogo = Some((
                caminho.to_path_buf(),
                zeebx::library::chave_do_jogo(caminho),
            ));
            // Com o login a caminho também: o rcheevos espera ele terminar antes de carregar.
            self.ligado = false;
            if (self.conectado() || self.entrando.is_some())
                && let Some(cliente) = &self.cliente
            {
                sessao.liga_conquistas(Rc::clone(cliente));
                self.ligado = true;
            }
        }

        pub fn desliga(&mut self, sessao: &mut Session) {
            sessao.desliga_conquistas();
            self.jogo = None;
            self.ligado = false;
            self.contadores.clear();
        }

        /// Depois de cada volta: os pedidos andam com o jogo parado, os eventos viram avisos, e o
        /// rewind recusado também.
        pub fn depois_do_passo(
            &mut self,
            sessao: &mut Session,
            pausada: bool,
            rewind_recusado: bool,
            settings: &mut Settings,
            catalogo: &Catalog,
        ) -> DaVolta {
            let mut volta = DaVolta::default();
            if rewind_recusado {
                volta.avisos.push(AvisoDeConquista {
                    titulo: catalogo.get("achievements.hardcore_blocked").to_string(),
                    texto: String::new(),
                    icone: String::new(),
                });
            }
            let Some(cliente) = self.cliente.clone() else {
                return volta;
            };
            // O login terminou com o jogo já aberto: a sessão ganha o cliente agora. No hardcore,
            // não: o jogo rodou até aqui sem as travas, e o RA exige o hardcore desde o começo
            // da partida. O jogo recomeça, e a abertura liga o cliente.
            if !self.ligado && self.jogo.is_some() && self.conectado() {
                if settings.conquistas.hardcore {
                    volta.reiniciar = true;
                    return volta;
                }
                sessao.liga_conquistas(Rc::clone(&cliente));
                self.ligado = true;
            }
            if !self.ligado {
                return volta;
            }
            if pausada {
                sessao.conquistas_ociosas();
            }
            self.icones().recolhe();
            let eventos = cliente.borrow_mut().eventos();
            for evento in eventos {
                if self.do_login(&evento, settings) {
                    let _ = settings.save();
                }
                let simples = |titulo: String| AvisoDeConquista {
                    titulo,
                    texto: String::new(),
                    icone: String::new(),
                };
                match evento {
                    Evento::Entrou(_) => {}
                    Evento::JogoCarregado(Ok(())) => {
                        let (resumo, jogo) = {
                            let c = cliente.borrow();
                            (c.resumo().unwrap_or_default(), c.jogo())
                        };
                        let modo = match settings.conquistas.hardcore {
                            true => catalogo.get("achievements.mode.hardcore"),
                            false => catalogo.get("achievements.mode.casual"),
                        };
                        let icone = self.icone(jogo.as_ref().and_then(|j| j.icone.as_deref()));
                        volta.avisos.push(AvisoDeConquista {
                            titulo: jogo.map(|j| j.titulo).unwrap_or_default(),
                            texto: catalogo.format(
                                "achievements.game.start",
                                &[
                                    ("unlocked", &resumo.desbloqueadas.to_string()),
                                    ("total", &resumo.total.to_string()),
                                    ("mode", modo),
                                ],
                            ),
                            icone,
                        });
                        self.guarda_resumo(settings);
                        // Os ícones de todas, já: o aviso de desbloqueio sai com o ícone.
                        let lista = cliente.borrow_mut().lista();
                        for c in lista.into_iter().flat_map(|(_, itens)| itens) {
                            for url in [c.icone, c.icone_bloqueado].into_iter().flatten() {
                                self.icones().caminho(&url);
                            }
                        }
                    }
                    Evento::JogoCarregado(Err(erro)) => {
                        let texto = match erro {
                            ErroAoCarregar::Desconhecido => {
                                catalogo.get("achievements.game.none").to_string()
                            }
                            ErroAoCarregar::Cancelado => continue,
                            ErroAoCarregar::Outro(motivo) => {
                                catalogo.format("achievements.game.error", &[("reason", &motivo)])
                            }
                        };
                        volta.avisos.push(simples(texto));
                    }
                    Evento::Desbloqueou(c) => {
                        self.guarda_resumo(settings);
                        if !settings.conquistas.aviso_de_desbloqueio {
                            continue;
                        }
                        let icone = self.icone(c.icone.as_deref());
                        volta.avisos.push(AvisoDeConquista {
                            titulo: c.titulo,
                            texto: catalogo.format(
                                "achievements.unlocked",
                                &[("points", &c.pontos.to_string())],
                            ),
                            icone,
                        });
                    }
                    Evento::PlacarComecou { titulo, .. } => aviso_de_placar(
                        &mut volta,
                        settings,
                        catalogo,
                        "achievements.leaderboard.started",
                        &titulo,
                        "",
                    ),
                    Evento::PlacarFalhou { titulo, .. } => aviso_de_placar(
                        &mut volta,
                        settings,
                        catalogo,
                        "achievements.leaderboard.failed",
                        &titulo,
                        "",
                    ),
                    Evento::PlacarEnviado { titulo, valor, .. } => aviso_de_placar(
                        &mut volta,
                        settings,
                        catalogo,
                        "achievements.leaderboard.submitted",
                        &titulo,
                        &valor,
                    ),
                    Evento::Contador { id, texto } => {
                        self.contadores.insert(id, texto);
                    }
                    Evento::ContadorSumiu { id } => {
                        self.contadores.remove(&id);
                    }
                    Evento::Reiniciar => volta.reiniciar = true,
                    Evento::JogoCompleto => volta
                        .avisos
                        .push(simples(catalogo.get("achievements.completed").to_string())),
                    Evento::ErroDoServidor { mensagem, .. } => volta.avisos.push(simples(
                        catalogo.format("achievements.server_error", &[("message", &mensagem)]),
                    )),
                    Evento::Desconectado => volta.avisos.push(simples(
                        catalogo.get("achievements.disconnected").to_string(),
                    )),
                    Evento::Reconectado => volta.avisos.push(simples(
                        catalogo.get("achievements.reconnected").to_string(),
                    )),
                }
            }
            volta.contador = self
                .contadores
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join("  ·  ");
            volta
        }

        /// Guarda o "12/40" do jogo aberto, para os detalhes da biblioteca.
        fn guarda_resumo(&self, settings: &Settings) {
            let (Some(cliente), Some((_, Some(chave))), Some(usuario)) =
                (&self.cliente, &self.jogo, &settings.conquistas.usuario)
            else {
                return;
            };
            if let Some(resumo) = cliente.borrow().resumo() {
                resumos::grava(
                    chave,
                    resumos::Guardado {
                        usuario: usuario.clone(),
                        desbloqueadas: resumo.desbloqueadas,
                        total: resumo.total,
                    },
                );
            }
        }

        /// O "12/40" de um jogo nos detalhes da biblioteca, da última vez em que ele foi aberto
        /// com esta conta. Vazio sem conta ou sem resumo.
        pub fn resumo_guardado(
            &self,
            settings: &Settings,
            chave: &str,
            catalogo: &Catalog,
        ) -> String {
            let Some(usuario) = &settings.conquistas.usuario else {
                return String::new();
            };
            resumos::le(chave, usuario)
                .map(|g| {
                    catalogo.format(
                        "details.achievements.value",
                        &[
                            ("unlocked", &g.desbloqueadas.to_string()),
                            ("total", &g.total.to_string()),
                        ],
                    )
                })
                .unwrap_or_default()
        }
    }

    fn aviso_de_placar(
        volta: &mut DaVolta,
        settings: &Settings,
        catalogo: &Catalog,
        chave: &str,
        titulo: &str,
        valor: &str,
    ) {
        if !settings.conquistas.aviso_de_placar {
            return;
        }
        volta.avisos.push(AvisoDeConquista {
            titulo: catalogo.format(chave, &[("title", titulo), ("value", valor)]),
            texto: String::new(),
            icone: String::new(),
        });
    }

    fn chave_do_grupo(grupo: Grupo) -> &'static str {
        match grupo {
            Grupo::Bloqueadas => "achievements.group.locked",
            Grupo::Desbloqueadas => "achievements.group.unlocked",
            Grupo::SemSuporte => "achievements.group.unsupported",
            Grupo::NaoOficiais => "achievements.group.unofficial",
            Grupo::RecemDesbloqueadas => "achievements.group.recent",
            Grupo::DesafioEmAndamento => "achievements.group.challenge",
            Grupo::QuaseLa => "achievements.group.almost",
            Grupo::NaoEnviadas => "achievements.group.unsynced",
            Grupo::Outras => "achievements.group.other",
        }
    }
}
