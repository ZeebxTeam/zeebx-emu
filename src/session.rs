//! Um jogo em execução, do arquivo até os quadros na tela.
//!
//! Existe para separar o ciclo de vida do BREW — carregar o módulo, criar o applet, entregar o
//! `EVT_APP_START`, girar o laço de eventos — de quem o observa. A linha de comando roda esse
//! laço até um limite e imprime o resultado; a interface o toca um pedaço por quadro desenhado.
//! O que muda é quem chama [`Session::step`], não o que ele faz.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::cpu::unicorn::UnicornCpu;
use crate::display::Framebuffer;
use crate::input::Pad;
use crate::machine::{AppletResult, Machine, Outcome};
use crate::{archive, library, loader, modfile::ModImage};

/// Teto de instruções por fatia entre duas chamadas de API — evita que um laço infinito no
/// guest trave o emulador.
const INSTRUCTION_BUDGET: u64 = 500_000_000;

/// Sobre quanto tempo real medir a velocidade mostrada na interface.
///
/// Meio segundo é curto o bastante para a leitura acompanhar a troca de tela e longo o bastante
/// para não tremer a cada quadro.
const SPEED_WINDOW_MS: u64 = 500;

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
    machine: Machine<UnicornCpu>,
    /// A saída de som. Enquanto ela existe, o som toca; largá-la fecha o fluxo.
    audio: Option<crate::audio::Output>,
    title: String,
    /// Instante e leitura do relógio virtual quando a execução começou, que é o par com que se
    /// mede se o jogo está adiantado.
    started: Instant,
    clock_base: u64,
    stopped: Option<Outcome>,
    /// Começo da janela de medição da velocidade: o instante real e o relógio virtual de então.
    speed_window: (Instant, u64),
    /// Última leitura fechada da velocidade, em porcentagem.
    speed: u32,
}

impl Session {
    /// Carrega o módulo, cria o applet e entrega o `EVT_APP_START`.
    ///
    /// Um `.zip` é extraído para o cache antes: o jogo grava (o Peteca tem um `.sav`), e
    /// escrever de volta num pacote não é coisa que se queira fazer.
    pub fn start(path: &Path) -> Result<Self, StartError> {
        let extracted;
        let path = match path.extension().and_then(|e| e.to_str()) {
            Some("zip") => {
                extracted = archive::extract(path).map_err(StartError::Unreadable)?;
                extracted.as_path()
            }
            _ => path,
        };
        let bytes = std::fs::read(path).map_err(StartError::Unreadable)?;
        let image = ModImage::parse(bytes).map_err(|e| StartError::NotAModule(e.to_string()))?;
        let module = loader::load(&image).map_err(|e| StartError::NotLoadable(e.to_string()))?;

        // A raiz do sistema de arquivos do jogo é o diretório onde o `.mod` está: é lá que o
        // console guarda os arquivos do título.
        let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let cpu = UnicornCpu::new().map_err(|e| StartError::NotLoadable(e.to_string()))?;
        let mut machine = Machine::new(cpu, module, root);

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
        let started = machine
            .start_applet(applet, clsid, INSTRUCTION_BUDGET)
            .map_err(|e| StartError::NotLoadable(e.to_string()))?;
        if !matches!(started, Outcome::Returned { .. }) {
            return Err(StartError::Stopped(started));
        }

        let clock_base = u64::from(machine.clock_ms());
        Ok(Self {
            machine,
            audio: None,
            title: library::title_for(path),
            started: Instant::now(),
            clock_base,
            stopped: None,
            speed_window: (Instant::now(), clock_base),
            speed: 100,
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
        let before = self.machine.gl_swaps();
        loop {
            if speed_limit && self.ahead_ms() > 0 {
                return Step::Ahead;
            }
            match self.advance_once() {
                Some(step) => return step,
                None if self.machine.gl_swaps() != before => return Step::Presented,
                None if Instant::now() >= deadline => return Step::Running,
                None => {}
            }
        }
    }

    /// Uma volta do laço de eventos. `Some` quando há desfecho, `None` para continuar.
    fn advance_once(&mut self) -> Option<Step> {
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
        if let Some(bad) = outcomes
            .iter()
            .find(|outcome| !matches!(outcome, Outcome::Returned { .. }))
        {
            self.stopped = Some(bad.clone());
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
        let now = Instant::now();
        let elapsed = (now - self.speed_window.0).as_millis() as u64;
        if elapsed < SPEED_WINDOW_MS {
            return;
        }
        let clock = u64::from(self.machine.clock_ms());
        let advanced = clock.saturating_sub(self.speed_window.1);
        self.speed = (advanced * 100 / elapsed).min(999) as u32;
        self.speed_window = (now, clock);
    }

    /// A que fração da velocidade do console o jogo está rodando, em porcentagem.
    pub fn speed_percent(&self) -> u32 {
        self.speed
    }

    /// Liga ou desliga o som, com o volume em `0..=100`.
    ///
    /// Um host sem placa de áudio não pode impedir o jogo de rodar: o motivo é devolvido para
    /// quem quiser mostrá-lo, e o emulador segue mudo.
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

    pub fn set_pad(&mut self, pad: Pad) {
        self.machine.set_pad(pad);
    }

    /// A tela, como está agora.
    pub fn screen(&self) -> &Framebuffer {
        self.machine.screen()
    }

    /// Quantos quadros o jogo apresentou.
    pub fn frames(&self) -> u32 {
        self.machine.gl_swaps()
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
                crate::aee::describe(*addr)
            ),
            Outcome::Fault { addr, pc, .. } => {
                format!("acesso inválido a {addr:#010x}, em {pc:#010x}")
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn um_arquivo_que_nao_existe_diz_que_nao_deu_para_ler() {
        let err = Session::start(&std::env::temp_dir().join("zeebx-nao-existe.mod"));
        assert!(matches!(err, Err(StartError::Unreadable(_))));
    }

    #[test]
    fn um_arquivo_que_nao_e_modulo_e_recusado_com_motivo() {
        // O que importa não é em qual etapa o lixo é barrado — o cabeçalho do `.mod` é frouxo,
        // e quem recusa acaba sendo o carregador —, e sim que a recusa venha com um motivo
        // legível, porque é ele que a interface mostra.
        let path = std::env::temp_dir().join("zeebx-teste-lixo.mod");
        std::fs::write(&path, b"isto nao e um modulo").unwrap();
        let Err(err) = Session::start(&path) else {
            panic!("um arquivo de lixo não podia virar uma sessão");
        };
        assert!(!err.to_string().is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
