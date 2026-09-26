//! Síntese por banco de amostras (SoundFont) para o MIDI.
//!
//! **Por que existe.** A comparação com o Zeebulator mediu o que a tabela de timbres pode e o que
//! ela não pode: depois de refeita contra a medição, o erro de centroide dela caiu de 7635 para
//! 701 Hz, mas três coisas continuaram fora de alcance, e todas as três são **material de amostra**:
//!
//! | | amostra real | tabela de timbres |
//! |---|---|---|
//! | razão de harmônicos das cordas | 8,37 | 0,60 |
//! | razão de harmônicos da distorção | 4,58 | 0,84 |
//! | centroide da bateria | 7092 Hz | 6661 Hz |
//!
//! Aqui a partitura é tocada com o banco, e essas três passam a vir do próprio banco.
//!
//! **Por que `rustysynth` e não o código do Zeebulator.** Mesmo com a licença compatível, o
//! `rustysynth` é **MIT** e **Rust puro**, então entra no core Libretro sem trazer biblioteca de
//! host nenhuma, que é a regra do projeto.
//!
//! **Por que o banco não vem embutido.** São 32 MB (medido: GeneralUser GS, 32.319.396 B) e a
//! carga pede +64 MiB de RSS, porque as amostras viram `float`. Embutido, o `.so` do core sairia de
//! 15,6 MB para ~48 MB em seis alvos de CI — e um core que baixa 48 MB para rodar um jogo de 1 MB
//! não se paga. O banco é **opcional**, procurado na pasta do aparelho, e sem ele o MIDI volta para
//! a tabela de timbres, que é o que já toca hoje.
//!
//! O caminho de busca é o mesmo da fonte do sistema: a pasta do aparelho que o frontend entregou
//! primeiro, e depois o perfil do desktop. `ZEEBX_SOUNDFONT` aponta um arquivo direto, para
//! experimentar sem instalar nada.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::audio::wav::Sound;

/// Teto de duração de uma música sintetizada, em segundos.
///
/// O mesmo do sintetizador de tabela, e pela mesma razão: uma partitura com defeito não pode
/// reservar memória sem fim.
///
/// **Não é um teto de conveniência: é o mesmo número dos dois caminhos.** A primeira versão tinha
/// 36 s aqui, e a música do Double Dragon — 47,5 s — saía cortada, o que só apareceu porque a
/// medição comparou a duração com a da referência. Dois caminhos que dizem tocar a mesma partitura
/// não podem ter tetos diferentes.
const MAX_SEGUNDOS: f64 = 300.0;

/// Quantos quadros por bloco na renderização. O sequenciador do `rustysynth` trabalha em blocos.
const BLOCO: usize = 1024;

/// A taxa em que o banco é sintetizado, em Hz.
///
/// **Não é a taxa da tabela de timbres, e a diferença é medida.** A tabela sintetiza a 22.050 Hz
/// porque o que ela produz é soma de harmônicos que ela mesma escolhe, e acima de 11 kHz não há
/// nada ali. O banco é o contrário: das 920 amostras do `GeneralUser-GS.sf2`, 373 são gravadas a
/// 44.100 Hz e há amostra a 48.000 — sintetizar a 22.050 joga fora **todo** o conteúdo acima de
/// 11 kHz, e o misturador, que só interpola linearmente, não tem como devolver o que foi cortado.
/// O resultado é o brilho que some: prato, chimbau e ataque de metal ficam abafados.
///
/// 44.100 é a taxa do próprio misturador do core (`SAMPLE_RATE` do frontend Libretro), então aqui
/// não há reamostragem nenhuma no caminho — e é a mesma escolha que o emulador de referência
/// `zeemu` faz no caminho dele de SoundFont.
pub const TAXA_BANCO: u32 = 44_100;

/// A taxa escolhida agora, que começa em [`TAXA_BANCO`] e o frontend pode mudar.
///
/// **É um global, e isso é escolha consciente.** A alternativa seria levar a taxa por parâmetro de
/// `Session::start_*` até `Machine` e daí até aqui, como se fez com a política de sintetizador —
/// e naquele caso valeu a pena, porque a escolha muda o que a máquina **é** quando nasce. Esta
/// não: ela vale para a próxima música sintetizada, e uma música já sintetizada não muda de taxa.
/// Um parâmetro a mais em cinco assinaturas públicas para um valor que ninguém precisa no
/// nascimento é custo sem troco. Este módulo já guarda um global pelo mesmo motivo — o cache de
/// bancos abertos.
static TAXA_ESCOLHIDA: AtomicU32 = AtomicU32::new(TAXA_BANCO);

/// Muda a taxa em que o banco será sintetizado daqui para a frente.
///
/// Valores fora de 8.000–48.000 são ignorados: o `rustysynth` recusa fora de 16.000–192.000, e uma
/// taxa absurda vinda de um `.opt` editado à mão não pode derrubar o som.
pub fn define_taxa(taxa: u32) {
    if (8_000..=48_000).contains(&taxa) {
        TAXA_ESCOLHIDA.store(taxa, Ordering::Relaxed);
    }
}

/// A taxa em que o banco é sintetizado agora.
pub fn taxa() -> u32 {
    TAXA_ESCOLHIDA.load(Ordering::Relaxed)
}

/// As vozes escolhidas agora. Ver [`VOZES`] para o porquê do padrão.
static VOZES_ESCOLHIDAS: AtomicUsize = AtomicUsize::new(VOZES);

/// Muda o teto de vozes simultâneas daqui para a frente.
///
/// O `rustysynth` aceita de 8 a 256 e recusa fora disso, então o valor é preso à faixa em vez de
/// recusado: quem pediu 512 quer o máximo, e falhar a síntese inteira por causa disso seria pior.
pub fn define_vozes(vozes: usize) {
    VOZES_ESCOLHIDAS.store(vozes.clamp(8, 256), Ordering::Relaxed);
}

/// Quantas vozes o banco pode tocar ao mesmo tempo.
///
/// O padrão do `rustysynth` é 64, e a trilha do Double Dragon usa até nove canais simultâneos com
/// acordes: 64 vozes roubam nota em trecho denso, e roubo de voz soa como nota que some. 128 é o
/// que o `zeemu` usa, e o teto do `rustysynth` é 256.
const VOZES: usize = 128;

/// O volume mestre do sintetizador, antes de qualquer soma.
///
/// **É ganho fixo, e não normalização — essa é a correção.** A versão anterior normalizava o pico
/// de cada música para 0,8, e isso é medida errada por construção: uma trilha calma e esparsa
/// subia até encostar no mesmo teto de uma trilha densa e cheia, de modo que o jogo perdia a
/// diferença de intensidade entre elas e o equilíbrio com os efeitos (que são WAVE e **não** são
/// normalizados) mudava a cada música que entrava.
///
/// Os dois motores de referência usam ganho fixo pela mesma razão: o `zeebulator` aplica -16 dB e
/// o `zeemu` aplica -8 dB, ambos **antes** da soma interna do sintetizador, que é onde o corte
/// aconteceria. Aqui o número é o padrão do próprio `rustysynth` (0,5, ou -6 dB), que é o ponto em
/// que a trilha do Double Dragon foi medida com pico de 0,53 — perto do teto, sem encostar.
///
/// Fica como constante para ser calibrado de ouvido, que é como os dois motores de referência
/// chegaram aos números deles.
const VOLUME_MESTRE: f32 = 0.5;

/// O teto do pico depois da soma.
///
/// Acima disto a onda **corta** no misturador, e corte é distorção. Abaixo, nada é mexido: é
/// limitador, não normalizador — só desce o que passou do teto, e nunca sobe o que está baixo.
const TETO: f32 = 0.95;

/// Onde o banco é procurado, em ordem de preferência.
///
/// `ZEEBX_SOUNDFONT` primeiro porque é o caminho de quem está experimentando; depois a pasta do
/// aparelho, que é o que o frontend controla; e por último o perfil do desktop.
pub fn candidatos(aparelho: &Path) -> Vec<PathBuf> {
    let mut saida = Vec::new();
    if let Some(caminho) = std::env::var_os("ZEEBX_SOUNDFONT") {
        saida.push(PathBuf::from(caminho));
    }
    saida.extend(candidatos_em(&pastas_padrao(aparelho)));
    saida
}

/// As pastas onde o banco é procurado, na ordem: a do aparelho, que o frontend controla, e a do
/// perfil do desktop.
///
/// Separada de [`candidatos`] para os testes poderem afirmar sem depender do perfil de quem roda.
/// **Um `.sf2` no `~/.config/zeebx` — que é onde o LEIAME manda pôr — deixava a bateria vermelha**,
/// porque duas provas perguntavam "não há banco nenhum" com o disco do desenvolvedor cheio.
pub fn pastas_padrao(aparelho: &Path) -> Vec<PathBuf> {
    vec![
        aparelho.join("soundfonts"),
        crate::config::config_dir().join("aparelho").join("soundfonts"),
    ]
}

/// Os bancos das pastas dadas, em ordem. Sem o perfil e sem o `ZEEBX_SOUNDFONT`.
pub fn candidatos_em(pastas: &[PathBuf]) -> Vec<PathBuf> {
    let mut saida = Vec::new();
    for base in pastas {
        for nome in bancos_em(base) {
            saida.push(nome);
        }
    }
    saida
}

/// Os `.sf2` de uma pasta, em ordem estável.
///
/// Estável porque dois bancos no mesmo diretório não podem dar resultados diferentes entre
/// execuções por causa da ordem em que o sistema de arquivos lista — a mesma razão da fonte.
fn bancos_em(base: &Path) -> Vec<PathBuf> {
    let Ok(entradas) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut bancos: Vec<PathBuf> = entradas
        .filter_map(Result::ok)
        .map(|entrada| entrada.path())
        .filter(|caminho| {
            caminho
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sf2"))
        })
        .collect();
    bancos.sort();
    bancos
}

/// O primeiro banco que existir de verdade, entre os candidatos.
pub fn primeiro_banco(aparelho: &Path) -> Option<PathBuf> {
    primeiro_banco_em(&pastas_padrao(aparelho))
}

/// O primeiro banco de verdade **só** nas pastas dadas: a busca sem o perfil de quem roda.
pub fn primeiro_banco_em(pastas: &[PathBuf]) -> Option<PathBuf> {
    candidatos_em(pastas).into_iter().find(|c| c.is_file())
}

/// O tamanho declarado do bloco `smpl` do `sdta`, em bytes, se o arquivo o declarar.
///
/// Percorre a cadeia do RIFF como o `rustysynth` a percorre — `RIFF`/`sfbk` e, dentro do
/// `LIST`/`sdta`, os sub-blocos — porque é **o `smpl` do `sdta`** que a dependência lê com
/// `slice::from_raw_parts_mut` (`binary_reader.rs`, `read_wave_data`). Devolve `None` para tudo o
/// que não chegue até lá: quem julga um arquivo estranho é a biblioteca, não esta varredura, e o
/// caminho normal (o `.sf2` do usuário) tem de continuar chegando inteiro ao sintetizador.
fn tamanho_do_smpl(bytes: &[u8]) -> Option<u32> {
    /// O identificador de quatro letras na posição `pos`, se houver quatro bytes lá.
    fn id(bytes: &[u8], pos: usize) -> Option<&[u8]> {
        bytes.get(pos..pos.checked_add(4)?)
    }

    /// O tamanho declarado na cabeça do bloco que começa em `pos`, em bytes.
    fn tamanho(bytes: &[u8], pos: usize) -> Option<usize> {
        let campo: [u8; 4] = bytes.get(pos.checked_add(4)?..pos.checked_add(8)?)?.try_into().ok()?;
        Some(u32::from_le_bytes(campo) as usize)
    }

    if id(bytes, 0)? != b"RIFF" || id(bytes, 8)? != b"sfbk" {
        return None;
    }
    // Os blocos de primeiro nível, a partir do fim do cabeçalho do RIFF. O avanço é o tamanho
    // declarado mais o preenchimento par do RIFF, como no `bloco` dos testes.
    let mut pos = 12usize;
    while pos.checked_add(8).is_some_and(|fim| fim <= bytes.len()) {
        let Some(bloco) = id(bytes, pos) else { return None };
        let Some(declarado) = tamanho(bytes, pos) else { return None };
        let corpo = pos + 8;
        if bloco == b"LIST" && id(bytes, corpo) == Some(b"sdta".as_slice()) {
            // Dentro do `sdta`: o `smpl` das amostras e o `sm24` dos oito bits extras.
            let fim = corpo.saturating_add(declarado).min(bytes.len());
            let mut sub = corpo + 4;
            while sub.checked_add(8).is_some_and(|f| f <= fim) {
                let (Some(nome), Some(tam)) = (id(bytes, sub), tamanho(bytes, sub)) else {
                    break;
                };
                if nome == b"smpl" {
                    return Some(tam as u32);
                }
                let Some(proximo) = sub.checked_add(8 + tam + (tam & 1)) else {
                    break;
                };
                sub = proximo;
            }
        }
        let Some(proximo) = corpo.checked_add(declarado) else {
            return None;
        };
        pos = proximo + (declarado & 1);
    }
    None
}

/// Um banco carregado.
///
/// Caro de construir (32 MB de amostras convertidas para `float`) e barato de reusar, então fica
/// guardado por caminho: um jogo que toca doze músicas carrega o banco uma vez.
pub struct Banco {
    /// `Arc` porque o `Synthesizer` do `rustysynth` pede a fonte por `Arc`: uma voz nova por
    /// música, e o banco de 32 MB não é copiado junto.
    fonte: Arc<rustysynth::SoundFont>,
}

impl Banco {
    fn carrega(caminho: &Path) -> Option<Self> {
        let t0 = Instant::now();
        let bytes = std::fs::read(caminho).ok()?;
        let read_elapsed = t0.elapsed();
        // **A guarda contra a escrita de um byte além da alocação.** O `rustysynth` 1.3.6
        // (`binary_reader.rs`, `read_wave_data`) reserva `Vec<i16>` de `tamanho / 2` elementos e
        // cria com `slice::from_raw_parts_mut` uma fatia de `tamanho` **bytes**: com o `smpl` de
        // tamanho ímpar a fatia é um byte maior que a alocação, e a leitura escreve fora dela. É
        // UB acionada por arquivo do usuário — banco truncado ou montado por outra ferramenta —,
        // não por jogo, então a recusa é nossa e vem antes do parse. O banco inteiro é recusado, e
        // `None` faz o MIDI voltar para a tabela de timbres, como em qualquer banco que não abre.
        if let Some(tamanho) = tamanho_do_smpl(&bytes)
            && tamanho % 2 == 1
        {
            crate::registro!(
                crate::registro::Nivel::Erro,
                "soundfont",
                "banco {} recusado: o bloco `smpl` tem {tamanho} bytes (tamanho ímpar) e o \
                 `rustysynth` escreveria um byte além da alocação; o bloco `smpl` tem de ter \
                 tamanho par, e o banco tem de ser regerado",
                caminho.display()
            );
            return None;
        }
        let t_parse = Instant::now();
        let mut leitor = std::io::Cursor::new(bytes);
        let fonte = Arc::new(rustysynth::SoundFont::new(&mut leitor).ok()?);
        let parse_elapsed = t_parse.elapsed();
        crate::registro!(
            crate::registro::Nivel::Depuracao,
            "midi",
            "banco {} carregado em {:.1}ms (leitura: {:.1}ms, parse/amostras: {:.1}ms, presets: {})",
            caminho.display(),
            t0.elapsed().as_secs_f64() * 1000.0,
            read_elapsed.as_secs_f64() * 1000.0,
            parse_elapsed.as_secs_f64() * 1000.0,
            fonte.get_presets().len()
        );
        Some(Self { fonte })
    }

    /// Quantos presets o banco tem, para o relatório dizer que ele é um banco de verdade.
    pub fn presets(&self) -> usize {
        self.fonte.get_presets().len()
    }
}

/// O banco já carregado, por caminho. Um só na prática; o mapa tolera que o caminho mude.
static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Banco>>>> = OnceLock::new();

/// Carrega o banco de `caminho`, reusando o que já estiver em memória.
pub fn abre(caminho: &Path) -> Option<Arc<Banco>> {
    let guarda = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut mapa = guarda.lock().ok()?;
    if let Some(banco) = mapa.get(caminho) {
        crate::registro!(
            crate::registro::Nivel::Depuracao,
            "soundfont",
            "banco {} reutilizado do cache",
            caminho.display()
        );
        return Some(banco.clone());
    }
    let comeco = Instant::now();
    let banco = Arc::new(Banco::carrega(caminho)?);
    // O tempo de carga do banco é o que explica a demora da primeira música no portátil; ele
    // vive aqui, e não no mixer, porque é aqui que o trabalho acontece.
    crate::registro!(
        crate::registro::Nivel::Informacao,
        "soundfont",
        "banco {} de {} preset(s) aberto em {} ms",
        caminho.display(),
        banco.presets(),
        comeco.elapsed().as_millis()
    );
    mapa.insert(caminho.to_path_buf(), banco.clone());
    Some(banco)
}

/// Toca uma partitura SMF com o banco, devolvendo PCM mono na taxa pedida.
///
/// `None` quando o banco não abre, quando os bytes não são um SMF que o `rustysynth` aceite, ou
/// quando não sobra nenhuma amostra — e aí quem chamou segue para a tabela de timbres.
pub fn toca(banco: &Banco, bytes: &[u8], taxa: u32) -> Option<Sound> {
    let t0 = Instant::now();
    let mut leitor = std::io::Cursor::new(bytes);
    let midi = rustysynth::MidiFile::new(&mut leitor).ok()?;
    let comprimento = midi.get_length().min(MAX_SEGUNDOS);
    let mut ajustes = rustysynth::SynthesizerSettings::new(taxa as i32);
    // Perfil de síntese seco e eficiente:
    // 1. `block_size = 1024`: reduz em ~1,9x o overhead de blocos e sincronização do sequenciador.
    // 2. `enable_reverb_and_chorus = false`: aproxima o áudio do comportamento seco nativo do
    //    console / CMX e do TinySoundFont (que não implementa efeitos de reverberação/chorus).
    ajustes.block_size = BLOCO;
    ajustes.enable_reverb_and_chorus = false;
    // 3. `maximum_polyphony`: ver [`VOZES`] — o padrão de 64 rouba nota em trecho denso.
    ajustes.maximum_polyphony = VOZES_ESCOLHIDAS.load(Ordering::Relaxed);
    let mut sintetizador = rustysynth::Synthesizer::new(&banco.fonte, &ajustes).ok()?;
    // 4. O volume mestre é fixo, e não vem de normalização depois. Ver [`VOLUME_MESTRE`].
    sintetizador.set_master_volume(VOLUME_MESTRE);
    let mut sequencia = rustysynth::MidiFileSequencer::new(sintetizador);
    sequencia.play(&Arc::new(midi), false);

    let total = ((comprimento + 0.5) * f64::from(taxa)) as usize + 1;
    let mut amostras = Vec::with_capacity(total);
    let (mut esquerda, mut direita) = (vec![0.0f32; BLOCO], vec![0.0f32; BLOCO]);
    while amostras.len() < total {
        sequencia.render(&mut esquerda, &mut direita);
        for i in 0..BLOCO {
            if amostras.len() >= total {
                break;
            }
            // Mono: a média dos dois canais. O console tem uma caixa só, e o mixer duplica depois.
            amostras.push(0.5 * (esquerda[i] + direita[i]));
        }
        if sequencia.end_of_sequence() {
            break;
        }
    }
    // A cauda: o banco pode terminar antes do comprimento declarado, e um som mais curto que a
    // partitura faria o jogo achar que a música acabou cedo.
    amostras.resize(total, 0.0);
    // **Limitar, e não normalizar.** O ganho já foi dado uma vez, fixo, no volume mestre do
    // sintetizador (ver [`VOLUME_MESTRE`]): o que sobra aqui é só impedir que uma soma densa passe
    // do teto e corte. Uma música que ficou baixa **continua** baixa, porque é assim que ela é.
    let pico = limita(&mut amostras);
    let elapsed = t0.elapsed();
    crate::registro!(
        crate::registro::Nivel::Informacao,
        "midi",
        "banco: {} bytes de SMF -> {:.1}s de áudio ({} amostras @ {}Hz, pico bruto {:.3}) sintetizados em {:.1}ms ({:.2}x tempo real)",
        bytes.len(),
        comprimento,
        amostras.len(),
        taxa,
        pico,
        elapsed.as_secs_f64() * 1000.0,
        if elapsed.as_secs_f64() > 0.0 { comprimento / elapsed.as_secs_f64() } else { 0.0 }
    );
    Some(Sound {
        rate: taxa,
        channels: 1,
        samples: amostras,
    })
}

/// Desce o volume só quando o pico passou de [`TETO`]. Devolve o pico **antes** de mexer.
///
/// É o contrário de normalizar: normalizar iguala o pico de toda música, e com isso apaga a
/// diferença de intensidade entre uma trilha calma e uma cheia. Aqui, música baixa continua baixa,
/// e só a que encostaria no teto desce — pelo fator da música inteira, o que preserva a proporção
/// entre as vozes dela.
///
/// Devolver o pico bruto é o que permite calibrar [`VOLUME_MESTRE`] de ouvido com número na mão:
/// sem isso, saber se o ganho fixo está perto do teto exige gravar o áudio e medir fora.
fn limita(amostras: &mut [f32]) -> f32 {
    let pico = amostras.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    if pico > TETO {
        let fator = TETO / pico;
        for amostra in amostras.iter_mut() {
            *amostra *= fator;
        }
    }
    pico
}


/// Onde o banco deve ficar, e se já há um — em uma linha, para o frontend mostrar.
///
/// Existe porque a busca é por diretório e a pasta não é óbvia: no core Libretro ela sai da raiz
/// de sistema que o frontend entregou, e quem instala o core não tem como adivinhar o caminho. A
/// alternativa — inventar mais um diretório de busca — troca um aviso claro por um palpite, e
/// palpite em caminho de arquivo é o defeito que se paga com "não funciona e não diz por quê".
pub fn relato(aparelho: &Path) -> String {
    relato_de(aparelho, primeiro_banco(aparelho))
}

/// O mesmo relato, com o banco já resolvido.
///
/// O `Option` entra por parâmetro para a prova do lado "sem banco" não depender do perfil de quem
/// roda — ver [`pastas_padrao`].
fn relato_de(aparelho: &Path, banco: Option<PathBuf>) -> String {
    match banco {
        Some(caminho) => format!(
            "Zeebx: banco de amostras do MIDI em {}; a trilha toca com as amostras",
            caminho.display()
        ),
        None => {
            let pasta = aparelho.join("soundfonts");
            format!(
                "Zeebx: sem banco de amostras do MIDI; a trilha toca com a tabela de timbres. \
                 Para ouvir com amostras, ponha um .sf2 em {} (ou aponte ZEEBX_SOUNDFONT)",
                pasta.display()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A taxa e as vozes escolhidas pelo frontend param no que o sintetizador aceita.
    ///
    /// **O que se cobra é o valor absurdo não passar.** As duas vêm de um arquivo que o usuário
    /// edita à mão, e o `rustysynth` recusa a criação do sintetizador fora das faixas dele — uma
    /// recusa que chegaria ao jogo como música que simplesmente não toca, sem dizer por quê.
    ///
    /// O teste devolve os padrões no fim porque o estado é global e compartilhado pelos outros
    /// testes deste módulo (ver [`TAXA_ESCOLHIDA`]); deixá-lo sujo faria a ordem dos testes mudar
    /// o resultado deles.
    #[test]
    fn a_taxa_e_as_vozes_escolhidas_ficam_na_faixa_que_o_sintetizador_aceita() {
        define_taxa(22_050);
        assert_eq!(taxa(), 22_050);
        // Fora da faixa é **ignorado**, e não preso: uma taxa absurda costuma ser arquivo
        // estragado, e herdar a anterior é mais seguro que inventar um número.
        define_taxa(1);
        assert_eq!(taxa(), 22_050, "taxa absurda não podia ter passado");
        define_taxa(999_999);
        assert_eq!(taxa(), 22_050, "taxa absurda não podia ter passado");
        define_taxa(TAXA_BANCO);
        assert_eq!(taxa(), TAXA_BANCO);

        // As vozes são **presas** à faixa, e não ignoradas: quem pede 512 quer o máximo, e o
        // máximo é um pedido que dá para atender.
        define_vozes(4);
        assert_eq!(VOZES_ESCOLHIDAS.load(Ordering::Relaxed), 8);
        define_vozes(512);
        assert_eq!(VOZES_ESCOLHIDAS.load(Ordering::Relaxed), 256);
        define_vozes(48);
        assert_eq!(VOZES_ESCOLHIDAS.load(Ordering::Relaxed), 48);
        define_vozes(VOZES);
    }

    /// O banco de teste, do harness de comparação que vive fora do repositório.
    fn banco_de_teste() -> Option<PathBuf> {
        let candidatos = [
            std::env::var_os("ZEEBX_SOUNDFONT").map(PathBuf::from),
            Some(PathBuf::from(
                "/home/rafaelfrequiao/zeebx-midi-harness/zbfont/GeneralUser-GS.sf2",
            )),
        ];
        candidatos.into_iter().flatten().find(|c| c.is_file())
    }

    /// Um SMF de uma nota, para não depender de arquivo.
    fn uma_nota(programa: u8, nota: u8, _pulsos: u32) -> Vec<u8> {
        let mut trilha = vec![0x00, 0xc0, programa, 0x00, 0x90, nota, 100];
        trilha.extend([0x81, 0x70]);
        trilha.extend([0x80, nota, 0x40, 0x00, 0xff, 0x2f, 0x00]);
        let mut out = b"MThd".to_vec();
        out.extend(6u32.to_be_bytes());
        out.extend(0u16.to_be_bytes());
        out.extend(1u16.to_be_bytes());
        out.extend(96i16.to_be_bytes());
        out.extend(b"MTrk");
        out.extend((trilha.len() as u32).to_be_bytes());
        out.extend(trilha);
        out
    }

    /// **O banco carrega e toca.** O teste se declara dispensado quando não há banco no disco, e
    /// diz isso: um teste que passa sem provar nada é pior que um teste ausente.
    #[test]
    fn o_banco_toca_a_partitura() {
        let Some(caminho) = banco_de_teste() else {
            eprintln!("sem banco .sf2 no disco: teste dispensado (use ZEEBX_SOUNDFONT)");
            return;
        };
        let banco = abre(&caminho).expect("o banco abre");
        assert!(
            banco.presets() > 100,
            "um banco General MIDI tem centenas de presets, tem {}",
            banco.presets()
        );
        let som = toca(&banco, &uma_nota(0, 69, 240), 22_050).expect("toca");
        assert_eq!(som.rate, 22_050);
        assert_eq!(som.channels, 1);
        let pico = som.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(pico > 0.01, "o piano tinha de soar, pico {pico}");
    }


    /// Um SoundFont2 mínimo, montado byte a byte, para o teste não depender de arquivo no disco.
    ///
    /// **Por que isto existe.** Os testes que usam o banco de verdade se dispensam quando não há
    /// banco na máquina — e no CI não há. Um teste que passa sem provar nada é pior que um teste
    /// ausente, então o caminho do banco precisa de um caso que **sempre** rode.
    ///
    /// A estrutura não foi adivinhada: ela foi lida do parser do `rustysynth`, porque o formato
    /// tem duas armadilhas que custaram duas tentativas — o registro de zona tem **4 bytes**
    /// (`wGenNdx`, `wModNdx`) e não quatro números, e o `SAMPLE_ID` (53) tem de ser o **último**
    /// gerador da zona, senão o parser trata a primeira zona como zona global e não sobra região
    /// nenhuma para tocar.
    fn banco_minimo() -> Vec<u8> {
        let taxa = 22_050u32;
        let quadros = 64usize;
        let cru = amostras_do_banco_minimo(taxa, quadros);
        banco_minimo_com_smpl(&cru, taxa, quadros)
    }

    /// As amostras do banco mínimo como bytes crus: 64 quadros de seno a 440 Hz mais os 46 zeros
    /// que o `smpl` exige no fim.
    fn amostras_do_banco_minimo(taxa: u32, quadros: usize) -> Vec<u8> {
        let mut cru = Vec::new();
        for i in 0..quadros {
            let angulo = std::f64::consts::TAU * 440.0 * i as f64 / f64::from(taxa);
            cru.extend(((angulo.sin() * 20_000.0) as i16).to_le_bytes());
        }
        cru.extend(std::iter::repeat_n(0u8, 46 * 2));
        cru
    }

    /// O mesmo banco, com os bytes crus do `smpl` entregues por quem chama.
    ///
    /// **Por que os bytes crus, e não as amostras.** É o que permite montar o `smpl` de tamanho
    /// **ímpar**, o caso da guarda contra a escrita além da alocação. O resto da estrutura continua
    /// a de um banco que o parser aceita — sem isso o teste provaria que um arquivo estragado é
    /// recusado, e não que o bloco ímpar é barrado **antes** de chegar à dependência.
    fn banco_minimo_com_smpl(cru: &[u8], taxa: u32, quadros: usize) -> Vec<u8> {
        fn bloco(nome: &[u8; 4], corpo: &[u8]) -> Vec<u8> {
            let mut out = nome.to_vec();
            out.extend((corpo.len() as u32).to_le_bytes());
            out.extend(corpo);
            if corpo.len() % 2 == 1 {
                out.push(0);
            }
            out
        }

        fn lista(tipo: &[u8; 4], partes: &[Vec<u8>]) -> Vec<u8> {
            let mut corpo = tipo.to_vec();
            for parte in partes {
                corpo.extend(parte);
            }
            bloco(b"LIST", &corpo)
        }

        fn nome_fixo(nome: &str) -> Vec<u8> {
            let mut out = nome.as_bytes().to_vec();
            out.resize(20, 0);
            out
        }

        /// Um registro de zona: 4 bytes, `wGenNdx` e `wModNdx`.
        fn zona(geradores: u16, moduladores: u16) -> Vec<u8> {
            [geradores.to_le_bytes(), moduladores.to_le_bytes()].concat()
        }

        /// Os geradores de uma zona, com o terminador que o formato pede.
        fn geradores(itens: &[(u16, u16)]) -> Vec<u8> {
            let mut corpo = Vec::new();
            for (tipo, valor) in itens {
                corpo.extend(tipo.to_le_bytes());
                corpo.extend(valor.to_le_bytes());
            }
            corpo.extend([0u8; 4]);
            corpo
        }

        // --- INFO: só a versão e o nome ---
        let info = lista(
            b"INFO",
            &[
                bloco(b"ifil", &[2u16.to_le_bytes(), 1u16.to_le_bytes()].concat()),
                // **`INAM` em maiúsculas, e não `inam`**: o parser aceita só as quatro letras
                // exatas do formato, e a minúscula derruba a carga inteira com
                // `ListContainsUnknownId`. Custou uma tentativa.
                bloco(b"INAM", &{
                    let mut n = b"Banco minimo do teste".to_vec();
                    n.push(0);
                    n
                }),
            ],
        );

        // --- sdta: as amostras ---
        // **Sem o preenchimento par do RIFF, e não pelo `bloco`.** É a única artificialidade do
        // arquivo, e ela é do teste, não do formato: o `rustysynth` lê os sub-blocos até o tamanho
        // do `LIST` e **não** pula o preenchimento. Com o `smpl` de tamanho par (o caso do
        // `banco_minimo`) não há preenchimento nenhum, e o `LIST` sai byte a byte igual ao do
        // `bloco`; com o `smpl` ímpar do outro teste, o byte de preenchimento ficaria entre as
        // amostras e o `LIST` da `pdta`, e o parser o leria como o começo do identificador — o
        // arquivo seria recusado por desalinhamento, e não pela guarda, que é o que se mede aqui.
        let smpl = {
            let mut out = b"smpl".to_vec();
            out.extend((cru.len() as u32).to_le_bytes());
            out.extend(cru);
            out
        };
        let sdta = {
            let mut corpo = b"sdta".to_vec();
            corpo.extend(smpl);
            let mut out = b"LIST".to_vec();
            out.extend((corpo.len() as u32).to_le_bytes());
            out.extend(corpo);
            out
        };

        // --- pdta ---
        // phdr: preset 0 e o terminador. `wPresetBagNdx` do terminador é o número de zonas.
        let mut phdr = nome_fixo("Piano do teste");
        phdr.extend(0u16.to_le_bytes()); // wPreset
        phdr.extend(0u16.to_le_bytes()); // wBank
        phdr.extend(0u16.to_le_bytes()); // wPresetBagNdx
        phdr.extend(0u32.to_le_bytes()); // wLibrary
        phdr.extend(0u32.to_le_bytes()); // wGenre
        phdr.extend(0u32.to_le_bytes()); // wMorphology
        let mut phdr_fim = nome_fixo("EOP");
        phdr_fim.extend(0u16.to_le_bytes()); // wPreset
        phdr_fim.extend(0u16.to_le_bytes()); // wBank
        phdr_fim.extend(1u16.to_le_bytes()); // wPresetBagNdx: uma zona de preset
        phdr_fim.extend([0u8; 12]); // wLibrary, wGenre, wMorphology
        let phdr = bloco(b"phdr", &[phdr, phdr_fim].concat());

        // Uma zona de preset, com a faixa de teclas e o instrumento 0. **O `INSTRUMENT` (41) por
        // último**, pela mesma razão do `SAMPLE_ID` do lado do instrumento: com ele no meio, o
        // parser trata esta zona como global e o preset fica sem região — e o sintoma é silêncio,
        // não erro.
        let pbag = bloco(b"pbag", &[zona(0, 0), zona(2, 0)].concat());
        let pmod = bloco(b"pmod", &[0u8; 10]);
        let pgen = bloco(b"pgen", &geradores(&[(43, 0x7f00), (41, 0)]));

        // inst: instrumento 0 e o terminador.
        let mut inst0 = nome_fixo("Inst do teste");
        inst0.extend(0u16.to_le_bytes());
        let mut inst_fim = nome_fixo("EOI");
        inst_fim.extend(1u16.to_le_bytes());
        let inst = bloco(b"inst", &[inst0, inst_fim].concat());

        // Uma zona de instrumento. **`SAMPLE_ID` por último**: com ele no meio, o parser trata esta
        // zona como global e o instrumento fica sem região.
        let ibag = bloco(b"ibag", &[zona(0, 0), zona(4, 0)].concat());
        let imod = bloco(b"imod", &[0u8; 10]);
        let igen = bloco(b"igen", &geradores(&[(43, 0x7f00), (54, 1), (58, 69), (53, 0)]));

        // shdr: a amostra 0 e o terminador.
        let mut shdr = nome_fixo("Amostra do teste");
        shdr.extend(0u32.to_le_bytes()); // inicio
        shdr.extend((quadros as u32 - 1).to_le_bytes()); // fim
        shdr.extend(0u32.to_le_bytes()); // inicio do laço
        shdr.extend((quadros as u32 - 1).to_le_bytes()); // fim do laço
        shdr.extend(taxa.to_le_bytes());
        shdr.push(60); // nota original
        shdr.push(0); // correção de tom
        shdr.extend(0u16.to_le_bytes()); // link
        shdr.extend(1u16.to_le_bytes()); // tipo: mono
        let mut eos = nome_fixo("EOS");
        eos.extend([0u8; 26]);
        let shdr = bloco(b"shdr", &[shdr, eos].concat());

        let pdta = lista(b"pdta", &[phdr, pbag, pmod, pgen, inst, ibag, imod, igen, shdr]);

        let mut sfbk = b"sfbk".to_vec();
        sfbk.extend(info);
        sfbk.extend(sdta);
        sfbk.extend(pdta);
        bloco(b"RIFF", &sfbk)
    }


    /// **Um banco corrompido não pode derrubar o jogo.** Ele é um arquivo que o usuário baixa e
    /// copia à mão: um download truncado ou um `.sf2` de outro formato é cenário real, não
    /// hipótese. O motor tem de seguir com a tabela de timbres e não dizer nada de errado.
    #[test]
    fn um_banco_estragado_nao_derruba_nada() {
        let pasta = std::env::temp_dir().join("zeebx-banco-estragado.sf2");
        // Cabeçalho de RIFF válido e o resto lixo: passa a checagem mais óbvia e falha na leitura
        // das listas, que é onde um arquivo truncado de verdade falha.
        let mut lixo = b"RIFF".to_vec();
        lixo.extend(1_000u32.to_le_bytes());
        lixo.extend(b"sfbk".to_vec());
        lixo.extend(vec![0x7f; 4_000]);
        std::fs::write(&pasta, &lixo).expect("escreve o lixo");

        assert!(
            abre(&pasta).is_none(),
            "um banco estragado tem de ser recusado, e não aceito pela metade"
        );
        // Com o banco recusado, o MIDI segue pela tabela de timbres — o caminho de produção
        // (`machine::media`) pergunta por `banco_de_som`, que é `None`, e cai no sintetizador.
        let som = crate::audio::midi::decode(&uma_nota(0, 69, 240)).expect("a tabela atende");
        assert!(som.samples.iter().any(|s| *s != 0.0), "a tabela tinha de soar");
        let _ = std::fs::remove_file(&pasta);
    }

    /// **Um `smpl` de tamanho ímpar é recusado antes de chegar à dependência.**
    ///
    /// O `rustysynth` 1.3.6 (`binary_reader.rs`, `read_wave_data`) reserva `Vec<i16>` de
    /// `tamanho / 2` elementos e cria com `slice::from_raw_parts_mut` uma fatia de `tamanho`
    /// **bytes**: com o `smpl` de tamanho ímpar a fatia é um byte maior que a alocação, e a leitura
    /// escreve fora dela. É UB no caminho do banco do usuário — arquivo que ele copia à mão, e o
    /// defeito está na biblioteca —, então a guarda é nossa e vem antes do parse.
    ///
    /// **Sem a guarda este teste falha, e é isso que ele mede:** o banco montado aqui é válido em
    /// tudo o mais (o mesmo do `banco_minimo`), então o `rustysynth` o aceita — com a escrita de um
    /// byte além da alocação — e `abre` devolve `Some` em vez de `None`.
    #[test]
    fn banco_com_smpl_impar_e_recusado() {
        let pasta = std::env::temp_dir().join("zeebx-banco-smpl-impar.sf2");
        let (taxa, quadros) = (22_050u32, 64usize);
        let mut cru = amostras_do_banco_minimo(taxa, quadros);
        // O byte a mais: com ele o `smpl` declara um tamanho ímpar, e o `Vec<i16>` do `rustysynth`
        // passa a ter um byte a menos que a fatia que a biblioteca cria por cima dele.
        cru.push(0);
        assert_eq!(cru.len() % 2, 1, "o caso é o do bloco ímpar");
        std::fs::write(&pasta, banco_minimo_com_smpl(&cru, taxa, quadros)).expect("escreve o banco");

        assert!(
            abre(&pasta).is_none(),
            "um `smpl` de tamanho ímpar tem de ser recusado antes do parse"
        );
        let _ = std::fs::remove_file(&pasta);
    }

    /// **Um caminho que não existe também é recusado em silêncio**, e sem gastar a carga.
    #[test]
    fn banco_ausente_nao_e_erro() {
        let caminho = std::env::temp_dir().join("zeebx-banco-que-nao-existe.sf2");
        let _ = std::fs::remove_file(&caminho);
        assert!(abre(&caminho).is_none());
        // Pelas pastas dadas, e não pelo perfil: com um banco em `~/.config/zeebx` esta afirmação
        // caía — e é para lá que o LEIAME manda copiar o `.sf2`.
        assert!(primeiro_banco_em(&[std::env::temp_dir().join("zeebx-sem-pasta")]).is_none());
    }


    /// **O relato diz onde pôr o banco**, senão "o banco não funciona" fica indistinguível de
    /// "o arquivo está no lugar errado". Este é o lado da build **com** o sintetizador; o lado
    /// sem ele é cobrado em `audio::mod`, e os dois existem porque silêncio numa das builds leva à
    /// conclusão errada.
    #[cfg(feature = "soundfont")]
    #[test]
    fn o_relato_diz_onde_por_o_banco() {
        let aparelho = std::env::temp_dir().join("zeebx-aparelho-sem-banco");
        let _ = std::fs::remove_dir_all(&aparelho);
        let texto = relato_de(&aparelho, None);
        assert!(
            texto.contains("soundfonts"),
            "o relato tem de dizer a pasta: {texto}"
        );
        assert!(
            texto.contains("tabela de timbres"),
            "sem banco, o relato tem de dizer com o que a trilha toca: {texto}"
        );
    }

    /// **O caminho do banco é exercitado sempre**, com um banco montado aqui.
    ///
    /// Os outros testes do módulo usam um `.sf2` de verdade e se dispensam quando não há um — e no
    /// CI não há. Este não se dispensa: monta o banco mínimo, carrega e toca.
    #[test]
    fn o_caminho_do_banco_toca_um_banco_minimo() {
        let pasta = std::env::temp_dir().join("zeebx-banco-minimo.sf2");
        std::fs::write(&pasta, banco_minimo()).expect("escreve o banco");
        let banco = abre(&pasta).expect("o banco mínimo abre");
        assert!(banco.presets() >= 1, "o banco tem de declarar o preset");
        let som = toca(&banco, &uma_nota(0, 69, 240), 22_050).expect("toca");
        assert_eq!(som.channels, 1);
        let pico = som.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(pico > 0.01, "o banco mínimo tinha de soar, pico {pico}");
        // A duração segue a partitura, como no caminho da tabela: som curto demais faria o jogo
        // achar que a música acabou cedo.
        let esperado = 22_050 * 2; // nota de 240 pulsos a 96 por batida e 120 por minuto
        assert!(
            som.samples.len() >= esperado / 2,
            "som curto demais: {} amostras",
            som.samples.len()
        );
        let _ = std::fs::remove_file(&pasta);
    }

    /// **O banco distingue o arco da palheta.** Era o defeito mais audível da tabela de timbres:
    /// 29 (*overdrive*), 30 (distorcida), 42 (violoncelo) e 48 (cordas) saíam com a **mesma onda**.
    ///
    /// A medida é o **ataque**, e não o centroide: numa nota só os dois têm centroide parecido
    /// (medido: 435 Hz e 426 Hz), então centroide não separa. O que separa é o comportamento —
    /// palheta ataca no talo, arco começa macio e cresce. No soundfont de referência a guitarra
    /// começa em 1,00 do próprio pico e o violoncelo em 0,46.
    #[test]
    fn o_banco_distingue_o_arco_da_palheta() {
        let Some(caminho) = banco_de_teste() else {
            eprintln!("sem banco .sf2 no disco: teste dispensado (use ZEEBX_SOUNDFONT)");
            return;
        };
        let banco = abre(&caminho).expect("o banco abre");
        let envelope = |programa: u8| -> Vec<f32> {
            let som = toca(&banco, &uma_nota(programa, 60, 480), 22_050).expect("toca");
            let janela = som.rate as usize / 5;
            let mut saida: Vec<f32> = (0..5)
                .map(|i| {
                    let trecho = &som.samples[i * janela..(i + 1) * janela];
                    (trecho.iter().map(|s| s * s).sum::<f32>() / janela as f32).sqrt()
                })
                .collect();
            let pico = saida.iter().fold(0.0f32, |a, s| a.max(*s));
            if pico > 0.0 {
                for valor in &mut saida {
                    *valor /= pico;
                }
            }
            saida
        };
        let palheta = envelope(29);
        let arco = envelope(42);
        assert!(
            arco[0] < palheta[0] * 0.75,
            "o arco tinha de atacar mais macio que a palheta: {:.2} contra {:.2}",
            arco[0],
            palheta[0]
        );
    }

    #[test]
    fn parse_da_politica_midi_backend() {
        use crate::audio::MidiBackend;
        use std::str::FromStr;

        assert_eq!(MidiBackend::from_str("Auto"), Ok(MidiBackend::Auto));
        assert_eq!(MidiBackend::from_str("automático"), Ok(MidiBackend::Auto));
        assert_eq!(MidiBackend::from_str("Tabela de timbres"), Ok(MidiBackend::Timbres));
        assert_eq!(MidiBackend::from_str("timbres"), Ok(MidiBackend::Timbres));
        assert_eq!(MidiBackend::from_str("SoundFont"), Ok(MidiBackend::SoundFont));
        assert_eq!(MidiBackend::from_str("sf2"), Ok(MidiBackend::SoundFont));
        assert_eq!(MidiBackend::from_str("invalido"), Err(()));
    }
}
