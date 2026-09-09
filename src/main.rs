//! Zeebx — emulador de Zeebo / Qualcomm BREW.

mod aee;
mod aee_helpers;
mod aee_slots;
mod archive;
mod atc;
mod audio;
mod bindings;
mod cformat;
mod cpu;
mod crypto;
mod display;
mod fmath;
mod font;
mod gamepads;
mod gif;
mod gles;
mod heap;
mod i18n;
mod icon;
mod input;
mod library;
mod loader;
mod machine;
mod mem;
mod miffile;
mod modfile;
mod mp3;
mod objects;
mod padview;
mod paltex;
mod ponte;
mod rasterizer;
mod rede;
mod resfile;
mod session;
mod settings;
mod sql;
mod ui;
mod vfs;
mod wav;
mod window;

use std::process::ExitCode;

use cpu::{CpuBackend, unicorn::UnicornCpu};
use machine::{AppletResult, Machine, Outcome};
use modfile::{ModImage, Variant};

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
            let dump_heap = args.iter().any(|a| a == "--dump-heap");
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
                    dump_heap,
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
                        None => PORTAS_PADRAO,
                    },
                },
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
                             [--trace[=trecho]] [--watch=0xADDR] [--dump-heap]
                             [--code=0xINI:0xFIM] [--frames=N]
                             [--profile] [--wall=SEGUNDOS] [--sonda=0xCLSID,...]
                             [--sem-rede] [--servidor=MAQUINA[:PORTA]] [--ponte]
                             [--portas=controle|teclado|nenhum,...] [--teclas=ms:nome,...]"
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
const LOGO: &[u8] = include_bytes!("../assets/zeebx.png");

/// O maior lado do ícone. A logo tem mais de mil pixels de lado, e um ícone desse tamanho é
/// megabytes de textura para desenhar algo que nunca passa de alguns pixels na barra.
const ICON_SIDE: usize = 256;

/// Identificador da janela para o desktop. Casa com `assets/zeebx.desktop` — se um mudar, o
/// outro muda junto, senão o ícone some no Wayland.
const APP_ID: &str = "zeebx";

/// A logo no tamanho de ícone. `None` se ela não abrir — uma janela sem ícone é melhor que uma
/// janela que não abre.
fn window_icon() -> Option<eframe::egui::IconData> {
    let logo = icon::decode(LOGO)
        .inspect_err(|err| eprintln!("ícone da janela: {err}"))
        .ok()?
        .downscaled(ICON_SIDE);
    Some(eframe::egui::IconData {
        width: logo.width as u32,
        height: logo.height as u32,
        rgba: logo.rgba,
    })
}

fn launch() -> ExitCode {
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([960.0, 720.0])
        .with_min_inner_size([480.0, 360.0])
        .with_title("Zeebx")
        // No Wayland não existe ícone em pixels: o compositor casa este `app_id` com o
        // `zeebx.desktop` instalado e tira o ícone de lá. Sem ele, a janela fica com o
        // genérico do sistema. Precisa ser igual ao nome do arquivo `.desktop`.
        .with_app_id(APP_ID);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let app = |context: &eframe::CreationContext<'_>| {
        Ok(Box::new(ui::App::new(context)) as Box<dyn eframe::App>)
    };
    match eframe::run_native("Zeebx", options, Box::new(app)) {
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
    dump_heap: bool,
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
}

/// Lê `1000:select,2000:down` e devolve `(instante em ms, código AVK)`.
///
/// Os nomes são os de [`input::avk::por_nome`]: `up`, `down`, `left`, `right`, `select`, `clr`,
/// `star`, `pound` e os dígitos. O que não for reconhecido é descartado com aviso, porque um
/// roteiro com uma tecla errada ainda vale pelas outras.
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

/// O padrão sem janela: um controle na primeira porta, a segunda livre. É o que sempre houve.
const PORTAS_PADRAO: [Option<bindings::Aparelho>; input::PORTAS] =
    [Some(bindings::Aparelho::Controle), None];

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
            "controle" | "pad" => Some(bindings::Aparelho::Controle),
            "teclado" | "keyboard" => Some(bindings::Aparelho::Teclado),
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
        dump_heap,
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
    } = options;
    // Um jogo em `.zip` é extraído para o cache e rodado de lá, como na interface.
    let extracted;
    let path = match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some("zip") => {
            extracted = archive::extract(std::path::Path::new(path))?;
            println!("extraído:  {}", extracted.display());
            extracted.to_str().unwrap_or(path)
        }
        _ => path,
    };
    let image = ModImage::parse(std::fs::read(path)?)?;
    let module = loader::load(&image)?;

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
    let mut machine = Machine::new(UnicornCpu::new()?, module, root);
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
                            if matches!(started, Outcome::Returned { .. }) {
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
        for (ms, nome, down) in &toques {
            let acao = match down {
                true => "aperta",
                false => "solta ",
            };
            println!("  {ms:>7} ms  {acao} {nome}");
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
    machine: &mut Machine<UnicornCpu>,
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

        if let Some(bad) = outcomes
            .iter()
            .find(|outcome| !matches!(outcome, Outcome::Returned { .. }))
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
        // Onde o jogo gasta o tempo é sempre um punhado de laços; vinte linhas cobrem com
        // folga, e o resto é cauda.
        for &(addr, n) in linhas.iter().take(PROFILE_LINES) {
            println!(
                "  {addr:#010x}  {:5.1}%  {n:>12} instrução(ões)",
                n as f64 / total.max(1) as f64 * 100.0
            );
        }
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
                std::fs::write(
                    "heap.bin",
                    machine.dump(loader::HEAP_BASE, loader::HEAP_SIZE)?,
                )?;
                // A imagem do módulo também: em runtime ela já passou pelas relocações que o
                // stub do `elf2mod` aplica sozinho, e o arquivo em disco ainda não.
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
