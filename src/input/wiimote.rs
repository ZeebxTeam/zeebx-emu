//! O Wii Remote, lido direto dos dispositivos de entrada do Linux.
//!
//! O driver `hid-wiimote` do kernel divide cada controle em vários dispositivos: os botões em
//! "Nintendo Wii Remote" e o acelerômetro em "Nintendo Wii Remote Accelerometer", que só
//! começa a mandar dados depois de aberto. O gilrs enxerga, no máximo, os botões; o
//! acelerômetro é eixo de outro dispositivo, então a leitura é nossa.
//!
//! Cada dispositivo tem uma thread que lê os eventos brutos (`struct input_event`) e atualiza um
//! estado compartilhado. Uma thread de busca procura controles novos a cada segundo, e a leitura
//! de um controle que desconectou simplesmente termina com erro.
//!
//! Fora do Linux não há leitura: [`Wiimotes::estado`] devolve `None`.

use std::sync::{Arc, Mutex};

use crate::input::bindings::Source;

/// O que um Wii Remote está fazendo agora.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EstadoWiimote {
    /// Um bit por botão, na ordem de [`BOTOES`].
    pub botoes: u16,
    /// A aceleração em g, no referencial do controle: `x` para a direita, `y` para a frente (a
    /// ponta do infravermelho) e `z` para cima, saindo da face dos botões. Parado com a face para
    /// cima, `z` é `+1`.
    pub aceleracao: [f32; 3],
    /// Se o acelerômetro já mandou alguma leitura.
    pub com_acelerometro: bool,
}

impl Default for EstadoWiimote {
    fn default() -> Self {
        Self {
            botoes: 0,
            aceleracao: [0.0, 0.0, 1.0],
            com_acelerometro: false,
        }
    }
}

/// Os botões do Wii Remote, com o código de tecla que o `hid-wiimote` usa para cada um.
pub const BOTOES: [(&str, u16); 11] = [
    ("up", 103),     // KEY_UP
    ("down", 108),   // KEY_DOWN
    ("left", 105),   // KEY_LEFT
    ("right", 106),  // KEY_RIGHT
    ("a", 0x130),    // BTN_A
    ("b", 0x131),    // BTN_B
    ("1", 0x101),    // BTN_1
    ("2", 0x102),    // BTN_2
    ("plus", 407),   // KEY_NEXT
    ("minus", 412),  // KEY_PREVIOUS
    ("home", 0x13c), // BTN_MODE
];

/// Quanto o `hid-wiimote` reporta para 1 g. O acelerômetro tem 10 bits e o driver só tira o
/// centro (`0x200`); a gravidade fica perto de 100 unidades em todos os eixos.
///
/// Só o caminho do Linux lê direto do `hid-wiimote`; nos outros sistemas o acelerômetro vem do
/// gilrs, e a constante ficaria sem uso — o que já rendeu aviso de código morto no macOS.
#[cfg(target_os = "linux")]
const UNIDADES_POR_G: f32 = 100.0;

/// O nome de cada botão de [`BOTOES`] no mapeamento, na mesma ordem.
///
/// Os botões do Wii Remote são origens como as do gilrs, e é assim que ele serve de Z-Pad com o
/// mapeamento que o jogador quiser. O prefixo os separa dos botões do gilrs, que têm nomes como
/// `South` e `DPadUp`: uma porta com o Wii Remote escolhido lê estes, e só estes.
pub const FONTES: [&str; 11] = [
    "WiiUp", "WiiDown", "WiiLeft", "WiiRight", "WiiA", "WiiB", "Wii1", "Wii2", "WiiPlus",
    "WiiMinus", "WiiHome",
];

impl EstadoWiimote {
    pub fn apertado(&self, nome: &str) -> bool {
        BOTOES
            .iter()
            .position(|(n, _)| *n == nome)
            .is_some_and(|i| self.botoes & (1 << i) != 0)
    }

    /// Se a origem é um botão deste controle e está apertada.
    pub fn fonte_acionada(&self, fonte: &Source) -> bool {
        match fonte {
            Source::Button { name } => FONTES
                .iter()
                .position(|n| n == name)
                .is_some_and(|i| self.botoes & (1 << i) != 0),
            _ => false,
        }
    }

    /// O primeiro botão apertado, para a tela de configuração capturar.
    pub fn primeira_fonte(&self) -> Option<Source> {
        (0..FONTES.len())
            .find(|&i| self.botoes & (1 << i) != 0)
            .map(|i| Source::button(FONTES[i]))
    }
}

/// O começo do nome de um Wii Remote na lista de controles: "Wii Remote 1", "Wii Remote 2".
const PREFIXO: &str = "Wii Remote ";

/// Os controles encontrados, pela ordem em que apareceram.
#[derive(Clone, Default)]
pub struct Wiimotes {
    estados: Arc<Mutex<Vec<(String, EstadoWiimote)>>>,
}

impl Wiimotes {
    /// Começa a procurar controles em segundo plano.
    pub fn inicia() -> Self {
        let wiimotes = Self::default();
        #[cfg(target_os = "linux")]
        {
            let estados = wiimotes.estados.clone();
            let _ = std::thread::Builder::new()
                .name("wiimote-busca".into())
                .spawn(move || linux::busca(estados));
        }
        wiimotes
    }

    /// Quantos controles estão conectados.
    pub fn quantos(&self) -> usize {
        self.estados.lock().map(|e| e.len()).unwrap_or(0)
    }

    /// O nome com que o `indice`-ésimo controle aparece na lista de controles.
    pub fn nome(indice: usize) -> String {
        format!("{PREFIXO}{}", indice + 1)
    }

    /// O índice de um nome dado por [`Wiimotes::nome`].
    pub fn indice_do_nome(nome: &str) -> Option<usize> {
        nome.strip_prefix(PREFIXO)?.parse::<usize>().ok()?.checked_sub(1)
    }

    /// O estado do `indice`-ésimo controle conectado.
    pub fn estado(&self, indice: usize) -> Option<EstadoWiimote> {
        self.estados.lock().ok()?.get(indice).map(|(_, estado)| *estado)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::collections::HashSet;
    use std::io::Read;
    use std::path::{Path, PathBuf};

    const EV_KEY: u16 = 1;
    const EV_ABS: u16 = 3;
    const ABS_RX: u16 = 3;
    const ABS_RY: u16 = 4;
    const ABS_RZ: u16 = 5;
    /// `struct input_event` em 64 bits: `timeval` (16), `type` (2), `code` (2), `value` (4).
    const TAMANHO_DO_EVENTO: usize = 24;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Parte {
        Botoes,
        Acelerometro,
    }

    /// Procura controles para sempre, abrindo uma leitura para cada dispositivo novo.
    pub(super) fn busca(estados: Arc<Mutex<Vec<(String, EstadoWiimote)>>>) {
        let abertos: Arc<Mutex<HashSet<PathBuf>>> = Default::default();
        loop {
            for (evento, controle, parte) in dispositivos() {
                let novo = abertos.lock().is_ok_and(|mut a| a.insert(evento.clone()));
                if !novo {
                    continue;
                }
                let (estados, abertos) = (estados.clone(), abertos.clone());
                let _ = std::thread::Builder::new()
                    .name("wiimote".into())
                    .spawn(move || {
                        le(&evento, &controle, parte, &estados);
                        if let Ok(mut a) = abertos.lock() {
                            a.remove(&evento);
                        }
                        // O controle some da lista quando os botões param: é o dispositivo que
                        // existe enquanto ele está pareado.
                        if parte == Parte::Botoes
                            && let Ok(mut e) = estados.lock()
                        {
                            e.retain(|(nome, _)| *nome != controle);
                        }
                    });
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    /// `(nó em /dev/input, identificador do controle, parte)` de cada dispositivo de Wii Remote.
    ///
    /// O identificador é o dispositivo HID pai: botões e acelerômetro do mesmo controle
    /// compartilham o mesmo, e é assim que os dois se juntam.
    fn dispositivos() -> Vec<(PathBuf, String, Parte)> {
        let Ok(entradas) = std::fs::read_dir("/sys/class/input") else {
            return Vec::new();
        };
        let mut achados: Vec<_> = entradas
            .flatten()
            .filter_map(|entrada| {
                let nome = entrada.file_name().into_string().ok()?;
                if !nome.starts_with("event") {
                    return None;
                }
                let base = entrada.path().join("device");
                let rotulo = std::fs::read_to_string(base.join("name")).ok()?;
                let parte = match rotulo.trim() {
                    "Nintendo Wii Remote" => Parte::Botoes,
                    "Nintendo Wii Remote Accelerometer" => Parte::Acelerometro,
                    _ => return None,
                };
                let controle = controle_de(&base)?;
                Some((Path::new("/dev/input").join(nome), controle, parte))
            })
            .collect();
        achados.sort_by(|a, b| a.0.cmp(&b.0));
        achados
    }

    fn controle_de(dispositivo: &Path) -> Option<String> {
        let real = std::fs::canonicalize(dispositivo).ok()?;
        // `.../uhid/0005:057E:0306.0018/input/input46` -> `0005:057E:0306.0018`
        Some(real.parent()?.parent()?.file_name()?.to_str()?.to_string())
    }

    fn le(
        evento: &Path,
        controle: &str,
        parte: Parte,
        estados: &Mutex<Vec<(String, EstadoWiimote)>>,
    ) {
        let Ok(mut arquivo) = std::fs::File::open(evento) else {
            return;
        };
        // O controle entra na lista ao abrir os botões, e não no primeiro aperto: parado, ele
        // não manda evento nenhum, e o acelerômetro não teria onde escrever.
        if parte == Parte::Botoes
            && let Ok(mut lista) = estados.lock()
            && !lista.iter().any(|(nome, _)| nome == controle)
        {
            lista.push((controle.to_string(), EstadoWiimote::default()));
        }
        let mut bruto = [0u8; TAMANHO_DO_EVENTO];
        let mut aceleracao = [0f32; 3];
        while arquivo.read_exact(&mut bruto).is_ok() {
            let tipo = u16::from_ne_bytes([bruto[16], bruto[17]]);
            let codigo = u16::from_ne_bytes([bruto[18], bruto[19]]);
            let valor = i32::from_ne_bytes([bruto[20], bruto[21], bruto[22], bruto[23]]);
            let Ok(mut lista) = estados.lock() else {
                return;
            };
            // Um acelerômetro sem o dispositivo dos botões é resto de um controle que está saindo.
            let Some(posicao) = lista.iter().position(|(nome, _)| nome == controle) else {
                continue;
            };
            let estado = &mut lista[posicao].1;
            match (tipo, parte) {
                (EV_KEY, Parte::Botoes) => {
                    if let Some(i) = BOTOES.iter().position(|(_, c)| *c == codigo) {
                        match valor {
                            0 => estado.botoes &= !(1 << i),
                            _ => estado.botoes |= 1 << i,
                        }
                    }
                }
                (EV_ABS, Parte::Acelerometro) => {
                    let eixo = match codigo {
                        ABS_RX => 0,
                        ABS_RY => 1,
                        ABS_RZ => 2,
                        _ => continue,
                    };
                    aceleracao[eixo] = valor as f32 / UNIDADES_POR_G;
                    estado.aceleracao = aceleracao;
                    estado.com_acelerometro = true;
                }
                _ => {}
            }
        }
    }
}
