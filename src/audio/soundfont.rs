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
//! **Por que `rustysynth` e não o código do Zeebulator.** O Zeebulator é GPLv3 e o Zeebx é
//! `GPL-2.0-only`; as duas licenças não se combinam, então nada dele pode ser copiado — nem o
//! invólucro em volta do TinySoundFont. O `rustysynth` é **MIT** e **Rust puro**, então entra no
//! core Libretro sem trazer biblioteca de host nenhuma, que é a regra do projeto.
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
use std::sync::{Arc, Mutex, OnceLock};

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

/// Onde o banco é procurado, em ordem de preferência.
///
/// `ZEEBX_SOUNDFONT` primeiro porque é o caminho de quem está experimentando; depois a pasta do
/// aparelho, que é o que o frontend controla; e por último o perfil do desktop.
pub fn candidatos(aparelho: &Path) -> Vec<PathBuf> {
    let mut saida = Vec::new();
    if let Some(caminho) = std::env::var_os("ZEEBX_SOUNDFONT") {
        saida.push(PathBuf::from(caminho));
    }
    for base in [
        aparelho.join("soundfonts"),
        crate::config::config_dir().join("aparelho").join("soundfonts"),
    ] {
        for nome in bancos_em(&base) {
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
    candidatos(aparelho).into_iter().find(|c| c.is_file())
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
        let bytes = std::fs::read(caminho).ok()?;
        let mut leitor = std::io::Cursor::new(bytes);
        let fonte = Arc::new(rustysynth::SoundFont::new(&mut leitor).ok()?);
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
        return Some(banco.clone());
    }
    let banco = Arc::new(Banco::carrega(caminho)?);
    mapa.insert(caminho.to_path_buf(), banco.clone());
    Some(banco)
}

/// Toca uma partitura SMF com o banco, devolvendo PCM mono na taxa pedida.
///
/// `None` quando o banco não abre, quando os bytes não são um SMF que o `rustysynth` aceite, ou
/// quando não sobra nenhuma amostra — e aí quem chamou segue para a tabela de timbres.
pub fn toca(banco: &Banco, bytes: &[u8], taxa: u32) -> Option<Sound> {
    let mut leitor = std::io::Cursor::new(bytes);
    let midi = rustysynth::MidiFile::new(&mut leitor).ok()?;
    let comprimento = midi.get_length().min(MAX_SEGUNDOS);
    let ajustes = rustysynth::SynthesizerSettings::new(taxa as i32);
    let sintetizador = rustysynth::Synthesizer::new(&banco.fonte, &ajustes).ok()?;
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
    // **O nível é o mesmo dos dois caminhos.** A tabela de timbres deixa o pico em 0,8 (ver
    // `midi::normaliza`), e o banco saía em 0,53: a mesma música trocava de volume conforme o
    // aparelho tivesse ou não um `.sf2` instalado, e no jogo isso muda o balanço entre a trilha e
    // os efeitos, que passam pelo mesmo misturador.
    crate::audio::midi::normaliza(&mut amostras);
    Some(Sound {
        rate: taxa,
        channels: 1,
        samples: amostras,
    })
}


/// Onde o banco deve ficar, e se já há um — em uma linha, para o frontend mostrar.
///
/// Existe porque a busca é por diretório e a pasta não é óbvia: no core Libretro ela sai da raiz
/// de sistema que o frontend entregou, e quem instala o core não tem como adivinhar o caminho. A
/// alternativa — inventar mais um diretório de busca — troca um aviso claro por um palpite, e
/// palpite em caminho de arquivo é o defeito que se paga com "não funciona e não diz por quê".
pub fn relato(aparelho: &Path) -> String {
    match primeiro_banco(aparelho) {
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

        // --- amostras: 64 quadros de seno a 440 Hz, mais os 46 zeros que o `smpl` exige no fim ---
        let mut amostras: Vec<i16> = (0..quadros)
            .map(|i| {
                let angulo = std::f64::consts::TAU * 440.0 * i as f64 / f64::from(taxa);
                (angulo.sin() * 20_000.0) as i16
            })
            .collect();
        amostras.extend(std::iter::repeat_n(0i16, 46));

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
        let mut cru = Vec::new();
        for valor in &amostras {
            cru.extend(valor.to_le_bytes());
        }
        let sdta = lista(b"sdta", &[bloco(b"smpl", &cru)]);

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

    /// **Um caminho que não existe também é recusado em silêncio**, e sem gastar a carga.
    #[test]
    fn banco_ausente_nao_e_erro() {
        let caminho = std::env::temp_dir().join("zeebx-banco-que-nao-existe.sf2");
        let _ = std::fs::remove_file(&caminho);
        assert!(abre(&caminho).is_none());
        assert!(primeiro_banco(&std::env::temp_dir().join("zeebx-sem-pasta")) .is_none());
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
        let texto = relato(&aparelho);
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
}
