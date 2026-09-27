//! Um jogo em execução, do arquivo até os quadros na tela.
//!
//! Existe para separar o ciclo de vida do BREW — carregar o módulo, criar o applet, entregar o
//! `EVT_APP_START`, girar o laço de eventos — de quem o observa. A linha de comando roda esse
//! laço até um limite e imprime o resultado; a interface o toca um pedaço por quadro desenhado.
//! O que muda é quem chama [`Session::step`], não o que ele faz.

use std::path::Path;
use std::time::{Duration, Instant};

/// No desktop e no core nativo a sessão recompila os blocos. No `wasm32` o JIT não tem
/// arquitetura de destino — o bloco emitido não roda no navegador — e o interpretador ocupa
/// o mesmo lugar.
#[cfg(not(target_arch = "wasm32"))]
use crate::cpu::dynarmic::DynarmicCpu as CpuDaSessao;
#[cfg(target_arch = "wasm32")]
use crate::cpu::interpretador::Interpretador as CpuDaSessao;
use crate::input::Pad;
use crate::library;
use crate::loader;
use crate::loader::archive;
use crate::loader::modfile::ModImage;
use crate::machine::{AppletResult, Machine, Outcome};
use crate::storage::StoragePaths;
use crate::video::display::Framebuffer;

/// Maior fatia de tempo real que os frontends podem pedir em uma volta.
///
/// Compartilhada para que desktop, headless e Android não reintroduzam um teto de 16 ms e
/// reduzam jogos rápidos como Crash Nitro Kart a uma fração da velocidade.
pub const FATIA_MAXIMA: Duration = Duration::from_millis(100);

/// Teto de instruções por fatia entre duas chamadas de API — evita que um laço infinito no
/// guest trave o emulador.
const INSTRUCTION_BUDGET: u64 = 500_000_000;

/// Sobre quanto tempo real medir a velocidade mostrada na interface.
///
/// Meio segundo é curto o bastante para a leitura acompanhar a troca de tela e longo o bastante
/// para não tremer a cada quadro.
const SPEED_WINDOW_MS: u64 = 500;

/// Quantas amostras o gráfico guarda. A meio segundo cada, é um minuto de história.
const HISTORY: usize = 120;

/// Quantos quadros do jogo uma volta da janela pode rodar para alcançar o relógio do mundo.
/// Ver [`Session::step`].
const QUADROS_POR_VOLTA: u32 = 4;

/// O atraso a partir do qual o jogo recupera quadros: um quadro de 60 Hz.
const ATRASO_PARA_RECUPERAR_MS: u64 = 17;

/// Milissegundos de um quadro a 60 Hz: o passo que um frontend Libretro avança por chamada.
const FRAME_MS: u64 = 16;

/// Teto de voltas internas por quadro virtual, contra um guest que não avança nem apresenta.
const MAX_STEPS_PER_FRAME: u32 = 200_000;

/// O maior atraso que o jogo recupera. Além dele o atraso é perdoado, e não corrido atrás: uma
/// pausa, um carregamento ou a janela arrastada param o relógio virtual enquanto o real anda, e
/// recuperar tudo depois faria o jogo disparar.
const ATRASO_MAXIMO_MS: u64 = 250;

/// Por que um jogo não conseguiu começar.
#[derive(Debug)]
pub enum StartError {
    /// O arquivo não pôde ser lido.
    Unreadable(std::io::Error),
    /// Não é um `.mod` que saibamos ler.
    NotAModule(String),
    /// O módulo não coube na memória do guest.
    NotLoadable(String),
    /// O emulador parou antes de o jogo começar.
    Stopped(Outcome),
    /// Não há `.mif` ao lado do módulo, então não sabemos qual applet criar.
    NoApplet,
    /// `CreateInstance` recusou, com o erro do BREW.
    Refused(u32),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(err) => write!(f, "não deu para ler o arquivo: {err}"),
            Self::NotAModule(err) => write!(f, "não é um módulo válido: {err}"),
            Self::NotLoadable(err) => write!(f, "não deu para carregar o módulo: {err}"),
            Self::Stopped(outcome) => write!(f, "parou antes de começar ({outcome:?})"),
            Self::NoApplet => write!(f, "nenhum .mif ao lado do módulo diz qual applet criar"),
            Self::Refused(code) => write!(f, "CreateInstance recusou com o erro {code}"),
        }
    }
}

/// O que uma fatia de execução produziu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// O jogo apresentou pelo menos um quadro novo.
    Presented,
    /// A fatia acabou sem quadro novo — normal, o jogo continua.
    Running,
    /// O jogo está adiantado em relação ao relógio do mundo e foi segurado.
    Ahead,
    /// O jogo terminou ou quebrou. O motivo está em [`Session::stopped`].
    Stopped,
}

pub struct Session {
    /// O mesmo agendador BREW usado pela bancada e pela linha de comando, com o núcleo que
    /// recompila os blocos ARM do módulo. O Kingdom Hearts desenha a intro no seu próprio
    /// rasterizador ARM; por isso a sessão usa o JIT diretamente. No `wasm32` esse lugar é o
    /// interpretador.
    machine: Machine<CpuDaSessao>,
    /// O applet criado e ainda **não** iniciado, com o ClassID dele.
    ///
    /// O `EVT_APP_START` é despachado na primeira volta do laço, não aqui. Rodá-lo dentro do
    /// `start` fazia o jogo começar antes de existir janela e antes de haver saída de som: a
    /// Z-Wheel toca o `sounds_loading.wav` na partida, e ele saía com a tela vazia — ou, depois
    /// que o som passou a ser ligado só com a janela pronta, não saía de jeito nenhum, porque o
    /// jogo já tinha tocado.
    partida: Option<(u32, u32)>,
    /// A saída de som do host. Um frontend Libretro não tem esta peça: ele recebe o mixer.
    #[cfg(feature = "audio")]
    audio: Option<crate::audio::Output>,
    title: String,
    /// O ClassID do applet desta sessão.
    classe: u32,
    /// A tela intermediária à mostra, quando há uma. Ver [`Session::mostra_quadro_intermediario`].
    intermediario: Option<Framebuffer>,
    /// Instante e leitura do relógio virtual quando a execução começou, que é o par com que se
    /// mede se o jogo está adiantado.
    started: Instant,
    clock_base: u64,
    stopped: Option<Outcome>,
    /// Começo da janela de medição: o instante real, o relógio virtual, as instruções e os
    /// quadros de então. Tudo que o painel de depuração mostra sai da diferença entre duas
    /// dessas leituras.
    window: Marca,
    /// A última amostra fechada.
    sample: Sample,
    /// As amostras recentes, para o gráfico. A mais nova no fim.
    history: std::collections::VecDeque<Sample>,
}

/// Uma leitura dos contadores num instante.
#[derive(Debug, Clone, Copy)]
struct Marca {
    real: Instant,
    clock_ms: u64,
    instructions: u64,
    frames: u32,
}

/// O que aconteceu numa janela de medição.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sample {
    /// Fração da velocidade do console, em porcentagem.
    pub speed: u32,
    /// Quadros apresentados por segundo de tempo real.
    pub fps: u32,
    /// Instruções do guest executadas por segundo de tempo real.
    ///
    /// Não é em milhões: numa tela de espera o jogo cede a vez e quase não executa, e
    /// arredondar para milhões mostraria zero justamente quando o número interessa.
    pub ips: u64,
}

impl Session {
    /// Carrega o módulo, cria o applet e entrega o `EVT_APP_START`.
    ///
    /// Um `.zip` é extraído para o cache antes: o jogo grava (o Peteca tem um `.sav`), e
    /// escrever de volta num pacote não é coisa que se queira fazer.
    /// Como [`Session::start`], mas com o aparelho já configurado antes de o jogo começar.
    ///
    /// **A ordem importa.** O `start` roda o `AEEMod_Load`, cria o applet e despacha o
    /// `EVT_APP_START` — tudo antes de devolver. Um jogo que enumera o `IHID` na partida, como a
    /// Z-Wheel, já perguntou o que está ligado antes de qualquer ajuste feito depois: com as
    /// portas aplicadas só na volta, ela via um controle e nenhum teclado, por mais que a
    /// configuração dissesse o contrário.
    pub fn start_with(
        path: &Path,
        portas: [Option<crate::input::bindings::Aparelho>; crate::input::PORTAS],
        serial: Option<&Path>,
        placa: bool,
        contexto: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
    ) -> Result<Self, StartError> {
        Self::start_inner(path, Some(portas), serial, placa, contexto, z_wheel, &[])
    }

    /// Como [`Session::start_with`], mas instala os módulos antes do boot do guest.
    ///
    /// A Z-Wheel enumera os jogos durante `EVT_APP_START`. Instalar depois que a sessão já existe
    /// é tarde demais: a lista dela nasce vazia e o gesto nunca pode escolher um jogo.
    pub fn start_with_installed(
        path: &Path,
        portas: [Option<crate::input::bindings::Aparelho>; crate::input::PORTAS],
        serial: Option<&Path>,
        placa: bool,
        contexto: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        instalados: &[(u32, String)],
    ) -> Result<Self, StartError> {
        Self::start_inner(path, Some(portas), serial, placa, contexto, z_wheel, instalados)
    }

    /// Como [`Session::start_with`], mas recebe a raiz persistente explicitamente.
    ///
    /// É a fronteira que o frontend Libretro usará: cache de pacote e `fs:/` deixam de depender
    /// da configuração da UI desktop.
    #[allow(dead_code)] // consumido por `frontends/libretro`, ainda não criado.
    pub fn start_with_storage(
        path: &Path,
        portas: [Option<crate::input::bindings::Aparelho>; crate::input::PORTAS],
        serial: Option<&Path>,
        placa: bool,
        contexto: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage(
            path,
            Some(portas),
            serial,
            placa,
            contexto,
            z_wheel,
            storage,
            &[],
        )
    }

    /// Como [`Session::start_with_storage`], com os applets instalados antes do boot.
    ///
    /// O core Libretro usa esta entrada para a Z-Wheel: ela enumera a biblioteca no primeiro
    /// `EVT_APP_START`, portanto `set_installed_applets` depois do retorno não basta.
    #[allow(dead_code)]
    pub fn start_with_storage_installed(
        path: &Path,
        portas: [Option<crate::input::bindings::Aparelho>; crate::input::PORTAS],
        serial: Option<&Path>,
        placa: bool,
        contexto: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        instalados: &[(u32, String)],
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage(
            path,
            Some(portas),
            serial,
            placa,
            contexto,
            z_wheel,
            storage,
            instalados,
        )
    }

    /// Inicia o motor sem janela, dispositivo de áudio ou contexto gráfico do host.
    ///
    /// Sem chamador até `frontends/libretro` existir; o aviso de código morto está silenciado
    /// de propósito.
    ///
    /// Esta é a entrada do core Libretro: vídeo, áudio e input são fornecidos por callbacks do
    /// frontend, e o armazenamento já vem delimitado em [`StoragePaths`].
    #[allow(dead_code)] // consumido por `frontends/libretro`, ainda não criado.
    pub fn start_software_with_storage(
        path: &Path,
        portas: [Option<crate::input::bindings::Aparelho>; crate::input::PORTAS],
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage(path, Some(portas), None, false, None, z_wheel, storage, &[])
    }

    /// Variante software de [`Session::start_software_with_storage`] com a biblioteca conhecida.
    #[allow(dead_code)]
    pub fn start_software_with_storage_installed(
        path: &Path,
        portas: [Option<crate::input::bindings::Aparelho>; crate::input::PORTAS],
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        instalados: &[(u32, String)],
    ) -> Result<Self, StartError> {
        Self::start_software_with_storage_installed_policy(
            path,
            portas,
            z_wheel,
            storage,
            instalados,
            crate::audio::MidiBackend::Auto,
        )
    }

    /// Variante software recebendo explicitamente a política MIDI.
    #[allow(dead_code)]
    pub fn start_software_with_storage_installed_policy(
        path: &Path,
        portas: [Option<crate::input::bindings::Aparelho>; crate::input::PORTAS],
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        instalados: &[(u32, String)],
        midi_policy: crate::audio::MidiBackend,
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage_policy(
            path,
            Some(portas),
            None,
            false,
            None,
            z_wheel,
            storage,
            instalados,
            midi_policy,
        )
    }

    /// Variante com aceleração por hardware recebendo explicitamente a política MIDI.
    #[allow(dead_code)]
    pub fn start_with_storage_installed_policy(
        path: &Path,
        portas: [Option<crate::input::bindings::Aparelho>; crate::input::PORTAS],
        serial: Option<&Path>,
        placa: bool,
        contexto: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        instalados: &[(u32, String)],
        midi_policy: crate::audio::MidiBackend,
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage_policy(
            path,
            Some(portas),
            serial,
            placa,
            contexto,
            z_wheel,
            storage,
            instalados,
            midi_policy,
        )
    }

    /// A serial entra **antes de o módulo ser criado**, e não depois de a sessão existir.
    ///
    /// O construtor do applet roda dentro do `CreateInstance`, aqui dentro: ligar a captura só
    /// depois deixava de fora tudo o que ele faz ao nascer — inclusive o `Zeeboids v 1.1.1402`,
    /// que aparecia no relatório e não na captura. Uma captura com buraco no começo é pior que
    /// nenhuma, porque não se sabe que há buraco.
    fn start_inner(
        path: &Path,
        portas: Option<[Option<crate::input::bindings::Aparelho>; crate::input::PORTAS]>,
        serial: Option<&Path>,
        placa: bool,
        contexto: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        instalados: &[(u32, String)],
    ) -> Result<Self, StartError> {
        let storage = StoragePaths::from_root(crate::config::config_dir());
        Self::start_inner_with_storage(path, portas, serial, placa, contexto, z_wheel, &storage, instalados)
    }

    fn start_inner_with_storage(
        path: &Path,
        portas: Option<[Option<crate::input::bindings::Aparelho>; crate::input::PORTAS]>,
        serial: Option<&Path>,
        placa: bool,
        contexto: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        instalados: &[(u32, String)],
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage_policy(
            path,
            portas,
            serial,
            placa,
            contexto,
            z_wheel,
            storage,
            instalados,
            crate::audio::MidiBackend::Auto,
        )
    }

    fn start_inner_with_storage_policy(
        path: &Path,
        portas: Option<[Option<crate::input::bindings::Aparelho>; crate::input::PORTAS]>,
        serial: Option<&Path>,
        placa: bool,
        contexto: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        instalados: &[(u32, String)],
        midi_policy: crate::audio::MidiBackend,
    ) -> Result<Self, StartError> {
        // Caminho escolhido no frontend, antes de extrair: é ele que identifica o conteúdo.
        let conteudo = path;
        let extracted;
        let path = match path.extension().and_then(|e| e.to_str()) {
            // O `.7z` extrai pelo mesmo caminho: quem separa os formatos é o descompactador.
            Some("zip" | "7z") => {
                extracted =
                    archive::extract_in(path, &storage.cache).map_err(StartError::Unreadable)?;
                extracted.as_path()
            }
            _ => path,
        };
        crate::registro!(
            crate::registro::Nivel::Depuracao,
            "session",
            "conteúdo {} resolvido para {}",
            conteudo.display(),
            path.display()
        );
        let bytes = std::fs::read(path).map_err(StartError::Unreadable)?;
        crate::registro!(
            crate::registro::Nivel::Depuracao,
            "session",
            "{} bytes lidos; analisando o módulo",
            bytes.len()
        );
        let image = ModImage::parse(bytes).map_err(|e| StartError::NotAModule(e.to_string()))?;
        let extensoes = extensoes_de(path);
        let module = loader::load_with(&image, &extensoes)
            .map_err(|e| StartError::NotLoadable(e.to_string()))?;

        // A raiz do sistema de arquivos do jogo é o diretório onde o `.mod` está: é lá que o
        // console guarda os arquivos do título.
        let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
        // O overlay é por conteúdo: o mesmo jogo extraído de novo continua lendo o mesmo save, e
        // um pacote diferente não herda o save do outro. O hash é do arquivo escolhido pelo
        // frontend (`.zip` ou `.mod`), antes de qualquer extração.
        let save_root = match storage.overlay {
            true => {
                let id = storage
                    .content_id(conteudo)
                    .map_err(|e| StartError::Unreadable(e))?;
                Some(storage.save_for(&id))
            }
            false => None,
        };
        let cpu = CpuDaSessao::new().map_err(|e| StartError::NotLoadable(e.to_string()))?;
        let mut machine = Machine::new_with_storage_policy(cpu, module, root, storage, save_root, midi_policy);
        // A lista precisa existir antes de `run` e `create_applet`: a Z-Wheel a enumera no boot.
        machine.set_installed_applets(instalados.iter().cloned());
        // Antes de qualquer desenho: ver [`Machine::usa_placa`].
        let tem_contexto = contexto.is_some();
        machine.usa_placa(placa, contexto);
        machine.configura_z_wheel(z_wheel);
        // A tela com que o console abre a Z-Wheel. Ver [`SPLASH_DA_Z_WHEEL`].
        if library::applet_clsid(path) == Some(Z_WHEEL) {
            if let Some(imagem) = path
                .parent()
                .and_then(|dir| std::fs::read(dir.join(SPLASH_DA_Z_WHEEL)).ok())
            {
                machine.pinta_tela_rgb565(&imagem);
            }
        }
        if let Some(caminho) = serial {
            if let Some(dir) = caminho.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(erro) = machine.liga_serial(caminho) {
                crate::registro!(
                    crate::registro::Nivel::Aviso,
                    "session",
                    "a captura de serial não abriu: {erro}"
                );
            }
        }
        if let Some(portas) = portas {
            machine.set_portas(portas);
        }

        let outcome = machine
            .run(INSTRUCTION_BUDGET)
            .map_err(|e| StartError::NotLoadable(e.to_string()))?;
        if !matches!(outcome, Outcome::Returned { code: 0 }) {
            return Err(StartError::Stopped(outcome));
        }

        let clsid = library::applet_clsid(path).ok_or(StartError::NoApplet)?;
        let created = machine
            .create_applet(clsid, INSTRUCTION_BUDGET)
            .map_err(|e| StartError::NotLoadable(e.to_string()))?;
        let applet = match created {
            AppletResult::Called { code: 0, applet } if applet != 0 => applet,
            AppletResult::Called { code, .. } => return Err(StartError::Refused(code)),
            AppletResult::Stopped(stop) => return Err(StartError::Stopped(stop)),
            AppletResult::NoModule => return Err(StartError::NoApplet),
        };
        let title = library::title_for(path);
        // **Uma linha de INFO por sessão, com o que responde "o que está rodando e como".** É o
        // par que faltava no relatório do core: o título dizia o jogo e nada dizia o rasterizador.
        crate::registro!(
            crate::registro::Nivel::Informacao,
            "session",
            "abriu {} (classe {clsid:#010x}) com o {} e {} applet(s) instalado(s)",
            // O nome do **conteúdo pedido**, e não o `title`: num `.zip` o título da sessão sai
            // da pasta do cache, que carrega tamanho e data e não diz nada a quem lê o log.
            conteudo
                .file_name()
                .map(|nome| nome.to_string_lossy().into_owned())
                .unwrap_or_else(|| conteudo.display().to_string()),
            match placa {
                true => "rasterizador de placa",
                false => "rasterizador de processador",
            },
            instalados.len()
        );
        if placa && !tem_contexto {
            crate::registro!(
                crate::registro::Nivel::Aviso,
                "session",
                "pediram a placa sem contexto de GL: o desenho fica no processador"
            );
        }
        let clock_base = u64::from(machine.clock_ms());
        let window = Marca {
            real: Instant::now(),
            clock_ms: clock_base,
            instructions: machine.instructions(),
            frames: machine.gl_swaps(),
        };
        Ok(Self {
            machine,
            partida: Some((applet, clsid)),
            #[cfg(feature = "audio")]
            audio: None,
            title,
            classe: clsid,
            intermediario: None,
            started: Instant::now(),
            clock_base,
            stopped: None,
            window,
            sample: Sample::default(),
            history: std::collections::VecDeque::new(),
        })
    }

    /// Avança o jogo até apresentar um quadro, gastar `budget` de tempo real, ou parar.
    ///
    /// O teto de tempo real é o que mantém a interface viva: uma volta do laço do jogo pode ser
    /// uma fatia minúscula de instruções, e devolver o controle regularmente é o que permite
    /// redesenhar e atender o teclado enquanto o jogo roda.
    pub fn step(&mut self, budget: Duration, speed_limit: bool) -> Step {
        if self.stopped.is_some() {
            return Step::Stopped;
        }
        self.sample_speed();
        let deadline = Instant::now() + budget;
        let mut before = self.machine.gl_swaps();
        let mut quadros = 0;
        loop {
            if speed_limit && self.ahead_ms() > 0 {
                return Step::Ahead;
            }
            match self.advance_once() {
                Some(step) => return step,
                None if self.machine.gl_swaps() != before => {
                    // **Um quadro por volta da janela era o teto do jogo.** Com a janela abaixo de
                    // 60 quadros por segundo — a emulação e a interface somadas passando do
                    // retraço —, o jogo andava na mesma proporção, liso e lento: o Quake rodava
                    // em câmera lenta sem engasgar. Atrasado, ele roda mais quadros nesta volta, e
                    // só o último vai para a tela.
                    quadros += 1;
                    let recupera = speed_limit
                        && quadros < QUADROS_POR_VOLTA
                        && Instant::now() < deadline
                        && self.atraso_ms() > ATRASO_PARA_RECUPERAR_MS;
                    if !recupera {
                        return Step::Presented;
                    }
                    before = self.machine.gl_swaps();
                }
                None if Instant::now() >= deadline => return Step::Running,
                None => {}
            }
        }
    }

    /// Avança **um quadro virtual**, sem consultar o relógio de parede.
    ///
    /// É a unidade que um frontend repete: o tempo do jogo anda pelo relógio virtual, e nenhuma
    /// decisão depende de quão rápido o host executa. [`Session::step`] continua sendo o caminho
    /// da janela, que precisa devolver o controle ao sistema operacional de tempos em tempos.
    pub fn run_frame(&mut self, limita_velocidade: bool) -> Step {
        if self.stopped.is_some() {
            return Step::Stopped;
        }
        if limita_velocidade {
            // O desktop faz o mesmo freio devolvendo `Step::Ahead` para a janela. O Libretro não
            // tem uma volta assíncrona que possa receber "volte depois": `retro_run` tem de
            // devolver um quadro nesta chamada. Dormir **antes** de avançar é a tradução correta
            // do mesmo contrato: o áudio do quadro anterior toca enquanto espera, e o próximo
            // quadro só nasce quando o relógio real alcançou o virtual.
            //
            // Cinquenta ms é teto defensivo contra um salto anômalo do relógio virtual durante
            // carregamento. No caso normal o adiantamento é um período (16–17 ms); sem o teto
            // uma ROM que se adiantasse segundos congelaria o frontend numa chamada só.
            let espera = self.ahead_ms().min(50);
            if espera > 0 {
                std::thread::sleep(Duration::from_millis(espera));
            }
        }
        let inicio = u64::from(self.machine.clock_ms());
        for _ in 0..MAX_STEPS_PER_FRAME {
            match self.advance_once() {
                Some(step) => return step,
                None => {
                    if u64::from(self.machine.clock_ms()).saturating_sub(inicio) >= FRAME_MS {
                        return Step::Presented;
                    }
                }
            }
        }
        // Um guest que não avança o relógio nem apresenta nada: devolver o controle é melhor que
        // travar o frontend para sempre.
        Step::Running
    }

    /// Quantos milissegundos o jogo está atrasado em relação ao relógio do mundo.
    ///
    /// Um atraso maior que [`ATRASO_MAXIMO_MS`] é perdoado aqui mesmo: o começo da medição anda
    /// para a frente até sobrar só o máximo.
    fn atraso_ms(&mut self) -> u64 {
        let jogo = u64::from(self.machine.clock_ms()).saturating_sub(self.clock_base);
        let real = self.started.elapsed().as_millis() as u64;
        let atraso = real.saturating_sub(jogo);
        if atraso > ATRASO_MAXIMO_MS {
            self.started += Duration::from_millis(atraso - ATRASO_MAXIMO_MS);
            return ATRASO_MAXIMO_MS;
        }
        atraso
    }

    /// A partida do jogo: o `EVT_APP_START` entregue ao applet, **uma vez**.
    ///
    /// `Some(Step::Stopped)` quando ela quebrou; `None` quando foi bem ou já tinha acontecido.
    ///
    /// Fica separada da volta do laço porque as duas apontam para lugares diferentes: quebrar no
    /// evento inicial é problema do que o applet faz ao nascer, e quebrar na primeira volta é
    /// problema do laço de quadros dele. A varredura de ROMs classifica por essa diferença, e
    /// misturá-las fazia um jogo que morre no laço aparecer como morto na partida.
    fn parte(&mut self) -> Option<Step> {
        let (applet, clsid) = self.partida.take()?;
        let started = match self.machine.start_applet(applet, clsid, INSTRUCTION_BUDGET) {
            Ok(started) => started,
            Err(_) => {
                self.stopped = Some(Outcome::Exception { pc: 0 });
                return Some(Step::Stopped);
            }
        };
        if !matches!(started, Outcome::Returned { .. } | Outcome::Budget) {
            self.stopped = Some(started);
            return Some(Step::Stopped);
        }
        None
    }

    /// Uma volta do laço de eventos. `Some` quando há desfecho, `None` para continuar.
    fn advance_once(&mut self) -> Option<Step> {
        // Telas intermediárias da volta anterior ainda não mostradas: a janela as mostra antes
        // de o jogo andar. Ver [`Session::mostra_quadro_intermediario`].
        if self.machine.tem_quadros_do_update() {
            return Some(Step::Presented);
        }
        self.machine.comeca_volta();
        let passo = self.advance_once_inner();
        self.machine.fecha_volta();
        passo
    }

    fn advance_once_inner(&mut self) -> Option<Step> {
        // A partida do jogo é a primeira coisa desta volta, e não do `start`: assim ela
        // acontece com a janela já na tela e o som já ligado.
        if let Some(step) = self.parte() {
            return Some(step);
        }

        let outcomes = match self.machine.advance(INSTRUCTION_BUDGET) {
            Ok(outcomes) => outcomes,
            Err(err) => {
                self.stopped = Some(Outcome::Exception { pc: 0 });
                let _ = err;
                return Some(Step::Stopped);
            }
        };
        if self.machine.deliver_signals(INSTRUCTION_BUDGET).is_err()
            || self.machine.deliver_callbacks(INSTRUCTION_BUDGET).is_err()
        {
            self.stopped = Some(Outcome::Exception { pc: 0 });
            return Some(Step::Stopped);
        }
        // O teto de instruções de um trecho **não** é fim de jogo: é o pedido de vez que o
        // laço de quadros faz para poder entregar a entrada e conferir o relógio. Tratá-lo como
        // desfecho ruim parava a Z-Wheel na primeira volta — ela repete a abertura enquanto
        // ninguém toca, e cada repetição gasta orçamento.
        if let Some(bad) = outcomes
            .iter()
            .find(|outcome| !matches!(outcome, Outcome::Returned { .. } | Outcome::Budget))
        {
            self.stopped = Some(bad.clone());
            return Some(Step::Stopped);
        }
        // O applet pediu para fechar: recebe o `EVT_APP_STOP` e a sessão termina como uma saída
        // normal, que é o que a janela lê para voltar à Z-Wheel.
        if self.machine.pediu_para_fechar() {
            let _ = self.machine.encerra_applet();
            self.stopped = Some(Outcome::Returned { code: 0 });
            return Some(Step::Stopped);
        }
        // Sem timer armado nem trabalho pendente, nada mais vai acontecer.
        if outcomes.is_empty() && self.machine.is_idle() {
            self.stopped = Some(Outcome::Returned { code: 0 });
            return Some(Step::Stopped);
        }
        None
    }

    /// Quantos milissegundos o jogo está adiantado em relação ao relógio do mundo.
    ///
    /// O relógio virtual adianta o tempo ocioso em vez de gastá-lo — é o que mantém honesto o
    /// tempo que o jogo *mede* —, e sem esse freio o emulador termina antes da hora: o Crash
    /// rodava doze segundos de jogo em um e meio de relógio real.
    fn ahead_ms(&self) -> u64 {
        u64::from(self.machine.clock_ms())
            .saturating_sub(self.clock_base)
            .saturating_sub(self.started.elapsed().as_millis() as u64)
    }

    /// Fecha a janela de medição quando ela vence e guarda a velocidade do trecho.
    ///
    /// A conta é sobre os últimos [`SPEED_WINDOW_MS`], não sobre a sessão inteira. Uma média
    /// desde o início demora minutos para reagir a uma tela mais pesada: o número desce devagar
    /// muito depois de a queda ter acontecido, e parece uma piora contínua onde o jogo já está
    /// rodando estável.
    fn sample_speed(&mut self) {
        let agora = Marca {
            real: Instant::now(),
            clock_ms: u64::from(self.machine.clock_ms()),
            instructions: self.machine.instructions(),
            frames: self.machine.quadros(),
        };
        let elapsed = (agora.real - self.window.real).as_millis() as u64;
        if elapsed < SPEED_WINDOW_MS {
            return;
        }
        let por_segundo = |quanto: u64| quanto * 1000 / elapsed;
        self.sample = Sample {
            speed: (por_segundo(agora.clock_ms.saturating_sub(self.window.clock_ms)) / 10).min(999)
                as u32,
            fps: por_segundo(u64::from(agora.frames.saturating_sub(self.window.frames))) as u32,
            ips: por_segundo(agora.instructions.saturating_sub(self.window.instructions)),
        };
        if self.history.len() >= HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(self.sample);
        self.window = agora;
    }

    /// A última amostra fechada.
    pub fn sample(&self) -> Sample {
        self.sample
    }

    /// As amostras recentes, da mais antiga para a mais nova.
    pub fn history(&self) -> impl ExactSizeIterator<Item = &Sample> {
        self.history.iter()
    }

    /// Bytes do heap do guest já entregues, e quantos objetos nossos estão vivos.
    pub fn memory(&self) -> (u32, usize) {
        (self.machine.heap_used(), self.machine.live_objects())
    }

    /// O retrato do heap do jogo: buracos, maior bloco, livre e usado.
    ///
    /// `memory()` diz **quanto** está em uso; este diz **como** o que sobra está repartido — e é
    /// isso que separa "o heap encheu" de "o heap se despedaçou".
    pub fn heap_retrato(&self) -> crate::brew::heap::Retrato {
        self.machine.heap_retrato()
    }

    /// As alocações e as checagens que o heap recusou, com o tamanho pedido e quem pediu.
    pub fn heap_recusas(&self) -> (Vec<(u32, u32)>, Vec<(u32, u32)>) {
        (
            self.machine.refused_allocations(),
            self.machine.refused_availability_checks(),
        )
    }

    /// O relógio do jogo, em milissegundos.
    pub fn clock_ms(&self) -> u32 {
        self.machine.clock_ms()
    }

    /// Liga o censo do acessador por classe de widget: o que cada classe recebe, por seletor.
    ///
    /// É o mesmo que a varredura liga por `ZEEBX_ROM_SELETORES`, e o core não tinha como pedir —
    /// `Machine` é privado do motor. Serve para responder, **no aparelho**, o que a família de
    /// widgets não distingue: o que cada classe proprietária espera.
    pub fn liga_censo_de_widgets(&mut self) {
        self.machine.liga_censo_de_widgets();
    }

    /// Liga a captura de serial: onde a **instrumentação** do motor escreve.
    ///
    /// Classes criadas, bancos abertos, SQL, propriedades de widget e a árvore de widgets da
    /// primeira tecla saem por aqui, sem se misturar com o log do jogo. A varredura e o `run` têm
    /// isso por `ZEEBX_ROM_SERIAL` e `--serial` desde sempre; o core não tinha, e é a diferença
    /// entre poder olhar o que o applet faz **no aparelho** e só poder supor.
    pub fn liga_serial(&mut self, caminho: &std::path::Path) -> std::io::Result<()> {
        self.machine.liga_serial(caminho)
    }

    /// O log da execução.
    ///
    /// Junta o que o jogo escreveu com o que o emulador tem a dizer sobre ele. A parte do
    /// emulador é a que quase sempre existe: a maioria dos jogos não usa `DBGPRINTF`, e uma
    /// janela vazia não ajuda ninguém a entender o que está acontecendo.
    ///
    /// As repetições do log do jogo vêm agrupadas, que é como o emulador as guarda — um jogo
    /// que escreve a mesma linha por quadro encheria a janela sem dizer mais nada.
    pub fn log(&self) -> Vec<String> {
        let mut linhas = Vec::new();

        // A rede vem primeiro porque é o que se está caçando: quem abre a janela de log depois
        // de mandar sincronizar quer ver o endereço que o jogo pediu, não rolar até o fim.
        let urls = self.machine.web_requests();
        if !urls.is_empty() {
            linhas.push("— endereços que o jogo pediu pelo IWeb —".to_string());
            linhas.extend(urls.iter().map(|url| format!("  {url}")));
        }
        let ignoradas = self.machine.ignored_gl();
        if !ignoradas.is_empty() {
            linhas.push("— GL atendido sem fazer nada —".to_string());
            linhas.push(format!("  {}", ignoradas.join(" ")));
        }
        let entregues = self.machine.delivered();
        if !entregues.is_empty() {
            linhas.push("— a ponte, e o que ela fez com a resposta —".to_string());
            linhas.extend(entregues.iter().map(|l| format!("  {l}")));
        }
        let claros = self.machine.plaintexts();
        if !claros.is_empty() {
            linhas.push("— o que o jogo cifrou, em claro —".to_string());
            for bloco in &claros {
                let hex: String = bloco.iter().map(|b| format!("{b:02x}")).collect();
                linhas.push(format!("  {} bytes  {hex}", bloco.len()));
                linhas.push(format!("    {:?}", String::from_utf8_lossy(bloco)));
            }
        }
        let toques = self.machine.pad_log();
        if !toques.is_empty() {
            linhas.push("— toques entregues ao jogo —".to_string());
            // A porta entra no registro porque, com duas ligadas nas mesmas teclas, o mesmo
            // toque aparece duas vezes — e sem dizer de onde veio, isso parece defeito.
            linhas.extend(toques.iter().map(|&(ms, porta, nome, down)| {
                let acao = match down {
                    true => "aperta",
                    false => "solta ",
                };
                format!("  {ms:>7} ms  porta {}  {acao} {nome}", porta + 1)
            }));
        }
        let midia = self.machine.media_log();
        if !midia.is_empty() {
            linhas.push("— o que o jogo fez com o som —".to_string());
            linhas.extend(midia.iter().map(|(ms, objeto, chamada, vezes)| {
                let repete = match vezes {
                    1 => String::new(),
                    n => format!("  ({n}x)"),
                };
                format!("  {ms:>7} ms  {objeto:#010x}  {chamada}{repete}")
            }));
        }
        let classes = self.machine.unknown_classes();
        if !classes.is_empty() {
            linhas.push("— classes que o jogo pediu e não temos —".to_string());
            linhas.extend(classes.iter().map(|id| format!("  {id:#010x}")));
        }
        let falhas = self.machine.swallowed_faults();
        if !falhas.is_empty() {
            linhas.push("— acessos inválidos que o jogo seguiu por cima —".to_string());
            linhas.extend(falhas.iter().map(|nota| format!("  {nota}")));
        }
        let apis = self.machine.missing_apis();
        if !apis.is_empty() {
            linhas.push("— APIs que faltaram —".to_string());
            linhas.extend(apis.iter().map(|nota| format!("  {nota}")));
        }
        let arquivos = self.machine.missing_files();
        if !arquivos.is_empty() {
            linhas.push("— arquivos não encontrados —".to_string());
            linhas.extend(arquivos.iter().map(|nome| format!("  {nome}")));
        }
        let hipoteses = self.machine.assumptions();
        if !hipoteses.is_empty() {
            linhas.push("— APIs atendidas por hipótese —".to_string());
            linhas.extend(hipoteses.iter().map(|nota| format!("  {nota}")));
        }
        let ponteiros = self.machine.bad_pointers();
        if !ponteiros.is_empty() {
            linhas.push("— ponteiros recusados —".to_string());
            linhas.extend(ponteiros.iter().map(|nota| format!("  {nota}")));
        }

        let jogo = self.machine.debug_output();
        if !jogo.is_empty() {
            linhas.push("— log do jogo —".to_string());
            linhas.extend(jogo.iter().map(|(linha, vezes)| match vezes {
                1 => format!("  {linha}"),
                n => format!("  {linha}   ({n}x)"),
            }));
        }
        let semihosting = self.machine.cpu().semihosting();
        if !semihosting.trim().is_empty() {
            linhas.push("— log por semihosting —".to_string());
            linhas.extend(semihosting.lines().map(|linha| format!("  {linha}")));
        }
        linhas
    }

    /// Mistura o som num `Mixer` sem placa, para gravar: quem chama pede os quadros com
    /// `render` no ritmo do relógio virtual.
    pub fn grava_audio(&mut self, taxa: u32) -> crate::audio::Mixer {
        let mixer = crate::audio::Mixer::silent(taxa);
        self.machine.set_audio(Some(mixer.clone()));
        mixer
    }

    /// Liga ou desliga o som, com o volume em `0..=100`.
    ///
    /// Um host sem placa de áudio não pode impedir o jogo de rodar: o motivo é devolvido para
    /// quem quiser mostrá-lo, e o emulador segue mudo.
    #[cfg(feature = "audio")]
    pub fn set_audio(&mut self, enabled: bool, volume: u8) -> Option<String> {
        let level = f32::from(volume.min(100)) / 100.0;
        if !enabled {
            self.audio = None;
            self.machine.set_audio(None);
            return None;
        }
        if let Some(output) = &self.audio {
            output.mixer().set_master(level, false);
            return None;
        }
        match crate::audio::Output::open(level, false) {
            Ok(output) => {
                self.machine.set_audio(Some(output.mixer()));
                self.audio = Some(output);
                None
            }
            Err(err) => Some(err),
        }
    }

    /// O controle de uma porta.
    pub fn set_port_pad(&mut self, porta: usize, pad: Pad) {
        self.machine.set_port_pad(porta, pad);
    }

    /// As calibrações do movimento que o jogo começou e terminou. Ver
    /// [`crate::machine::Machine::calibracao`].
    pub fn calibracao(&self) -> (u32, u32) {
        self.machine.calibracao()
    }

    /// A aceleração de uma porta com Boomerang. Ver [`crate::machine::Machine::set_port_motion`].
    pub fn set_port_motion(&mut self, porta: usize, aceleracao: [f32; 3]) {
        self.machine.set_port_motion(porta, aceleracao);
    }

    /// Uma tecla do teclado, apertada ou solta.
    ///
    /// O console tem teclado além dos dois controles, e o BREW o entrega como evento ao
    /// aplicativo, não pelo `IHID` — ver [`crate::input::EVT_KEY`]. A Z-Wheel depende disso: o
    /// formulário de abertura só sai do lugar com `AVK_0` ou `AVK_CLR`, que botão de controle
    /// nenhum produz.
    pub fn set_installed_applets(&mut self, classes: impl IntoIterator<Item = (u32, String)>) {
        self.machine.set_installed_applets(classes);
    }

    pub fn take_launch_request(&mut self) -> Option<u32> {
        self.machine.take_launch_request()
    }

    /// Entrega um evento de widget ao applet. Ver [`Machine::entrega_evento_ao_applet`].
    pub fn entrega_evento_ao_applet(&mut self, evt: u32, w: u16) -> Result<u32, crate::cpu::CpuError> {
        self.machine.entrega_evento_ao_applet(evt, w)
    }

    pub fn set_key(&mut self, avk: u32, apertada: bool) {
        self.machine.set_key(avk, apertada);
    }

    /// Diz que aparelho o console vê em cada porta.
    pub fn set_portas(
        &mut self,
        portas: [Option<crate::input::bindings::Aparelho>; crate::input::PORTAS],
    ) {
        self.machine.set_portas(portas);
    }

    /// Grava o estado da máquina, no formato versionado de [`crate::save_state`].
    pub fn grava_estado(&self) -> Vec<u8> {
        self.machine.grava_estado()
    }

    /// Põe de volta um estado gravado por [`Session::grava_estado`].
    pub fn restaura_estado(&mut self, arquivo: &[u8]) -> Result<(), crate::save_state::Erro> {
        self.machine.restaura_estado(arquivo)?;

        // O save state guarda o relógio do console, não os relógios do host. Se mantivermos
        // started/clock_base do instante anterior ao Load, o limitador compara o relógio
        // restaurado com uma linha do tempo que já não existe: voltar dez minutos pode parecer
        // dez minutos atrasado; avançar para um estado mais novo pode parecer adiantado e travar
        // em Step::Ahead. A nova âncora começa exatamente no instante virtual restaurado.
        let agora = Instant::now();
        let clock_ms = u64::from(self.machine.clock_ms());
        self.started = agora;
        self.clock_base = clock_ms;
        self.window = Marca {
            real: agora,
            clock_ms,
            instructions: self.machine.instructions(),
            frames: self.machine.quadros(),
        };
        self.sample = Sample::default();
        self.history.clear();

        // Estas duas peças vivem na Session, não na Machine: ambas descrevem o que o host
        // estava mostrando depois do ponto salvo. A máquina restaurada volta a ser a fonte da
        // verdade no próximo quadro.
        self.intermediario = None;
        self.stopped = None;
        Ok(())
    }

    /// Se dá para gravar agora. Ver [`crate::machine::Machine::pode_salvar`].
    pub fn pode_salvar(&mut self) -> Result<(), String> {
        self.machine.pode_salvar()
    }

    /// Assinatura do conteúdo da tela, para o frontend evitar reenvio de quadro repetido.
    pub fn screen_signature(&self) -> u64 {
        self.screen().signature()
    }

    /// A raiz do sistema de arquivos do jogo: onde a extração vive.
    ///
    /// O frontend precisa dela para podar o cache sem apagar o jogo em execução.
    pub fn content_root(&self) -> &Path {
        self.machine.file_root()
    }

    /// Traz para a tela da CPU o quadro que o `eglSwapBuffers` deixou pendente.
    ///
    /// **Quem lê os pixels da tela precisa chamar isto antes.** O desenho 2D chama sozinho, e a
    /// janela que apresenta a textura da placa não precisa dos pixels. Quem converte a tela para
    /// bytes — o despejo de quadro, um frontend de fora, a análise da varredura — precisa, senão
    /// recebe o quadro anterior. Ver [`crate::machine::Machine::present_gl`].
    pub fn materializa_quadro_gl(&mut self) {
        self.machine.materializa_quadro_gl();
    }

    /// Quantas trocas de buffer houve e quantas delas precisaram trazer o quadro para a CPU.
    pub fn leituras_do_quadro_gl(&self) -> (u32, u32) {
        (
            self.machine.gl_swaps(),
            self.machine.materializacoes_do_quadro_gl(),
        )
    }

    /// Chamadas de estado enviadas à placa e quantas o espelho poupou.
    pub fn estado_enviado_e_poupado(&self) -> (u64, u64) {
        self.machine.estado_enviado_e_poupado()
    }

    /// A tela, como está agora.
    pub fn screen(&self) -> &Framebuffer {
        self.intermediario
            .as_ref()
            .unwrap_or_else(|| self.machine.screen())
    }

    /// Põe à mostra a próxima tela intermediária, se houver, e diz se pôs.
    ///
    /// Enquanto houver, quem mostra a sessão deve exibir uma por quadro sem avançar o jogo:
    /// é o que faz uma animação síncrona, como a transição da Z-Wheel, aparecer. Ver
    /// [`crate::machine::Machine::toma_quadro_do_update`].
    pub fn mostra_quadro_intermediario(&mut self) -> bool {
        self.intermediario = self.machine.toma_quadro_do_update();
        self.intermediario.is_some()
    }

    /// A parada foi uma **saída normal**, e não uma quebra.
    ///
    /// `Outcome::Returned` é o applet que pediu para fechar (`EVT_APP_STOP`) ou que acabou sem
    /// nada pendente — o mesmo desfecho que a janela lê para voltar à Z-Wheel. Os outros são
    /// falha de verdade: API que não existe aqui, salto para endereço inválido.
    ///
    /// Existe porque o [`stopped_reason`](Self::stopped_reason) devolve só texto, e quem só tem
    /// o texto não consegue escolher entre "voltei ao menu" e "quebrou" — era o que fazia a saída
    /// limpa de um jogo aparecer como erro no Android.
    pub fn saiu_normalmente(&self) -> bool {
        matches!(self.stopped, Some(Outcome::Returned { .. }))
    }

    /// O motivo da parada, se o jogo parou, em texto que sirva para quem está olhando a tela.
    ///
    /// O `Debug` do desfecho traz endereços crus, e "método não implementado em `0xf0014034`"
    /// não diz nada a ninguém — o nome da interface e do método, sim.
    pub fn stopped_reason(&self) -> Option<String> {
        Some(match self.stopped.as_ref()? {
            Outcome::Returned { .. } => "o jogo terminou".to_string(),
            Outcome::Unimplemented { addr, caller, .. } => format!(
                "o jogo chamou {}, que ainda não existe aqui (de {caller:#010x})",
                crate::brew::aee::describe(*addr)
            ),
            // O `lr` entra junto: num salto para o endereço zero o `pc` não diz nada, e quem
            // chamou é a única pista de qual ponteiro estava vazio.
            Outcome::Fault { addr, pc, lr } => {
                format!("acesso inválido a {addr:#010x}, em {pc:#010x} (de {lr:#010x})")
            }
            Outcome::Exception { pc } => format!("exceção do núcleo ARM em {pc:#010x}"),
            Outcome::Budget => "o jogo passou do orçamento de instruções".to_string(),
            Outcome::CallLimit { calls } => {
                format!("teto de {calls} chamadas de API atingido — provável laço de repetição")
            }
        })
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// O último quadro do rasterizador GL, quando há um. Serve para separar o que o 3D desenhou
    /// do que chegou à tela composto.
    pub fn quadro_gl(&mut self) -> Option<Framebuffer> {
        self.machine.gl_frame()
    }

    /// Os registradores `r0`–`r11` e os prováveis endereços de retorno na pilha, da última falha.
    pub fn falha(&self) -> ([u32; 12], &[u32], Option<u32>) {
        let lr = match self.stopped {
            Some(Outcome::Fault { lr, .. }) => Some(lr),
            _ => None,
        };
        (self.machine.fault_regs(), self.machine.fault_stack(), lr)
    }

    /// O quadro 3D na resolução interna, quando é ele que está à mostra. Ver
    /// [`crate::machine::Machine::quadro_na_placa`].
    pub fn quadro_na_placa(&self) -> Option<crate::video::rasterizer::QuadroNaPlaca> {
        match self.intermediario {
            Some(_) => None,
            None => self.machine.quadro_na_placa(),
        }
    }

    /// O quadro 3D na resolução interna, para gravar sem janela.
    pub fn quadro_grande(&mut self) -> Option<Framebuffer> {
        self.machine.quadro_grande()
    }

    /// O quadro que a janela mostra, em RGB de oito bits por canal: largura, altura e os bytes.
    ///
    /// **O grande só quando é ele que está à mostra**, pela mesma pergunta do
    /// [`Session::quadro_na_placa`]. O [`Session::quadro_grande`] sozinho lê a placa mesmo com um
    /// HUD desenhado depois, e o print sairia sem o HUD. Fora disso vale a tela do console, em
    /// 640×480. Com a placa, é GL: quem chama deixa o contexto do rasterizador corrente. Ver
    /// `docs/implementacao/22-screenshots.md`.
    pub fn captura(&mut self) -> (u32, u32, Vec<u8>) {
        if self.quadro_na_placa().is_some()
            && let Some((largura, altura, rgba)) = self.machine.quadro_grande_rgba()
        {
            // Sem o alfa: há jogo que limpa o fundo com alfa zero, e o print sairia transparente.
            let rgb = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            return (largura as u32, altura as u32, rgb);
        }
        let tela = self.screen();
        let rgb = tela
            .to_argb()
            .into_iter()
            .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8])
            .collect();
        (tela.width(), tela.height(), rgb)
    }

    /// Liga a contagem de tempo real por método de API. Ver [`Session::perfil_de_api`].
    pub fn liga_perfil_de_api(&mut self) {
        self.machine.enable_api_profile();
    }

    /// Quanto tempo real cada método de API custou, do mais caro para o mais barato, em ns.
    pub fn perfil_de_api(&self) -> Vec<(String, u64)> {
        self.machine.api_profile()
    }

    /// Quantas instruções ARM o jogo já executou.
    pub fn instrucoes(&self) -> u64 {
        self.machine.instructions()
    }

    /// Quantos quadros de GL o jogo já apresentou.
    pub fn quadros_apresentados(&self) -> u64 {
        u64::from(self.machine.gl_swaps())
    }

    /// Quantos quadros o jogo desenhou na placa — **trocas de buffer ou `glClear`**.
    ///
    /// É diferente de [`Session::quadros_apresentados`], que conta só trocas: a Z-Wheel desenha o
    /// palco num pbuffer e nunca troca buffer, e um contador de trocas diria que ela não desenha
    /// nada. Quem pergunta "a placa desenhou neste quadro?" precisa deste.
    pub fn quadros_da_placa(&self) -> u32 {
        self.machine.quadros()
    }

    /// Antialias (amostras por pixel) e filtro anisotrópico do 3D na placa; valem na hora.
    pub fn define_melhorias(&mut self, amostras: usize, anisotropico: usize) {
        self.machine.define_melhorias(amostras, anisotropico);
    }

    /// Se a névoa do jogo vale; vale na hora.
    pub fn define_neblina(&mut self, permitida: bool) {
        self.machine.define_neblina(permitida);
    }

    /// Se o quadro de agora deve pular o desenho 3D e a limpeza de tela. Ver
    /// [`crate::machine::Machine::define_pula_desenho`].
    pub fn define_pula_desenho(&mut self, pula: bool) {
        self.machine.define_pula_desenho(pula);
    }

    /// Se o jogo já usou `glReadPixels` e, por segurança, desabilitou frameskip de rasterização.
    pub fn leu_pixels(&self) -> bool {
        self.machine.leu_pixels()
    }

    /// Muda a resolução interna do 3D; vale a partir do próximo quadro.
    pub fn define_resolucao_interna(&mut self, escala: usize) {
        self.machine.define_resolucao_interna(escala);
    }

    /// Diz ao rasterizador de placa para descartar profundidade e estêncil depois do quadro.
    pub fn define_descarte_de_tiles(&mut self, descartar: bool) {
        self.machine.define_descarte_de_tiles(descartar);
    }

    /// Reduz a resolução interna do 3D no rasterizador de **processador**, desenhando numa
    /// superfície menor e ampliando na apresentação. Ver [`Rasterizador::define_reducao`].
    pub fn define_reducao(&mut self, reducao: usize) {
        self.machine.define_reducao(reducao);
    }

    /// A proporção experimental do 3D, largura sobre altura; `None` é o 4:3 do console.
    /// Faz o desenho sair no framebuffer do frontend, quando ele entrega um.
    ///
    /// **É o que o core precisa para o `SET_HW_RENDER`**, e por isso tem caminho público: o motor
    /// já sabe desenhar no framebuffer de fora (`Machine::desenha_no_fbo`, verificado pelo teste
    /// `o_motor_desenha_no_framebuffer_do_frontend`), mas quem tem o framebuffer em mãos é o core,
    /// a cada quadro, pelo `get_current_framebuffer` do `retro_hw_render_callback`.
    ///
    /// `Some(0)` é o framebuffer padrão do frontend; `None` devolve o desenho ao framebuffer do
    /// próprio motor, que é o caminho de sempre.
    pub fn desenha_no_fbo(&mut self, fbo: Option<u32>) {
        self.machine.desenha_no_fbo(fbo);
    }

    pub fn define_proporcao(&mut self, aspecto: Option<f32>) {
        self.machine.define_proporcao(aspecto);
    }

    /// O ClassID do applet que roda nesta sessão.
    pub fn classe(&self) -> u32 {
        self.classe
    }

    /// Começa com a tela que o applet anterior deixou, antes do primeiro desenho deste.
    ///
    /// O framebuffer do aparelho não é apagado na troca de applet. Ao abrir um jogo, a Z-Wheel
    /// deixa na tela o "Aguarde enquanto o aplicativo é carregado" (ver [`SPLASH_DA_Z_WHEEL`]), e
    /// é ele que se vê até o jogo desenhar o primeiro quadro.
    pub fn herda_tela(&mut self, rgb565: &[u8]) {
        self.machine.pinta_tela_rgb565(rgb565);
    }

    /// Se o applet saiu por conta própria, e não por falha.
    pub fn saiu_sozinho(&self) -> bool {
        matches!(self.stopped, Some(Outcome::Returned { .. }))
    }
}

/// O ClassID da Z-Wheel, a tela inicial do console.
///
/// **Escolher um jogo nela é fechá-la.** O caminho está no módulo: ao confirmar, ela grava o
/// marcador `ttgmrun.tmp` e o `StringLastAppRan`, desmonta as telas e, no timer de `0x81c38`,
/// sai. O console a reabre por ser a tela inicial, e na partida ela vê o marcador (`0x81904`),
/// apaga-o e abre o jogo gravado dois segundos depois. Quem roda a Z-Wheel precisa reabri-la
/// quando ela sai sozinha — sem isso o jogo escolhido nunca abria.
pub const Z_WHEEL: u32 = 0x0107_0798;

/// A imagem RGB565 de 640×480 que o console mostra ao abrir a Z-Wheel, no diretório dela.
///
/// No boot é o "Bem-Vindo ao Zeebo". **Para abrir um jogo, a Z-Wheel troca o arquivo antes de
/// sair**: se existe `gamestartrgb.sav`, ela renomeia este para `zeebosplash.rgb565.sav` e o
/// `gamestartrgb.sav` para este nome (`0x81db0`), e na reabertura desfaz a troca logo no
/// primeiro milissegundo (`0x822bc`). Quem lê o arquivo entre as duas é o console, ao
/// reabri-la — e o que aparece é "Aguarde enquanto o aplicativo é carregado", que fica na tela
/// nos dois segundos até o jogo abrir, porque a Z-Wheel reaberta não desenha nada nesse tempo.
pub const SPLASH_DA_Z_WHEEL: &str = "zeebosplash.rgb565.raw";

/// A execução por dentro, para a varredura de ROMs — ver [`crate::varredura`].
///
/// O [`Session::log`] monta texto para a janela; o levantamento precisa das listas cruas e do
/// desfecho como enum, para classificar e comparar com o que já se sabia do jogo. Só o teste
/// usa isto, e por isso não faz parte da interface da sessão.
#[cfg(test)]
impl Session {
    /// O motor, para quem precisa ligar uma medição antes de rodar.
    ///
    /// Existe para o perfil de tempo por método: ligar o cronômetro por chamada é coisa que se faz
    /// **antes** do laço, e a varredura precisa fazer isso de fora da sessão.
    pub(crate) fn machine_mut(&mut self) -> &mut Machine<CpuDaSessao> {
        &mut self.machine
    }

    pub(crate) fn machine(&self) -> &Machine<CpuDaSessao> {
        &self.machine
    }

    pub(crate) fn stopped(&self) -> Option<&Outcome> {
        self.stopped.as_ref()
    }

    /// A partida, sem a volta do laço que vem junto no [`Session::step`].
    pub(crate) fn partida(&mut self) -> Option<Step> {
        self.parte()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sessao_minima_para_save_state() -> Session {
        let mut mem = crate::cpu::mem::GuestMemory::new();
        let mut codigo = 0xe12f_ff1eu32.to_le_bytes().to_vec(); // bx lr
        codigo.resize(0x1000, 0);
        mem.map("code", 0, codigo, true).unwrap();
        mem.map_zeroed("data", 0x1000, 0x1000).unwrap();
        mem.map_zeroed("stack", loader::STACK_BASE, loader::STACK_SIZE)
            .unwrap();
        let modulo = loader::LoadedModule {
            mem,
            entry: 0,
            shell: loader::OBJECT_BASE,
            helpers: 0,
            out_module: loader::OBJECT_BASE,
            extensions: Vec::new(),
        };
        let mut cpu = CpuDaSessao::new().unwrap();
        crate::cpu::CpuBackend::reset(&mut cpu, &modulo.mem).unwrap();
        let machine = Machine::new(
            cpu,
            modulo,
            std::env::temp_dir().join("zeebx-session-state-clock"),
        );
        let agora = Instant::now();
        let clock_ms = u64::from(machine.clock_ms());
        let window = Marca {
            real: agora,
            clock_ms,
            instructions: machine.instructions(),
            frames: machine.quadros(),
        };
        Session {
            machine,
            partida: None,
            #[cfg(feature = "audio")]
            audio: None,
            title: "teste".to_string(),
            classe: 0,
            intermediario: None,
            started: agora,
            clock_base: clock_ms,
            stopped: None,
            window,
            sample: Sample::default(),
            history: std::collections::VecDeque::new(),
        }
    }

    #[test]
    fn restaurar_estado_reancora_o_relogio_real_da_sessao() {
        let mut sessao = sessao_minima_para_save_state();
        let estado = sessao.grava_estado();

        // Simula uma sessão que continuou muito tempo depois do ponto salvo. Estas peças são do
        // host e não entram no arquivo ZBXS; se sobreviverem ao Load, o limitador compara duas
        // linhas do tempo diferentes e pode acelerar ou segurar o jogo indevidamente.
        sessao.started = Instant::now() - Duration::from_secs(30);
        sessao.clock_base = 123_456;
        sessao.window.real = Instant::now() - Duration::from_secs(5);
        sessao.window.clock_ms = 987_654;
        sessao.sample = Sample {
            speed: 321,
            fps: 99,
            ips: 123,
        };
        sessao.history.push_back(sessao.sample);
        sessao.intermediario = Some(Framebuffer::new(2, 2));
        sessao.stopped = Some(Outcome::Budget);

        sessao.restaura_estado(&estado).unwrap();

        assert_eq!(sessao.clock_base, u64::from(sessao.machine.clock_ms()));
        assert_eq!(sessao.ahead_ms(), 0, "o estado restaurado não pode nascer adiantado");
        assert!(sessao.atraso_ms() < 100, "o estado restaurado nasceu artificialmente atrasado");
        assert!(sessao.history.is_empty());
        assert_eq!(sessao.sample.speed, 0);
        assert_eq!(sessao.sample.fps, 0);
        assert_eq!(sessao.sample.ips, 0);
        assert!(sessao.intermediario.is_none());
        assert!(sessao.stopped.is_none());
    }

/// **O motor desenha no framebuffer que o frontend entrega, e o teste prova isso.**
///
/// É a peça que faltava para o item 5: no `libretro`, quem apresenta o quadro é o frontend, e o
/// core desenha no framebuffer que ele indica. Aqui o teste cria um framebuffer **próprio**, manda
/// o motor desenhar nele e lê os pixels **dele** — não do framebuffer interno do motor. Se o
/// desenho não estivesse indo para lá, o framebuffer de fora continuaria com o conteúdo
/// indefinido com que nasceu, e a leitura devolveria preto.
///
/// A comparação é contra o rasterizador de software no mesmo instante virtual, como no teste
/// irmão [`os_dois_rasterizadores_desenham_o_mesmo_quadro`].
#[cfg(feature = "gpu")]
#[test]
fn o_motor_desenha_no_framebuffer_do_frontend() {
    use glow::HasContext;
    use std::time::Duration;

    let Ok(rom) = std::env::var("ZEEBX_TESTE_ROM") else {
        eprintln!("sem ZEEBX_TESTE_ROM: nada a comparar");
        return;
    };
    let ms: u64 = std::env::var("ZEEBX_TESTE_MS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(3000);
    let contexto = match crate::video::contexto::Contexto::novo() {
        Ok(contexto) => contexto,
        Err(porque) => {
            eprintln!("sem placa fora de tela: {porque}");
            return;
        }
    };
    let gl = contexto.gl.clone();

    // O framebuffer de fora, com a textura de cor: é ele que faz o papel do que o frontend daria.
    let (fbo, textura) = unsafe {
        let fbo = gl.create_framebuffer().expect("framebuffer de fora");
        let textura = gl.create_texture().expect("textura de fora");
        gl.bind_texture(glow::TEXTURE_2D, Some(textura));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            640,
            480,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(textura),
            0,
        );
        // Nasce com conteúdo indefinido: pintar de verde garante que qualquer pixel não-preto
        // depois seja desenho de verdade, e não sobra de alocação.
        gl.clear_color(0.0, 1.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        (fbo, textura)
    };

    let mut session = Session::start_with(
        &std::path::PathBuf::from(&rom),
        crate::PORTAS_PADRAO,
        None,
        true,
        Some(gl.clone()),
        Default::default(),
    )
    .expect("a sessão de placa abriu");
    session.machine_mut().desenha_no_fbo(Some(fbo.0.get()));

    let base = session.clock_ms();
    // **A cerca.** Metade do tempo depois, o framebuffer de fora volta a ser pintado de verde.
    // Sem isto o teste não pega o defeito que existiu: o alvo do frontend era respeitado só na
    // **criação** do destino, e todo quadro seguinte religava o framebuffer interno — o de fora
    // ficava com o que o primeiro quadro deixou, e uma verificação de "mudou alguma coisa" passava
    // com a tela preta no frontend. Com a cerca, o verde só desaparece se os quadros **novos**
    // estiverem indo para lá.
    let cerca = ms / 2;
    let mut ja_cercou = false;
    loop {
        let decorrido = session.clock_ms().saturating_sub(base);
        if decorrido >= ms as u32 {
            break;
        }
        if !ja_cercou && decorrido >= cerca as u32 {
            ja_cercou = true;
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.clear_color(0.0, 1.0, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
        }
        match session.step(Duration::ZERO, false) {
            Step::Stopped => break,
            Step::Presented => {
                while session.mostra_quadro_intermediario() {}
            }
            Step::Running | Step::Ahead => {}
        }
    }

    // Lê do framebuffer **de fora**: é o que prova que o desenho foi para lá.
    let mut pixels = vec![0u8; 640 * 480 * 4];
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.read_pixels(
            0,
            0,
            640,
            480,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    }
    let verdes = pixels
        .chunks_exact(4)
        .filter(|p| p[0] == 0 && p[1] == 255 && p[2] == 0 && p[3] == 255)
        .count();
    let total = 640 * 480;
    eprintln!(
        "framebuffer de fora: {} de {total} pixel(s) ainda com a cor de nascença ({:.1}%)",
        verdes,
        verdes as f64 * 100.0 / total as f64
    );
    assert!(
        verdes < total,
        "nada foi desenhado no framebuffer do frontend: ele ficou como nasceu"
    );
    // E o que mais importa: os quadros **posteriores à cerca** também foram para lá.
    assert!(
        verdes < total / 10,
        "o framebuffer do frontend ficou com a cor da cerca: os quadros novos não foram para lá"
    );

    unsafe {
        gl.delete_framebuffer(fbo);
        gl.delete_texture(textura);
    }
}


/// **Salvar e carregar no rasterizador de placa dá o mesmo quadro.**
///
/// É a prova que faltava do item 6: a via de gravação do `GpuState` estava ligada e sem teste. O
/// teste segue a sequência que o RetroArch faz, e vai além do "os campos voltam": ele **exige que o
/// desenho continue igual**.
///
/// O caminho é este: roda um trecho, **salva**; roda mais um trecho, e guarda o quadro (é o que o
/// jogo faz depois do save); **carrega** o estado e roda o mesmo trecho de novo. Os dois quadros têm
/// de ser idênticos — mesma matriz, mesmas texturas, mesmo quadro. Sem isso o save state "volta" e a
/// cena sai diferente, que é o modo de falhar mais caro de descobrir.
#[cfg(feature = "gpu")]
#[test]
fn o_estado_da_placa_continua_o_mesmo_desenho() {
    use glow::HasContext;
    use std::time::Duration;

    let Ok(rom) = std::env::var("ZEEBX_TESTE_ROM") else {
        eprintln!("sem ZEEBX_TESTE_ROM: nada a comparar");
        return;
    };
    let ms: u64 = std::env::var("ZEEBX_TESTE_MS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(2000);
    let contexto = match crate::video::contexto::Contexto::novo() {
        Ok(contexto) => contexto,
        Err(porque) => {
            eprintln!("sem placa fora de tela: {porque}");
            return;
        }
    };
    let gl = contexto.gl.clone();
    let (fbo, textura) = unsafe {
        let fbo = gl.create_framebuffer().expect("framebuffer de fora");
        let textura = gl.create_texture().expect("textura de fora");
        gl.bind_texture(glow::TEXTURE_2D, Some(textura));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            640,
            480,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(textura),
            0,
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        (fbo, textura)
    };

    let mut session = Session::start_with(
        &std::path::PathBuf::from(&rom),
        crate::PORTAS_PADRAO,
        None,
        true,
        Some(gl.clone()),
        Default::default(),
    )
    .expect("a sessão de placa abriu");
    session.machine_mut().desenha_no_fbo(Some(fbo.0.get()));

    // Roda `ms` virtuais e lê o quadro do framebuffer de fora.
    let roda_e_le = |session: &mut Session, ms: u64| -> Vec<u8> {
        let base = session.clock_ms();
        while session.clock_ms().saturating_sub(base) < ms as u32 {
            match session.step(Duration::ZERO, false) {
                Step::Stopped => break,
                Step::Presented => {
                    while session.mostra_quadro_intermediario() {}
                }
                Step::Running | Step::Ahead => {}
            }
        }
        let mut pixels = vec![0u8; 640 * 480 * 4];
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.read_pixels(
                0,
                0,
                640,
                480,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        pixels
    };

    // 1) Um trecho, e o ponto de salvamento.
    let _ = roda_e_le(&mut session, ms);
    assert!(session.pode_salvar().is_ok(), "o motor recusou salvar no fim do quadro");
    let estado = session.grava_estado();
    assert!(estado.len() > 1024, "o estado saiu pequeno demais: {}", estado.len());

    // 2) Mais um trecho, e o quadro que o jogo mostra depois do save.
    let esperado = roda_e_le(&mut session, ms);

    // 3) Volta ao ponto de salvamento, e o mesmo trecho de novo.
    session.restaura_estado(&estado).expect("carregou");
    let depois = roda_e_le(&mut session, ms);

    let iguais = esperado.iter().zip(depois.iter()).filter(|(a, b)| a == b).count();
    let total = esperado.len();
    eprintln!(
        "o quadro depois do save e o quadro depois de carregar: {iguais} de {total} byte(s) iguais \
         ({:.2}%)",
        iguais as f64 * 100.0 / total as f64
    );
    assert_eq!(
        iguais, total,
        "o desenho não continuou igual depois de carregar o estado"
    );

    unsafe {
        gl.delete_framebuffer(fbo);
        gl.delete_texture(textura);
    }
}

/// **O mesmo jogo pelos dois rasterizadores, no mesmo instante virtual.**
///
/// É a verificação que faltava para o render em hardware. Com a janela fechada, os dois caminhos
/// rodam o mesmo conteúdo pelo mesmo tempo virtual, e os quadros são comparados byte a byte. Sem
/// isto, ligar o render em hardware seria trocar um caminho medido por um caminho que ninguém
/// olhou — e é por isso que o `SET_HW_RENDER` fica por fazer no core até esta conta existir.
///
/// **Sem `ZEEBX_TESTE_ROM` não roda**, e **sem placa também não**: num terminal sem EGL o caminho
/// fora de tela responde que não existe, e isso é o caso normal, não uma falha do teste.
#[cfg(feature = "gpu")]
#[test]
fn os_dois_rasterizadores_desenham_o_mesmo_quadro() {
    use std::time::Duration;

    let Ok(rom) = std::env::var("ZEEBX_TESTE_ROM") else {
        eprintln!("sem ZEEBX_TESTE_ROM: nada a comparar");
        return;
    };
    let origem = std::path::PathBuf::from(&rom);
    let ms: u64 = std::env::var("ZEEBX_TESTE_MS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(3000);

    // O contexto precisa ficar vivo enquanto a sessão roda: as funções de GL moram nele.
    let contexto = match crate::video::contexto::Contexto::novo() {
        Ok(contexto) => contexto,
        Err(porque) => {
            eprintln!("sem placa fora de tela: {porque}");
            return;
        }
    };

    let tela_de = |placa: bool, contexto: Option<std::sync::Arc<glow::Context>>| {
        let mut session = Session::start_with(
            &origem,
            crate::PORTAS_PADRAO,
            None,
            placa,
            contexto,
            Default::default(),
        )
        .ok()?;
        let base = session.clock_ms();
        while session.clock_ms().saturating_sub(base) < ms as u32 {
            match session.step(Duration::ZERO, false) {
                Step::Stopped => break,
                Step::Presented => {
                    while session.mostra_quadro_intermediario() {}
                }
                Step::Running | Step::Ahead => {}
            }
        }
        // O quadro do OpenGL pode estar pendente — ver [`Session::materializa_quadro_gl`]. Sem
        // isto, o caminho de placa entregaria a tela anterior, e a comparação lá embaixo seria
        // entre duas telas velhas: passaria sem comparar imagem nenhuma.
        session.materializa_quadro_gl();
        let (trocas, leituras) = session.leituras_do_quadro_gl();
        let (enviadas, poupadas) = session.estado_enviado_e_poupado();
        eprintln!(
            "  {}: {trocas} troca(s) de buffer, {leituras} leitura(s) do quadro para a CPU, \
{enviadas} estado(s) enviado(s) e {poupadas} poupado(s) pelo espelho",
            match placa {
                true => "placa",
                false => "processador",
            }
        );
        let tela = session.screen();
        let (largura, altura) = (tela.width(), tela.height());
        let mut bytes = Vec::new();
        tela.write_rgb565_into(&mut bytes);
        Some((largura, altura, bytes))
    };

    let software = tela_de(false, None).expect("a sessão de software abriu");
    let placa = tela_de(true, Some(contexto.gl.clone())).expect("a sessão de placa abriu");

    assert_eq!(
        (software.0, software.1),
        (placa.0, placa.1),
        "os dois caminhos desenham em tamanhos diferentes"
    );
    assert_eq!(software.2.len(), placa.2.len());

    // Comparação de pixel RGB565: quantos diferem e por quanto. Um rasterizador na placa e outro
    // no processador não dão o **mesmo** quadro — a diferença é arredondamento e ordem de
    // operações. O que se cobra aqui é que desenhem a mesma imagem, e não que sejam idênticos.
    let mut diferentes = 0usize;
    let mut grosseiras = 0usize;
    let mut soma = 0u64;
    let mut pior = 0u16;
    for (a, b) in software.2.chunks_exact(2).zip(placa.2.chunks_exact(2)) {
        let (a, b) = (
            u16::from_le_bytes([a[0], a[1]]),
            u16::from_le_bytes([b[0], b[1]]),
        );
        let (r1, g1, b1) = (a >> 11 & 0x1f, a >> 5 & 0x3f, a & 0x1f);
        let (r2, g2, b2) = (b >> 11 & 0x1f, b >> 5 & 0x3f, b & 0x1f);
        let d = (r1.abs_diff(r2) as u64)
            + (g1.abs_diff(g2) as u64)
            + (b1.abs_diff(b2) as u64);
        if d > 0 {
            diferentes += 1;
        }
        // Um passo de canal em 565 é o **arredondamento** dos dois conversores, e não imagem
        // diferente: o rasterizador de software trunca (`>> 3`), e o `glReadPixels` em
        // `UNSIGNED_SHORT_5_6_5` arredonda. Medido no Crash: 98,33% dos pixels diferem por 1 ou 2
        // passos, com pior 2 e média 1,93 — mesma imagem. O que a conta abaixo guarda é a
        // diferença que **não** é arredondamento.
        if d > 2 {
            grosseiras += 1;
        }
        soma += d;
        pior = pior.max(d as u16);
    }
    let total = software.2.len() / 2;
    // **Guarda contra passe vazio.** Se as duas telas estiverem apagadas, a comparação abaixo
    // passa sem ter comparado imagem nenhuma — foi o que aconteceu quando o quadro da placa
    // passou a ser adiado e este teste não materializava antes de ler.
    let acesos = software
        .2
        .chunks_exact(2)
        .filter(|p| p[0] != 0 || p[1] != 0)
        .count();
    assert!(
        acesos > total / 100,
        "o quadro saiu apagado ({acesos} de {total} pixel(is) aceso(s)): não há imagem para comparar"
    );
    let percentual = diferentes as f64 * 100.0 / total as f64;
    let grosseiro = grosseiras as f64 * 100.0 / total as f64;
    let media = soma as f64 / total as f64;
    eprintln!(
        "software x placa: {diferentes} de {total} pixel(s) diferentes ({percentual:.2}%), \
         diferença média {media:.3} por pixel, pior {pior}, acima de 2 passos: {grosseiro:.2}%"
    );
    // **O que este teste guarda é a imagem, não o arredondamento.** Uma imagem diferente erra por
    // muito mais que dois passos de canal; o arredondamento dos dois conversores erra por um ou
    // dois. Sem esta separação o teste ficava vermelho por 1,9 passo de média — e um teste que
    // ninguém pode deixar verde deixa de guardar coisa alguma.
    assert!(
        grosseiro < 1.0,
        "os dois rasterizadores desenham imagens diferentes: {grosseiro:.2}% dos pixels diferem \
         por mais de 2 passos de canal (média {media:.3}, pior {pior})"
    );
    assert!(media <= 2.0, "diferença média de {media:.3} passos por pixel");
}


    #[test]
    fn um_arquivo_que_nao_existe_diz_que_nao_deu_para_ler() {
        let err = Session::start_inner(
            &std::env::temp_dir().join("zeebx-nao-existe.mod"),
            None,
            None,
            false,
            None,
            Default::default(),
            &[],
        );
        assert!(matches!(err, Err(StartError::Unreadable(_))));
    }

    #[test]
    fn um_arquivo_que_nao_e_modulo_e_recusado_com_motivo() {
        // O que importa não é em qual etapa o lixo é barrado — o cabeçalho do `.mod` é frouxo,
        // e quem recusa acaba sendo o carregador —, e sim que a recusa venha com um motivo
        // legível, porque é ele que a interface mostra.
        let path = std::env::temp_dir().join("zeebx-teste-lixo.mod");
        std::fs::write(&path, b"isto nao e um modulo").unwrap();
        let Err(err) = Session::start_inner(&path, None, None, false, None, Default::default(), &[])
        else {
            panic!("um arquivo de lixo não podia virar uma sessão");
        };
        assert!(!err.to_string().is_empty());
        let _ = std::fs::remove_file(&path);
    }
}

/// Lê os módulos de extensão que acompanham um `.mod` e os deixa prontos para o carregador.
///
/// Um `.mod` que não abra é ignorado em silêncio: a extensão é um extra do pacote, e recusar o
/// jogo inteiro porque um módulo secundário está corrompido seria trocar um jogo que roda em
/// parte por um que não roda.
pub fn extensoes_de(mod_path: &std::path::Path) -> Vec<loader::ExtensionImage> {
    crate::library::extensoes(mod_path)
        .into_iter()
        .filter_map(|(caminho, classes)| {
            let bytes = std::fs::read(caminho).ok()?;
            let image = ModImage::parse(bytes).ok()?;
            Some(loader::ExtensionImage { image, classes })
        })
        .collect()
}
