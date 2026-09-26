//! O Zeebx no Android: a mesma emulação, outra tela.
//!
//! Este pacote é o frontend inteiro. Ele não sabe emular nada — quem faz isso é a
//! [`zeebx::session::Session`], a mesma que a janela do desktop e a linha de comando giram. O
//! que existe aqui é o que muda de plataforma: de onde vêm as ROMs, como o quadro chega à tela
//! e como o botão do aparelho vira botão do console.
//!
//! **O laço de eventos é nosso, e não do eframe.** A razão está em [`entrada`]: pelo winit os
//! botões do controle chegam como `Key::Unidentified` e os eixos do manche não chegam. Girando
//! a [`android_activity::AndroidApp`] direto, a entrada é a que o Android mandou. O preço é o
//! ciclo de vida da janela e o contexto de GL, que ficam em [`tela`].
//!
//! As telas estão uma por arquivo — [`biblioteca`], [`ajustes`], [`seletor`] e [`jogo`] —, e os
//! textos e as opções saem do próprio núcleo: o mesmo `Settings` do desktop, o mesmo catálogo
//! de idiomas, o mesmo painel de depuração.
//!
//! **O 3D pode rodar na placa.** O contexto de GL da [`tela`] é o mesmo que a sessão recebe, o
//! que liga aqui o `gpu_rasterizer` do desktop — e com ele a resolução interna, o antialias, o
//! anisotrópico e a proporção larga, que no rasterizador de software são funções vazias.

mod ajustes;
mod biblioteca;
mod entrada;
mod estado;
mod jogo;
mod seletor;
mod sistema;
mod tela;
mod tema;
mod widgets;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent, WindowManagerFlags};

use zeebx::input::Pad;
use zeebx::session::Session;
use zeebx::storage::StoragePaths;
use zeebx::ui::i18n::Catalog;
use zeebx::ui::library::{self, Game};
use zeebx::ui::settings::Settings;

use entrada::Entrada;
use tela::{Placa, Tela};

/// O ponto em que o Android entra.
///
/// A `NativeActivity` carrega o `.so`, acha este símbolo e o chama numa linha de execução
/// própria. Daqui para baixo, tudo é nosso.
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("Zeebx"),
    );
    // **O registro do núcleo tem um destino no Android.** Até aqui ele era o único frontend que
    // não drenava o anel: o desktop imprime no `stderr`, o core manda pelo `retro_log`, o headless
    // imprime — e o Android não tinha onde. Aqui o destino é o `log`, que o `android_logger`
    // acabou de instalar e que sai em `logcat` com a etiqueta e a gravidade certas.
    //
    // O nível começa no padrão do núcleo (`Aviso`); a variável `ZEEBX_LOG` vale se existir.
    zeebx::registro::le_do_ambiente();

    // Um pânico do Rust escreve no stderr, e o Android joga o stderr fora: sem isto o
    // aplicativo morre sem deixar uma linha no `logcat`. O gancho manda a mensagem e o lugar
    // para o log do sistema antes de o processo cair.
    std::panic::set_hook(Box::new(|info| {
        let onde = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "lugar desconhecido".to_string());
        log::error!("pânico em {onde}: {info}");
    }));

    log::info!("Zeebx começando");

    let interno = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp"));

    // **O Android não define `HOME`.** Sem ele o `settings::config_dir` do núcleo cai no seu
    // último recurso, o diretório corrente — que num aplicativo Android é `/`. O carregador
    // então tenta extrair os `.zip` em `/cache` e montar a raiz do aparelho em `/aparelho`, e
    // as duas coisas falham com "permission denied" num erro que *parece* ser da ROM.
    //
    // Definir `HOME` para o diretório privado do aplicativo faz todas as regras que já existem
    // apontarem para o lugar certo, sem uma linha de `cfg` no núcleo.
    unsafe { std::env::set_var("HOME", &interno) };

    // Os ajustes moram no diretório interno do aplicativo, que é só dele: o `Settings` do núcleo
    // é o mesmo do desktop, e aceita um caminho explícito justamente para isto.
    let arquivo = interno.join("zeebx.json");

    // A pasta de ROMs é escolhida pelo usuário e guardada nos ajustes. O padrão é `zeebo/roms`
    // na raiz do armazenamento, que é onde uma coleção costuma estar — e lê-la exige o "acesso a
    // todos os arquivos", que o manifesto declara e o aplicativo pede.
    let externo = app.external_data_path().map(|dir| dir.join("roms"));
    if let Some(dir) = &externo {
        let _ = std::fs::create_dir_all(dir);
    }
    let padrao = PathBuf::from("/sdcard/zeebo/roms");

    // A pasta interna do aplicativo serve de atalho no seletor: ela é a única que o Android
    // deixa ler sem permissão nenhuma.
    let minha_pasta = interno.join("roms");
    let _ = std::fs::create_dir_all(&minha_pasta);

    // Tela cheia de verdade: num aparelho de mão a barra de status só rouba altura do jogo — e
    // era ela que cobria o caminho no seletor de pastas.
    app.set_window_flags(WindowManagerFlags::FULLSCREEN, WindowManagerFlags::empty());
    // A de navegação a bandeira não tira: ela é da view, não da janela, e some por JNI.
    sistema::esconde_a_barra_de_navegacao(&app);

    let mut emulador = Emulador::novo(arquivo, padrao, externo, minha_pasta, app.clone());
    gira(app, &mut emulador);
}

/// Manda para o `logcat` o que o núcleo registrou desde a última volta.
///
/// A gravidade é preservada: um aviso do núcleo sai como `WARN` no `logcat`, e não como `INFO`
/// como tudo o mais. É o que faz `adb logcat *:W` mostrar o que interessa sem o resto.
fn despeja_o_registro() {
    use zeebx::registro::Nivel;
    for linha in zeebx::registro::drena() {
        let texto = format!("[{}] {}: {}", linha.nivel.etiqueta(), linha.alvo, linha.texto);
        match linha.nivel {
            Nivel::Depuracao => log::debug!("{texto}"),
            Nivel::Informacao => log::info!("{texto}"),
            Nivel::Aviso => log::warn!("{texto}"),
            Nivel::Erro | Nivel::Fatal => log::error!("{texto}"),
        }
    }
}

/// O laço: colhe eventos, deixa o jogo andar, desenha, repete.
fn gira(app: AndroidApp, emulador: &mut Emulador) {
    let ctx = egui::Context::default();
    // A interface do núcleo foi desenhada para um monitor. Num aparelho de mão, à distância de
    // um braço, o mesmo tamanho em pontos fica ilegível: a densidade do aparelho é o piso, e
    // daí para cima é o que o dedo precisa para acertar um botão.
    let pontos_por_pixel = escala(&app);
    ctx.set_pixels_per_point(pontos_por_pixel);
    tema::aplica(&ctx);

    let mut entrada = Entrada::nova(pontos_por_pixel);
    // A placa nasce com a primeira janela e não morre mais: ver [`tela::Placa`]. A tela é a
    // superfície, e essa vai e vem.
    let mut placa: Option<Placa> = None;
    let mut tela: Option<Tela> = None;
    let mut visivel = false;
    let mut sair = false;
    let comeco = Instant::now();

    while !sair {
        // O registro do núcleo sai por aqui, uma vez por volta do laço. Com o anel vazio — o
        // caso comum, no nível padrão — isto é um cadeado e uma leitura.
        despeja_o_registro();
        // Com janela, o laço gira o mais rápido que a troca de buffers deixar; sem ela, não há
        // o que desenhar e esperar é o certo — é o que mantém o aplicativo parado em segundo
        // plano em vez de queimar bateria.
        let espera = match visivel && tela.is_some() {
            true => Some(Duration::ZERO),
            false => None,
        };

        app.poll_events(espera, |evento| match evento {
            PollEvent::Main(MainEvent::InitWindow { .. }) => {
                let limitar = emulador.settings.graphics.speed_limit;
                let feita = match placa.as_mut() {
                    // A segunda janela em diante reaproveita o contexto: é o que mantém vivos
                    // os objetos de GL que a sessão criou.
                    Some(placa) => placa.refaz_a_tela(&app, limitar),
                    None => Placa::nova(&app, limitar).map(|(nova, primeira)| {
                        emulador.gl = Some(nova.gl.clone());
                        placa = Some(nova);
                        primeira
                    }),
                };
                match feita {
                    Ok(nova) => tela = Some(nova),
                    Err(erro) => log::error!("a tela não subiu: {erro}"),
                }
            }
            PollEvent::Main(MainEvent::TerminateWindow { .. }) => tela = None,
            PollEvent::Main(MainEvent::WindowResized { .. })
            | PollEvent::Main(MainEvent::ContentRectChanged { .. })
            | PollEvent::Main(MainEvent::ConfigChanged { .. }) => {
                if let (Some(placa), Some(tela)) = (&placa, &mut tela) {
                    placa.redimensiona(tela, &app);
                }
            }
            PollEvent::Main(MainEvent::Resume { .. }) | PollEvent::Main(MainEvent::GainedFocus) => {
                visivel = true;
                if let (Some(placa), Some(tela)) = (&placa, &tela) {
                    placa.retoma(tela);
                }
            }
            PollEvent::Main(MainEvent::Pause) | PollEvent::Main(MainEvent::LostFocus) => {
                // O Android pode tirar o foco antes de entregar o KeyUp/MotionEvent final do
                // controle. Se mantivermos o estado, a direção fica presa quando a atividade
                // volta. No console, perder o aparelho equivale a soltar tudo.
                emulador.pad = Pad::default();
                visivel = false
            }
            PollEvent::Main(MainEvent::Destroy) => sair = true,
            _ => {}
        });

        if sair {
            break;
        }

        entrada.comeca_quadro();
        if let Ok(mut fila) = app.input_events_iter() {
            // O iterador entrega um evento por chamada e quer a resposta na hora: `Handled`
            // diz ao Android que o evento morre aqui — é isso que impede o "voltar" de fechar
            // a atividade por baixo de nós.
            while fila.next(|evento| {
                entrada.recebe(evento, &mut emulador.pad);
                InputStatus::Handled
            }) {}
        }
        // O "voltar" do Android vale em qualquer tela. O botão 2 do controle vale em todas menos
        // no jogo, onde ele é do jogador -- ali quem fecha é o "voltar" do aparelho.
        if entrada.voltar || (entrada.voltar_da_interface && emulador.onde != Onde::Jogo) {
            emulador.voltar();
        }

        let (Some(placa), Some(tela)) = (placa.as_mut(), tela.as_ref()) else {
            continue;
        };
        if !visivel {
            continue;
        }

        let [largura, altura] = tela.tamanho;
        let cru = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(
                    largura as f32 / pontos_por_pixel,
                    altura as f32 / pontos_por_pixel,
                ),
            )),
            time: Some(comeco.elapsed().as_secs_f64()),
            // O limite da GPU do aparelho, e não o palpite do egui: é ele que decide até onde o
            // atlas de fontes pode crescer.
            max_texture_side: Some(placa.pincel.max_texture_side()),
            events: std::mem::take(&mut entrada.eventos),
            ..Default::default()
        };

        let saida = ctx.run(cru, |ctx| emulador.desenha(ctx));
        let primitivas = ctx.tessellate(saida.shapes, saida.pixels_per_point);
        placa.pinta(
            tela,
            &primitivas,
            &saida.textures_delta,
            saida.pixels_per_point,
            emulador.settings.graphics.speed_limit,
        );
    }

    log::info!("Zeebx encerrando");
}

/// Qual tela está no ar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Onde {
    /// A grade de jogos.
    Biblioteca,
    /// As configurações.
    Ajustes,
    /// O navegador de pastas, na pasta dada.
    Seletor(PathBuf),
    /// Um jogo rodando.
    Jogo,
}

/// O aplicativo inteiro: o que está na tela e o que sustenta cada tela.
pub struct Emulador {
    /// Onde os ajustes são lidos e gravados. É um arquivo do mesmo formato do desktop.
    arquivo: PathBuf,
    settings: Settings,
    /// Os textos da interface, do mesmo catálogo que o desktop usa. Os idiomas de fábrica
    /// vêm embutidos no binário, então funcionam sem nenhum arquivo no aparelho.
    catalogo: Catalog,
    onde: Onde,
    aba: ajustes::Aba,
    /// Qual dica de configuração está aberta. Os textos de ajuda do Zeebx são longos, e todos
    /// abertos ao mesmo tempo viram uma parede: só um por vez.
    dica: widgets::Aberta,
    /// A pasta privada do aplicativo. Sempre legível, sem permissão nenhuma — é o atalho que
    /// funciona mesmo quando o "acesso a todos os arquivos" não foi concedido.
    minha_pasta: PathBuf,
    /// Raiz privada do aplicativo. Os save states ficam aqui, separados das ROMs escolhidas pelo
    /// usuário: atualizar/trocar a coleção não apaga o ponto em que cada jogo estava.
    estados_raiz: PathBuf,
    /// Cache, sistema compartilhado e saves persistentes fornecidos explicitamente ao núcleo.
    storage: StoragePaths,
    /// A ponte para a atividade: é por ela que se pergunta e se pede a permissão.
    app: AndroidApp,
    /// O contexto de GL da tela, quando ela já subiu. É o que a sessão usa para preencher o 3D
    /// na placa; sem ele, o 3D é da CPU.
    gl: Option<std::sync::Arc<glow::Context>>,
    /// Quem põe o quadro da placa na tela. Mora atrás de um `Arc<Mutex<_>>` porque quem o usa é
    /// um fecho que o egui guarda e chama no meio da pintura — ele não pode emprestar do
    /// `Emulador`. Nasce na primeira pintura, que é a primeira vez que há contexto corrente.
    pintor: std::sync::Arc<std::sync::Mutex<Option<zeebx::ui::gpu::Pintor>>>,
    /// Os jogos da pasta escolhida, com título e capa, lidos pelo mesmo `library::scan` do
    /// desktop.
    jogos: Vec<Game>,
    /// Resultado de uma varredura em segundo plano. Enquanto existe, a lista antiga continua
    /// utilizavel e a interface nao trava lendo ZIPs/MIFs.
    varredura: Option<
        std::sync::mpsc::Receiver<(PathBuf, Vec<Game>, Vec<(u32, String)>)>,
    >,
    /// Títulos normalizados uma vez por varredura, para a busca não refazer lowercase por frame.
    titulos_busca: Vec<String>,
    /// Índices que a busca atual deixou passar.
    filtrados: Vec<usize>,
    /// Applets conhecidos pela biblioteca, preparados fora da linha de UI para o boot da Z-Wheel.
    instalados: Vec<(u32, String)>,
    /// O filtro da busca.
    busca: String,
    /// Qual cartão o direcional está apontando, entre os que a busca deixou passar.
    selecionado: usize,
    /// O escolhido mudou e precisa entrar na área visível no próximo quadro.
    rolar: bool,
    /// Quantas colunas a grade desenhou da última vez. É o passo do direcional para cima e
    /// para baixo — e só a pintura sabe esse número.
    colunas: usize,
    /// As capas já subidas para a placa, por caminho do jogo. Sem isto cada quadro refaria a
    /// textura de todos os cartões à vista.
    capas: HashMap<PathBuf, egui::TextureHandle>,
    sessao: Option<Session>,
    /// O quadro do console como textura do egui, trocada a cada quadro novo.
    textura: Option<egui::TextureHandle>,
    /// O que está subido na textura: a série e as escritas do quadro, mais o filtro. A tela
    /// repinta mais vezes que o jogo desenha, e sem esta chave cada repintura refaria a
    /// conversão inteira.
    quadro: Option<(u64, u64, bool)>,
    /// Quem recebe o foco quando a seta sobe da primeira fila da grade.
    ///
    /// É um widget do egui, e por isso a barra de cima só é alcançável por foco do egui -- a
    /// grade tem cursor próprio e os dois não podem andar juntos. A `Id` sai da pintura da
    /// barra, porque é ela quem cria o botão.
    foco_da_barra: Option<egui::Id>,
    /// O quadro 2D em RGB565 para o caminho direto pela placa.
    ///
    /// O Android pode repintar a janela várias vezes sem o jogo tocar no framebuffer. Guardar a
    /// conversão evita recriar/copiar cerca de 600 KiB por repaint numa tela 640x480.
    ///
    /// **São dois caches, e nenhum sobra.** Este guarda os bytes e poupa a conversão na CPU; o
    /// `ultimo_quadro` do `Pintor` guarda o que já subiu e poupa a ida ao barramento. A mesma
    /// chave `(série, escritas)` governa os dois.
    quadro_565: Option<(u64, u64, std::sync::Arc<[u8]>)>,
    /// Os botões apertados agora, alimentados pela fila nativa.
    pad: Pad,
    /// O "voltar" foi apertado com um jogo aberto: a pergunta está na tela.
    confirmando: bool,
    /// O jogo está parado por escolha, e não por falha.
    pausado: bool,
    /// Identidade BLAKE3 do arquivo que abriu a sessão, a mesma regra do armazenamento do núcleo.
    estado_id: Option<String>,
    /// Os cinco slots persistentes do jogo atual.
    estados: [Option<estado::Slot>; estado::SLOTS],
    /// Miniaturas já convertidas/subidas ao egui, por slot.
    miniaturas_estado: HashMap<usize, egui::TextureHandle>,
    /// Slot ocupado que pediu uma segunda confirmação antes de ser substituído.
    estado_sobrescrever: Option<usize>,
    /// Resultado da última operação, mostrado no menu sem fechar a janela.
    estado_mensagem: Option<String>,
    /// Por que o último jogo não abriu, quando não abriu.
    erro: Option<String>,
    ultimo: Instant,
}

impl Emulador {
    fn novo(
        arquivo: PathBuf,
        padrao: PathBuf,
        externo: Option<PathBuf>,
        minha_pasta: PathBuf,
        app: AndroidApp,
    ) -> Self {
        let estados_raiz = minha_pasta
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| minha_pasta.clone());
        // Mantem cache e aparelho nos caminhos historicos de Android
        // (files/.config/zeebx), mas liga o overlay persistente em saves/.
        let storage_base = estados_raiz.join(".config");
        let storage = StoragePaths::for_frontend(&storage_base, Some(&storage_base));
        if let Err(erro) = storage.create_dirs() {
            log::error!("nao criou o armazenamento persistente: {erro}");
        }
        let mut settings = Settings::load_from(&arquivo);
        // Sem pasta escolhida ainda: o padrão, se ele existir, senão o diretório do aplicativo,
        // que sempre existe e nunca pede permissão.
        if settings.roms_dir.is_none() {
            settings.roms_dir = match padrao.is_dir() {
                true => Some(padrao),
                false => externo,
            };
        }

        // O idioma guardado, ou o do aparelho. O catálogo só conhece os embutidos mais o que
        // houver em `lang/` dentro da pasta do aplicativo.
        let mut catalogo = Catalog::new(&[minha_pasta.with_file_name("lang")]);
        match &settings.language {
            Some(codigo) => {
                catalogo.select(codigo);
            }
            None => {
                catalogo.select_best(&idioma_do_aparelho(&app));
            }
        }

        let mut emulador = Self {
            onde: Onde::Biblioteca,
            aba: ajustes::Aba::Geral,
            dica: None,
            minha_pasta,
            estados_raiz,
            storage,
            app,
            gl: None,
            pintor: Default::default(),
            arquivo,
            settings,
            catalogo,
            jogos: Vec::new(),
            varredura: None,
            titulos_busca: Vec::new(),
            filtrados: Vec::new(),
            instalados: Vec::new(),
            busca: String::new(),
            selecionado: 0,
            rolar: false,
            colunas: 4,
            capas: HashMap::new(),
            sessao: None,
            textura: None,
            quadro: None,
            foco_da_barra: None,
            quadro_565: None,
            pad: Pad::default(),
            confirmando: false,
            pausado: false,
            estado_id: None,
            estados: std::array::from_fn(|_| None),
            miniaturas_estado: HashMap::new(),
            estado_sobrescrever: None,
            estado_mensagem: None,
            erro: None,
            ultimo: Instant::now(),
        };
        emulador.recarrega();
        emulador
    }

    /// Um texto da interface.
    fn tr<'a>(&'a self, chave: &'a str) -> &'a str {
        self.catalogo.get(chave)
    }

    /// Relê a pasta escolhida sem bloquear a linha que desenha a interface.
    fn recarrega(&mut self) {
        let pasta = self.roms();
        if pasta.as_os_str().is_empty() {
            self.varredura = None;
            self.aplica_jogos(pasta, Vec::new(), Vec::new());
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let pasta_da_thread = pasta.clone();
        match std::thread::Builder::new()
            .name("zeebx-library".to_string())
            .spawn(move || {
                let jogos = library::scan(&pasta_da_thread);
                let instalados = jogos
                    .iter()
                    .filter_map(|jogo| {
                        Some((jogo.clsid?, library::id_do_modulo(&jogo.path)?))
                    })
                    .collect();
                let _ = tx.send((pasta_da_thread, jogos, instalados));
            })
        {
            Ok(_) => self.varredura = Some(rx),
            Err(erro) => {
                log::error!("nao iniciou a varredura da biblioteca: {erro}");
                self.varredura = None;
            }
        }
    }

    /// Publica uma varredura concluída, ignorando resultado de uma pasta que já deixou de valer.
    fn conclui_recarrega(&mut self) {
        let resultado = match self.varredura.as_ref() {
            Some(rx) => match rx.try_recv() {
                Ok(resultado) => Some(Ok(resultado)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(())),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
            },
            None => None,
        };
        match resultado {
            Some(Ok((pasta, jogos, instalados))) => {
                self.varredura = None;
                if pasta == self.roms() {
                    self.aplica_jogos(pasta, jogos, instalados);
                }
            }
            Some(Err(())) => {
                self.varredura = None;
                log::error!("a varredura da biblioteca terminou sem resultado");
            }
            None => {}
        }
    }

    fn aplica_jogos(
        &mut self,
        pasta: PathBuf,
        jogos: Vec<Game>,
        instalados: Vec<(u32, String)>,
    ) {
        self.jogos = jogos;
        self.instalados = instalados;
        self.titulos_busca = self
            .jogos
            .iter()
            .map(|jogo| jogo.title.to_lowercase())
            .collect();
        self.atualiza_filtro();
        self.capas.clear();
        self.selecionado = 0;
        self.rolar = true;
        log::info!("{} jogos em {}", self.jogos.len(), pasta.display());
    }

    fn atualiza_filtro(&mut self) {
        let busca = self.busca.trim().to_lowercase();
        self.filtrados.clear();
        self.filtrados.extend(
            self.titulos_busca
                .iter()
                .enumerate()
                .filter(|(_, titulo)| busca.is_empty() || titulo.contains(&busca))
                .map(|(indice, _)| indice),
        );
        if self.filtrados.is_empty() {
            self.selecionado = 0;
        } else {
            self.selecionado = self.selecionado.min(self.filtrados.len() - 1);
        }
    }

    /// A pasta escolhida agora. Vazia quando nenhuma foi escolhida ainda.
    fn roms(&self) -> PathBuf {
        self.settings.roms_dir.clone().unwrap_or_default()
    }

    /// Grava as preferências. Toda mudança de ajuste passa por aqui.
    fn salva(&self) {
        if let Err(erro) = self.settings.save_to(&self.arquivo) {
            log::error!("não gravou os ajustes: {erro}");
        }
    }

    /// Abre um jogo, com os ajustes de gráficos e som que estão valendo.
    fn abre(&mut self, caminho: &std::path::Path) {
        self.erro = None;
        let content = match self.storage.content_id(caminho) {
            Ok(id) => id,
            Err(erro) => {
                let motivo = erro.to_string();
                log::error!("nao identificou o conteudo: {motivo}");
                self.erro = Some(
                    self.catalogo
                        .format("play.failed", &[("reason", &motivo)]),
                );
                return;
            }
        };
        let estado_id = Some(content.as_str().to_string());
        match self
            .storage
            .migrate_legacy_package_writes(caminho, &content)
        {
            Ok(bytes) if bytes > 0 => {
                log::info!("migrou {bytes} bytes de saves antigos para o overlay");
            }
            Ok(_) => {}
            Err(erro) => {
                // A copia antiga continua no cache e o VFS ainda consegue le-la. Falhar a
                // migracao nao deve impedir o jogo de abrir.
                log::warn!("nao migrou saves antigos: {erro}");
            }
        }
        let graficos = self.settings.graphics.clone();
        // O 3D na placa vale só se houver placa: antes da primeira janela não há contexto, e a
        // sessão aberta sem ele cai no rasterizador de software sozinha.
        let na_placa = graficos.gpu_rasterizer && self.gl.is_some();
        match Session::start_with_storage_installed(
            caminho,
            zeebx::PORTAS_PADRAO,
            None,
            na_placa,
            self.gl.clone().filter(|_| na_placa),
            self.settings.z_wheel,
            &self.storage,
            &self.instalados,
        ) {
            Ok(mut sessao) => {
                log::info!("abriu {}", sessao.title());
                sessao.define_resolucao_interna(graficos.resolucao_interna as usize);
                sessao.define_proporcao(graficos.proporcao.aspecto(16.0 / 9.0));
                sessao.define_melhorias(
                    graficos.antialias as usize,
                    graficos.anisotropico as usize,
                );
                sessao.define_neblina(graficos.neblina);
                // Ligar o som aqui é seguro **porque o jogo ainda não começou**: o `start_with`
                // só prepara, e o `EVT_APP_START` sai na primeira volta do laço.
                let audio = self.settings.audio.clone();
                if let Some(erro) = sessao.set_audio(audio.enabled, audio.volume) {
                    log::error!("sem som: {erro}");
                }
                self.sessao = Some(sessao);
                self.estado_id = estado_id;
                self.estados = self
                    .estado_id
                    .as_deref()
                    .map(|id| estado::lista(&self.estados_raiz, id))
                    .unwrap_or_else(|| std::array::from_fn(|_| None));
                self.miniaturas_estado.clear();
                self.estado_sobrescrever = None;
                self.estado_mensagem = None;
                self.pausado = false;
                self.confirmando = false;
                self.onde = Onde::Jogo;
                self.ultimo = Instant::now();
            }
            Err(erro) => {
                log::error!("não abriu {}: {erro:?}", caminho.display());
                self.erro = Some(
                    self.catalogo
                        .format("play.failed", &[("reason", &erro.to_string())]),
                );
            }
        }
    }

    /// Fecha o jogo e volta para a biblioteca.
    fn fecha(&mut self) {
        self.sessao = None;
        self.textura = None;
        self.quadro = None;
        self.quadro_565 = None;
        self.estado_id = None;
        self.estados = std::array::from_fn(|_| None);
        self.miniaturas_estado.clear();
        self.estado_sobrescrever = None;
        self.estado_mensagem = None;
        self.confirmando = false;
        self.pausado = false;
        self.pad = Pad::default();
        self.onde = Onde::Biblioteca;
    }

    /// O botão "voltar" do Android.
    ///
    /// Ele nunca fecha o aplicativo por conta própria: com um jogo aberto ele pergunta; nas
    /// outras telas ele sobe um nível. Na biblioteca, sair é jogo do sistema.
    fn voltar(&mut self) {
        match &self.onde {
            Onde::Jogo if self.confirmando => self.confirmando = false,
            Onde::Jogo => self.confirmando = true,
            // No seletor, subir uma pasta é o que o "voltar" quer dizer; na primeira, sair dele.
            Onde::Seletor(atual) => {
                self.onde = match atual.parent() {
                    Some(acima) if atual != &self.minha_pasta => Onde::Seletor(acima.to_path_buf()),
                    _ => Onde::Ajustes,
                }
            }
            Onde::Ajustes => self.onde = Onde::Biblioteca,
            Onde::Biblioteca => {}
        }
    }

    /// Qual das telas está no ar.
    fn desenha(&mut self, ctx: &egui::Context) {
        self.conclui_recarrega();
        match self.onde.clone() {
            Onde::Jogo => self.jogo(ctx),
            Onde::Ajustes => self.ajustes(ctx),
            Onde::Seletor(atual) => self.seletor(ctx, &atual),
            Onde::Biblioteca => self.biblioteca(ctx),
        }
    }
}

/// Quantos pixels vale um ponto do egui.
///
/// A densidade do aparelho sozinha não serve. Um aparelho de mão como o RG505 se declara
/// `xhdpi` — 320 dpi, dois pixels por ponto —, e a tela dele tem 960 pixels de largura: a
/// interface inteira caberia em 480 pontos, que é menos do que cabe numa grade de jogos. A
/// densidade existe para um celular segurado a trinta centímetros; um portátil é segurado com
/// as duas mãos, mais longe, e a tela dele é mais larga do que alta.
///
/// Então a densidade entra como **teto**, e o piso é caber uma tela de trabalho: nunca menos
/// de [`PONTOS_MINIMOS`] de largura útil.
fn escala(app: &AndroidApp) -> f32 {
    let densidade = app
        .config()
        .density()
        .map(|dpi| dpi as f32 / 160.0)
        .unwrap_or(2.0);
    let largura = app
        .native_window()
        .map(|janela| janela.width() as f32)
        .unwrap_or(PONTOS_MINIMOS);
    densidade.min(largura / PONTOS_MINIMOS).clamp(1.0, 3.0)
}

/// Quantos pontos do egui a tela precisa ter de largura para a interface caber.
///
/// Seiscentos é o que segura uma grade de quatro colunas com o título legível embaixo, e ainda
/// deixa a barra de cima com nome, contagem, busca e dois botões na mesma linha.
const PONTOS_MINIMOS: f32 = 600.0;

/// O idioma do aparelho, como `pt-BR`. Vazio quando o Android não diz.
fn idioma_do_aparelho(app: &AndroidApp) -> String {
    let config = app.config();
    match (config.language(), config.country()) {
        (Some(lingua), Some(pais)) => format!("{lingua}-{pais}"),
        (Some(lingua), None) => lingua.to_string(),
        _ => String::new(),
    }
}

/// Uma imagem do núcleo virando textura do egui.
fn textura_de(ctx: &egui::Context, nome: &str, imagem: &zeebx::video::icon::Image) -> egui::TextureHandle {
    let cor = egui::ColorImage::from_rgba_unmultiplied(
        [imagem.width, imagem.height],
        &imagem.rgba,
    );
    // As capas são ícones de `.mif` ampliados: interpolar é o que evita o bloco gigante.
    ctx.load_texture(nome, cor, egui::TextureOptions::LINEAR)
}
