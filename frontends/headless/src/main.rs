//! Zeebx sem interface: o emulador por linha de comando, configurado por um `config.ini`.
//!
//! Existe para quem já tem um frontend. Um programa que simula a carcaça do console, uma
//! estante de jogos, um arcade de fliperama — nenhum deles quer a biblioteca do Zeebx por cima
//! da tela que ele mesmo desenhou. Este binário abre o jogo que lhe derem, obedece ao arquivo
//! de configuração e não mostra mais nada.
//!
//! **O núcleo é o mesmo.** Gráficos, áudio e mapeamento de controle são os campos que a
//! interface do desktop grava no `settings.json`; aqui eles chegam por INI, que é o que se
//! edita à mão. Ver [`config`].

mod config;
mod console;
mod despejo;
mod entrada;
mod ini;
mod janela;

use std::path::PathBuf;
use std::process::ExitCode;

use crate::config::Video;
use crate::console::{Console, Fim};

const AJUDA: &str = "\
Zeebx headless — the Zeebo emulator on the command line.

  zeebx-headless [OPTIONS] GAME

GAME is required: a `.mod`, or the `.zip` that holds it. To boot into the
Z-Wheel, pass the Z-Wheel — it is a game like any other.

Options:
  --config=PATH    the configuration file to use. Without it, looks for a
                   `config.ini` next to the executable and then in the system
                   configuration directory. If none exists, writes one there.
  --controllers    list the controllers the system can see, with the names
                   `[port1] controller` expects, and exit.
  --example        write a commented `config.ini` with the factory values to
                   standard output, and exit.
  --help           this.
  --version        the version.

Everything else — video, audio, controls — lives in the `config.ini`.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{AJUDA}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version") {
        println!("zeebx-headless {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--example") {
        print!("{}", config::modelo());
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--controllers") {
        return lista_controles();
    }

    let arquivo = args
        .iter()
        .find_map(|a| a.strip_prefix("--config="))
        .map(PathBuf::from);
    // O que não começa com `--` é o jogo. Um só: abrir dois não quer dizer nada.
    let jogo = args.iter().find(|a| !a.starts_with("--")).map(PathBuf::from);
    if let Some(desconhecida) = args
        .iter()
        .find(|a| a.starts_with("--") && !a.starts_with("--config="))
    {
        eprintln!("error: unknown option `{desconhecida}`. `--help` lists what exists.");
        return ExitCode::FAILURE;
    }

    let lido = config::carrega(arquivo.as_deref());
    for aviso in &lido.avisos {
        eprintln!("config: {aviso}");
    }
    let origem = lido.origem;
    let modo = lido.headless.video;
    let console = Console::novo(lido.settings, lido.headless);

    // Primeira execução: deixa um `config.ini` de fábrica escrito, comentado linha a linha, em
    // vez de só rodar com os padrões e não dizer onde se muda nada.
    if let config::Origem::Faltando(onde) = &origem {
        match config::cria(onde) {
            Ok(()) => eprintln!("config: none found; wrote one at {}", onde.display()),
            Err(erro) => eprintln!("config: no file, using factory settings ({erro})"),
        }
    }

    // **O jogo é obrigatório, e não tem padrão.** Este binário é chamado por outro programa,
    // que sabe o que quer abrir; adivinhar alguma coisa quando ele não diz nada seria abrir o
    // que ninguém pediu. Quem quiser a Z-Wheel a passa como qualquer outro jogo.
    let Some(caminho) = jogo else {
        eprintln!(
            "error: nothing to run.\n\
             \n\
             Pass the `.mod`, or the `.zip` that holds it:\n\
             \x20   zeebx-headless PATH/TO/GAME.zip"
        );
        return ExitCode::FAILURE;
    };

    match modo {
        Video::Nenhum => sem_janela(console, &caminho),
        _ => janela::roda(console, caminho),
    }
}

/// O modo sem vídeo: nada na tela, e o quadro sai pelo `[despejo]`.
fn sem_janela(mut console: Console, caminho: &std::path::Path) -> ExitCode {
    // Sem janela não há contexto de GL vindo de uma, mas o 3D na placa continua possível: o
    // núcleo sabe abrir um contexto fora de tela, que é o que o `zeebx run` já usa para medir.
    let contexto = match console.settings.graphics.gpu_rasterizer {
        true => match zeebx::video::contexto::Contexto::novo() {
            Ok(contexto) => Some(contexto),
            Err(erro) => {
                eprintln!("warning: no reachable GPU, 3D stays in software: {erro}");
                None
            }
        },
        false => None,
    };
    console.usa_contexto(contexto.as_ref().map(|c| c.gl.clone()));

    let mut saida = match despejo::Saida::abre(&console.opcoes.despejo.clone()) {
        Ok(saida) => saida,
        Err(erro) => {
            eprintln!("error: {erro}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(erro) = console.abre(caminho) {
        eprintln!("error: {}: {erro}", caminho.display());
        return ExitCode::FAILURE;
    }

    // O quadro só é escrito quando muda: o laço do emulador gira muito mais vezes que o jogo
    // desenha, e repetir o mesmo quadro encheria o cano de cópias idênticas.
    let mut visto = None;
    loop {
        match console.passo() {
            Fim::Segue => {}
            Fim::Acabou(motivo) => {
                eprintln!("stopped: {motivo}");
                return ExitCode::SUCCESS;
            }
            Fim::Erro(erro) => {
                eprintln!("error: {erro}");
                return ExitCode::FAILURE;
            }
        }
        if console.adiantado() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let grande = console.opcoes.despejo.formato;
        let Some(sessao) = console.sessao_mut() else {
            return ExitCode::SUCCESS;
        };
        let agora = {
            let tela = sessao.screen();
            (tela.serie(), tela.escritas())
        };
        if visto == Some(agora) {
            continue;
        }
        visto = Some(agora);
        // **Os formatos crus saem sempre no quadro do console, e só o `.png` acompanha a
        // resolução interna.** Um fluxo de bytes sem cabeçalho tem de ter o quadro do mesmo
        // tamanho do começo ao fim: o 3D na placa sai grande e o menu 2D sai pequeno, e quem
        // conta bytes do outro lado perderia o passo na primeira troca. O `.png` diz o próprio
        // tamanho em cada arquivo, então ali a imagem maior não atrapalha ninguém.
        let quadro = match grande == config::Formato::Png {
            true => sessao.quadro_grande(),
            false => None,
        };
        let quadro = quadro.as_ref().unwrap_or_else(|| sessao.screen());
        if let Err(erro) = saida.escreve(quadro) {
            // A outra ponta fechou o cano. Não é falha de quem rodou: é o fim normal de
            // `zeebx-headless ... | frontend` quando o frontend sai.
            eprintln!("stopped: {erro} ({} frames)", saida.contados());
            return ExitCode::SUCCESS;
        }
        if saida.cheia() {
            eprintln!("stopped: wrote the {} frames requested", saida.contados());
            return ExitCode::SUCCESS;
        }
    }
}

/// Os controles que o sistema enxerga, pelos nomes que o `config.ini` espera.
fn lista_controles() -> ExitCode {
    let pads = zeebx::input::gamepads::Gamepads::new();
    let nomes = pads.names();
    if nomes.is_empty() {
        println!("no controllers visible to the system");
        return ExitCode::SUCCESS;
    }
    println!("controllers seen, in system order:");
    for (i, nome) in nomes.iter().enumerate() {
        println!("  {i}: {nome}");
    }
    println!("\nin [port1] of config.ini, for example:\n  controller = \"{}\"", nomes[0]);
    ExitCode::SUCCESS
}
