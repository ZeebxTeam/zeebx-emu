//! O laço do jogo. O hospedeiro entrega o controle e recebe o quadro e o som; a Z-Wheel,
//! quando é ela que está aberta, reabre como no aparelho — escolher um jogo ali é ela sair,
//! e sem a reabertura o título marcado nunca abre.

use std::ffi::CString;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use zeebx::audio::soundfont;
use zeebx::config::ZWheel;
use zeebx::input::{self, Pad};
use zeebx::library::{self, Game};
use zeebx::session::{self, FATIA_MAXIMA, Session, Step};
use zeebx::storage::StoragePaths;
use zeebx::velocidade::Ritmo;
use zeebx::PORTAS_PADRAO;

use crate::ajustes::Ajustes;
use crate::entrada::{self, Leitura};
use crate::lancamento::Lancamento;
use crate::som::{self, TAXA};
use crate::ZeebxHospedeiro;

struct Parada {
    normal: bool,
    motivo: String,
    viu_quadro: bool,
    pela_z_wheel: bool,
    classe: u32,
}

enum Fim {
    Saiu,
    Pedido(u32),
    Parou(Parada),
}

struct Jogo {
    sessao: Session,
    mixer: zeebx::audio::Mixer,
    pad_antes: Pad,
    ultimo: Instant,
    relogio: u32,
    ultimo_som: Instant,
    mix: Vec<f32>,
    pcm: Vec<i16>,
    corda: Option<Instant>,
    pela_z_wheel: bool,
    primeiro: bool,
}

pub fn roda(args: &Lancamento, host: &ZeebxHospedeiro) -> i32 {
    let log = args.system.join("zeebx.log");
    let destino = log.clone();
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&destino)
            .and_then(|mut f| {
                use std::io::Write;
                writeln!(f, "panico: {info}")
            });
    }));
    anota(&log, &format!("abrindo {}", args.rom.display()));

    let ajustes = crate::ajustes::carrega(&args.config);
    let storage = StoragePaths::for_frontend(&args.saves, Some(&args.system));
    if let Err(erro) = storage.create_dirs() {
        return falha(host, &log, &format!("não consegui criar as pastas: {erro}"));
    }
    prepara_o_midi(&args.system, &storage, &ajustes);

    let jogos = library::scan(&args.roms);
    let instalados = instalados(&jogos);
    let mut caminho = args.rom.clone();
    let mut pela_z_wheel = false;
    let mut saidas_sem_quadro = 0u32;

    loop {
        let jogo = match abre(&caminho, &storage, &instalados, &ajustes, pela_z_wheel) {
            Ok(jogo) => jogo,
            Err(erro) => return falha(host, &log, &erro),
        };
        match laco(jogo, host, &ajustes) {
            Fim::Saiu => {
                anota(&log, "voltou ao tico");
                return 0;
            }
            Fim::Pedido(classe) => {
                let Some(proximo) = jogos.iter().find(|jogo| jogo.clsid == Some(classe)) else {
                    return falha(
                        host,
                        &log,
                        &format!("o shell pediu {classe:#010x}, que não está na pasta"),
                    );
                };
                pela_z_wheel = classe != session::Z_WHEEL;
                caminho = proximo.path.clone();
                saidas_sem_quadro = 0;
            }
            Fim::Parou(parada) => {
                if !parada.normal {
                    return falha(host, &log, &parada.motivo);
                }
                if parada.viu_quadro {
                    saidas_sem_quadro = 0;
                } else {
                    saidas_sem_quadro += 1;
                }
                // A Z-Wheel confirma o jogo e sai. O aparelho a reabre, e na volta ela lê o
                // marcador e abre o título. Três saídas seguidas sem quadro nenhum é laço, não
                // escolha: aí sim volta ao tico, senão um módulo quebrado nunca devolve.
                let reabre = parada.classe == session::Z_WHEEL || parada.pela_z_wheel;
                if reabre && saidas_sem_quadro < 3 {
                    if let Some(roda) = library::z_wheel_de(Some(&args.roms), &jogos) {
                        anota(&log, "a Z-Wheel saiu; reabrindo");
                        caminho = roda;
                        pela_z_wheel = false;
                        continue;
                    }
                }
                anota(&log, "voltou ao tico");
                return 0;
            }
        }
    }
}

fn instalados(jogos: &[Game]) -> Vec<(u32, String)> {
    jogos
        .iter()
        .filter_map(|jogo| Some((jogo.clsid?, library::id_do_modulo(&jogo.path)?)))
        .collect()
}

fn prepara_o_midi(system: &Path, storage: &StoragePaths, ajustes: &Ajustes) {
    soundfont::define_efeitos(ajustes.efeitos);
    let pastas = [
        system.join("soundfonts"),
        system.to_path_buf(),
        storage.device.join("soundfonts"),
    ];
    if let Some(banco) = soundfont::primeiro_banco_em(&pastas) {
        soundfont::define_banco(Some(banco));
    }
}

fn abre(
    caminho: &Path,
    storage: &StoragePaths,
    instalados: &[(u32, String)],
    ajustes: &Ajustes,
    pela_z_wheel: bool,
) -> Result<Jogo, String> {
    let mut sessao = Session::start_with_storage_installed_policy(
        caminho,
        PORTAS_PADRAO,
        None,
        false,
        None,
        ZWheel::default(),
        storage,
        instalados,
        ajustes.midi,
    )
    .map_err(|erro| erro.to_string())?;
    sessao.define_reducao(ajustes.reducao);
    let relogio = sessao.clock_ms();
    let mixer = sessao.grava_audio(TAXA);
    mixer.set_master(ajustes.volume, !ajustes.audio);
    Ok(Jogo {
        sessao,
        mixer,
        pad_antes: Pad::default(),
        ultimo: Instant::now(),
        relogio,
        ultimo_som: Instant::now(),
        mix: Vec::new(),
        pcm: Vec::new(),
        corda: None,
        pela_z_wheel,
        primeiro: true,
    })
}

fn laco(mut jogo: Jogo, host: &ZeebxHospedeiro, ajustes: &Ajustes) -> Fim {
    let ritmo = Ritmo::das_escolhas(ajustes.limite, ajustes.frameskip);
    let mut viu_quadro = false;
    loop {
        let mut leitura = Leitura::default();
        if let Some(entrada) = host.entrada {
            unsafe { entrada(&mut leitura) };
        }
        if entrada::sai_do_jogo(leitura.botoes) {
            if jogo.corda.is_none() {
                jogo.corda = Some(Instant::now());
            }
            if jogo
                .corda
                .is_some_and(|desde| entrada::tempo_de_saida(desde.elapsed()))
            {
                return Fim::Saiu;
            }
        } else {
            jogo.corda = None;
        }

        leitura.botoes = entrada::filtra_saida(leitura.botoes);
        let mut pad = entrada::para_pad(&leitura);
        if ajustes.direcional_no_manche {
            pad.espelha_o_direcional_nos_eixos();
        }
        if jogo.primeiro {
            jogo.pad_antes = pad;
            jogo.primeiro = false;
        }
        let antes = jogo.pad_antes;
        jogo.sessao.set_port_pad(0, pad);
        for (avk, apertada) in input::teclas_do_controle(&antes, &pad) {
            jogo.sessao.set_key(avk, apertada);
        }
        jogo.pad_antes = pad;

        let agora = Instant::now();
        let fatia = agora.saturating_duration_since(jogo.ultimo).min(FATIA_MAXIMA);
        jogo.ultimo = agora;
        let passo = jogo.sessao.anda(fatia, &ritmo);
        if passo == Step::Presented {
            viu_quadro = true;
            if let Some(apresenta) = host.apresenta {
                let tela = jogo.sessao.screen();
                unsafe { apresenta(tela.pixels().as_ptr(), tela.width(), tela.height()) };
            }
        } else if passo == Step::Ahead {
            // Adiantado, a volta volta na hora. Sem esta espera o laço gira no vazio:
            // o `framebufferBegin` só bloqueia no retraco quando há quadro novo.
            std::thread::sleep(Duration::from_millis(1));
        }

        if let Some(audio) = host.audio {
            let agora_ms = jogo.sessao.clock_ms();
            let virt = u64::from(agora_ms.wrapping_sub(jogo.relogio));
            jogo.relogio = agora_ms;
            let real = jogo.ultimo_som.elapsed().as_millis() as u64;
            jogo.ultimo_som = Instant::now();
            let devidas = som::amostras_devidas(real, virt, TAXA);
            jogo.mixer.render_into(devidas, &mut jogo.mix);
            jogo.pcm.clear();
            jogo.pcm.extend(
                jogo.mix
                    .iter()
                    .map(|amostra| (amostra.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16),
            );
            if !jogo.pcm.is_empty() {
                unsafe { audio(jogo.pcm.as_ptr(), (jogo.pcm.len() / 2) as u32) };
            }
        }

        if let Some(classe) = jogo.sessao.take_launch_request() {
            return Fim::Pedido(classe);
        }
        if passo == Step::Stopped {
            return Fim::Parou(Parada {
                normal: jogo.sessao.saiu_sozinho(),
                motivo: jogo
                    .sessao
                    .stopped_reason()
                    .unwrap_or_else(|| "o jogo parou".to_string()),
                viu_quadro,
                pela_z_wheel: jogo.pela_z_wheel,
                classe: jogo.sessao.classe(),
            });
        }
    }
}

fn falha(host: &ZeebxHospedeiro, log: &Path, motivo: &str) -> i32 {
    anota(log, motivo);
    avisa(host, motivo);
    1
}

pub fn avisa(host: &ZeebxHospedeiro, motivo: &str) {
    let Ok(texto) = CString::new(motivo.replace('\0', "")) else {
        return;
    };
    if let Some(erro) = host.erro {
        unsafe { erro(texto.as_ptr()) };
    }
}

fn anota(log: &Path, linha: &str) {
    let _ = (|| -> std::io::Result<()> {
        if let Some(pasta) = log.parent() {
            std::fs::create_dir_all(pasta)?;
        }
        let mut arquivo = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)?;
        writeln!(arquivo, "{linha}")
    })();
}
