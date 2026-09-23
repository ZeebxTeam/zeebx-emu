//! Zeebx — emulador de Zeebo / Qualcomm BREW.

// No Windows, a versão de distribuição abre sem a janela de console atrás da interface. O preço é
// a linha de comando (`zeebx run`, `zeebx sessao`) não escrever no terminal nessa versão; para
// ela, o build de desenvolvimento continua com console.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

// O binário consome o motor como biblioteca: nada aqui redeclara módulos, e `desktop` decide
// quais deles entram na compilação.
use zeebx::{audio, cpu, input, library, loader, machine, session, ui};

use std::process::ExitCode;

use zeebx::brew::aee;
use zeebx::cpu::{BackendPadrao, CpuBackend, dynarmic::DynarmicCpu};
use zeebx::input::bindings;
use zeebx::loader::archive;
use zeebx::loader::modfile::{ModImage, Variant};
use zeebx::machine::{AppletResult, Machine, Outcome};
use zeebx::ui::window;
use zeebx::video::icon;

/// Teto de instruções por fatia entre duas chamadas de API — evita que um laço infinito no
/// guest trave o emulador. Precisa ser generoso: a inicialização do Bejeweled Twist passa
/// dezenas de milhões de instruções sem chamar API nenhuma.
const INSTRUCTION_BUDGET: u64 = 500_000_000;

/// Quanto tempo do jogo rodar quando ninguém pede outra coisa.
const DEFAULT_SECONDS: u32 = 10;
/// Teto de voltas do laço de eventos — uma rede de segurança, não um limite de uso: quem
/// decide quando parar é o `--seconds` ou o fechar da janela.
const MAX_ROUNDS: u32 = 10_000_000;

/// De quanto em quanto tempo de relógio real a janela é atualizada, no mínimo.
const REFRESH_PERIOD: std::time::Duration = std::time::Duration::from_millis(8);

/// Taxa em que o `--dump-audio` grava. Não precisa ser a da placa: o que se quer é conferir o
/// que o jogo mandou tocar, e 44100 cobre a maior taxa que os jogos usam.
const RECORD_RATE: u32 = 44_100;

/// Teto de instruções registradas pelo `--code`, para não encher a memória do host.
const TRACE_STEPS: usize = 200_000;

/// Quantos blocos o `--profile` mostra.
const PROFILE_LINES: usize = 20;

/// Quantas linhas do log por semihosting o relatório mostra.
const SEMIHOSTING_LINES: usize = 40;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("info") if args.len() == 2 => report(info(&args[1])),
        Some("run") if args.len() >= 2 => {
            let tracing = args.iter().any(|a| a.starts_with("--trace"));
            let trace_filter = args
                .iter()
                .find_map(|a| a.strip_prefix("--trace="))
                .map(str::to_owned);
            let seconds = args
                .iter()
                .find_map(|a| a.strip_prefix("--seconds="))
                .and_then(|n| n.parse::<u32>().ok());
            let rounds = args
                .iter()
                .find_map(|a| a.strip_prefix("--frames="))
                .and_then(|n| n.parse().ok())
                .unwrap_or(MAX_ROUNDS);
            let watch = args
                .iter()
                .find_map(|a| a.strip_prefix("--watch="))
                .and_then(|n| u32::from_str_radix(n.trim_start_matches("0x"), 16).ok());
            // `--sonda=0xCLSID[,0xCLSID...]` atende classes desconhecidas com um objeto de
            // observação, em vez de recusá-las, e diz no fim o que o jogo chamou nele.
            let probe: Vec<u32> = args
                .iter()
                .filter_map(|a| a.strip_prefix("--sonda="))
                .flat_map(|lista| lista.split(','))
                .filter_map(|n| u32::from_str_radix(n.trim().trim_start_matches("0x"), 16).ok())
                .collect();
            // `--sonda-resposta=0xCLSID:SLOT=VALOR` combina o que um slot de sonda responde.
            // Sem isso não há como sair de um laço em que o jogo espera "acabou" — a sonda diz
            // sucesso para sempre e ele nunca sai.
            let probe_answers: Vec<(u32, u32, u32)> = args
                .iter()
                .filter_map(|a| a.strip_prefix("--sonda-resposta="))
                .filter_map(|spec| {
                    let (classe, resto) = spec.split_once(':')?;
                    let (slot, valor) = resto.split_once('=')?;
                    let numero = |t: &str| {
                        let t = t.trim();
                        match t.strip_prefix("0x") {
                            Some(hex) => u32::from_str_radix(hex, 16).ok(),
                            None => t.parse().ok(),
                        }
                    };
                    Some((numero(classe)?, numero(slot)?, numero(valor)?))
                })
                .collect();
            // `--serial=CAMINHO` grava o que sairia pela UART de depuração do console: cada
            // `DBGPRINTF` em ordem, com o instante do relógio virtual. O relatório agrupa
            // repetições; isto não agrupa, que é o que serve para analisar uma sequência.
            let serial = args
                .iter()
                .find_map(|a| a.strip_prefix("--serial="))
                .map(std::path::PathBuf::from);
            let dump_heap = args.iter().any(|a| a == "--dump-heap");
            let dump_surfaces = args
                .iter()
                .find_map(|a| a.strip_prefix("--dump-surfaces="))
                .map(str::to_owned);
            let dump_gl = args
                .iter()
                .find_map(|a| a.strip_prefix("--dump-gl="))
                .map(str::to_owned);
            let dump_audio = args
                .iter()
                .find_map(|a| a.strip_prefix("--dump-audio="))
                .map(str::to_owned);
            let window = args.iter().any(|a| a == "--window");
            let profile = args.iter().any(|a| a == "--profile");
            let wall = args
                .iter()
                .find_map(|a| a.strip_prefix("--wall="))
                .and_then(|n| n.parse::<u64>().ok());
            let keys = args
                .iter()
                .find_map(|a| a.strip_prefix("--keys="))
                .map(input::Script::parse)
                .transpose();
            let keys = match keys {
                Ok(keys) => keys.unwrap_or_default(),
                Err(err) => {
                    eprintln!("erro: {err}");
                    return ExitCode::FAILURE;
                }
            };
            // Com janela, o padrão é rodar até fecharem; sem ela, alguém precisa dizer
            // quando parar, e o padrão são poucos segundos de tempo do jogo.
            let seconds = match (seconds, window) {
                (limit @ Some(_), _) => limit,
                (None, true) => None,
                (None, false) => Some(DEFAULT_SECONDS),
            };
            let trace_range = args.iter().find_map(|a| {
                let spec = a.strip_prefix("--code=")?;
                let (begin, end) = spec.split_once(':')?;
                let parse = |t: &str| u32::from_str_radix(t.trim_start_matches("0x"), 16).ok();
                Some((parse(begin)?, parse(end)?))
            });
            report(run(
                &args[1],
                Options {
                    tracing,
                    trace_filter,
                    rounds,
                    seconds,
                    watch,
                    serial,
                    dump_heap,
                    dump_surfaces,
                    probe,
                    probe_answers,
                    window,
                    keys,
                    trace_range,
                    dump_gl,
                    dump_audio,
                    profile,
                    wall,
                    network: !args.iter().any(|a| a == "--sem-rede"),
                    network_to: args
                        .iter()
                        .find_map(|a| a.strip_prefix("--servidor="))
                        .map(str::to_owned),
                    bridge: args.iter().any(|a| a == "--ponte"),
                    teclas: args
                        .iter()
                        .find_map(|a| a.strip_prefix("--teclas="))
                        .map(teclado)
                        .unwrap_or_default(),
                    instalados: args
                        .iter()
                        .find_map(|a| a.strip_prefix("--instalados="))
                        .map(instalados)
                        .unwrap_or_default(),
                    portas: match args.iter().find_map(|a| a.strip_prefix("--portas=")) {
                        Some(lista) => match aparelhos(lista) {
                            Some(portas) => portas,
                            None => {
                                eprintln!(
                                    "erro: --portas espera nomes separados por vírgula, entre \
                                     'controle', 'teclado' e 'nenhum'"
                                );
                                return ExitCode::FAILURE;
                            }
                        },
                        None => zeebx::PORTAS_PADRAO,
                    },
                },
            ))
        }
        // O JIT entra primeiro como bancada, não como backend implícito da interface. Assim a
        // mesma ROM pode ser comparada com o Unicorn sem esconder uma regressão de compatibilidade.
        // O que o emulador enxerga de controle, para quando a entrada não responde e não dá
        // para saber se o problema é o aparelho, o nome salvo ou o mapeamento.
        Some("controles") => {
            let pads = input::gamepads::Gamepads::new();
            let nomes = pads.names();
            match nomes.is_empty() {
                true => println!("nenhum controle visto pelo sistema"),
                false => {
                    println!("controles vistos, na ordem do sistema:");
                    for (i, nome) in nomes.iter().enumerate() {
                        println!("  {i}: {nome}");
                    }
                }
            }
            // O sensor de movimento de cada controle: é o que alimenta o Boomerang da porta, e
            // um controle com sensor que não mexe o Boomerang costuma ser o nó sem permissão.
            let sensores = input::sensores::Sensores::inicia();
            std::thread::sleep(std::time::Duration::from_millis(1500));
            for (i, nome) in nomes.iter().enumerate() {
                let sensor =
                    pads.identidade(Some(nome), i)
                        .and_then(|(so, vendor, product, ordem)| {
                            sensores.do_controle(&so, vendor, product, ordem)
                        });
                match sensor {
                    Some(s) if s.sem_permissao => {
                        println!(
                            "  sensor de {nome}: {} SEM PERMISSÃO — a regra do udev:",
                            s.nome
                        );
                        println!("    {}", input::sensores::REGRA_DO_UDEV);
                    }
                    Some(s) if s.com_leitura => println!(
                        "  sensor de {nome}: {}, [{:+.2} {:+.2} {:+.2}] g",
                        s.nome, s.aceleracao[0], s.aceleracao[1], s.aceleracao[2]
                    ),
                    Some(s) => println!("  sensor de {nome}: {}, sem leitura ainda", s.nome),
                    None => {}
                }
            }
            let settings = ui::settings::Settings::load();
            for porta in 0..input::PORTAS {
                let Some(jogador) = settings.controls.player(porta) else {
                    continue;
                };
                let escolhido = jogador.device.as_deref();
                let achado = match escolhido {
                    Some(nome) => match nomes.iter().position(|n| n == nome) {
                        Some(i) => format!("posição {i}"),
                        None => "NÃO ESTÁ LIGADO".to_string(),
                    },
                    None => match nomes.len() > porta {
                        true => format!("posição {porta}, por ser a porta {}", porta + 1),
                        false => "nenhum nessa posição".to_string(),
                    },
                };
                println!(
                    "porta {}: {}, aparelho {:?}, escolhido {:?} -> {achado}",
                    porta + 1,
                    match jogador.ligada {
                        true => "ligada",
                        false => "desligada",
                    },
                    jogador.aparelho,
                    escolhido.unwrap_or("nenhum"),
                );
            }
            // Com `--ler`, fica um tempo mostrando o que chega: é a única forma de separar
            // "o controle não é visto" de "o controle é visto e o mapeamento não bate".
            if args.iter().any(|a| a == "--ler") {
                let mut pads = pads;
                println!("lendo por 15 segundos — aperte os botões");
                let fim = std::time::Instant::now() + std::time::Duration::from_secs(15);
                let mut antes: Vec<String> = vec![String::new(); input::PORTAS];
                let mut antes_cru: Vec<String> = vec![String::new(); nomes.len()];
                while std::time::Instant::now() < fim {
                    pads.poll();
                    // Cru, por controle: separa "o sistema não entrega nada" de "entrega e o
                    // mapeamento da porta não aproveita".
                    for (i, nome) in nomes.iter().enumerate() {
                        let bruto = match pads.first_active(Some(nome), i) {
                            Some(source) => format!("{source:?}"),
                            None => String::new(),
                        };
                        if bruto != antes_cru[i] {
                            match bruto.is_empty() {
                                true => println!("controle {i} ({nome}): nada"),
                                false => println!("controle {i} ({nome}): {bruto}"),
                            }
                            antes_cru[i] = bruto;
                        }
                    }
                    for porta in 0..input::PORTAS {
                        let Some(jogador) = settings.controls.player(porta) else {
                            continue;
                        };
                        let device = jogador.device.clone();
                        let pad = jogador.pad(
                            |source| pads.is_active(device.as_deref(), porta, source),
                            |axis| pads.value(device.as_deref(), porta, axis),
                        );
                        let apertados: Vec<&str> = input::BUTTON_NAMES
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| pad.is_down(*i))
                            .map(|(_, nome)| *nome)
                            .collect();
                        let agora = apertados.join(" ");
                        if agora != antes[porta] {
                            match agora.is_empty() {
                                true => println!("porta {}: nada", porta + 1),
                                false => println!("porta {}: {agora}", porta + 1),
                            }
                            antes[porta] = agora;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
            }
            ExitCode::SUCCESS
        }
        Some("bench") if args.len() >= 2 => {
            let seconds = args
                .iter()
                .find_map(|a| a.strip_prefix("--seconds="))
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(DEFAULT_SECONDS);
            let dump = args.iter().find_map(|a| a.strip_prefix("--dump="));
            let keys = match args
                .iter()
                .find_map(|a| a.strip_prefix("--keys="))
                .map(input::Script::parse)
                .transpose()
            {
                Ok(keys) => keys.unwrap_or_default(),
                Err(err) => {
                    eprintln!("erro: {err}");
                    return ExitCode::FAILURE;
                }
            };
            let teclas = args
                .iter()
                .find_map(|a| a.strip_prefix("--teclas="))
                .map(teclado)
                .unwrap_or_default();
            let instalados = args
                .iter()
                .find_map(|a| a.strip_prefix("--instalados="))
                .map(instalados)
                .unwrap_or_default();
            let superficies = args.iter().find_map(|a| a.strip_prefix("--dump-surfaces="));
            report(bench_dynarmic(
                &args[1],
                seconds,
                dump,
                &keys,
                &teclas,
                instalados,
                superficies,
            ))
        }
        // Mostra o Wii Remote ao vivo: botões e aceleração, para conferir a leitura sem janela.
        Some("wiimote") => {
            let segundos: u64 = args
                .iter()
                .find_map(|a| a.strip_prefix("--seconds="))
                .and_then(|s| s.parse().ok())
                .unwrap_or(10);
            let wiimotes = input::wiimote::Wiimotes::inicia();
            let fim = std::time::Instant::now() + std::time::Duration::from_secs(segundos);
            while std::time::Instant::now() < fim {
                std::thread::sleep(std::time::Duration::from_millis(250));
                match wiimotes.estado(0) {
                    Some(e) => {
                        let apertados: Vec<&str> = input::wiimote::BOTOES
                            .iter()
                            .map(|(nome, _)| *nome)
                            .filter(|nome| e.apertado(nome))
                            .collect();
                        let [x, y, z] = e.aceleracao;
                        println!(
                            "x {x:+.2} y {y:+.2} z {z:+.2} g  acelerômetro: {}  botões: {apertados:?}",
                            if e.com_acelerometro {
                                "sim"
                            } else {
                                "ainda não"
                            }
                        );
                    }
                    None => println!("nenhum Wii Remote"),
                }
            }
            ExitCode::SUCCESS
        }
        Some("sessao") if args.len() >= 2 => {
            let seconds = args
                .iter()
                .find_map(|a| a.strip_prefix("--seconds="))
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(DEFAULT_SECONDS);
            let dump = args.iter().find_map(|a| a.strip_prefix("--dump="));
            let keys = match args
                .iter()
                .find_map(|a| a.strip_prefix("--keys="))
                .map(input::Script::parse)
                .transpose()
            {
                Ok(keys) => keys.unwrap_or_default(),
                Err(err) => {
                    eprintln!("erro: {err}");
                    return ExitCode::FAILURE;
                }
            };
            let fotos: Vec<u32> = args
                .iter()
                .find_map(|a| a.strip_prefix("--fotos="))
                .map(|lista| {
                    lista
                        .split(',')
                        .filter_map(|t| t.trim().parse().ok())
                        .collect()
                })
                .unwrap_or_default();
            let placa = args.iter().any(|a| a == "--placa");
            let serial = args.iter().find_map(|a| a.strip_prefix("--serial="));
            // Sem nada, valem as preferências da Z-Wheel gravadas; `--fabrica` usa a cfg do pacote
            // como veio, e os outros dois trocam uma opção só.
            let mut z_wheel = match args.iter().any(|a| a == "--fabrica") {
                true => ui::settings::ZWheel {
                    fim_de_vida: true,
                    transicoes_sempre: false,
                },
                false => ui::settings::Settings::load().z_wheel,
            };
            if args.iter().any(|a| a == "--sem-fim-de-vida") {
                z_wheel.fim_de_vida = false;
            }
            if args.iter().any(|a| a == "--sem-transicoes") {
                z_wheel.transicoes_sempre = false;
            }
            let escala = args
                .iter()
                .find_map(|a| a.strip_prefix("--escala="))
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(1);
            let numero = |prefixo: &str| {
                args.iter()
                    .find_map(|a| a.strip_prefix(prefixo))
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(1)
            };
            let melhorias = (numero("--msaa="), numero("--aniso="));
            // `--perfil` mede tudo; `--perfil=MS` só a partir desse instante virtual.
            let perfil = args.iter().find_map(|a| match a.as_str() {
                "--perfil" => Some(0),
                outro => outro
                    .strip_prefix("--perfil=")
                    .and_then(|n| n.parse::<u32>().ok()),
            });
            // `--boomerang` põe um Boomerang na porta um; `--movimento=ms:x:y:z,...` diz a
            // aceleração dele a partir de cada instante, em g.
            let movimento: Vec<(u32, [f32; 3])> = args
                .iter()
                .find_map(|a| a.strip_prefix("--movimento="))
                .map(|lista| {
                    lista
                        .split(',')
                        .filter_map(|item| {
                            let partes: Vec<&str> = item.split(':').collect();
                            let [quando, x, y, z] = partes.as_slice() else {
                                return None;
                            };
                            Some((
                                quando.parse().ok()?,
                                [x.parse().ok()?, y.parse().ok()?, z.parse().ok()?],
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let boomerang = (args.iter().any(|a| a == "--boomerang") || !movimento.is_empty())
                .then_some(movimento);
            // As portas, para testar o que só aparece com dois jogadores. Sem a opção fica o
            // padrão de sempre: um controle na primeira e a segunda livre.
            let portas = match args.iter().find_map(|a| a.strip_prefix("--portas=")) {
                Some(lista) => match aparelhos(lista) {
                    Some(portas) => Some(portas),
                    None => {
                        eprintln!(
                            "erro: --portas espera nomes separados por vírgula, entre \
                             'controle', 'zpad', 'teclado', 'boomerang' e 'nenhum'"
                        );
                        return ExitCode::FAILURE;
                    }
                },
                None => None,
            };
            report(sessao_sem_janela(
                &args[1], seconds, dump, &keys, &fotos, placa, serial, z_wheel, escala, melhorias,
                perfil, boomerang, portas,
            ))
        }
        // Sem argumento nenhum, o que se quer é o emulador, não a ajuda.
        None => launch(),
        _ => {
            eprintln!("uso: zeebx            abre a interface");
            eprintln!("     zeebx info <arquivo.mod>");
            eprintln!(
                "     zeebx run <arquivo.mod> [--window] [--seconds=N] [--keys=ms:tecla,...]
                             [--dump-gl=DIR] [--dump-audio=ARQUIVO.wav]
                             [--trace[=trecho]] [--watch=0xADDR] [--serial=CAMINHO] [--dump-heap]
                             [--dump-surfaces=DIR]
                             [--code=0xINI:0xFIM] [--frames=N]
                             [--profile] [--wall=SEGUNDOS] [--sonda=0xCLSID,...]
                             [--sem-rede] [--servidor=MAQUINA[:PORTA]] [--ponte]
                             [--portas=controle|teclado|nenhum,...] [--teclas=ms:nome,...]"
            );
            eprintln!(
                "     zeebx sessao <arquivo.zip> [--seconds=N] [--keys=ms:botão,...] [--dump=QUADRO.bmp] [--fotos=ms,...] [--placa] [--serial=CAMINHO] [--fabrica] [--sem-fim-de-vida] [--sem-transicoes] [--escala=N] [--msaa=N] [--aniso=N] [--perfil[=MS]] [--boomerang] [--movimento=ms:x:y:z,...] [--wiimote] [--proporcao=16:9] [--portas=controle,controle]  (a sessão da janela, sem janela)"
            );
            eprintln!(
                "     zeebx bench <arquivo.mod|zip> [--seconds=N] [--keys=ms:tecla,...] [--dump=QUADRO.bmp] [--teclas=ms:nome,...] [--instalados=0xCLSID[:id],...] [--dump-surfaces=DIR]  (Dynarmic, sem janela)"
            );
            ExitCode::FAILURE
        }
    }
}

/// Abre a interface.
///
/// É o caminho normal de uso; a linha de comando continua existindo para depuração, que é
/// onde ela é insubstituível — despejar quadros, rastrear chamadas, olhar a memória.
/// A logo do emulador, para o ícone da janela.
const LOGO: &[u8] = include_bytes!("../../../assets/zeebx.png");

/// O maior lado do ícone. A logo tem mais de mil pixels de lado, e um ícone desse tamanho é
/// megabytes de textura para desenhar algo que nunca passa de alguns pixels na barra.
const ICON_SIDE: usize = 256;

/// Identificador da janela para o desktop. Casa com `assets/zeebx.desktop` — se um mudar, o
/// outro muda junto, senão o ícone some no Wayland.
const APP_ID: &str = "zeebx";

/// A logo no tamanho de ícone. `None` se ela não abrir — uma janela sem ícone é melhor que uma
/// janela que não abre.
fn window_icon() -> Option<zeebx::eframe::egui::IconData> {
    let logo = icon::decode(LOGO)
        .inspect_err(|err| eprintln!("ícone da janela: {err}"))
        .ok()?
        .downscaled(ICON_SIDE);
    Some(zeebx::eframe::egui::IconData {
        width: logo.width as u32,
        height: logo.height as u32,
        rgba: logo.rgba,
    })
}

fn launch() -> ExitCode {
    let mut viewport = zeebx::eframe::egui::ViewportBuilder::default()
        .with_inner_size([960.0, 720.0])
        .with_min_inner_size([480.0, 360.0])
        .with_title("Zeebx")
        // No Wayland não existe ícone em pixels: o compositor casa este `app_id` com o
        // `zeebx.desktop` instalado e tira o ícone de lá. Sem ele, a janela fica com o
        // genérico do sistema. Precisa ser igual ao nome do arquivo `.desktop`.
        .with_app_id(APP_ID);
    viewport = ui::settings::Settings::load()
        .graphics
        .janela
        .no_construtor(viewport);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = zeebx::eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let app = |context: &zeebx::eframe::CreationContext<'_>| {
        Ok(Box::new(ui::App::new(context)) as Box<dyn zeebx::eframe::App>)
    };
    match zeebx::eframe::run_native("Zeebx", options, Box::new(app)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("erro: não deu para abrir a janela: {err}");
            ExitCode::FAILURE
        }
    }
}

fn report(result: Result<(), Box<dyn std::error::Error>>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("erro: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Carrega o módulo e executa a partir de `AEEMod_Load` até um desfecho.
/// As opções de linha de comando do `run`, reunidas porque quase toda função da execução
/// precisa de um punhado delas.
struct Options {
    tracing: bool,
    trace_filter: Option<String>,
    rounds: u32,
    seconds: Option<u32>,
    watch: Option<u32>,
    /// Caminho da captura de serial, com `--serial=CAMINHO`.
    serial: Option<std::path::PathBuf>,
    dump_heap: bool,
    /// Onde gravar um BMP por superfície viva, com `--dump-surfaces=DIR`.
    dump_surfaces: Option<String>,
    probe: Vec<u32>,
    probe_answers: Vec<(u32, u32, u32)>,
    window: bool,
    keys: input::Script,
    trace_range: Option<(u32, u32)>,
    dump_gl: Option<String>,
    dump_audio: Option<String>,
    profile: bool,
    wall: Option<u64>,
    /// Se o jogo pode falar com a rede. Ligada por padrão; o `--sem-rede` desliga.
    network: bool,
    /// Para onde desviar as conexões, com `--servidor=MAQUINA[:PORTA]`.
    network_to: Option<String>,
    /// Se a ponte do módulo entrega a resposta ao jogo, com `--ponte`.
    bridge: bool,
    /// O que o console vê em cada porta, com `--portas=controle,teclado`.
    portas: [Option<bindings::Aparelho>; input::PORTAS],
    /// Teclas a entregar, com `--teclas=ms:nome[,...]`.
    teclas: Vec<(u32, u32)>,
    /// Classes que o shell trata como instaladas, com `--instalados=0xCLSID[:id][,...]` — o id é
    /// a pasta do módulo, que o `EnumNextApplet` entrega. Na janela
    /// quem diz é a biblioteca; sem ela, é isto que deixa testar o lançamento de um jogo pela
    /// Z-Wheel.
    instalados: Vec<(u32, String)>,
}

/// Lê `1000:select,2000:down` e devolve `(instante em ms, código AVK)`.
///
/// Os nomes são os de [`input::avk::por_nome`]: `up`, `down`, `left`, `right`, `select`, `clr`,
/// `star`, `pound` e os dígitos. O que não for reconhecido é descartado com aviso, porque um
/// roteiro com uma tecla errada ainda vale pelas outras.
/// Lê `0xCLSID[:id][,...]`. Sem id, o id do módulo é a própria classe em hexadecimal.
fn instalados(lista: &str) -> Vec<(u32, String)> {
    lista
        .split(',')
        .filter_map(|item| {
            let (classe, id) = item.split_once(':').unwrap_or((item, ""));
            let classe = u32::from_str_radix(classe.trim().trim_start_matches("0x"), 16).ok()?;
            let id = match id.is_empty() {
                true => format!("{classe:x}"),
                false => id.to_string(),
            };
            Some((classe, id))
        })
        .collect()
}

/// `--proporcao=16:9`: a proporção larga experimental do 3D, largura sobre altura.
fn proporcao_da_linha() -> Option<f32> {
    std::env::args()
        .find_map(|a| a.strip_prefix("--proporcao=").map(str::to_string))
        .and_then(|p| {
            let (largura, altura) = p.split_once(':')?;
            Some(largura.parse::<f32>().ok()? / altura.parse::<f32>().ok()?)
        })
}

fn teclado(lista: &str) -> Vec<(u32, u32)> {
    lista
        .split(',')
        .filter(|item| !item.is_empty())
        .filter_map(|item| {
            let (quando, nome) = item.split_once(':')?;
            let quando = quando.parse().ok()?;
            match input::avk::por_nome(nome.trim()) {
                Some(avk) => Some((quando, avk)),
                None => {
                    eprintln!("aviso: tecla desconhecida {nome:?}");
                    None
                }
            }
        })
        .collect()
}

/// Lê `controle,teclado` e afins. `None` quando algum nome não existe.
///
/// Existe para que as duas portas sejam **testáveis sem janela**, que é como tudo aqui se
/// verifica: sem isso, a única forma de saber se um jogo enxerga o segundo controle seria abrir
/// a interface e olhar.
fn aparelhos(lista: &str) -> Option<[Option<bindings::Aparelho>; input::PORTAS]> {
    let mut portas = [None; input::PORTAS];
    for (n, nome) in lista.split(',').enumerate() {
        let porta = portas.get_mut(n)?;
        *porta = match nome.trim() {
            "controle" | "pad" | "dragon" => Some(bindings::Aparelho::Controle),
            "zpad" | "z-pad" => Some(bindings::Aparelho::ZPad),
            "teclado" | "keyboard" => Some(bindings::Aparelho::Teclado),
            "boomerang" => Some(bindings::Aparelho::Boomerang),
            "nenhum" | "none" | "" => None,
            _ => return None,
        };
    }
    Some(portas)
}

fn run(path: &str, options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let Options {
        tracing,
        trace_filter,
        rounds,
        seconds,
        watch,
        serial,
        dump_heap,
        dump_surfaces,
        probe,
        probe_answers,
        window,
        keys,
        trace_range,
        dump_gl,
        dump_audio,
        profile,
        wall,
        network,
        network_to,
        bridge,
        portas,
        teclas,
        instalados,
    } = options;
    // Um jogo em `.zip` ou `.7z` é extraído para o cache e rodado de lá, como na interface.
    let extracted;
    let path = match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some("zip" | "7z") => {
            extracted = archive::extract(std::path::Path::new(path))?;
            println!("extraído:  {}", extracted.display());
            extracted.to_str().unwrap_or(path)
        }
        _ => path,
    };
    let image = ModImage::parse(std::fs::read(path)?)?;
    let extensoes = zeebx::session::extensoes_de(std::path::Path::new(path));
    for extensao in &extensoes {
        println!(
            "extensão:  fornece {}",
            extensao
                .classes
                .iter()
                .map(|c| format!("{c:#010x}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    let module = loader::load_with(&image, &extensoes)?;

    println!("carregado: {path}");
    println!("entry:     {:#010x}", module.entry);
    for region in module.mem.regions() {
        println!(
            "  {:<8} {:#010x} .. {:#010x} {}",
            region.name,
            region.base,
            region.base as u64 + region.bytes.len() as u64,
            if region.writable { "rw" } else { "ro" }
        );
    }

    // A raiz do sistema de arquivos do jogo é o diretório onde o `.mod` está: é lá que o
    // console guarda os arquivos do título.
    let root = std::path::Path::new(path)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let mut machine = Machine::new(BackendPadrao::new()?, module, root);
    println!("arquivos:  {}", machine.file_root().display());
    machine.set_tracing(tracing);
    machine.set_trace_filter(trace_filter);
    // Rastreio de código: mostra o caminho que a execução realmente tomou numa faixa.
    if let Some((begin, end)) = trace_range {
        machine.cpu_mut().trace_code(begin, end, TRACE_STEPS)?;
    }
    // Watchpoint de depuração: registra toda escrita na palavra pedida, com o PC de origem.
    if !probe.is_empty() {
        machine.probe_classes(&probe);
    }
    for (classe, slot, valor) in &probe_answers {
        machine.probe_answer(*classe, *slot, *valor);
    }
    machine.set_portas(portas);
    machine.set_installed_applets(instalados);
    machine.set_network(network);
    if network_to.is_some() {
        machine.set_network_to(network_to);
    }
    if bridge {
        machine.set_bridge(true);
    }
    if profile {
        machine.cpu_mut().enable_profile();
        machine.enable_api_profile();
    }
    if let Some(segundos) = wall {
        machine
            .cpu_mut()
            .set_wall_limit(std::time::Duration::from_secs(segundos));
    }
    if let Some(caminho) = &serial {
        machine.liga_serial(caminho)?;
        println!("serial:    {}", caminho.display());
    }
    if let Some(addr) = watch {
        // Faixa generosa de propósito: um `stm`/`strd` dispara o hook com o endereço inicial
        // do bloco, então vigiar só a palavra perde a escrita que a cobre por dentro.
        machine.cpu_mut().watch(addr.saturating_sub(64), 128)?;
    }
    let outcome = machine.run(INSTRUCTION_BUDGET)?;

    print!("parou:     ");
    describe_outcome(&outcome);
    if let Outcome::Returned { .. } = outcome {
        let out = machine.module().out_module;
        println!(
            "           IModule* = {:#010x}",
            machine.cpu().read_u32(out)?
        );
    }

    // Com o módulo carregado, o próximo passo é instanciar o applet. O ClassID vem do `.mif`
    // que acompanha o módulo.
    let mut rodou_quadros = false;
    if let Outcome::Returned { code: 0 } = outcome {
        match library::applet_clsid(std::path::Path::new(path)) {
            Some(clsid) => {
                println!("applet:    ClassID {clsid:#010x} (do .mif)");
                match machine.create_applet(clsid, INSTRUCTION_BUDGET)? {
                    AppletResult::Called { code, applet } => {
                        println!(
                            "           CreateInstance retornou {code}, IApplet* = {applet:#010x}"
                        );
                        if code == 0 && applet != 0 {
                            print!("start:     EVT_APP_START → ");
                            let started =
                                machine.start_applet(applet, clsid, INSTRUCTION_BUDGET)?;
                            describe_outcome_com_estado(&started, &machine);
                            // O código de retorno do `HandleEvent` pertence ao applet, não ao
                            // despachante: Kingdom Hearts devolve 1 no EVT_APP_START e segue
                            // armando o laço normalmente. A Session já aceita qualquer retorno;
                            // a linha de comando precisa fazer o mesmo, senão ela nunca chega
                            // ao trecho que permite perfilar a intro.
                            if matches!(started, Outcome::Returned { .. } | Outcome::Budget) {
                                rodou_quadros = true;
                                run_frames(
                                    &mut machine,
                                    rounds,
                                    seconds,
                                    path,
                                    dump_heap,
                                    window,
                                    &keys,
                                    &teclas,
                                    dump_gl.as_deref(),
                                    dump_audio.as_deref(),
                                    profile,
                                )?;
                            }
                        }
                    }
                    AppletResult::NoModule => println!("           sem IModule* para chamar"),
                    AppletResult::Stopped(stop) => {
                        print!("           parou em ");
                        describe_outcome(&stop);
                    }
                }
            }
            None => println!("applet:    nenhum .mif encontrado ao lado do módulo"),
        }
    }
    // Sem laço de quadros não houve quem imprimisse o perfil nem despejasse a memória, e é o
    // caso em que os dois mais servem: o jogo gastou o orçamento antes de existir.
    if profile && !rodou_quadros {
        mostra_perfil(&machine);
    }
    if dump_heap && !rodou_quadros {
        despeja_memoria(&machine)?;
    }
    #[cfg(debug_assertions)]
    {
        let (total, maior, repetidos) = machine.retrato_widgets();
        println!(
            "widgets:   {total} vivos, maior lista de anexos {maior}, anexos repetidos {repetidos}"
        );
    }
    if let Some(dir) = &dump_surfaces {
        despeja_superficies(&machine, dir)?;
    }

    if trace_range.is_some() {
        let steps = machine.cpu().steps();
        println!("código:    {} instrução(ões) na faixa", steps.len());
        for (address, r0, lr) in steps.iter() {
            println!("  {address:#010x}  r0={r0:#x} lr={lr:#x}");
        }
    }
    if watch.is_some() {
        let writes = machine.cpu().writes();
        let current = machine
            .dump(watch.unwrap_or(0), 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .unwrap_or(0);
        println!(
            "watch:     {} acesso(s), valor atual {current:#x}",
            writes.len()
        );
        // Todas, não as últimas N: cortar a lista esconde justamente as escritas do
        // construtor, que são as primeiras — e foi assim que uma investigação concluiu que um
        // campo "nunca era escrito" quando ele era, logo no começo.
        for w in writes.iter() {
            // Um valor negativo marca uma leitura, e o módulo é o tamanho lida.
            if w.value < 0 {
                println!(
                    "  {:#010x} lido ({} bytes) (pc {:#010x}, lr {:#010x})",
                    w.addr, -w.value, w.pc, w.lr
                );
            } else {
                println!(
                    "  {:#010x} = {:#x} (pc {:#010x}, lr {:#010x})",
                    w.addr, w.value, w.pc, w.lr
                );
            }
        }
    }
    println!(
        "heap:      {} bytes em uso, {} objetos vivos",
        machine.heap_used(),
        machine.live_objects()
    );
    if !machine.suspicious_objects().is_empty() {
        println!(
            "atenção:   {} chamadas com ponteiro `this` inesperado",
            machine.suspicious_objects().len()
        );
    }
    let toques = machine.pad_log();
    if !toques.is_empty() {
        println!("toques:    {} entregue(s) ao jogo", toques.len());
        for (ms, porta, nome, down) in &toques {
            let acao = match down {
                true => "aperta",
                false => "solta ",
            };
            println!("  {ms:>7} ms  porta {}  {acao} {nome}", porta + 1);
        }
    }
    if let Some(classe) = machine.take_launch_request() {
        println!("lançar:    o shell pediu para abrir {classe:#010x}");
    }
    let midia = machine.media_log();
    if !midia.is_empty() {
        println!(
            "som:       {} linha(s) do que o jogo fez com a mídia",
            midia.len()
        );
        for (ms, objeto, chamada, vezes) in &midia {
            let repete = match vezes {
                1 => String::new(),
                n => format!("  ({n}x)"),
            };
            println!("  {ms:>7} ms  {objeto:#010x}  {chamada}{repete}");
        }
    }
    let urls = machine.web_requests();
    if !urls.is_empty() {
        println!(
            "rede:      o jogo pediu {} endereço(s) pelo IWeb",
            urls.len()
        );
        for url in &urls {
            println!("  {url}");
        }
    }
    let resposta = machine.web_response();
    if !resposta.is_empty() {
        let texto = String::from_utf8_lossy(resposta);
        println!("  resposta: {} bytes, {texto:?}", resposta.len());
    }
    let ignoradas = machine.ignored_gl();
    if !ignoradas.is_empty() {
        println!("gl:        atendidas sem fazer nada ({})", ignoradas.len());
        println!("  {}", ignoradas.join(" "));
    }
    let entregues = machine.delivered();
    if !entregues.is_empty() {
        println!("ponte:     o que ela fez com a resposta");
        for linha in entregues {
            println!("  {linha}");
        }
    }
    let claros = machine.plaintexts();
    if !claros.is_empty() {
        println!("cifrado:   o que o jogo cifrou, em claro");
        for bloco in &claros {
            let hex: String = bloco.iter().map(|b| format!("{b:02x}")).collect();
            println!("  {} bytes  {hex}", bloco.len());
            println!("    {:?}", String::from_utf8_lossy(bloco));
        }
    }
    let chaves = machine.cipher_keys();
    if !chaves.is_empty() {
        println!("cifra:     o jogo cifrou dados com");
        for chave in &chaves {
            println!("  {chave}");
        }
    }
    if let Some(fonte) = machine.font_source() {
        println!("fonte:     {fonte}, do próprio pacote do jogo");
    }
    if !machine.pending_text().is_empty() {
        println!("texto na tela (ainda sem fonte para desenhar):");
        for text in machine.pending_text() {
            println!("  {text:?}");
        }
    }
    if machine.screen().is_dirty() {
        let out = std::path::Path::new(path).with_extension("bmp");
        let out = out.file_name().map(std::path::Path::new).unwrap_or(&out);
        std::fs::write(out, machine.screen().to_bmp())?;
        println!(
            "tela:      {} ({}x{}, {} superfícies)",
            out.display(),
            machine.screen().width(),
            machine.screen().height(),
            machine.bitmap_count()
        );
    }
    if let Some(frame) = machine.gl_frame() {
        let out = std::path::Path::new(path).with_extension("gl.bmp");
        let out = out.file_name().map(std::path::Path::new).unwrap_or(&out);
        std::fs::write(out, frame.to_bmp())?;
        println!(
            "opengl:    {} ({} quadros apresentados)",
            out.display(),
            machine.gl_swaps()
        );
    }
    // O semihosting do ARM é o outro caminho de log que os jogos usam: o `SVC #0xAB` que o
    // Peggle e o Zuma's Revenge emitem sai por aqui, e não pelo `DBGPRINTF`.
    let semihosting = machine.cpu().semihosting();
    if !semihosting.trim().is_empty() {
        println!("log por semihosting:");
        for linha in semihosting.lines().take(SEMIHOSTING_LINES) {
            println!("  {linha}");
        }
    }
    if !machine.debug_output().is_empty() {
        println!("log do jogo:");
        for (line, count) in machine.debug_output() {
            match count {
                1 => println!("  {line}"),
                n => println!("  {line}   ({n}x)"),
            }
        }
    }
    // Entrega o que estiver na fila de sinais: o jogo se registra para eventos e espera os
    // callbacks. Sem entrada do host ainda, a fila costuma estar vazia.
    let delivered = machine.deliver_signals(INSTRUCTION_BUDGET)?;
    if !delivered.is_empty() {
        println!("sinais:    {} callback(s) entregue(s)", delivered.len());
    }
    let played = machine.deliver_callbacks(INSTRUCTION_BUDGET)?;
    if !played.is_empty() {
        println!("callbacks: {} entregue(s)", played.len());
    }
    // A região de objetos tem teto, e um jogo que chega perto dele está vazando referência —
    // o sintoma disso é uma classe qualquer parando de ser criada, que não se parece com a
    // causa. A lista só aparece quando há muito objeto vivo.
    let vivos = machine.live_objects_by_kind();
    let total: usize = vivos.iter().map(|(_, n)| n).sum();
    if total >= 256 {
        println!("objetos vivos por interface ({total} ao todo):");
        for (nome, quantos) in vivos.iter().take(8) {
            println!("  {quantos:6}x {nome}");
        }
    }
    if !machine.swallowed_faults().is_empty() {
        println!("acessos inválidos que o jogo seguiu por cima:");
        for nota in machine.swallowed_faults() {
            println!("  {nota}");
        }
    }
    if !machine.missing_apis().is_empty() {
        println!("APIs que faltaram:");
        for nota in machine.missing_apis() {
            println!("  {nota}");
        }
    }
    if !machine.missing_files().is_empty() {
        println!("arquivos não encontrados:");
        for name in machine.missing_files() {
            println!("  {name}");
        }
    }
    if !machine.bad_pointers().is_empty() {
        println!("ponteiros inválidos recebidos:");
        for line in machine.bad_pointers() {
            println!("  {line}");
        }
    }
    if !machine.assumptions().is_empty() {
        println!("hipóteses em uso:");
        for note in machine.assumptions() {
            println!("  {note}");
        }
    }
    if tracing {
        println!("rastreamento:");
        for line in machine.trace() {
            println!("  {line}");
        }
    }
    let unknown = machine.unknown_classes();
    if !unknown.is_empty() {
        print!("classes desconhecidas:");
        for clsid in unknown {
            print!(" {clsid:#010x}");
        }
        println!();
    }
    let sonda = machine.probe_log();
    if !sonda.is_empty() {
        println!("sonda:     o que os jogos chamaram nas classes atendidas por observação");
        let mut objeto_atual = 0;
        for (clsid, objeto, slot, args, textos, vezes) in sonda {
            if *objeto != objeto_atual {
                objeto_atual = *objeto;
                println!("  classe {clsid:#010x}, objeto {objeto:#x}:");
            }
            let mostrar = |i: usize| match &textos[i] {
                Some(texto) => format!("{texto:?}"),
                None => format!("{:#x}", args[i]),
            };
            let repetido = match vezes {
                1 => String::new(),
                n => format!("   ({n}x)"),
            };
            println!(
                "    slot[{slot:>2}] ({}, {}, {}){repetido}",
                mostrar(1),
                mostrar(2),
                mostrar(3)
            );
        }
    }
    let log = machine.call_log();
    if !log.is_empty() {
        println!("chamadas:");
        for (name, count) in log {
            println!("  {count:>4}x {name}");
        }
    }
    Ok(())
}

/// Mede uma ROM inteira no Dynarmic, sem janela e sem trocar o backend normal do emulador.
///
/// Esta não é uma segunda implementação do comando `run`: é uma bancada estreita para a
/// pergunta que motivou o JIT — quantos milissegundos virtuais o ARM recompilado consegue
/// entregar por segundo de parede? Quando os números e os quadros concordarem com o Unicorn,
/// o backend poderá subir para a sessão e a interface.
fn bench_dynarmic(
    path: &str,
    seconds: u32,
    dump: Option<&str>,
    keys: &input::Script,
    teclas: &[(u32, u32)],
    instalados: Vec<(u32, String)>,
    superficies: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let extracted;
    let path = match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some("zip" | "7z") => {
            extracted = archive::extract(std::path::Path::new(path))?;
            extracted.as_path()
        }
        _ => std::path::Path::new(path),
    };
    let image = ModImage::parse(std::fs::read(path)?)?;
    let extensoes = zeebx::session::extensoes_de(path);
    let module = loader::load_with(&image, &extensoes)?;
    let root = path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let mut machine = Machine::new(DynarmicCpu::new()?, module, root);
    machine.set_installed_applets(instalados);
    let boot = machine.run(INSTRUCTION_BUDGET)?;
    if !matches!(boot, Outcome::Returned { code: 0 }) {
        return Err(format!("carga parou em {boot:?}").into());
    }
    let clsid = library::applet_clsid(path).ok_or("nenhum .mif encontrou o applet")?;
    let applet = match machine.create_applet(clsid, INSTRUCTION_BUDGET)? {
        AppletResult::Called { code: 0, applet } if applet != 0 => applet,
        other => return Err(format!("CreateInstance parou em {other:?}").into()),
    };
    let start = machine.start_applet(applet, clsid, INSTRUCTION_BUDGET)?;
    if !matches!(start, Outcome::Returned { .. } | Outcome::Budget) {
        return Err(format!("EVT_APP_START parou em {start:?}").into());
    }

    let wall = std::time::Instant::now();
    let base_clock = machine.clock_ms();
    let base_instructions = machine.instructions();
    let until = base_clock.saturating_add(seconds.saturating_mul(1000));
    let mut turns = 0u64;
    let mut pad = input::Pad::default();
    // Cada tecla vira aperto e soltura quando o relógio passa do instante. Com `--dump`, o quadro
    // de meio segundo depois de cada uma também é gravado, numerado: é o que mostra a navegação
    // passo a passo, e não só onde ela terminou.
    let mut pendentes: std::collections::VecDeque<(u32, u32)> = {
        let mut ordenadas = teclas.to_vec();
        ordenadas.sort_by_key(|&(quando, _)| quando);
        ordenadas.into()
    };
    let mut fotos: std::collections::VecDeque<u32> = std::collections::VecDeque::new();
    let mut numero_da_foto = 0;
    while machine.clock_ms() < until && !machine.is_idle() {
        while pendentes
            .front()
            .is_some_and(|&(quando, _)| machine.clock_ms() >= quando)
        {
            let (quando, avk) = pendentes.pop_front().unwrap_or_default();
            machine.set_key(avk, true);
            machine.set_key(avk, false);
            fotos.push_back(quando.saturating_add(500));
        }
        if let Some(path) = dump {
            while fotos
                .front()
                .is_some_and(|&quando| machine.clock_ms() >= quando)
            {
                fotos.pop_front();
                numero_da_foto += 1;
                let nome = match path.strip_suffix(".bmp") {
                    Some(base) => format!("{base}.{numero_da_foto}.bmp"),
                    None => format!("{path}.{numero_da_foto}"),
                };
                std::fs::write(nome, machine.screen().to_bmp())?;
            }
        }
        keys.apply(machine.clock_ms(), &mut pad);
        machine.set_pad(pad);
        let outcomes = machine.advance(INSTRUCTION_BUDGET)?;
        machine.deliver_signals(INSTRUCTION_BUDGET)?;
        machine.deliver_callbacks(INSTRUCTION_BUDGET)?;
        turns += 1;
        if let Some(bad) = outcomes
            .iter()
            .find(|outcome| !matches!(outcome, Outcome::Returned { .. } | Outcome::Budget))
        {
            return Err(format!("laço parou em {bad:?}").into());
        }
    }
    let elapsed = wall.elapsed();
    let virtual_ms = machine.clock_ms().saturating_sub(base_clock);
    let instructions = machine.instructions().saturating_sub(base_instructions);
    let ratio = virtual_ms as f64 / elapsed.as_secs_f64().max(f64::MIN_POSITIVE) / 10.0;
    println!("backend:   Dynarmic ARMv6K");
    println!(
        "tempo:     {virtual_ms} ms virtuais em {:.3} s reais ({ratio:.1}% da velocidade)",
        elapsed.as_secs_f64()
    );
    println!(
        "cpu:       {} milhões de instruções ({:.1} MIPS)",
        instructions / 1_000_000,
        instructions as f64 / elapsed.as_secs_f64().max(f64::MIN_POSITIVE) / 1_000_000.0
    );
    println!(
        "laço:      {turns} voltas, {} timer(s), {} quadro(s) GL",
        machine.armed_timers(),
        machine.gl_swaps()
    );
    let media: Vec<_> = machine
        .call_log()
        .into_iter()
        .filter(|(name, _)| name.starts_with("IMedia::"))
        .collect();
    if !media.is_empty() {
        println!("mídia:");
        for (name, count) in media {
            println!("  {count:>4}x {name}");
        }
    }
    if let Some(path) = dump {
        std::fs::write(path, machine.screen().to_bmp())?;
        println!("quadro:    {path}");
    }
    if let Some(dir) = superficies {
        despeja_superficies(&machine, dir)?;
    }
    if let Some(classe) = machine.take_launch_request() {
        println!("lançar:    o shell pediu para abrir {classe:#010x}");
    }
    Ok(())
}

/// Roda um jogo pelo mesmo caminho da janela, sem abri-la.
///
/// A bancada monta a máquina por conta própria, e o que ela mostra pode não ser o que a janela
/// mostra: a janela instala todos os jogos da biblioteca, passa o controle pela sessão e traduz o
/// direcional em teclas. Aqui entram as mesmas peças — `Session`, a biblioteca das configurações
/// e [`ui::App::teclas_do_controle`] —, e o roteiro é de **botões do controle**, como quem joga.
/// Com `--dump`, sai um quadro meio segundo depois de cada aperto; o relatório vai inteiro para a
/// saída no fim.
fn sessao_sem_janela(
    path: &str,
    seconds: u32,
    dump: Option<&str>,
    keys: &input::Script,
    instantes: &[u32],
    placa: bool,
    serial: Option<&str>,
    z_wheel: ui::settings::ZWheel,
    escala: usize,
    melhorias: (usize, usize),
    perfil: Option<u32>,
    boomerang: Option<Vec<(u32, [f32; 3])>>,
    portas_pedidas: Option<[Option<bindings::Aparelho>; input::PORTAS]>,
) -> Result<(), Box<dyn std::error::Error>> {
    let serial = serial.map(std::path::Path::new);
    let settings = ui::settings::Settings::load();
    let games = settings
        .roms_dir
        .as_deref()
        .map(library::scan)
        .unwrap_or_default();
    // `--wiimote` usa o Wii Remote conectado no lugar do roteiro.
    let wiimote = std::env::args()
        .any(|a| a == "--wiimote")
        .then(input::wiimote::Wiimotes::inicia);
    let boomerang = boomerang.or(wiimote.as_ref().map(|_| Vec::new()));
    let portas = match (portas_pedidas, &boomerang) {
        (Some(portas), _) => portas,
        (None, Some(_)) => [Some(bindings::Aparelho::Boomerang), None],
        (None, None) => zeebx::PORTAS_PADRAO,
    };
    let mut session = session::Session::start_with(
        std::path::Path::new(path),
        portas,
        serial,
        placa,
        None,
        z_wheel,
    )
    .map_err(|err| format!("{err:?}"))?;
    session.define_resolucao_interna(escala);
    session.define_proporcao(proporcao_da_linha());
    session.define_melhorias(melhorias.0, melhorias.1);
    let mut perfil_ligado_em: Option<(u32, std::time::Instant, u64)> = None;
    session.set_installed_applets(
        games
            .iter()
            .filter_map(|game| Some((game.clsid?, library::id_do_modulo(&game.path)?))),
    );
    let inicio_real = std::time::Instant::now();
    let mut fim = seconds.saturating_mul(1000);
    let mut reaberta = false;
    let mut pad = input::Pad::default();
    // `--dump-audio=A.wav` também aqui: a sessão é a da janela, e é nela que o som acontece.
    let gravacao = std::env::args()
        .find_map(|a| a.strip_prefix("--dump-audio=").map(str::to_string))
        .map(|caminho| (caminho, session.grava_audio(RECORD_RATE)));
    let mut gravado: Vec<f32> = Vec::new();
    let mut gravado_ms = 0u64;
    let mut fotos: std::collections::VecDeque<u32> = instantes.iter().copied().collect();
    let mut numero = 0;
    while session.clock_ms() < fim {
        if let (Some(desde), None) = (perfil, perfil_ligado_em) {
            if session.clock_ms() >= desde {
                session.liga_perfil_de_api();
                perfil_ligado_em = Some((
                    session.clock_ms(),
                    std::time::Instant::now(),
                    session.instrucoes(),
                ));
            }
        }
        let antes = pad;
        if !reaberta {
            keys.apply(session.clock_ms(), &mut pad);
        } else {
            pad = input::Pad::default();
        }
        session.set_port_pad(0, pad);
        if let Some(wiimotes) = &wiimote {
            // O mesmo giro para o referencial do Boomerang que a janela aplica.
            let [x, y, z] = wiimotes
                .estado(0)
                .filter(|e| e.com_acelerometro)
                .map_or([0.0, 0.0, 1.0], |e| e.aceleracao);
            session.set_port_motion(0, [y, x, z]);
        } else if let Some(roteiro) = &boomerang {
            // O último movimento cujo instante já passou; antes do primeiro, parado de face para
            // cima.
            let agora = roteiro
                .iter()
                .rev()
                .find(|(quando, _)| session.clock_ms() >= *quando)
                .map_or([0.0, 0.0, 1.0], |(_, g)| *g);
            session.set_port_motion(0, agora);
        }
        for (avk, apertada) in input::teclas_do_controle(&antes, &pad) {
            session.set_key(avk, apertada);
        }
        // As telas intermediárias passam como na janela, sem avançar o relógio; não viram foto.
        if session.mostra_quadro_intermediario() {
            continue;
        }
        if (0..input::BUTTONS).any(|b| pad.is_down(b) && !antes.is_down(b)) {
            fotos.push_back(session.clock_ms().saturating_add(500));
            fotos.make_contiguous().sort_unstable();
        }
        let parou = matches!(
            session.step(std::time::Duration::from_millis(16), false),
            session::Step::Stopped
        );
        if let Some((_, mixer)) = &gravacao {
            let agora = u64::from(session.clock_ms());
            let quadros = (agora.saturating_sub(gravado_ms) * u64::from(RECORD_RATE)) / 1000;
            if quadros > 0 {
                gravado.extend(mixer.render(quadros as usize));
                gravado_ms = agora;
            }
        }
        if let Some(path) = dump {
            while fotos.front().is_some_and(|&t| session.clock_ms() >= t) {
                fotos.pop_front();
                numero += 1;
                let nome = match path.strip_suffix(".bmp") {
                    Some(base) => format!("{base}.{numero}.bmp"),
                    None => format!("{path}.{numero}"),
                };
                if let Some(gl) = session.quadro_gl() {
                    std::fs::write(nome.replace(".bmp", ".gl.bmp"), gl.to_bmp())?;
                }
                println!(
                    "foto {numero}:    {} ms virtuais, {} ms reais, {} quadros de GL até aqui",
                    session.clock_ms(),
                    inicio_real.elapsed().as_millis(),
                    session.quadros_apresentados()
                );
                let na_janela = session.quadro_na_placa().is_some();
                if let Some(grande) = session.quadro_grande() {
                    std::fs::write(nome.replace(".bmp", ".grande.bmp"), grande.to_bmp())?;
                    println!(
                        "grande:    {}x{} aos {} ms, {}",
                        grande.width(),
                        grande.height(),
                        session.clock_ms(),
                        match na_janela {
                            true => "a janela mostra este",
                            false => "a janela mostra a tela de 640×480 (há 2D por cima)",
                        }
                    );
                }
                std::fs::write(nome, session.screen().to_bmp())?;
            }
        }
        if let Some(classe) = session.take_launch_request() {
            let jogo = games.iter().find(|game| game.clsid == Some(classe));
            println!(
                "lançar:    {classe:#010x} aos {} ms → {}",
                session.clock_ms(),
                jogo.map_or(
                    "nenhum jogo da biblioteca com essa classe".to_string(),
                    |g| g.path.display().to_string()
                )
            );
            break;
        }
        if parou && session.classe() == session::Z_WHEEL && session.saiu_sozinho() {
            // O mesmo que a janela faz: ver `session::Z_WHEEL`.
            println!(
                "reabrir:   a Z-Wheel saiu aos {} ms; reabrindo",
                session.clock_ms()
            );
            let deslocamento = session.clock_ms();
            session = session::Session::start_with(
                std::path::Path::new(path),
                zeebx::PORTAS_PADRAO,
                serial,
                placa,
                None,
                z_wheel,
            )
            .map_err(|err| format!("{err:?}"))?;
            session.define_resolucao_interna(escala);
            session.define_proporcao(proporcao_da_linha());
            session.define_melhorias(melhorias.0, melhorias.1);
            session.set_installed_applets(
                games
                    .iter()
                    .filter_map(|game| Some((game.clsid?, library::id_do_modulo(&game.path)?))),
            );
            let _ = deslocamento;
            reaberta = true;
            fim = 4000;
            continue;
        }
        if parou {
            println!("parou:     {:?}", session.stopped_reason());
            let (regs, pilha, lr) = session.falha();
            if let Some(lr) = lr {
                let regs: Vec<String> = regs
                    .iter()
                    .enumerate()
                    .map(|(i, v)| format!("r{i}={v:#x}"))
                    .collect();
                println!("           {} lr={lr:#x}", regs.join(" "));
                let pilha: Vec<String> = pilha.iter().map(|v| format!("{v:#x}")).collect();
                println!("           pilha: {}", pilha.join(" "));
            }
            break;
        }
    }
    if let Some(path) = dump {
        std::fs::write(path, session.screen().to_bmp())?;
    }
    println!("tempo:     {} ms virtuais", session.clock_ms());
    let (heap, objetos) = session.memory();
    println!("heap:      {heap} bytes em uso, {objetos} objetos vivos");
    if let (Some((caminho, _)), false) = (&gravacao, gravado.is_empty()) {
        std::fs::write(caminho, audio::to_wav(&gravado, RECORD_RATE))?;
        let pico = gravado.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        println!("áudio:     {caminho} (pico {pico:.3})");
    }
    if let Some((desde, inicio, instrucoes_antes)) = perfil_ligado_em {
        let real = inicio.elapsed();
        let virtual_ms = session.clock_ms().saturating_sub(desde);
        let api = session.perfil_de_api();
        let total: u64 = api.iter().map(|(_, ns)| ns).sum();
        println!(
            "perfil:    desde {desde} ms: {} ms reais para {virtual_ms} ms virtuais ({:.0}% do console), {} ms atendendo a API ({:.0}%)",
            real.as_millis(),
            f64::from(virtual_ms) / real.as_millis().max(1) as f64 * 100.0,
            total / 1_000_000,
            total as f64 / real.as_nanos().max(1) as f64 * 100.0,
        );
        let instrucoes = session.instrucoes().saturating_sub(instrucoes_antes);
        println!(
            "           {:.0} milhões de instruções ARM: {:.0} por segundo real, {:.0} por segundo virtual (o console faz 528)",
            instrucoes as f64 / 1e6,
            instrucoes as f64 / 1e6 / real.as_secs_f64().max(1e-9),
            instrucoes as f64 / 1e6 / (f64::from(virtual_ms) / 1000.0).max(1e-9),
        );
        for (nome, ns) in api.iter().take(PROFILE_LINES) {
            println!(
                "  {:5.1}%  {:>8} ms  {nome}",
                *ns as f64 / total.max(1) as f64 * 100.0,
                ns / 1_000_000
            );
        }
    }
    for linha in session.log() {
        println!("{linha}");
    }
    Ok(())
}

/// Grava o heap e a imagem do módulo como estão na memória.
///
/// A imagem do módulo vale a pena junto do heap: em runtime ela já passou pelas relocações que o
/// stub do `elf2mod` aplica sozinho, e o arquivo em disco ainda não.
///
/// Como o perfil, isto também serve quando o jogo **não** chega a começar: o Need For Speed monta
/// a tabela de sons dele antes de o applet existir, e é nessa tabela que está a resposta de por
/// que ele para.
fn despeja_memoria(
    machine: &machine::Machine<BackendPadrao>,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::write(
        "heap.bin",
        machine.dump(loader::HEAP_BASE, loader::HEAP_SIZE)?,
    )?;
    let (base, len) = {
        let region = &machine.module().mem.regions()[0];
        (region.base, region.bytes.len())
    };
    std::fs::write("module.bin", machine.dump(base, len)?)?;
    println!(
        "           heap.bin (base {:#010x}) e module.bin (base {:#010x})",
        loader::HEAP_BASE,
        base
    );
    Ok(())
}

/// Grava um BMP por superfície viva.
///
/// "O jogo desenha e a tela fica preta" tem duas causas possíveis, e só o conteúdo das
/// superfícies as separa: ou ele desenhou em algo que não vai para a tela, ou não desenhou. Ver
/// o conteúdo delas resolveu o texto do Tekken 2 em minutos depois de horas de suposição.
fn despeja_superficies<C: cpu::CpuBackend>(
    machine: &machine::Machine<C>,
    dir: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let superficies = machine.superficies();
    for (addr, largura, altura, bmp) in &superficies {
        std::fs::write(format!("{dir}/{addr:08x}-{largura}x{altura}.bmp"), bmp)?;
    }
    println!("superfícies: {} em {dir}/", superficies.len());
    Ok(())
}

/// Onde o tempo foi gasto: primeiro o que o emulador gastou atendendo o jogo, depois os blocos
/// de código do guest que mais executaram.
///
/// Fica em função própria porque **o perfil interessa mesmo quando o jogo não chega a começar**.
/// O Need For Speed queima os 500 milhões de instruções dentro do `CreateInstance`, e enquanto
/// esta impressão vivia só no laço de quadros o `--profile` dele saía vazio — justamente no caso
/// em que a pergunta "onde?" é a única que importa.
fn mostra_perfil(machine: &Machine<BackendPadrao>) {
    let api = machine.api_profile();
    let total_api: u64 = api.iter().map(|(_, ns)| ns).sum();
    if total_api > 0 {
        println!(
            "perfil da API: {} ms no total, do emulador atendendo o jogo",
            total_api / 1_000_000
        );
        for (nome, ns) in api.iter().take(PROFILE_LINES) {
            println!(
                "  {:5.1}%  {:>8} ms  {nome}",
                *ns as f64 / total_api as f64 * 100.0,
                ns / 1_000_000
            );
        }
    }
    let linhas = machine.cpu().profile();
    let total: u64 = linhas.iter().map(|&(_, n)| n).sum();
    println!("perfil:    {} blocos distintos executados", linhas.len());
    // Onde o jogo gasta o tempo é sempre um punhado de laços; vinte linhas cobrem com folga, e o
    // resto é cauda.
    for &(addr, n) in linhas.iter().take(PROFILE_LINES) {
        println!(
            "  {addr:#010x}  {:5.1}%  {n:>12} instrução(ões)",
            n as f64 / total.max(1) as f64 * 100.0
        );
    }
}

/// Escreve o desfecho e, quando ele é uma falha de memória, os registradores e a pilha.
///
/// Sem os registradores, "acesso inválido a 0x00000000" diz que alguma coisa era nula e não diz
/// **qual** — e essa é justamente a pergunta. Eles já eram guardados; faltava mostrá-los também
/// nas falhas de dentro do `EVT_APP_START`, que é onde os aplicativos morrem.
fn describe_outcome_com_estado<C: cpu::CpuBackend>(
    outcome: &Outcome,
    machine: &machine::Machine<C>,
) {
    describe_outcome(outcome);
    if !matches!(outcome, Outcome::Fault { .. } | Outcome::Exception { .. }) {
        return;
    }
    let regs = machine.fault_regs();
    print!("           ");
    for (i, valor) in regs.iter().enumerate() {
        print!("r{i}={valor:#x} ");
    }
    println!();
    let pilha = machine.fault_stack();
    if !pilha.is_empty() {
        let itens: Vec<String> = pilha.iter().map(|v| format!("{v:#x}")).collect();
        println!("           pilha: {}", itens.join(" "));
    }
}

fn describe_outcome(outcome: &Outcome) {
    match outcome {
        Outcome::Returned { code } => println!("retornou {code}"),
        Outcome::Unimplemented { addr, args, caller } => {
            println!("API não implementada — {}", aee::describe(*addr));
            println!(
                "           args r0={:#x} r1={:#x} r2={:#x} r3={:#x}  (chamada de {caller:#010x})",
                args[0], args[1], args[2], args[3]
            );
        }
        Outcome::Fault { addr, pc, lr } => {
            println!("acesso inválido a {addr:#010x} (pc {pc:#010x}, lr {lr:#010x})")
        }
        Outcome::Exception { pc } => println!("exceção do núcleo ARM em pc {pc:#010x}"),
        Outcome::Budget => println!("orçamento de instruções esgotado"),
        Outcome::CallLimit { calls } => {
            println!("teto de {calls} chamadas de API atingido — provável laço de repetição")
        }
    }
}

/// Roda o laço de quadros: avança o relógio virtual, dispara os timers que venceram e entrega
/// os callbacks pendentes.
///
/// O jogo desenha dentro do callback do timer que ele mesmo rearma, então é o disparo do timer
/// que produz cada quadro. Para quando o jogo não tem mais nada armado — sem timer nem callback
/// pendente, nada mais vai acontecer.
#[allow(clippy::too_many_arguments)]
fn run_frames(
    machine: &mut Machine<BackendPadrao>,
    rounds: u32,
    seconds: Option<u32>,
    path: &str,
    dump_heap: bool,
    show: bool,
    keys: &input::Script,
    teclas: &[(u32, u32)],
    dump_gl: Option<&str>,
    dump_audio: Option<&str>,
    profile: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Com janela, o som sai pela placa; sem ela, o que se quer é despejar quadros, e um fluxo
    // de áudio aberto só atrapalharia.
    let speaker = match show {
        true => match audio::Output::open(1.0, false) {
            Ok(output) => {
                machine.set_audio(Some(output.mixer()));
                Some(output)
            }
            Err(err) => {
                println!("som:       sem saída de áudio ({err})");
                None
            }
        },
        false => None,
    };
    let mut window = match show {
        true => Some(window::Window::open(
            "Zeebx",
            machine.screen().width(),
            machine.screen().height(),
        )?),
        false => None,
    };
    let mut drawn = 0;
    let mut teclas_pendentes: std::collections::VecDeque<(u32, u32)> = {
        let mut ordenadas = teclas.to_vec();
        ordenadas.sort_by_key(|&(quando, _)| quando);
        ordenadas.into()
    };
    let mut turns = 0;
    let mut stopped = None;
    let mut pad = input::Pad::default();
    let deadline = seconds.map(|s| s.saturating_mul(1000));
    if let Some(dir) = dump_gl {
        std::fs::create_dir_all(dir)?;
    }
    let mut dumped = 0;
    // O som é misturado aqui mesmo, no ritmo do relógio virtual, em vez de sair pela placa: é
    // assim que dá para **conferir** o áudio sem ouvi-lo — e o arquivo que sai é comparável
    // com os sons que o jogo entregou.
    let recorder = dump_audio.map(|_| {
        let mixer = audio::Mixer::silent(RECORD_RATE);
        machine.set_audio(Some(mixer.clone()));
        mixer
    });
    let mut recorded: Vec<f32> = Vec::new();
    let mut recorded_ms = 0u64;
    let mut shown = 0;
    let mut refreshed = std::time::Instant::now();
    // O relógio virtual anda sozinho: quando o jogo dorme, `skip_idle_time` adianta o tempo em
    // vez de gastá-lo, e um trecho ocioso passa em quase nada de tempo real. Isso é o que faz o
    // tempo que o jogo *mede* ficar certo, mas nada segurava o emulador no ritmo do mundo — o
    // Crash rodava 12 segundos virtuais em 1,5 real. Só a janela precisa do freio; sem ela, o
    // que se quer é justamente terminar rápido.
    let started = std::time::Instant::now();
    let clock_base = u64::from(machine.clock_ms());
    for round in 0..rounds {
        if let Some(window) = &mut window {
            if !window.is_open() {
                break;
            }
            let ahead = u64::from(machine.clock_ms())
                .saturating_sub(clock_base)
                .saturating_sub(started.elapsed().as_millis() as u64);
            if ahead > 0 {
                std::thread::sleep(std::time::Duration::from_millis(ahead));
            }
            // Uma volta do laço não é um quadro: o jogo pode rodar o laço principal inteiro
            // dentro de um callback só, e cada volta daqui devolve apenas a fatia de
            // instruções que coube. No Peteca são um milhão e meio de voltas para algumas
            // dezenas de quadros — redesenhar em todas afoga a janela, que nunca chega a ser
            // composta e aparece em branco. Quem manda no redesenho é o `eglSwapBuffers`.
            let swaps = machine.gl_swaps();
            let presented = swaps != shown;
            // Um jogo 2D nunca apresenta pelo OpenGL, e mesmo o 3D precisa atender ao teclado
            // e ao pedido de fechar entre um quadro e outro: daí o intervalo de relógio real.
            if presented || refreshed.elapsed() >= REFRESH_PERIOD {
                shown = swaps;
                refreshed = std::time::Instant::now();
                window.show(machine.screen())?;
                pad = window.pad();
            }
        }
        // As teclas do roteiro entram quando o relógio do jogo passa do instante marcado. Cada
        // uma vai como aperto e soltura seguidos, que é o que um toque é.
        while teclas_pendentes
            .front()
            .is_some_and(|&(quando, _)| machine.clock_ms() >= quando)
        {
            let (_, avk) = teclas_pendentes.pop_front().unwrap_or_default();
            machine.set_key(avk, true);
            machine.set_key(avk, false);
        }
        if deadline.is_some_and(|limit| machine.clock_ms() >= limit) {
            break;
        }
        // O teto de tempo real encerra a execução inteira: recomeçar a fatia seguinte só
        // gastaria mais relógio para parar de novo no primeiro bloco.
        if machine.cpu().wall_expired() {
            println!("           teto de tempo real atingido");
            break;
        }
        // O roteiro entra por cima do teclado: assim dá para conferir a entrada sem janela e,
        // com ela, ver o que o roteiro faz.
        keys.apply(machine.clock_ms(), &mut pad);
        machine.set_pad(pad);
        turns += 1;
        let outcomes = machine.advance(INSTRUCTION_BUDGET)?;
        machine.deliver_signals(INSTRUCTION_BUDGET)?;
        machine.deliver_callbacks(INSTRUCTION_BUDGET)?;

        // O teto de instruções de um trecho é pedido de vez, não desfecho ruim. Ver a mesma
        // decisão em `session.rs`.
        if let Some(bad) = outcomes
            .iter()
            .find(|outcome| !matches!(outcome, Outcome::Returned { .. } | Outcome::Budget))
        {
            stopped = Some((round, bad.clone()));
            break;
        }
        // Um quadro por arquivo: comparar dois quadros vizinhos é o que diz se um artefato
        // é do desenho ou de um estado que ficou preso de um quadro para o outro.
        if let Some(dir) = dump_gl {
            while dumped < machine.gl_swaps() {
                dumped += 1;
                if let Some(frame) = machine.gl_frame() {
                    let out = format!("{dir}/{dumped:05}.bmp");
                    std::fs::write(out, frame.to_bmp())?;
                }
            }
        }
        // Grava o som correspondente ao tempo virtual que passou desde a volta anterior.
        if let Some(mixer) = &recorder {
            let now = u64::from(machine.clock_ms());
            let frames = (now.saturating_sub(recorded_ms) * u64::from(RECORD_RATE)) / 1000;
            if frames > 0 {
                recorded.extend(mixer.render(frames as usize));
                recorded_ms = now;
            }
        }
        drawn += outcomes.len();
        if outcomes.is_empty() && machine.is_idle() {
            break;
        }
    }

    drop(speaker);
    if let (Some(path), false) = (dump_audio, recorded.is_empty()) {
        std::fs::write(path, audio::to_wav(&recorded, RECORD_RATE))?;
        let loudest = recorded.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        println!(
            "áudio:     {path} ({:.1} s, pico {loudest:.3})",
            recorded.len() as f32 / (RECORD_RATE * 2) as f32
        );
    }
    println!(
        "tempo:     {drawn} disparo(s) de timer em {} ms virtuais, {} timer(s) armado(s)",
        machine.clock_ms(),
        machine.armed_timers()
    );
    if profile {
        mostra_perfil(machine);
    }
    println!(
        "           {} milhões de instruções em {turns} volta(s) do laço",
        machine.instructions() / 1_000_000,
    );
    if let Some((frame, outcome)) = stopped {
        print!("           parou na volta {frame} em ");
        describe_outcome(&outcome);
        if let Outcome::Fault { .. } = outcome {
            let regs = machine.fault_regs();
            let dump: Vec<String> = regs
                .iter()
                .enumerate()
                .map(|(i, value)| format!("r{i}={value:#x}"))
                .collect();
            println!("           {}", dump.join(" "));
            let stack = machine.fault_stack();
            if !stack.is_empty() {
                let trail: Vec<String> = stack.iter().map(|a| format!("{a:#x}")).collect();
                println!("           pilha: {}", trail.join(" "));
            }
            // O que explica um ponteiro nulo é a struct que levou até ele, e ela só existe
            // na memória do guest — daí poder levá-la para fora e analisá-la com calma.
            if dump_heap {
                despeja_memoria(machine)?;
            }
        }
    }
    if drawn > 0 && machine.screen().is_dirty() {
        let out = std::path::Path::new(path).with_extension("frames.bmp");
        let out = out.file_name().map(std::path::Path::new).unwrap_or(&out);
        std::fs::write(out, machine.screen().to_bmp())?;
        println!("           último quadro em {}", out.display());
    }
    Ok(())
}

/// Imprime o que sabemos ler de um `.mod`.
fn info(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let data = std::fs::read(path)?;
    let size = data.len();
    let image = ModImage::parse(data)?;

    println!("arquivo:  {path}");
    println!("tamanho:  {size} bytes ({size:#x})");
    println!(
        "variante: {}",
        match image.variant() {
            Variant::Raw => "sem cabeçalho (código ARM direto)",
            Variant::BrewHeader => "cabeçalho BREW",
        }
    );
    println!("entry:    {:#010x}  (AEEMod_Load)", image.entry());

    if let Some(header) = image.header() {
        println!("versão:   {}", header.version);
        println!("code_off: {:#x}", header.code_offset);
        print!("campos não decifrados (0x14..0x3c):");
        for word in header.unknown {
            print!(" {word:#x}");
        }
        println!();
    }
    Ok(())
}
