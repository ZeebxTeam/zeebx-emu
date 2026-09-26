//! O estado da interface Qt: as configurações, a biblioteca, a entrada do desktop e o jogo aberto.
//!
//! **Mora num `thread_local`, e não num `QObject`.** O cxx-qt cria cada `QObject` a partir do QML,
//! sem argumentos, e a tela do jogo, a biblioteca e as configurações precisam ver o mesmo estado.
//! Tudo roda na thread da interface — o emulador também, ver `docs/implementacao/01-arquitetura.md`
//! —, então um `RefCell` basta; um `Mutex` seria sincronização sem ninguém do outro lado.
//!
//! **Nunca emitir sinal de dentro do [`com`].** Um sinal chama o QML na hora, o QML pode chamar de
//! volta um `QObject` que também entra no [`com`], e o segundo empréstimo do `RefCell` derruba o
//! programa. Quem chama pega o que precisa aqui dentro e emite depois.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use zeebx::eframe::egui::Key;
use zeebx::input;
use zeebx::input::padview::PadArt;
use zeebx::library::{self, Game};
use zeebx::session::Z_WHEEL;
use zeebx::input::bindings::Aparelho;
use zeebx::ui::calibracao::{Calibracao, Situacao};
use zeebx::ui::acervo::{self, Acervo, Ficha};
use zeebx::ui::depuracao;
use zeebx::ui::{atualizacao, discord};
use zeebx::ui::navegacao::{self, Comando, Navegacao};
use zeebx::video::icon::{self, Image};
use zeebx::ui::entrada::EntradaDoDesktop;
use zeebx::ui::i18n::{self, Catalog};
use zeebx::ui::partida::{self, Abertura, Partida, Relatorio, Saida};
use zeebx::ui::settings::{self, Proporcao, Settings};
use zeebx::video::rasterizer::QuadroNaPlaca;
use zeebx::video::display::Framebuffer;

use super::ponte::qobject as gl;

/// A imagem de quem não tem nenhuma: a logo do emulador, como no egui.
const RESERVA: &[u8] = include_bytes!("../../../../assets/zeebx.png");

thread_local! {
    static NUCLEO: RefCell<Option<Nucleo>> = const { RefCell::new(None) };
}

/// Roda `f` com o estado da interface, criando-o na primeira vez.
pub fn com<T>(f: impl FnOnce(&mut Nucleo) -> T) -> T {
    NUCLEO.with(|nucleo| {
        let mut nucleo = nucleo.borrow_mut();
        f(nucleo.get_or_insert_with(Nucleo::novo))
    })
}

/// Solta o jogo aberto com o contexto de GL dele ainda vivo. O `launch` chama isto depois de a
/// engine sair e antes de destruir o contexto — ver a ordem descrita lá.
pub fn encerra() {
    NUCLEO.with(|nucleo| {
        if let Some(mut nucleo) = nucleo.borrow_mut().take() {
            nucleo.fecha();
        }
    });
}

/// O que uma volta do jogo pede à janela. Ver [`Nucleo::passo`].
#[derive(Default)]
pub struct Volta {
    /// O jogo saiu sozinho e não há para onde voltar: a janela do jogo fecha.
    pub fechou: bool,
    /// Outro jogo abriu nesta volta — pela Z-Wheel —, e a linha de estado muda: o título dele, ou
    /// por que não abriu.
    pub estado: Option<String>,
    /// Por que o jogo parou, já traduzido; vazio enquanto ele roda. Um jogo que parou continua com
    /// o último quadro à mostra, e o motivo precisa aparecer em algum lugar.
    pub parou: String,
    /// O aviso de calibração do Boomerang neste quadro, se há um. Ver [`zeebx::ui::calibracao`].
    pub aviso: Option<AvisoNaTela>,
}

/// O aviso de calibração já com os textos, como a janela o desenha.
pub struct AvisoNaTela {
    pub titulo: String,
    /// Vazio depois de concluído.
    pub estado: String,
    pub parado: bool,
    /// Em radianos.
    pub giro: f32,
}

/// O que a tela do jogo mostra. Ver [`Nucleo::quadro`].
pub enum Quadro<'a> {
    /// A textura do rasterizador na placa, na resolução interna, sem voltar à CPU.
    Placa(QuadroNaPlaca),
    /// A tela do console em RGB565 — o 2D, o 3D por software, ou a placa com algo desenhado por
    /// cima em 2D.
    Tela(&'a Framebuffer),
}

pub struct Nucleo {
    pub settings: Settings,
    pub catalogo: Catalog,
    pub entrada: EntradaDoDesktop,
    /// O desenho do controle, com a silhueta de cada botão. Vazio se o desenho não abrir: a tela
    /// de controles continua servindo pela lista.
    pub arte: Option<PadArt>,
    partida: Option<Partida>,
    /// O contexto que o rasterizador na placa recebe emprestado. Criado na primeira abertura, e não
    /// no arranque: precisa do `QGuiApplication` de pé. Com ele, tudo que toca a sessão — o passo,
    /// a abertura, e o fim de uma partida, que solta texturas — acontece com o contexto corrente.
    gl: Option<Arc<glow::Context>>,
    /// "software", ou a placa que o contexto achou: vai para a linha de estado.
    placa: Option<String>,
    /// Os jogos da pasta de ROMs.
    pub jogos: Vec<Game>,
    /// O que a lista mostra, em ordem de título: `(título, índice em jogos)`. A Z-Wheel sai dela e
    /// abre pelo botão, como no egui, e a busca deixa só o que casa.
    pub lista: Vec<(String, usize)>,
    /// O que está escrito na busca.
    busca: String,
    pub z_wheel: Option<PathBuf>,
    /// Capas, nomes, logos e descrições que a Z-Wheel traz dos jogos.
    pub acervo: Option<Acervo>,
    /// A imagem de quem não tem nenhuma.
    reserva: Option<Image>,
    /// Muda a cada varredura. Vai no endereço das imagens: o Qt as guarda pelo endereço, e sem
    /// isto uma capa trocada na pasta continuaria a antiga depois de procurar de novo.
    pub geracao: u32,
    /// Se cada jogo tem uma capa ao lado, por índice em `jogos`: a resposta lê o disco, e o QML
    /// pergunta a cada linha que aparece. Refeito a cada varredura.
    capas_ao_lado: HashMap<usize, bool>,
    navegacao: Navegacao,
    /// A presença no Discord, acompanhando o que a interface mostra.
    pub presenca: discord::Acompanha,
    /// A procura por versão nova em andamento, e o que ela respondeu.
    procura_de_atualizacao: Option<Receiver<atualizacao::Resposta>>,
    pub atualizacao: Option<atualizacao::Resposta>,
    /// Uma versão nova chegou e o aviso dela ainda não foi dispensado.
    aviso_de_atualizacao: bool,
    /// O relatório da execução, gravado sozinho. Ver [`Relatorio`].
    relatorio: Relatorio,
    /// A janela de log foi fechada nesta execução. Zera ao abrir outro jogo: fechar dispensa o log
    /// **desta** execução, e não a preferência.
    pub log_dispensado: bool,
    calibracao: Calibracao,
    /// As teclas apertadas na janela do jogo, como `egui::Key`. Ver
    /// [`zeebx::ui::entrada::tecla_apertada`].
    teclas: HashSet<Key>,
}

impl Nucleo {
    fn novo() -> Self {
        let settings = Settings::load();
        // O mesmo idioma do egui: o escolhido nas configurações, ou o do sistema.
        let mut catalogo = Catalog::new(&settings::language_dirs());
        match &settings.language {
            Some(codigo) => {
                catalogo.select(codigo);
            }
            None => {
                catalogo.select_best(&i18n::system_language());
            }
        }
        let mut nucleo = Self {
            settings,
            catalogo,
            entrada: EntradaDoDesktop::inicia(),
            arte: PadArt::builtin()
                .inspect_err(|erro| eprintln!("desenho do controle: {erro:?}"))
                .ok(),
            partida: None,
            gl: None,
            placa: None,
            jogos: Vec::new(),
            lista: Vec::new(),
            busca: String::new(),
            z_wheel: None,
            acervo: None,
            reserva: icon::decode(RESERVA).ok(),
            geracao: 0,
            capas_ao_lado: HashMap::new(),
            navegacao: Navegacao::default(),
            presenca: discord::Acompanha::default(),
            procura_de_atualizacao: None,
            atualizacao: None,
            aviso_de_atualizacao: false,
            relatorio: Relatorio::default(),
            log_dispensado: false,
            calibracao: Calibracao::default(),
            teclas: HashSet::new(),
        };
        nucleo.procura_de_novo();
        // Na abertura, a pergunta ao GitHub, como no egui: a resposta chega pelo relógio da
        // biblioteca, e o aviso espera o de abertura sair da frente.
        if nucleo.settings.atualizacoes.ao_abrir {
            nucleo.procura_atualizacao();
        }
        nucleo
    }

    /// Varre a pasta de ROMs de novo, e relê a Z-Wheel e o acervo dela. É o "procurar de novo".
    pub fn procura_de_novo(&mut self) {
        self.jogos = self
            .settings
            .roms_dir
            .as_deref()
            .map(library::scan)
            .unwrap_or_default();
        if let Err(erro) = library::sync_catalog(&self.jogos) {
            eprintln!("catálogo de jogos: {erro}");
        }
        self.capas_ao_lado.clear();
        self.atualiza_z_wheel();
    }

    /// Resolve de onde abrir a Z-Wheel e relê o acervo dela, sem varrer a pasta de novo. É o que
    /// mudar a Z-Wheel nas configurações faz: a capa e o nome de cada jogo podem mudar junto.
    pub fn atualiza_z_wheel(&mut self) {
        self.z_wheel = library::z_wheel_de(self.settings.z_wheel_path.as_deref(), &self.jogos);
        self.acervo = self.z_wheel.as_deref().and_then(Acervo::carrega);
        self.geracao = self.geracao.wrapping_add(1);
        self.refaz_lista();
    }

    /// Troca o idioma da interface. Os nomes oficiais da Z-Wheel são por idioma, então a lista é
    /// refeita junto. Devolve se o idioma existe.
    pub fn muda_idioma(&mut self, codigo: &str) -> bool {
        if !self.catalogo.select(codigo) {
            return false;
        }
        self.settings.language = Some(codigo.to_string());
        self.refaz_lista();
        true
    }

    /// O que a interface faz a cada leitura, com ou sem jogo: diz ao Discord o que está
    /// acontecendo e recolhe a resposta da procura por versão nova.
    pub fn a_cada_quadro(&mut self) {
        let classe = self.partida.as_ref().map(|partida| partida.sessao().classe());
        let titulo = self.titulo();
        let discord = &self.settings.discord;
        self.presenca
            .atualiza(discord.ativo, &self.catalogo, classe, titulo, &discord.capas_url);
        if let Some(canal) = &self.procura_de_atualizacao {
            match canal.try_recv() {
                Ok(resposta) => {
                    self.aviso_de_atualizacao = matches!(resposta, atualizacao::Resposta::Nova(_));
                    self.atualizacao = Some(resposta);
                    self.procura_de_atualizacao = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.procura_de_atualizacao = None,
            }
        }
    }

    /// O aviso de abertura ainda não foi dispensado nesta versão. Uma versão nova mostra de novo.
    pub fn aviso_de_abertura_pendente(&self) -> bool {
        self.settings.aviso_dispensado_na_versao.as_deref() != Some(atualizacao::VERSAO_ATUAL)
    }

    /// O aviso de abertura saiu da frente. `de_vez` é o "não mostrar de novo": guarda a versão.
    pub fn dispensa_aviso_de_abertura(&mut self, de_vez: bool) {
        if de_vez {
            self.settings.aviso_dispensado_na_versao = Some(atualizacao::VERSAO_ATUAL.to_owned());
            if let Err(erro) = self.settings.save() {
                eprintln!("não deu para guardar as configurações: {erro}");
            }
        }
    }

    /// A versão nova a avisar, e a página dela, se há um aviso pendente.
    pub fn aviso_de_atualizacao(&self) -> Option<&atualizacao::Lancamento> {
        match (&self.atualizacao, self.aviso_de_atualizacao) {
            (Some(atualizacao::Resposta::Nova(lancamento)), true) => Some(lancamento),
            _ => None,
        }
    }

    pub fn dispensa_aviso_de_atualizacao(&mut self) {
        self.aviso_de_atualizacao = false;
    }

    /// O que o jogo aberto escreveu, e as queixas do emulador sobre ele.
    pub fn log(&self) -> Vec<String> {
        self.partida.as_ref().map(|p| p.sessao().log()).unwrap_or_default()
    }

    /// Onde o relatório desta execução é gravado sozinho.
    pub fn caminho_do_relatorio(&self) -> Option<PathBuf> {
        self.partida.as_ref().map(Partida::caminho_do_relatorio)
    }

    /// Pergunta ao GitHub se há versão nova. A resposta chega pelo [`Nucleo::a_cada_quadro`].
    pub fn procura_atualizacao(&mut self) {
        self.atualizacao = None;
        self.procura_de_atualizacao = Some(atualizacao::procura());
    }

    pub fn procurando_atualizacao(&self) -> bool {
        self.procura_de_atualizacao.is_some()
    }

    /// Mexe na sessão do jogo aberto, com o contexto de GL corrente: a resolução interna, a
    /// proporção e as melhorias refazem o destino na placa, e isso é GL.
    pub fn na_sessao(&mut self, f: impl FnOnce(&mut zeebx::session::Session)) {
        let com_placa = self.gl.is_some();
        if let Some(partida) = self.partida.as_mut() {
            no_contexto(com_placa, || f(partida.sessao_mut()));
        }
    }

    /// A busca mudou.
    pub fn define_busca(&mut self, busca: &str) {
        self.busca = busca.to_string();
        self.refaz_lista();
    }

    /// Refaz a lista sem varrer a pasta: a busca, o idioma ou a Z-Wheel mudaram.
    pub fn refaz_lista(&mut self) {
        let idioma = self.catalogo.current().to_string();
        let acervo = self.acervo.as_ref();
        let mut lista: Vec<(String, usize)> = (0..self.jogos.len())
            .filter(|&i| self.jogos[i].clsid != Some(Z_WHEEL))
            .map(|i| (acervo::titulo_de(acervo, &self.jogos[i], &idioma), i))
            .filter(|(titulo, _)| library::casa_com_a_busca(titulo, &self.busca))
            .collect();
        lista.sort_by_key(|(titulo, _)| titulo.to_lowercase());
        self.lista = lista;
    }

    /// Quantos jogos a biblioteca tem, fora a Z-Wheel e sem a busca.
    pub fn total(&self) -> usize {
        self.jogos.iter().filter(|jogo| jogo.clsid != Some(Z_WHEEL)).count()
    }

    pub fn busca_atual(&self) -> &str {
        &self.busca
    }

    pub fn buscando(&self) -> bool {
        !self.busca.trim().is_empty()
    }

    /// A ficha da Z-Wheel sobre o jogo da linha, se ela o conhece.
    pub fn ficha(&self, linha: usize) -> Option<&Ficha> {
        let (_, indice) = self.lista.get(linha)?;
        let classe = self.jogos[*indice].clsid?;
        self.acervo.as_ref()?.ficha(classe)
    }

    /// A imagem do jogo de índice `indice` em `jogos`. Ver [`acervo::imagem_do_jogo`].
    pub fn capa(&mut self, indice: usize) -> Option<&Image> {
        let jogo = self.jogos.get(indice)?;
        let ao_lado = *self
            .capas_ao_lado
            .entry(indice)
            .or_insert_with(|| acervo::capa_ao_lado(jogo));
        let ficha = jogo.clsid.and_then(|classe| self.acervo.as_ref()?.ficha(classe));
        acervo::imagem_do_jogo(jogo, ficha, self.reserva.as_ref(), ao_lado)
    }

    /// A imagem pedida pelo QML, pelo endereço sem o esquema: `capa/<geração>/<índice>`,
    /// `logo/<geração>/<classe>` ou `classificacao/<geração>/<classe>`.
    pub fn imagem(&mut self, endereco: &str) -> Option<&Image> {
        let mut partes = endereco.split('/');
        let (tipo, _geracao, chave) = (partes.next()?, partes.next()?, partes.next()?);
        let chave: usize = chave.parse().ok()?;
        match tipo {
            "capa" => self.capa(chave),
            "logo" => self.acervo.as_ref()?.ficha(chave as u32)?.logo.as_ref(),
            "classificacao" => self.acervo.as_ref()?.ficha(chave as u32)?.classificacao.as_ref(),
            _ => None,
        }
    }

    /// O desenho do controle para o QML: `controle/base`, ou `controle/parte/<índice>/<rrggbbaa>`
    /// — a silhueta de um botão já na cor pedida. A silhueta só guarda a opacidade; a cor é a que
    /// a tela escolhe na hora (aceso, sob o cursor, esperando a captura), como no egui.
    pub fn imagem_do_controle(&self, endereco: &str) -> Option<Image> {
        let arte = self.arte.as_ref()?;
        let mut partes = endereco.split('/').skip(1);
        match partes.next()? {
            "base" => Some(Image {
                width: arte.width,
                height: arte.height,
                rgba: arte.base.clone(),
            }),
            "parte" => {
                let parte = arte.parts().get(partes.next()?.parse::<usize>().ok()?)?;
                let cor = u32::from_str_radix(partes.next()?, 16).ok()?.to_be_bytes();
                let mut rgba = Vec::with_capacity(parte.alpha.len() * 4);
                for alfa in &parte.alpha {
                    let alfa = (u32::from(*alfa) * u32::from(cor[3]) / 255) as u8;
                    rgba.extend_from_slice(&[cor[0], cor[1], cor[2], alfa]);
                }
                Some(Image {
                    width: parte.width,
                    height: parte.height,
                    rgba,
                })
            }
            _ => None,
        }
    }

    /// As portas mudaram nas configurações: com um jogo aberto, vale na hora, como no egui.
    /// Guardar e só aplicar na próxima partida seria a configuração parecer que não pegou.
    pub fn portas_mudaram(&mut self) {
        let portas = std::array::from_fn(|porta| {
            self.settings
                .controls
                .player(porta)
                .filter(|jogador| jogador.ligada)
                .map(|jogador| jogador.aparelho)
        });
        if let Some(partida) = self.partida.as_mut() {
            partida.sessao_mut().set_portas(portas);
        }
    }

    /// Os comandos que o controle manda à biblioteca nesta leitura.
    ///
    /// **Só o controle.** O teclado da biblioteca é do QML, que já anda pela lista com as setas:
    /// passar as teclas aqui também faria uma seta mapeada no controle andar dois passos. Com um
    /// jogo aberto, ou a janela principal sem o foco, o controle não é da biblioteca.
    pub fn comandos(&mut self, escutando: bool) -> Vec<Comando> {
        if !escutando || self.partida.is_some() {
            self.navegacao.silencia();
            return Vec::new();
        }
        let pads: Vec<_> = self
            .entrada
            .pads(&self.settings.controls, &HashSet::new())
            .into_iter()
            .map(|(_, pad)| pad)
            .collect();
        let agora = navegacao::apertado([false; 5], &pads);
        self.navegacao.comandos(agora, Instant::now())
    }

    /// O que a tela do jogo mostra agora.
    ///
    /// **A textura grande só quando é ela que está na tela**: o [`Session::quadro_na_placa`] a
    /// devolve apenas se nada foi desenhado em 2D depois do último `eglSwapBuffers`. Com HUD pelo
    /// `IDisplay`, caixa de mensagem ou a Z-Wheel, que compõe o 3D na CPU, vale a tela de 640×480.
    ///
    /// [`Session::quadro_na_placa`]: zeebx::session::Session::quadro_na_placa
    pub fn quadro(&self) -> Option<Quadro<'_>> {
        let sessao = self.partida.as_ref()?.sessao();
        let placa = self.gl.as_ref().and(sessao.quadro_na_placa());
        Some(match placa {
            Some(quadro) => Quadro::Placa(quadro),
            None => Quadro::Tela(sessao.screen()),
        })
    }

    /// O título do jogo aberto e onde ele desenha.
    pub fn estado(&self) -> String {
        let Some(titulo) = self.titulo() else {
            return String::new();
        };
        let onde = match (&self.placa, self.settings.graphics.gpu_rasterizer) {
            (Some(placa), true) => format!("placa: {placa}"),
            _ => "software".to_string(),
        };
        format!("{titulo} — {onde}")
    }

    /// O nome do jogo aberto.
    ///
    /// A sessão só conhece a pasta de extração, que leva a impressão digital do pacote
    /// (`Zeebo-Extreme-Boia-Cross-21503726-1788761080`). O nome sai da biblioteca pelo ClassID — o
    /// oficial da Z-Wheel quando ela descreve o jogo —, e só na falta dela a pasta, sem os dois
    /// números do fim. A regra do egui.
    pub fn titulo(&self) -> Option<String> {
        let sessao = self.partida.as_ref()?.sessao();
        let classe = sessao.classe();
        if let Some(jogo) = self.jogos.iter().find(|jogo| jogo.clsid == Some(classe)) {
            return Some(acervo::titulo_de(self.acervo.as_ref(), jogo, self.catalogo.current()));
        }
        let titulo = library::sem_impressao_digital(sessao.title());
        (!titulo.is_empty()).then_some(titulo)
    }

    /// Abre um jogo escolhido na janela principal. Não foi a Z-Wheel quem pediu: quando ele sair
    /// sozinho, a janela do jogo fecha em vez de voltar para ela.
    pub fn abre_pela_biblioteca(&mut self, caminho: &Path) -> Result<(), String> {
        let aberta = self.abre(caminho);
        if let Some(partida) = self.partida.as_mut() {
            partida.aberta_pela_z_wheel = false;
        }
        aberta
    }

    /// Troca a partida pela do jogo em `caminho`. Falhando, a de antes continua, como no egui.
    fn abre(&mut self, caminho: &Path) -> Result<(), String> {
        if self.gl.is_none() {
            match contexto_do_qt() {
                Ok((contexto, placa)) => {
                    self.gl = Some(contexto);
                    self.placa = Some(placa);
                }
                Err(erro) => eprintln!("{erro} — seguindo no rasterizador de software"),
            }
        }
        self.teclas.clear();
        // As contagens de calibração são da sessão: a nova começa do zero.
        self.calibracao.reinicia();
        if let Some(partida) = self.partida.as_mut() {
            partida.esquece_entrada();
        }
        // A serial é ligada junto com o começo, e não depois: o construtor do applet roda dentro
        // da abertura, e o que ele faz ao nascer precisa estar na captura.
        let serial = self
            .settings
            .debug
            .log
            .then(|| partida::caminho_da_serial(&library::title_for(caminho)));
        self.relatorio.esquece();
        self.log_dispensado = false;
        let abertura = Abertura {
            settings: &self.settings,
            serial: serial.as_deref(),
            gl: self.gl.clone(),
            instalados: self
                .jogos
                .iter()
                .filter_map(|jogo| Some((jogo.clsid?, library::id_do_modulo(&jogo.path)?)))
                .collect(),
        };
        let anterior = self.partida.as_mut();
        let com_placa = self.gl.is_some();
        let partida = no_contexto(com_placa, || Partida::abre(caminho, abertura, anterior))
            .map_err(|erro| erro.to_string())?;
        // A de antes sai aqui, e com o contexto corrente: as texturas dela moram nele.
        no_contexto(com_placa, || self.partida = Some(partida));
        Ok(())
    }

    /// Solta a partida com o contexto dela corrente: soltar texturas com o contexto do Qt Quick
    /// corrente apagaria as dele.
    pub fn fecha(&mut self) {
        if self.partida.is_some() {
            no_contexto(self.gl.is_some(), || self.partida = None);
        }
        self.teclas.clear();
    }

    /// O painel de depuração, quando ligado: os textos e a linha do tempo, como `(velocidade,
    /// quadros)` da amostra mais antiga para a mais nova. Ver [`zeebx::ui::depuracao`].
    pub fn depuracao(&self) -> Option<(Vec<String>, Vec<(u32, u32)>)> {
        let debug = &self.settings.debug;
        let sessao = self.partida.as_ref()?.sessao();
        if !debug.overlay {
            return None;
        }
        let textos = depuracao::Textos::novos(
            &self.catalogo,
            debug,
            sessao.sample(),
            sessao.memory(),
            sessao.clock_ms(),
        );
        let textos = [textos.velocidade, textos.relogio, textos.memoria]
            .into_iter()
            .flatten()
            .collect();
        let historia = match debug.timeline {
            true => sessao.history().map(|a| (a.speed, a.fps)).collect(),
            false => Vec::new(),
        };
        Some((textos, historia))
    }

    pub fn alterna_pausa(&mut self) {
        if let Some(partida) = self.partida.as_mut() {
            partida.pausada = !partida.pausada;
        }
    }

    pub fn pausada(&self) -> bool {
        self.partida.as_ref().is_some_and(|partida| partida.pausada)
    }

    /// Uma tecla da janela do jogo. Cada evento vai à partida na hora: ver
    /// [`Partida::teclado_mudou`].
    pub fn tecla(&mut self, tecla: Key, apertada: bool) {
        match apertada {
            true => self.teclas.insert(tecla),
            false => self.teclas.remove(&tecla),
        };
        if let Some(partida) = self.partida.as_mut() {
            partida.teclado_mudou(self.teclas.iter().filter_map(|tecla| input::avk_de(*tecla)));
        }
    }

    /// A janela do jogo perdeu o foco: nenhuma tecla continua apertada.
    pub fn solta_teclas(&mut self) {
        self.teclas.clear();
    }

    /// Uma volta: entrada, emulação, e o que o jogo pediu depois — lançar outro, voltar à Z-Wheel,
    /// fechar.
    ///
    /// `area` é o tamanho da tela do jogo na janela, para a proporção "da janela".
    pub fn passo(&mut self, area: [f32; 2]) -> Volta {
        let Some(partida) = self.partida.as_mut() else {
            return Volta::default();
        };
        let pads = self.entrada.pads(&self.settings.controls, &self.teclas);
        let movimentos = self.entrada.movimentos(&self.settings.controls);
        let teclado = self.teclas.iter().filter_map(|tecla| input::avk_de(*tecla));
        let limite = self.settings.graphics.speed_limit;
        // A proporção "da janela" acompanha o tamanho dela; o destino só é refeito quando as
        // colunas a mais mudam. É GL, então vai com o contexto corrente.
        let da_janela = (self.settings.graphics.proporcao == Proporcao::Janela)
            .then(|| area[0] / area[1].max(1.0));
        let contexto = self.gl.clone();
        no_contexto(contexto.is_some(), || {
            if da_janela.is_some() {
                partida.sessao_mut().define_proporcao(da_janela);
            }
            partida.avanca(&pads, movimentos, teclado, limite);
            // **O que este contexto desenhou só fica garantido para o do Qt Quick depois de um
            // `glFinish`.** A textura do quadro é escrita aqui e lida lá, e a especificação do
            // OpenGL só promete a mudança visível a outro contexto quando o primeiro terminou o
            // trabalho. O `eglSwapBuffers` do jogo já lê o quadro de volta para a CPU, o que
            // espera a placa do mesmo jeito; o custo que sobra para este não foi medido.
            if let Some(contexto) = &contexto {
                use glow::HasContext;
                unsafe { contexto.finish() };
            }
        });

        // O pedido de lançar vem antes da saída: ver [`Partida::pedido_de_lancamento`].
        let lancado = partida.pedido_de_lancamento().and_then(|classe| {
            self.jogos
                .iter()
                .find(|jogo| jogo.clsid == Some(classe))
                .map(|jogo| jogo.path.clone())
        });
        let (proximo, pela_z_wheel) = match (lancado, partida.saida()) {
            (Some(caminho), _) => (Some(caminho), true),
            (None, Saida::Segue) => (None, false),
            (None, Saida::ReabreZWheel) if self.z_wheel.is_some() => (self.z_wheel.clone(), false),
            (None, Saida::ReabreZWheel | Saida::Fecha) => {
                self.fecha();
                return Volta {
                    fechou: true,
                    ..Volta::default()
                };
            }
        };
        if let Some(partida) = &self.partida {
            self.relatorio.grava(partida);
        }
        let mut volta = Volta::default();
        if let Some(caminho) = proximo {
            volta.estado = Some(match self.abre(&caminho) {
                Ok(()) => self.estado(),
                Err(erro) => format!("não abriu: {erro}"),
            });
            if let Some(partida) = self.partida.as_mut() {
                partida.aberta_pela_z_wheel |= pela_z_wheel;
                partida.reinicia_relogio();
            }
        }
        if let Some(motivo) = self.partida.as_ref().and_then(|p| p.sessao().stopped_reason()) {
            volta.parou = self.catalogo.format("play.failed", &[("reason", &motivo)]);
        }
        volta.aviso = self.aviso_de_calibracao();
        volta
    }

    /// O aviso de calibração neste quadro, com os mesmos textos do egui.
    fn aviso_de_calibracao(&mut self) -> Option<AvisoNaTela> {
        let calibracao = self.partida.as_ref()?.sessao().calibracao();
        let controles = &self.settings.controls;
        let porta = controles
            .ligadas()
            .find(|(_, jogador)| jogador.aparelho == Aparelho::Boomerang)
            .map(|(indice, _)| indice);
        let leitura = porta.map(|porta| {
            let movimento = self.entrada.movimento_da_porta(controles, porta);
            let com_sensor = self.entrada.movimento_bruto_da_porta(controles, porta).is_some();
            (movimento, com_sensor)
        });
        let ligado = self.settings.movimento.aviso_de_calibracao;
        let vista = self.calibracao.quadro(calibracao, leitura, ligado, Instant::now())?;
        let porta = porta?;
        let chave = match vista.concluido {
            true => "calibration.toast.done",
            false => "calibration.toast.title",
        };
        let estado = match vista.situacao {
            None => String::new(),
            Some(Situacao::SemSensor) => self
                .entrada
                .sensor_da_porta(controles, porta)
                .descreve(&self.catalogo),
            Some(Situacao::Parado) => self.catalogo.get("calibration.toast.still").to_string(),
            Some(Situacao::Mexendo) => self.catalogo.get("calibration.toast.moving").to_string(),
        };
        Some(AvisoNaTela {
            titulo: self.catalogo.get(chave).to_string(),
            estado,
            parado: vista.situacao == Some(Situacao::Parado),
            giro: vista.giro,
        })
    }
}

/// Roda `f` com o contexto do rasterizador corrente, quando há um.
fn no_contexto<T>(com_placa: bool, f: impl FnOnce() -> T) -> T {
    let corrente = com_placa && gl::gl_torna_corrente();
    let resultado = f();
    if corrente {
        gl::gl_solta();
    }
    resultado
}

/// O contexto fora de tela, pronto para o `glow`, e o nome da placa. `Err` diz por que não deu.
fn contexto_do_qt() -> Result<(Arc<glow::Context>, String), String> {
    if !gl::gl_cria() {
        return Err("o Qt não criou o contexto fora de tela".into());
    }
    if !gl::gl_torna_corrente() {
        return Err("o contexto fora de tela não ficou corrente".into());
    }
    let contexto = unsafe {
        glow::Context::from_loader_function_cstr(|nome| {
            nome.to_str().map_or(0, gl::gl_funcao) as *const std::ffi::c_void
        })
    };
    let placa = {
        use glow::HasContext;
        unsafe { contexto.get_parameter_string(glow::RENDERER) }
    };
    gl::gl_solta();
    Ok((Arc::new(contexto), placa))
}
