//! Varredura de uma ROM de verdade, dentro do `cargo test`.
//!
//! O relatório do `run` já diz o que precisamos saber de um jogo: onde parou, o que pediu e não
//! temos, quanto custou. O que faltava era **cobrar** isso. Uma varredura feita à mão, num laço
//! de shell, descobre a regressão na próxima vez que alguém a repetir — que pode ser semanas
//! depois do commit que a causou. Aqui o mesmo levantamento vira teste: a ROM avança, o
//! relatório sai estruturado, e o resumo pode ser comparado com o que já se sabia dela.
//!
//! Nenhuma ROM está na árvore, então **nada aqui roda sozinho**: os testes só fazem trabalho
//! quando `ZEEBX_ROM` aponta para um jogo, e sem isso avisam e passam — que é o certo para quem
//! clonou o repositório sem ter os arquivos. É também o que evita transformar `cargo test` numa
//! varredura de sessenta e cinco jogos sem ninguém ter pedido.
//!
//! ```bash
//! # um jogo, com o relatório inteiro na tela
//! ZEEBX_ROM="roms/Quake.zip" cargo test --release varredura -- --nocapture
//!
//! # a varredura inteira, gravando um relatório por jogo e cobrando a linha de base
//! ZEEBX_ROM=roms ZEEBX_ROM_MS=6000 ZEEBX_ROM_SAIDA=saida ZEEBX_ROM_BASE=docs/varredura \
//!   cargo test --release varredura -- --nocapture
//! ```
//!
//! | Variável | O que faz |
//! |---|---|
//! | `ZEEBX_ROM` | um `.mod`/`.zip`, uma lista separada por vírgula, ou um diretório |
//! | `ZEEBX_ROM_MS` | quanto tempo **virtual** rodar, em ms (padrão 6000, como o levantamento) |
//! | `ZEEBX_ROM_TETO` | teto de tempo **real** por jogo, em segundos (padrão 90) |
//! | `ZEEBX_ROM_SAIDA` | diretório onde gravar o relatório completo de cada jogo |
//! | `ZEEBX_ROM_BASE` | diretório da linha de base; o que não existe é gravado, o que existe é cobrado |
//!
//! O `--release` não é enfeite: em depuração o núcleo emulado roda uma ordem de grandeza mais
//! devagar, e o teto de tempo real classificaria jogo bom como "lento demais".

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::machine::Outcome;
use crate::session::{Session, StartError, Step};

/// Quanto tempo virtual rodar quando ninguém pede outro. Seis segundos é o que o levantamento
/// de compatibilidade usa, e manter o mesmo número deixa os dois comparáveis.
const MS_PADRAO: u32 = 6_000;

/// Teto de tempo real por jogo. Quem não cumprir o tempo virtual dentro dele é "lento demais",
/// que é uma categoria de compatibilidade e não uma falha do teste — o mesmo critério do
/// `timeout 90` da varredura à mão.
const TETO_PADRAO: u64 = 90;

/// Quanto tempo virtual o placar dá a cada ROM. Curto de propósito: é para responder "quem
/// abriu", não para jogar.
const MS_PLACAR: u32 = 5_000;

/// Teto de tempo real por ROM no placar. Um jogo que não cumpre isso é "lento demais" e a
/// varredura segue.
const TETO_PLACAR: u64 = 30;

/// Quantos métodos mais chamados o relatório mostra. É onde gargalo aparece.
const CHAMADAS_MOSTRADAS: usize = 12;

/// Lê os applets instalados de `ZEEBX_ROM_INSTALADOS`, no formato da bancada:
/// `0xCLSID:id,0xCLSID:id`.
///
/// **Sem isto a roda não pode abrir jogo nenhum.** O `IShell::StartApplet` responde
/// `ECLASSNOTSUPPORT` para classe que não está na lista, e a lista é do *host* — a bancada tem o
/// `--instalados=` e a varredura agora aceita a mesma declaração.
fn instalados_do_ambiente(rom: &Path) -> Vec<(u32, String)> {
    let Ok(valor) = std::env::var("ZEEBX_ROM_INSTALADOS") else {
        return Vec::new();
    };
    // **`auto` instala os jogos que estão ao lado da ROM**, com o número da pasta do módulo que
    // cada pacote traz — o mesmo que a interface e o core fazem. Sem o número, a Z-Wheel registra
    // `Tectoy.c:2925 No mod number for this game!!!` e **não lança**: medido, ela chega a pedir a
    // abertura e desiste. Com ele, `abertura pedida: 0x0108e356`. A pasta é a mesma que a varredura
    // já usa para o catálogo, e a lista sai ordenada e sem repetição, para o resultado ser o mesmo
    // entre execuções.
    if valor.trim().eq_ignore_ascii_case("auto") {
        let Some(pasta) = rom.parent() else {
            return Vec::new();
        };
        let mut vistas = std::collections::BTreeSet::new();
        return crate::library::scan(pasta)
            .into_iter()
            .filter_map(|jogo| {
                let classe = jogo.clsid?;
                let id = crate::library::id_do_modulo(&jogo.path)?;
                match vistas.insert((classe, id.clone())) {
                    true => Some((classe, id)),
                    false => None,
                }
            })
            .collect();
    }
    valor
        .split(',')
        .filter_map(|item| {
            let item = item.trim();
            if item.is_empty() {
                return None;
            }
            let (classe, id) = item.split_once(':').unwrap_or((item, ""));
            let classe = u32::from_str_radix(classe.trim().trim_start_matches("0x"), 16).ok()?;
            Some((classe, id.trim().to_string()))
        })
        .collect()
}

/// Lê o roteiro de controle de `ZEEBX_ROM_TECLAS`.
///
/// Formato: `ms:botão` separado por vírgula, aplicado no **relógio virtual** — `1000:b1,2000:up`.
/// Um passo sem botão (`3000:`) solta tudo. Os nomes são os de
/// [`crate::input::BUTTON_NAMES`], os mesmos da linha de comando e do mapeamento de teclas.
///
/// **Eixo entra como `ms:eixo=valor`**: `1000:x=-128` empurra o manche para a esquerda, e
/// `1500:x=0` devolve ao repouso. Os eixos são `x`, `y`, `z` e `rz`, com o curso do manche em
/// `-128..=128` — a mesma faixa que o `Pad` guarda. Isto não é enfeite: **a Z-Wheel é navegada
/// pelo manche**, e um roteiro só de botões não a move um pixel.
///
/// **Tecla do console entra como `ms:k0xe064`**, e o `k` é opcional desde que o nome não seja de
/// botão: `kselect` e `select` valem o mesmo **se** `select` não for nome de botão. Existe porque
/// há perguntas que só se respondem com alguém apertando o controle: se o caminho de entrada chega
/// ao guest, se a Z-Wheel aceita a escolha.
fn roteiro_de_teclas() -> Vec<(u64, Passo)> {
    let Ok(valor) = std::env::var("ZEEBX_ROM_TECLAS") else {
        return Vec::new();
    };
    let mut passos = Vec::new();
    for parte in valor.split(',') {
        let parte = parte.trim();
        if parte.is_empty() {
            continue;
        }
        let (quando, nome) = match parte.split_once(':') {
            Some(par) => par,
            None => continue,
        };
        let Ok(ms) = quando.trim().parse::<u64>() else {
            continue;
        };
        let nome = nome.trim();
        let passo = match nome.split_once('=') {
            // **O evento vem antes do eixo, e a ordem importa.** Eu o tinha posto depois, e o
            // ramo dos eixos o engolia com o `_ => continue`: `e0x7000=0x4ea` não virava evento
            // nenhum, e o passo era descartado em silêncio. A mesma armadilha do pad recriado,
            // uma camada acima.
            // `e0x7000=0x4ea`: evento de widget, com o número e o `wParam`.
            Some((evt, valor)) if evt.trim().starts_with('e') => {
                let evt = u32::from_str_radix(evt.trim().trim_start_matches('e').trim_start_matches("0x"), 16);
                let valor = valor.trim().trim_start_matches("0x");
                match (evt, u16::from_str_radix(valor, 16)) {
                    (Ok(evt), Ok(w)) => Passo::Evento(evt, w),
                    _ => continue,
                }
            }
            Some((eixo, valor)) => {
                let eixo = eixo_do_nome(eixo.trim());
                let valor = valor.trim().parse::<i32>().ok();
                match (eixo, valor) {
                    (Some(eixo), Some(valor)) => Passo::Eixo(eixo, valor),
                    _ => continue,
                }
            }
            None if nome.is_empty() => Passo::Solto,
            // `k0`, `kclr`, `kselect`, `k0xe063`: tecla do console, pelo nome que o `avk` conhece.
            None if nome.len() > 1 && nome[..1].eq_ignore_ascii_case("k") => {
                match crate::input::avk::por_nome(&nome[1..].to_ascii_lowercase()) {
                    Some(avk) => Passo::Tecla(avk),
                    None => continue,
                }
            }
            None => match crate::input::BUTTON_NAMES
                .iter()
                .position(|candidato| candidato.eq_ignore_ascii_case(nome))
            {
                Some(indice) => Passo::Botao(indice),
                // **O nome de tecla sem o `k` também vale.** Antes, `0xe064` não era botão nem
                // tecla (faltava o `k`) e o passo era descartado **em silêncio**: um roteiro
                // inteiro de teclas virou "nenhuma tecla", e a medição disse que a roda não
                // responde — quando quem não apertou nada foi o roteiro. A ordem importa: botão
                // primeiro, porque `up` e `down` são nomes dos dois.
                None => match crate::input::avk::por_nome(&nome.to_ascii_lowercase()) {
                    Some(avk) => Passo::Tecla(avk),
                    None => {
                        eprintln!(
                            "aviso: o passo {parte:?} do roteiro não é botão nem tecla, e foi \
                             descartado"
                        );
                        continue;
                    }
                },
            },
        };
        passos.push((ms, passo));
    }
    passos.sort_by_key(|(ms, _)| *ms);
    passos
}

/// O índice de um eixo pelo nome usado no roteiro: `x`, `y`, `z` e `rz`.
fn eixo_do_nome(nome: &str) -> Option<usize> {
    match nome.to_ascii_lowercase().as_str() {
        "x" => Some(0),
        "y" => Some(1),
        "z" => Some(2),
        "rz" => Some(3),
        _ => None,
    }
}

/// Aplica um passo do roteiro sobre o pad, devolvendo o pad do passo seguinte.
///
/// O pad **persiste**: cada passo muda só o que ele nomeia. É esta função que faz o passo vazio
/// (`"11000:"`) soltar tudo, e é ela que garante que segurar o manche e apertar um botão sejam dois
/// passos que se somam — antes disto o pad era recriado a cada passo e o aperto zerava o eixo.
fn aplica_o_passo(mut pad: crate::input::Pad, passo: Passo) -> crate::input::Pad {
    match passo {
        Passo::Botao(indice) => pad.press(indice, true),
        Passo::Eixo(eixo, valor) => pad.set_axis(eixo, valor),
        // Tecla e evento não vivem no pad: quem os entrega é `passos_vencidos`, pela sessão.
        Passo::Tecla(_) | Passo::Evento(_, _) => {}
        Passo::Solto => pad = crate::input::Pad::default(),
    }
    pad
}

/// Aplica os passos do roteiro que já venceram até `decorrido` ms, devolvendo o pad do passo
/// seguinte.
///
/// O pad entra e sai por valor: quem chama **tem** que guardá-lo entre um quadro e o seguinte. O
/// defeito que existiu aqui morava justamente nisso — o pad era recriado dentro do laço de quadros,
/// então o manche durava um passo só e o aperto do botão o soltava. O teste desta função mostra a
/// travessia de dois quadros para deixar o contrato escrito.
fn passos_vencidos(
    mut pad: crate::input::Pad,
    roteiro: &[(u64, Passo)],
    passo: &mut usize,
    decorrido: u64,
    teclas: &mut Vec<(u32, bool)>,
    eventos: &mut Vec<(u32, u16)>,
) -> crate::input::Pad {
    while *passo < roteiro.len() && roteiro[*passo].0 <= decorrido {
        match roteiro[*passo].1 {
            // Tecla e "soltar tudo" precisam do motor, e não só do pad: viram ordens para quem
            // chama, que é quem tem a sessão em mãos.
            Passo::Tecla(avk) => teclas.push((avk, true)),
            // O evento de widget precisa do motor, e a sessão está com quem chama.
            Passo::Evento(evt, w) => eventos.push((evt, w)),
            Passo::Solto => {
                let presas: Vec<u32> = teclas
                    .iter()
                    .filter(|(_, baixo)| *baixo)
                    .map(|(avk, _)| *avk)
                    .collect();
                for avk in presas {
                    teclas.push((avk, false));
                }
                pad = crate::input::Pad::default();
            }
            outro => pad = aplica_o_passo(pad, outro),
        }
        *passo += 1;
    }
    pad
}

/// Um passo do roteiro de controle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Passo {
    /// Aperta um botão, pelo índice do aparelho.
    Botao(usize),
    /// Põe um eixo no valor dado, no curso do manche.
    Eixo(usize, i32),
    /// Entrega ao applet um evento de **widget**, pelo número e pelo `wParam`.
    ///
    /// Existe para exercitar o que o widget faria e o emulador ainda não tem: no console, quem
    /// manda o `0x7000` que a Z-Wheel lê para abrir um jogo é o widget da lista. Com o roteiro
    /// entregando o evento, o resto do ciclo — pedido de abertura, troca de sessão e volta — fica
    /// exercitável sem ele. Formato: `ms:e0x7000=0x4ea`.
    Evento(u32, u16),
    /// Aperta uma **tecla do console** (`AVK_*`), que não sai de botão nenhum.
    ///
    /// É o que a Z-Wheel espera para o formulário de abertura andar: ela pede `AVK_0` e `AVK_CLR`,
    /// e nenhum dos dois cabe num botão de controle. Sem este passo, o roteiro só sabia fazer o que
    /// o controle faz — e a conclusão de que "o gesto não existe" vinha de um instrumento que não
    /// sabia fazer o gesto.
    Tecla(u32),
    /// Solta tudo: botões, eixos e teclas.
    Solto,
}

/// Lê o rastreio de chamadas com `ZEEBX_ROM_TRACO`.
///
/// Vale `1` (as últimas chamadas antes da falha) ou um filtro de texto: `ZEEBX_ROM_TRACO=IFile`
/// guarda só as chamadas cujo nome contém aquilo. Existe porque "acesso inválido a 0x0" diz que
/// algum ponteiro era nulo e não diz **qual** — e numa investigação o que responde é a última
/// chamada antes de a execução sair do prumo. Sem esta opção o rastreio existe no motor e ninguém o
/// liga numa varredura.
fn rastreio_pedido() -> Option<String> {
    let valor = std::env::var("ZEEBX_ROM_TRACO").ok()?;
    let valor = valor.trim().to_string();
    match valor.is_empty() || valor == "0" {
        true => None,
        false => Some(valor),
    }
}

/// Lê o caminho da captura de serial de `ZEEBX_ROM_SERIAL`.
///
/// A captura é onde a **instrumentação** escreve sem se misturar com o log do jogo: a árvore de
/// widgets da primeira tecla, as classes criadas, os bancos abertos. Ela existe no `run` desde
/// sempre (`--serial=`), e faltava aqui — e sem ela a pergunta "**para quem** foi esta tecla?"
/// só se respondia com janela aberta, que é justamente o que a varredura existe para não exigir.
fn serial_pedido() -> Option<std::path::PathBuf> {
    let valor = std::env::var("ZEEBX_ROM_SERIAL").ok()?;
    let valor = valor.trim();
    match valor.is_empty() {
        true => None,
        false => Some(std::path::PathBuf::from(valor)),
    }
}

/// Lê as classes a atender por **sonda** de `ZEEBX_ROM_SONDA`, como `0x01000000,0x0102c4e8`.
///
/// É o instrumento para a pergunta "que classe é esta, e o que o jogo chama nela?" sem desmontar
/// o módulo: a sonda devolve um objeto que responde sucesso a tudo e anota cada slot com os
/// argumentos e com os textos que os argumentos apontam. Foi por ela que o `IDownload` saiu: a
/// Z-Wheel pede `0x01000000` em `ShopAction_Init`, era recusado, e sem ele a biblioteca de jogos
/// da roda não monta — a grade nunca aparece e nenhuma tecla faz nada.
fn sonda_pedida() -> Option<Vec<u32>> {
    let valor = std::env::var("ZEEBX_ROM_SONDA").ok()?;
    let classes: Vec<u32> = valor
        .split(',')
        .filter_map(|parte| u32::from_str_radix(parte.trim().trim_start_matches("0x"), 16).ok())
        .collect();
    match classes.is_empty() {
        true => None,
        false => Some(classes),
    }
}

/// Lê o pedido de despejo de memória de `ZEEBX_ROM_DESPEJO`, no formato `endereço:tamanho`.
///
/// Existe porque a última pergunta de uma investigação costuma ser "o que tem **naquela**
/// memória?": o rastreio diz que o jogo montou uma tabela de entradas de 28 bytes e quebrou
/// chamando um ponteiro nulo; o que responde o resto é ler a tabela. O endereço muda de execução
/// para execução (é heap do guest), então o despejo é lido no fim, com o mesmo relógio virtual.
fn despejo_pedido() -> Option<(u32, usize)> {
    let valor = std::env::var("ZEEBX_ROM_DESPEJO").ok()?;
    let (endereco, tamanho) = valor.trim().split_once(':')?;
    let endereco = u32::from_str_radix(endereco.trim().trim_start_matches("0x"), 16).ok()?;
    let tamanho = tamanho.trim().parse::<usize>().ok()?.min(64 * 1024);
    Some((endereco, tamanho))
}

/// Em que taxa o mixer entrega o áudio da medição.
///
/// É a taxa que o core Libretro pede ao frontend, e a que o console entrega de fato — 44,1 kHz
/// estéreo. Medir noutra taxa daria um número que não corresponde ao que se ouve.
const TAXA_DE_AMOSTRAGEM: u32 = 44_100;

/// Quantas linhas do log do próprio jogo entram no relatório. As últimas, não as primeiras: o
/// que interessa num jogo que quebrou é o que ele dizia pouco antes.
const LOG_MOSTRADO: usize = 20;

/// O erro do sistema é "acabou o espaço"?
///
/// Vale o número cru e não o `ErrorKind`: 28 (`ENOSPC`) no Unix e 112 (`ERROR_DISK_FULL`) no
/// Windows, que é o que o `raw_os_error` devolve em cada um.
fn sem_espaco(erro: &std::io::Error) -> bool {
    matches!(erro.raw_os_error(), Some(28) | Some(112))
}

/// Em que estado a ROM ficou, nos mesmos baldes do levantamento de compatibilidade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Categoria {
    /// Não é um módulo que saibamos ler ou carregar. Nem chegou a haver máquina.
    NaoCarrega,
    /// O módulo carregou e a execução quebrou antes de haver applet: no `AEEMod_Load` ou
    /// dentro do `CreateInstance`. É onde para um arquivo que passa pelo cabeçalho frouxo do
    /// `.mod` sem ser código ARM.
    QuebrouAntesDoApplet,
    /// O módulo carregou e a execução voltou inteira, mas não há applet — nenhum `.mif` dizendo
    /// qual criar, ou `CreateInstance` recusando a classe.
    SemApplet,
    /// O applet nasceu e quebrou na primeira volta, que é onde o `EVT_APP_START` é entregue.
    QuebrouNaPartida,
    /// Quebrou já dentro do laço de quadros.
    QuebrouNoLaco,
    /// Não quebrou, mas não cumpriu o tempo virtual pedido dentro do teto de tempo real.
    LentoDemais,
    /// O jogo terminou sozinho antes do tempo pedido — sem timer armado e sem trabalho.
    Terminou,
    /// O disco acabou no meio da varredura. **Não é defeito do jogo, é o ambiente**: a extração
    /// da ROM para o cache não coube, e o que veio depois disso não vale como medição.
    SemEspaco,
    /// Cumpriu o tempo virtual pedido, de pé.
    Roda,
}

impl Categoria {
    /// Se este estado conta como teste passando.
    ///
    /// "Roda" e "terminou" passam; o resto é falha. **Pendência não é falha**: um jogo que roda
    /// pedindo três classes que não temos continua passando, porque é assim que ele se comporta
    /// no console também — o que cobra pendência é a linha de base, e só quando ela existe.
    pub fn passa(self) -> bool {
        matches!(self, Self::Roda | Self::Terminou)
    }

    fn rotulo(self) -> &'static str {
        match self {
            Self::NaoCarrega => "não carrega",
            Self::QuebrouAntesDoApplet => "quebrou antes de criar o applet",
            Self::SemApplet => "não cria o applet",
            Self::QuebrouNaPartida => "quebrou no EVT_APP_START",
            Self::QuebrouNoLaco => "quebrou no laço de quadros",
            Self::LentoDemais => "lento demais",
            Self::Terminou => "terminou sozinho",
            Self::SemEspaco => "sem espaço em disco",
            Self::Roda => "roda",
        }
    }
}

/// Quanto custou rodar. Nada daqui entra na comparação com a linha de base: são números da
/// máquina de quem rodou, e cobrá-los faria o teste falhar por causa do hardware.
#[derive(Debug, Clone, Copy, Default)]
pub struct Desempenho {
    /// Tempo real gasto de `start` até o applet existir. Um jogo pode levar segundos aqui.
    pub abertura: Duration,
    /// Tempo real gasto no laço de quadros.
    pub laco: Duration,
    /// Tempo virtual que o jogo chegou a medir, em ms.
    pub virtual_ms: u32,
    /// Voltas do laço de eventos. É o "parou na volta N" do levantamento.
    pub voltas: u64,
    /// Quadros que o jogo apresentou.
    pub quadros: u32,
    /// Escritas na tela. Um jogo 2D pode não apresentar quadro nenhum em poucos segundos e
    /// ainda assim estar desenhando — é o que separa "abriu" de "tela preta".
    pub pixels: u64,
    /// **Cores distintas** no quadro final, e a que mais aparece.
    ///
    /// O `pixels` conta escritas, e um jogo que pinta 307.200 pixels de preto tem `pixels` alto e
    /// tela preta: contar escritas **não** separa "desenhou" de "desenhou nada". Contar cores
    /// separa. É o número que faltava para a linha de base cobrar uma regressão que apaga a tela
    /// sem quebrar a execução — o sintoma mais fácil de passar despercebido numa varredura.
    pub cores: u32,
    /// A cor mais frequente do quadro final, em RGB565. Numa tela preta é `0x0000`.
    pub cor_dominante: u16,
    /// Se o quadro medido veio do rasterizador da placa, e não da tela do console.
    ///
    /// Muda a leitura: tela preta com `quadro_na_placa` é o jogo desenhando **em outro lugar**,
    /// e tela preta sem ele é o jogo não desenhando.
    pub quadro_na_placa: bool,
    /// Instruções ARM executadas.
    pub instrucoes: u64,
    /// Chamadas de API atendidas.
    pub chamadas: u64,
    /// Bytes de heap do guest entregues, e objetos nossos vivos no fim.
    pub heap: u32,
    pub objetos: usize,
}

impl Desempenho {
    /// Fração da velocidade do console, em porcentagem: tempo virtual sobre tempo real.
    pub fn velocidade(&self) -> u64 {
        let real = self.laco.as_millis() as u64;
        match real {
            0 => 0,
            real => u64::from(self.virtual_ms) * 100 / real,
        }
    }

    /// Instruções por segundo de tempo real, no laço.
    pub fn ips(&self) -> u64 {
        match self.laco.as_millis() as u64 {
            0 => 0,
            real => self.instrucoes * 1000 / real,
        }
    }

    /// Quadros por segundo de tempo **virtual** — a taxa que o jogo acha que está tendo.
    pub fn fps(&self) -> u64 {
        match self.virtual_ms {
            0 => 0,
            ms => u64::from(self.quadros) * 1000 / u64::from(ms),
        }
    }
}

/// O que o emulador tem a apontar sobre a execução. É esta parte que a linha de base cobra.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pendencias {
    /// Métodos de API que o jogo chamou e não implementamos, com o endereço de quem chamou.
    pub apis: Vec<String>,
    /// ClassIDs que o jogo pediu e não temos.
    pub classes: Vec<String>,
    /// Arquivos que o jogo abriu e não achou. **Sinal, não ruído**: foi esta lista que entregou
    /// o `font.fnz` dos ports de arcade e os doze estágios do Resident Evil 4.
    pub arquivos: Vec<String>,
    /// Ponteiros do guest que recusamos, com o motivo.
    pub ponteiros: Vec<String>,
    /// APIs atendidas por hipótese — o que é palpite medido, e não comportamento conhecido.
    pub hipoteses: Vec<String>,
    /// Acessos inválidos que aconteceram dentro de um callback e não derrubaram a execução.
    pub falhas: Vec<String>,
    /// O que o jogo perguntou ao `IFileMgr`, com caminho e retorno, em ordem.
    pub arquivos_chamados: Vec<String>,
    /// Todas as classes que o jogo pediu, com quantas vezes.
    pub classes_pedidas: Vec<String>,
    /// O texto que o jogo desenhou, com o instante virtual — o que está escrito na tela.
    pub desenhados: Vec<String>,
    /// O que o jogo chamou nas classes atendidas por **sonda** (`ZEEBX_ROM_SONDA`), slot a slot.
    pub sonda: Vec<String>,
    /// O que cada classe de **widget** recebeu no acessador, por seletor.
    pub seletores: Vec<String>,
    /// Alocações que o heap recusou, como `tamanho em quem pediu (lr)`.
    ///
    /// É a pista que faltava quando um jogo mostrasse a tela de falta de memória sem nada no
    /// relatório: o `malloc` devolve zero em silêncio.
    pub alocacoes: Vec<String>,
    /// Texto que o jogo mandou desenhar e não soubemos desenhar.
    pub texto: Vec<String>,
    /// Chamadas de GL atendidas sem fazer nada.
    pub gl_ignorado: Vec<String>,
}

impl Pendencias {
    fn de(session: &Session) -> Self {
        let machine = session.machine();
        let ordenar = |mut linhas: Vec<String>| {
            linhas.sort();
            linhas.dedup();
            linhas
        };
        Self {
            apis: ordenar(machine.missing_apis()),
            classes: ordenar(
                machine
                    .unknown_classes()
                    .iter()
                    .map(|id| format!("{id:#010x}"))
                    .collect(),
            ),
            arquivos: ordenar(machine.missing_files()),
            ponteiros: ordenar(machine.bad_pointers()),
            hipoteses: ordenar(
                machine
                    .assumptions()
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            falhas: ordenar(machine.swallowed_faults()),
            arquivos_chamados: machine.fs_log().cloned().collect(),
            classes_pedidas: machine
                .requested_classes()
                .into_iter()
                .map(|(classe, vezes)| format!("{classe:#010x}  ({vezes}x)"))
                .collect(),
            desenhados: machine
                .drawn_text()
                .map(|(ms, x, y, texto)| format!("{ms:>7} ms  ({x}, {y})  {texto}"))
                .collect(),
            seletores: machine
                .seletores_por_classe()
                .into_iter()
                .map(|(classe, seletor, vezes)| {
                    format!("classe {classe:#010x}  seletor {seletor:#x}  ({vezes}x)")
                })
                .collect(),
            sonda: machine
                .probe_log()
                .iter()
                .map(|(classe, objeto, slot, args, textos, vezes)| {
                    let mostrar = |i: usize| match &textos[i] {
                        Some(texto) => format!("{texto:?}"),
                        None => format!("{:#x}", args[i]),
                    };
                    let repetido = match vezes {
                        1 => String::new(),
                        n => format!("   ({n}x)"),
                    };
                    format!(
                        "classe {classe:#010x}, objeto {objeto:#x}: slot[{slot:>2}] ({}, {}, {}){repetido}",
                        mostrar(1),
                        mostrar(2),
                        mostrar(3)
                    )
                })
                .collect(),
            alocacoes: ordenar(
                machine
                    .refused_allocations()
                    .into_iter()
                    .map(|(tamanho, pc)| format!("malloc de {tamanho} bytes, pedido de {pc:#010x}"))
                    .chain(machine.refused_availability_checks().into_iter().map(
                        |(tamanho, pc)| {
                            format!("CheckAvail de {tamanho} bytes, perguntado em {pc:#010x}")
                        },
                    ))
                    .collect(),
            ),
            texto: ordenar(machine.pending_text().to_vec()),
            gl_ignorado: ordenar(machine.ignored_gl().iter().map(|s| s.to_string()).collect()),
        }
    }

    fn vazias(&self) -> bool {
        *self == Self::default()
    }

    /// As seções, na ordem em que valem a pena ser lidas — a mesma do relatório do `run`.
    fn secoes(&self) -> [(&'static str, &Vec<String>); 14] {
        [
            ("APIs que faltaram", &self.apis),
            ("classes que o jogo pediu e não temos", &self.classes),
            ("classes pedidas", &self.classes_pedidas),
            ("chamadas ao sistema de arquivos", &self.arquivos_chamados),
            ("arquivos não encontrados", &self.arquivos),
            ("acessos inválidos que o jogo seguiu por cima", &self.falhas),
            ("ponteiros recusados", &self.ponteiros),
            ("APIs atendidas por hipótese", &self.hipoteses),
            ("texto desenhado na tela", &self.desenhados),
            ("alocações recusadas pelo heap", &self.alocacoes),
            ("o que cada classe de widget recebeu no acessador", &self.seletores),
            ("o que o jogo chamou nas classes de sonda", &self.sonda),
            ("texto que não soubemos desenhar", &self.texto),
            ("GL atendido sem fazer nada", &self.gl_ignorado),
        ]
    }
}

/// O levantamento de uma ROM.
#[derive(Debug, Clone)]
pub struct Relatorio {
    pub arquivo: PathBuf,
    pub titulo: String,
    /// A classe que o shell pediu para abrir, quando **chegou a pedir** uma.
    ///
    /// E o desfecho que a Z-Wheel tem de produzir para trocar de aplicativo, e o unico sinal que
    /// nao engana: a contagem de cores sobe quando ela anima, e animacao nao e pedido. Vem de
    /// `Session::take_launch_request`, o mesmo gancho que o core usa para trocar de sessao.
    pub abertura_pedida: Option<u32>,
    pub categoria: Categoria,
    /// Onde parou, em texto legível. `None` quando o jogo seguia de pé no fim.
    pub motivo: Option<String>,
    /// `None` quando nem chegou a existir sessão para medir.
    pub desempenho: Option<Desempenho>,
    pub pendencias: Pendencias,
    /// Registradores e pilha no instante da falha, quando houve uma.
    ///
    /// Sem eles, "acesso inválido a 0x00000000" diz que alguma coisa era nula e não diz **qual**
    /// — e essa é a pergunta. Fica fora do resumo: o endereço e o `pc` já estão no motivo, e o
    /// resto é material de investigação, não de comparação.
    pub estado_da_falha: Option<String>,
    /// Os métodos mais chamados, do maior para o menor.
    pub chamadas_maiores: Vec<(String, u64)>,
    /// Onde o tempo real foi gasto, por método de API, do mais caro para o mais barato.
    ///
    /// Vazio a menos que `ZEEBX_ROM_PERFIL` esteja definido: ligar o cronômetro em cada chamada
    /// muda a velocidade medida, e o número de velocidade é o que a varredura existe para dar.
    /// Com a contagem de chamadas ao lado do custo, dá para separar **volume** de **peso**: o
    /// Rolima fazia 6,8 milhões de chamadas e o Ridge Racer 70 mil, e só o perfil diz qual dos
    /// dois custa mais por chamada.
    pub custo_maiores: Vec<(String, u64)>,
    /// O tempo real somado de todas as chamadas de API, em nanossegundos.
    pub custo_total: u64,
    /// O fim do log do próprio jogo, por `DBGPRINTF` e por semihosting.
    pub log_do_jogo: Vec<String>,
    /// O despejo de memória pedido em `ZEEBX_ROM_DESPEJO`, com o endereço de onde saiu.
    pub despejo: Option<(u32, Vec<u8>)>,
    /// As últimas chamadas do rastreio, quando `ZEEBX_ROM_TRACO` o pediu.
    ///
    /// Fica fora do resumo de propósito: é material de investigação, muda de uma execução para a
    /// outra com qualquer ajuste no motor, e cobrá-lo na linha de base seria acusar regressão em
    /// cima de uma ferramenta de depuração.
    pub rastreio: Vec<String>,
    /// O que o áudio fez, quando houve áudio para medir.
    pub audio: Option<Audio>,
}

/// O que o áudio do jogo fez, medido sem placa de som.
///
/// **O estalo tem assinatura numérica**: um salto grande entre uma amostra e a seguinte, no mesmo
/// canal. Guardar o maior salto e quantos passaram de meio curso permite dizer "este jogo estala"
/// sem gravar arquivo de áudio e ouvir — e a varredura das 62 ROMs passa a medir isso de graça,
/// que é o único jeito de descobrir quando o defeito apareceu.
///
/// Fica **fora do resumo**, pela mesma razão do estado da falha: são números de investigação, e
/// cobrá-los na linha de base acusaria regressão por causa de um limiar escolhido hoje.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Audio {
    /// Quadros estéreo entregues ao mixer.
    pub amostras: u64,
    /// O maior valor absoluto, em `0..=1`.
    pub pico: f32,
    /// O valor eficaz, que é o que se percebe como volume.
    pub rms: f32,
    /// Deslocamento contínuo: um valor médio longe de zero é assobio de corrente contínua.
    pub continuo: f32,
    /// O maior salto entre amostras do mesmo canal.
    pub maior_salto: f32,
    /// Quantos saltos passaram de [`LIMIAR_DE_ESTALO`].
    pub estalos: u64,
}

/// A partir de quanto um salto entre amostras conta como estalo.
///
/// Meio curso em uma amostra é descontinuidade forte para qualquer conteúdo que não seja ruído
/// branco de propósito. Não é lei: é um limiar declarado, para os números de dois jogos poderem
/// ser comparados entre si.
pub const LIMIAR_DE_ESTALO: f32 = 0.5;

impl Relatorio {
    /// O que a linha de base guarda: estado e pendências, sem número de desempenho.
    ///
    /// Tem de ser estável entre máquinas e entre execuções, ou a linha de base acusa regressão
    /// onde só houve um computador diferente. Por isso nada de tempo, de contagem de instrução
    /// nem de quadros aqui — só o que é comportamento do emulador diante daquele jogo.
    pub fn resumo(&self) -> String {
        let mut texto = format!("{}\nestado: {}\n", self.titulo, self.categoria.rotulo());
        if let Some(classe) = self.abertura_pedida {
            texto.push_str(&format!("abertura pedida: {classe:#010x}\n"));
        }
        if let Some(motivo) = &self.motivo {
            texto.push_str(&format!("motivo: {motivo}\n"));
        }
        if let Some(d) = &self.desempenho {
            // **A linha que impede uma tela preta de passar.** Um jogo que apaga tudo continua
            // apresentando quadro, executando instrução e respondendo API: só o número de cores
            // denuncia. A dominante diz *o que* ficou no lugar — preto é tela apagada, e um
            // `0xFFFF` no lugar dela é outra história.
            // Uma tela quase uniforme tem duas leituras, e o relatório diz as duas: ou o jogo não
            // desenha, ou **ainda está carregando**. Foi o que aconteceu com os dois títulos da
            // linha F.C.: com seis segundos virtuais apareciam com uma e seis cores, e com vinte
            // mostravam 2.410 e 4.133. Sem esta linha, quem lê o relatório acusa o jogo.
            let aviso = match d.cores {
                0..=2 => " (tela quase uniforme: pode ser carregamento — vale aumentar ZEEBX_ROM_MS)",
                _ => "",
            };
            texto.push_str(&format!(
                "tela: {} cor(es), dominante {:#06x}{}{}\n",
                d.cores,
                d.cor_dominante,
                match d.quadro_na_placa {
                    true => " (o quadro está na placa, não na tela de 2D)",
                    false => "",
                },
                aviso
            ));
        }
        for (nome, linhas) in self.pendencias.secoes() {
            if linhas.is_empty() {
                continue;
            }
            texto.push_str(&format!("{nome}:\n"));
            for linha in linhas {
                texto.push_str(&format!("  {linha}\n"));
            }
        }
        if self.pendencias.vazias() {
            texto.push_str("nada a apontar\n");
        }
        texto
    }

    /// O relatório inteiro, com desempenho, chamadas e o log do jogo.
    pub fn completo(&self) -> String {
        // O caminho fica fora do resumo de propósito: ele muda de máquina para máquina, e a
        // linha de base acusaria diferença só por isso.
        let mut texto = format!("arquivo: {}\n{}", self.arquivo.display(), self.resumo());
        if let Some(d) = &self.desempenho {
            texto.push_str(&format!(
                "desempenho:\n  \
                 abriu em {:.1} s, rodou {:.1} s reais para {} ms virtuais ({}% da velocidade do console)\n  \
                 {} volta(s), {} quadro(s) ({} fps virtuais)\n  \
                 {} instruções ({}/s), {} chamada(s) de API\n  \
                 {} KB de heap, {} objeto(s) vivo(s)\n",
                d.abertura.as_secs_f32(),
                d.laco.as_secs_f32(),
                d.virtual_ms,
                d.velocidade(),
                d.voltas,
                d.quadros,
                d.fps(),
                d.instrucoes,
                d.ips(),
                d.chamadas,
                d.heap / 1024,
                d.objetos,
            ));
        }
        if let Some(estado) = &self.estado_da_falha {
            texto.push_str(&format!("no instante da falha:\n{estado}"));
        }
        if !self.custo_maiores.is_empty() {
            texto.push_str(&format!(
                "onde o tempo foi ({} ms em chamadas de API):\n",
                self.custo_total / 1_000_000
            ));
            for (nome, ns) in &self.custo_maiores {
                texto.push_str(&format!(
                    "  {:>9.1} ms  {:>5.1}%  {nome}\n",
                    *ns as f64 / 1e6,
                    *ns as f64 * 100.0 / (self.custo_total.max(1)) as f64,
                ));
            }
        }
        if let Some((endereco, bytes)) = &self.despejo {
            texto.push_str(&format!("despejo de {endereco:#010x} ({} bytes):\n", bytes.len()));
            for (linha, pedaco) in bytes.chunks(16).enumerate() {
                let hexa: Vec<String> = pedaco.iter().map(|b| format!("{b:02x}")).collect();
                texto.push_str(&format!(
                    "  {:#010x}  {}\n",
                    endereco + (linha * 16) as u32,
                    hexa.join(" ")
                ));
            }
        }
        if !self.rastreio.is_empty() {
            texto.push_str("rastreio (últimas chamadas antes da parada):\n");
            for linha in &self.rastreio {
                texto.push_str(&format!("  {linha}\n"));
            }
        }
        if let Some(audio) = &self.audio {
            // Fica no relatório completo e não no resumo, pela mesma razão do estado da falha:
            // é número de investigação, e a linha de base não cobra limiar escolhido hoje.
            texto.push_str(&format!(
                "áudio:\n                   {} amostra(s) estéreo, pico {:.3}, rms {:.4}, contínuo {:+.5}\n                   maior salto entre amostras do mesmo canal {:.3}; {} acima de {:.2}\n",
                audio.amostras, audio.pico, audio.rms, audio.continuo,
                audio.maior_salto, audio.estalos, LIMIAR_DE_ESTALO,
            ));
        }
        if !self.chamadas_maiores.is_empty() {
            texto.push_str("chamadas que mais pesaram:\n");
            for (nome, vezes) in &self.chamadas_maiores {
                texto.push_str(&format!("  {vezes:>8}x {nome}\n"));
            }
        }
        if !self.log_do_jogo.is_empty() {
            texto.push_str("log do jogo:\n");
            for linha in &self.log_do_jogo {
                texto.push_str(&format!("  {linha}\n"));
            }
        }
        texto
    }

    /// Uma linha por jogo, para a tabela da varredura.
    pub fn linha(&self) -> String {
        let desempenho = match &self.desempenho {
            Some(d) => format!(
                "{} ms virtuais em {:.1} s, {} quadro(s), {}%",
                d.virtual_ms,
                d.laco.as_secs_f32(),
                d.quadros,
                d.velocidade()
            ),
            None => "não rodou".to_string(),
        };
        let pendencias: usize = self
            .pendencias
            .secoes()
            .iter()
            .map(|(_, linhas)| linhas.len())
            .sum();
        format!(
            "{:<44} {:<26} {desempenho}, {pendencias} pendência(s)",
            self.titulo,
            self.categoria.rotulo()
        )
    }

    /// Quando nem houve sessão, o relatório é só o motivo da recusa.
    fn recusado(arquivo: &Path, erro: &StartError) -> Self {
        // A recusa é classificada pelo que ela diz do jogo, e não pela etapa em que ocorreu:
        // "não temos a classe que ele pede" e "não é um módulo" são problemas diferentes, e o
        // levantamento os separa.
        let categoria = match erro {
            StartError::NoApplet | StartError::Refused(_) => Categoria::SemApplet,
            StartError::Stopped(_) => Categoria::QuebrouAntesDoApplet,
            // A falta de espaço chega como erro de leitura, e é a única dessa lista que **não** diz
            // nada sobre o jogo: separá-la evita registrar defeito onde havia disco cheio.
            StartError::Unreadable(erro) if sem_espaco(erro) => Categoria::SemEspaco,
            StartError::Unreadable(_) | StartError::NotAModule(_) | StartError::NotLoadable(_) => {
                Categoria::NaoCarrega
            }
        };
        Self {
            arquivo: arquivo.to_path_buf(),
            titulo: crate::library::title_for(arquivo),
            categoria,
            abertura_pedida: None,
            motivo: Some(erro.to_string()),
            desempenho: None,
            pendencias: Pendencias::default(),
            estado_da_falha: None,
            chamadas_maiores: Vec::new(),
            custo_maiores: Vec::new(),
            custo_total: 0,
            log_do_jogo: Vec::new(),
            rastreio: Vec::new(),
            despejo: None,
            audio: None,
        }
    }
}

/// Os registradores e a pilha de quem quebrou, quando a parada foi acesso inválido ou exceção.
fn estado_da_falha(session: &Session) -> Option<String> {
    if !matches!(
        session.stopped(),
        Some(Outcome::Fault { .. } | Outcome::Exception { .. })
    ) {
        return None;
    }
    let machine = session.machine();
    let registradores: Vec<String> = machine
        .fault_regs()
        .iter()
        .enumerate()
        .map(|(i, valor)| format!("r{i}={valor:#x}"))
        .collect();
    let mut texto = format!("  {}\n", registradores.join(" "));
    let pilha = machine.fault_stack();
    if !pilha.is_empty() {
        let itens: Vec<String> = pilha.iter().map(|valor| format!("{valor:#x}")).collect();
        texto.push_str(&format!("  pilha: {}\n", itens.join(" ")));
    }
    Some(texto)
}

/// Quantas cores distintas tem o quadro, e qual a mais frequente.
///
/// Um quadro de 640×480 são 307.200 pixels: contar cores distintas com um conjunto é barato, e a
/// contagem da dominante cabe num vetor de 65.536 posições — duas passadas simples, sem alocação
/// grande por jogo.
fn cores_do_quadro(tela: &crate::video::display::Framebuffer) -> (u32, u16) {
    let mut bytes = Vec::new();
    tela.write_rgb565_into(&mut bytes);
    let mut contagem = vec![0u32; 1 << 16];
    let mut distintas = 0u32;
    for par in bytes.chunks_exact(2) {
        let cor = u16::from_le_bytes([par[0], par[1]]);
        if contagem[cor as usize] == 0 {
            distintas += 1;
        }
        contagem[cor as usize] += 1;
    }
    let dominante = contagem
        .iter()
        .enumerate()
        .max_by_key(|(_, vezes)| *vezes)
        .map(|(cor, _)| cor as u16)
        .unwrap_or(0);
    (distintas, dominante)
}

/// Quanto tempo virtual passou desde `base`, para o roteiro de controle.
fn agora_ms(agora: u32, base: u32) -> u64 {
    u64::from(agora.wrapping_sub(base))
}

/// Carrega a ROM, entrega o `EVT_APP_START` e roda `ms_virtuais` de tempo de jogo.
///
/// O laço anda **uma volta por chamada** de [`Session::step`], com orçamento de tempo real
/// zerado: é o que permite dizer em que volta o jogo parou, como o relatório do `run` diz. O
/// freio de velocidade fica desligado, porque aqui ninguém está assistindo — o que se quer é o
/// tempo virtual cumprido o mais rápido que a máquina der.
pub fn examina(arquivo: &Path, ms_virtuais: u32, teto: Duration) -> Relatorio {
    let comeco = Instant::now();
    let instalados = instalados_do_ambiente(arquivo);
    let mut session = match Session::start_with_installed(
        arquivo,
        crate::PORTAS_PADRAO,
        None,
        false,
        None,
        Default::default(),
        &instalados,
    ) {
        Ok(session) => session,
        Err(erro) => return Relatorio::recusado(arquivo, &erro),
    };
    let abertura = comeco.elapsed();
    let base_ms = session.clock_ms();
    let mut medida = Desempenho {
        abertura,
        ..Default::default()
    };
    // **O áudio é medido, não ouvido.** Sem placa, o mixer entrega as amostras do relógio virtual —
    // a mesma cadência que o frontend Libretro usa —, e a conta do estalo sai daí.
    let mixer = session.grava_audio(TAXA_DE_AMOSTRAGEM);
    // Perfil de custo: opt-in, porque o cronômetro por chamada encarece a própria execução.
    let perfilando = std::env::var("ZEEBX_ROM_PERFIL").is_ok();
    if perfilando {
        session.machine_mut().enable_api_profile();
    }
    // **A biblioteca local entra no catálogo antes da partida.** A Z-Wheel não lê a pasta de
    // ROMs: ela lê o `tt_game_info` do perfil, e o que liga um ao outro é o `catalog.json` que a
    // interface grava (`library::sync_catalog`). A varredura não o alimentava, e a medida é esta:
    // com `ZEEBX_ROM_INSTALADOS`, o banco do perfil abre com as 59 linhas oficiais do pacote e a
    // `ZEEBX_LIBRARY` com **zero** — a roda não tem o que pôr na grade, e nenhuma tecla tem o que
    // mover. Só a pasta da ROM é varrida, e só quando `ZEEBX_ROM_INSTALADOS` o pede: o efeito
    // fora do diretório do jogo é o `catalog.json`, o mesmo que a interface mantém.
    if !instalados.is_empty()
        && let Some(pasta) = arquivo.parent()
    {
        let _ = crate::library::sync_catalog(&crate::library::scan(pasta));
    }
    // A sonda entra antes da partida, como o rastreio: o que interessa nela são as primeiras
    // chamadas do jogo, que é quando ele monta o que precisa.
    if let Some(sonda) = sonda_pedida() {
        session.machine_mut().probe_classes(&sonda);
    }
    // O censo do acessador por classe de widget é opt-in pelo mesmo motivo do perfil de custo: ele
    // acrescenta uma seção ao relatório, e a linha de base dos 62 jogos não pode mudar por
    // instrumento. `ZEEBX_ROM_SELETORES` o liga.
    if std::env::var("ZEEBX_ROM_SELETORES").is_ok() {
        session.machine_mut().liga_censo_de_widgets();
    }
    // A captura de serial entra antes da partida: o que interessa nela é o começo.
    if let Some(caminho) = serial_pedido() {
        let _ = session.machine_mut().liga_serial(&caminho);
    }
    let mut som = Audio::default();
    let mut soma = 0.0_f64;
    let mut soma_dos_quadrados = 0.0_f64;
    let mut ultimo_relogio = base_ms;
    // O roteiro de controle entra pelo mesmo caminho que o frontend usa: `set_port_pad`.
    let roteiro = roteiro_de_teclas();
    // O rastreio entra antes da partida: a chamada que interessa costuma ser das primeiras.
    let rastreio = rastreio_pedido();
    if let Some(filtro) = &rastreio {
        session.machine_mut().set_tracing(true);
        if filtro != "1" {
            session.machine_mut().set_trace_filter(Some(filtro.clone()));
        }
    }
    let mut passo_do_roteiro = 0usize;
    // A classe que o shell pediu para abrir, se pediu. É o desfecho que a Z-Wheel tem de produzir
    // para trocar de aplicativo: sem este campo, a única pista de que ela reagiu era a contagem de
    // cores, e animação também muda a contagem.
    let mut abertura_pedida = None;
    // O pad do roteiro **persiste entre os passos, e por isso vive fora do laço de quadros**.
    // Cada passo muda só o que ele nomeia, e o passo vazio (`"11000:"`) solta tudo. O defeito
    // anterior era silencioso e sobreviveu a uma primeira correção: o pad era recriado a cada
    // passo, então `"18000:x=128,20000:b1"` soltava o manche no instante do aperto. Como o
    // instrumento mostrou (`eixo0=255` num passo e `128` no seguinte), recriá-lo fora do passo não
    // bastava — ele precisa atravessar o laço inteiro. É isto que faz o gesto de "segurar o manche
    // e apertar" chegar ao jogo, que é como um menu pede para abrir.
    let mut pad_do_roteiro = crate::input::Pad::default();
    // Uma amostra anterior **por canal**: o fluxo é estéreo intercalado, e comparar `R` de um
    // quadro com `L` do mesmo quadro mediria a diferença entre os canais — que é música, e não
    // descontinuidade. Foi assim que a primeira versão desta conta deu salto zero em jogo com som.
    let mut anterior = [0.0_f32; 2];

    // A partida vem separada da primeira volta: é nela que o applet recebe o `EVT_APP_START`.
    // Distinguir as duas é o que separa "quebrou antes do primeiro quadro" de "quebrou no
    // laço", e essas duas linhas do levantamento apontam para trechos de código diferentes —
    // sem a separação, um jogo que morre na primeira volta do laço aparecia como morto na
    // partida.
    let laco = Instant::now();
    let mut categoria = match session.partida() {
        Some(_) => Categoria::QuebrouNaPartida,
        None => Categoria::Roda,
    };

    if categoria == Categoria::Roda {
        loop {
            if session.clock_ms().saturating_sub(base_ms) >= ms_virtuais {
                break;
            }
            if laco.elapsed() >= teto {
                categoria = Categoria::LentoDemais;
                break;
            }
            medida.voltas += 1;
            match session.step(Duration::ZERO, false) {
                // **A tela intermediária precisa ser consumida.** A janela a mostra e segue; aqui
                // ninguém a mostra, e sem tirá-la da fila a volta seguinte devolve a mesma tela
                // para sempre — o Quake ficava em "apresentou" sem o jogo andar um milissegundo.
                Step::Presented => {
                    medida.quadros += 1;
                    while session.mostra_quadro_intermediario() {}
                }
                Step::Stopped => {
                    categoria = Categoria::QuebrouNoLaco;
                    break;
                }
                Step::Running | Step::Ahead => {}
            }
            // O tempo decorrido vem do relógio virtual, e não de um número fixo: um jogo que
            // passa dois quadros entre duas voltas deve o dobro de amostras. É o mesmo cálculo do
            // `retro_run`, para a medição aqui valer para o que o frontend recebe.
            // O roteiro é aplicado no relógio **virtual**: o mesmo roteiro vale igual com a
            // máquina a 30% ou a 300% da velocidade do console.
            if !roteiro.is_empty() {
                let decorrido_total = agora_ms(session.clock_ms(), base_ms);
                let antes_do_roteiro = passo_do_roteiro;
                let mut ordens_de_tecla = Vec::new();
                let mut ordens_de_evento = Vec::new();
                pad_do_roteiro = passos_vencidos(
                    pad_do_roteiro,
                    &roteiro,
                    &mut passo_do_roteiro,
                    decorrido_total,
                    &mut ordens_de_tecla,
                    &mut ordens_de_evento,
                );
                for (avk, baixo) in ordens_de_tecla {
                    session.set_key(avk, baixo);
                }
                for (evt, w) in ordens_de_evento {
                    let _ = session.entrega_evento_ao_applet(evt, w);
                }
                // Com o rastreio ligado, o pad depois de cada passo sai no relatório. Sem isto, um
                // roteiro que não chega ao jogo é indistinguível de um jogo que o ignora — foi
                // exatamente o que custou a investigação da Z-Wheel.
                if rastreio.is_some() && passo_do_roteiro != antes_do_roteiro {
                    eprintln!(
                        "roteiro {decorrido_total} ms: passo {passo_do_roteiro} -> x={} y={} botoes={:#x}",
                        pad_do_roteiro.eixo_do_console(0),
                        pad_do_roteiro.eixo_do_console(1),
                        pad_do_roteiro.buttons
                    );
                }
                session.set_port_pad(0, pad_do_roteiro);
            }
            if abertura_pedida.is_none() {
                abertura_pedida = session.take_launch_request();
            }
            let agora = session.clock_ms();
            let decorrido = u64::from(agora.wrapping_sub(ultimo_relogio)).min(1000);
            ultimo_relogio = agora;
            let devidas = (decorrido * u64::from(TAXA_DE_AMOSTRAGEM) / 1000) as usize;
            for (i, amostra) in mixer.render(devidas).into_iter().enumerate() {
                soma += f64::from(amostra);
                soma_dos_quadrados += f64::from(amostra) * f64::from(amostra);
                som.amostras += 1;
                som.pico = som.pico.max(amostra.abs());
                // O salto se mede **dentro do mesmo canal**, dois índices atrás.
                let canal = i % 2;
                let salto = (amostra - anterior[canal]).abs();
                som.maior_salto = som.maior_salto.max(salto);
                if salto > LIMIAR_DE_ESTALO {
                    som.estalos += 1;
                }
                anterior[canal] = amostra;
            }
        }
    }

    // Fim por `Returned` não é quebra: é o jogo tendo terminado o que tinha para fazer, e é o
    // desfecho de um `helloworld` que desenha e sai.
    if matches!(session.stopped(), Some(Outcome::Returned { .. })) {
        categoria = Categoria::Terminou;
    }

    medida.laco = laco.elapsed();
    medida.virtual_ms = session.clock_ms().saturating_sub(base_ms);
    medida.instrucoes = session.machine().instructions();
    let mut chamadas = session.machine().call_log();
    medida.chamadas = chamadas.iter().map(|(_, vezes)| vezes).sum();
    let (heap, objetos) = session.memory();
    medida.heap = heap;
    medida.objetos = objetos;
    chamadas.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    chamadas.truncate(CHAMADAS_MOSTRADAS);

    let mut log: Vec<String> = session
        .machine()
        .debug_output()
        .iter()
        .map(|(linha, vezes)| match vezes {
            1 => linha.clone(),
            n => format!("{linha}   ({n}x)"),
        })
        .collect();
    log.extend(
        session
            .machine()
            .cpu()
            .semihosting()
            .lines()
            .map(str::to_string),
    );
    let sobra = log.len().saturating_sub(LOG_MOSTRADO);
    log.drain(..sobra);

    medida.pixels = session.screen().escritas();
    // O despejo é lido **no fim**, com o jogo já parado: é quando a tabela que ele montou está
    // pronta, e é ela que a investigação quer ver.
    let despejo = despejo_pedido().and_then(|(endereco, tamanho)| {
        session
            .machine()
            .dump(endereco, tamanho)
            .ok()
            .map(|bytes| (endereco, bytes))
    });
    let (medida_cores, medida_dominante) = cores_do_quadro(session.screen());
    medida.cores = medida_cores;
    medida.cor_dominante = medida_dominante;
    // **A tela do console pode não ser onde o quadro está.** Um jogo que desenha por OpenGL numa
    // superfície de dispositivo deixa a tela de 2D preta, e o quadro dele vive no rasterizador da
    // placa. Sem olhar os dois, a varredura acusaria tela apagada num jogo que desenha — foi o que
    // quase aconteceu com o Prey Evil, que faz 71 mil `MatrixMode` e tem 1 cor de tela.
    if medida.cores <= 1
        && let Some(grande) = session.quadro_grande()
    {
        let (cores, dominante) = cores_do_quadro(&grande);
        if cores > medida.cores {
            medida.cores = cores;
            medida.cor_dominante = dominante;
            medida.quadro_na_placa = true;
        }
    }
    // O total sai da lista inteira, e só depois ela é cortada: a fatia mostrada tem de ser
    // porcentagem do que o jogo gastou, e não do punhado que coube no relatório.
    let (mut custo, custo_total) = match perfilando {
        true => {
            let tudo = session.machine().api_profile();
            let total: u64 = tudo.iter().map(|(_, ns)| ns).sum();
            (tudo, total)
        }
        false => (Vec::new(), 0),
    };
    custo.truncate(CHAMADAS_MOSTRADAS);
    // Fecha as contas do áudio. `rms` e `continuo` só fazem sentido com amostra na conta, e um
    // jogo que não tocou nada fica com `None` em vez de zeros que pareceriam silêncio medido.
    if som.amostras > 0 {
        let quantas = som.amostras as f64;
        som.rms = (soma_dos_quadrados / quantas).sqrt() as f32;
        som.continuo = (soma / quantas) as f32;
    }
    let audio = (som.amostras > 0).then_some(som);
    Relatorio {
        arquivo: arquivo.to_path_buf(),
        // O título sai do arquivo que se pediu, e **não** do `Session::title`: para um `.zip` a
        // sessão nomeia o jogo pela pasta do cache, cujo nome carrega o tamanho e a data do
        // arquivo. Isso muda de máquina para máquina, e a linha de base — que é gravada com
        // esse nome e traz o título dentro — não pode depender de metadado do host.
        titulo: crate::library::title_for(arquivo),
        categoria,
        abertura_pedida,
        motivo: session.stopped_reason(),
        desempenho: Some(medida),
        pendencias: Pendencias::de(&session),
        estado_da_falha: estado_da_falha(&session),
        chamadas_maiores: chamadas,
        custo_maiores: custo,
        custo_total,
        log_do_jogo: log,
        rastreio: session.machine().trace().to_vec(),
        despejo,
        audio,
    }
}

/// Só carrega a ROM e cria o applet, sem rodar o laço.
///
/// É a pergunta mais barata que se pode fazer de um jogo — "abre, ou estoura na cara?" —, e a
/// que dá para fazer de sessenta e cinco de uma vez sem esperar minutos. O que ela mede é o
/// caminho até o applet existir: carga do `.mod`, `AEEMod_Load`, `.mif` e `CreateInstance`.
pub fn abre(arquivo: &Path) -> Result<Duration, String> {
    let comeco = Instant::now();
    match Session::start_with(arquivo, crate::PORTAS_PADRAO, None, false, None, Default::default()) {
        Ok(_) => Ok(comeco.elapsed()),
        Err(erro) => Err(erro.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As ROMs que o ambiente pediu. Vazio quando ninguém pediu nenhuma.
    ///
    /// Aceita arquivo, lista separada por vírgula e diretório. O diretório é a varredura
    /// inteira, e ela **só acontece quando alguém a escreve** — nunca por padrão.
    fn roms_pedidas() -> Vec<PathBuf> {
        let Ok(valor) = std::env::var("ZEEBX_ROM") else {
            return Vec::new();
        };
        let mut caminhos = Vec::new();
        // **O valor inteiro primeiro.** Todo nome No-Intro tem vírgula — `Double Dragon (Brazil)
        // (Es,Pt).zip` — e a lista separada por vírgula partia o caminho em dois, dizendo "não deu
        // para ler o arquivo" para ambos. Quem aponta um arquivo existente quer aquele arquivo.
        let partes: Vec<&str> = match PathBuf::from(valor.trim()).exists() {
            true => vec![valor.trim()],
            false => valor.split(',').map(str::trim).filter(|p| !p.is_empty()).collect(),
        };
        for parte in partes {
            let caminho = PathBuf::from(parte);
            if !caminho.is_dir() {
                caminhos.push(caminho);
                continue;
            }
            let Ok(entradas) = std::fs::read_dir(&caminho) else {
                continue;
            };
            let mut jogos: Vec<PathBuf> = entradas
                .flatten()
                .map(|entrada| entrada.path())
                .filter(|p| {
                    matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("zip" | "mod" | "ZIP" | "MOD" | "7z" | "7Z")
                    )
                })
                .collect();
            jogos.sort();
            caminhos.extend(jogos);
        }
        caminhos
    }

    fn numero(nome: &str, padrao: u64) -> u64 {
        std::env::var(nome)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(padrao)
    }

    fn diretorio(nome: &str) -> Option<PathBuf> {
        let caminho = PathBuf::from(std::env::var(nome).ok()?);
        std::fs::create_dir_all(&caminho).ok()?;
        Some(caminho)
    }

    /// A varredura gasta perto de 1 GB extraindo as ROMs para o cache, e sem espaço ela passa a
/// acusar jogos que estão certos.
///
/// A prova roda no começo e custa um instante; a alternativa é descobrir depois de uma hora de
/// varredura que a segunda metade do placar não valia.
fn exige_espaco(dirs: &[PathBuf]) {
    const PROVA: usize = 64 * 1024 * 1024;
    for dir in dirs {
        if let Err(erro) = crate::scratch::cabe_escrever(dir, PROVA) {
            panic!(
                "sem espaço em disco para a varredura: {erro}. A varredura extrai cada ROM para o                  cache e gasta perto de 1 GB; libere espaço e rode de novo."
            );
        }
    }
}

/// Nome de arquivo para o título de um jogo, sem depender do sistema de arquivos do host.
    fn arquivo_do_titulo(titulo: &str) -> String {
        let limpo: String = titulo
            .chars()
            .map(|c| match c.is_ascii_alphanumeric() {
                true => c.to_ascii_lowercase(),
                false => '-',
            })
            .collect();
        format!("{}.txt", limpo.trim_matches('-'))
    }

    /// Roda `corpo` para cada ROM pedida e só falha no fim, com todas as falhas juntas.
    ///
    /// Falhar na primeira esconde as outras, e numa varredura o que se quer ver é o placar
    /// inteiro — um jogo que quebrou não diz nada sobre os outros sessenta e quatro.
    fn com_cada_rom(nome: &str, mut corpo: impl FnMut(&Path) -> Result<String, String>) {
        let roms = roms_pedidas();
        if roms.is_empty() {
            eprintln!(
                "{nome}: nenhuma ROM para examinar. Aponte ZEEBX_ROM para um arquivo, uma lista \
                 separada por vírgula ou um diretório, e rode de novo com --release."
            );
            return;
        }
        let mut falhas = Vec::new();
        for rom in &roms {
            match corpo(rom) {
                Ok(linha) => println!("ok    {linha}"),
                Err(erro) => {
                    println!("FALHA {erro}");
                    falhas.push(erro);
                }
            }
        }
        // Disco cheio invalida o placar inteiro, inclusive os jogos que passaram, e quem lê precisa
        // saber disso antes de olhar qualquer outro número.
        if falhas
            .iter()
            .any(|falha| falha.contains(Categoria::SemEspaco.rotulo()))
        {
            eprintln!(
                "\n**O DISCO ENCHEU NO MEIO DA VARREDURA.** Os números a partir daí não valem — nem \
                 os que passaram —, porque a extração de cada ROM precisa de espaço. Libere espaço e \
                 rode de novo."
            );
        }
        assert!(
            falhas.is_empty(),
            "{} de {} ROM(s) com problema:\n{}",
            falhas.len(),
            roms.len(),
            falhas.join("\n")
        );
    }

    /// O placar: cada ROM roda alguns segundos e vira uma linha de tabela.
    ///
    /// É o teste de ida e volta rápida — o que se quer saber depois de um ajuste é "quem
    /// continua abrindo", e não o relatório inteiro de cada jogo. Cinco segundos virtuais por
    /// ROM bastam para separar quem chega ao menu de quem quebra na partida, e o teto de tempo
    /// real curto impede que um jogo pesado segure a varredura.
    ///
    /// **Este teste não falha por jogo quebrado**: a biblioteca tem jogos sabidamente
    /// incompatíveis, e falhar neles apagaria a única coisa que interessa aqui, que é a tabela.
    /// Quem cobra regressão é o [`a_rom_indicada_avanca`], com a linha de base.
    ///
    /// ```bash
    /// ZEEBX_ROM=roms cargo test --release placar -- --nocapture
    /// # mais rápido ainda, e gravando a tabela:
    /// ZEEBX_ROM=roms ZEEBX_PLACAR_MS=3000 ZEEBX_PLACAR_SAIDA=placar.md \
    ///   cargo test --release placar -- --nocapture
    /// ```
    #[test]
    fn o_placar_das_roms() {
        let roms = roms_pedidas();
        if roms.is_empty() {
            eprintln!(
                "o_placar_das_roms: nenhuma ROM para examinar. Aponte ZEEBX_ROM para um \
                 arquivo, uma lista separada por vírgula ou um diretório, e rode com --release."
            );
            return;
        }
        let ms = numero("ZEEBX_PLACAR_MS", u64::from(MS_PLACAR)) as u32;
        let teto = Duration::from_secs(numero("ZEEBX_PLACAR_TETO", TETO_PLACAR));
        let mut linhas = Vec::new();
        let mut contagem: std::collections::BTreeMap<&str, usize> = Default::default();
        for rom in &roms {
            let relatorio = examina(rom, ms, teto);
            let d = relatorio.desempenho.unwrap_or_default();
            let motivo = relatorio
                .motivo
                .clone()
                .unwrap_or_default()
                .replace('\n', " ");
            let tela = match (d.quadros, d.pixels) {
                (0, 0) => "tela preta".to_string(),
                (0, pixels) => format!("{pixels} pixel(s)"),
                (quadros, _) => format!("{quadros} quadro(s)"),
            };
            let linha = format!(
                "| {} | {} | {} ms | {} | {:.0}% | {} |",
                relatorio.titulo,
                relatorio.categoria.rotulo(),
                d.virtual_ms,
                tela,
                d.velocidade(),
                motivo
            );
            println!("{linha}");
            *contagem.entry(relatorio.categoria.rotulo()).or_default() += 1;
            linhas.push(linha);
        }
        let resumo: Vec<String> = contagem
            .iter()
            .map(|(estado, quantos)| format!("{quantos} {estado}"))
            .collect();
        let tabela = format!(
            "| Jogo | Estado | Tempo virtual | Desenho | Velocidade | Motivo |\n|---|---|---|---|---|---|\n{}\n\n{} ROM(s): {}\n",
            linhas.join("\n"),
            roms.len(),
            resumo.join(", ")
        );
        println!("\n{tabela}");
        if let Ok(caminho) = std::env::var("ZEEBX_PLACAR_SAIDA") {
            let _ = std::fs::write(caminho, &tabela);
        }
    }

    /// A pergunta genérica: a ROM abre, ou estoura de cara — e com que erro.
    ///
    /// Vale por si: os cinco jogos que "não criam o applet" no levantamento falham aqui, sem
    /// gastar os seis segundos virtuais que eles nunca vão chegar a rodar.
    #[test]
    fn a_rom_indicada_abre() {
        // A prova de espaço vale para os dois testes: os dois extraem a ROM para o cache.
        exige_espaco(&[crate::config::config_dir()]);
        com_cada_rom("a_rom_indicada_abre", |rom| {
            let titulo = crate::library::title_for(rom);
            match abre(rom) {
                Ok(quanto) => Ok(format!("{titulo}: abriu em {:.1} s", quanto.as_secs_f32())),
                Err(erro) => Err(format!("{titulo}: não abriu — {erro}")),
            }
        });
    }

    /// A ROM avança, e o que aconteceu vira relatório.
    ///
    /// Falha quando o jogo quebra, e quando o resumo difere da linha de base — que é o ponto
    /// todo: uma API que deixou de ser atendida, uma classe que passou a ser pedida ou um
    /// arquivo que sumiu aparecem aqui, no commit que os causou.
    #[test]
    fn a_rom_indicada_avanca() {
        let ms = numero("ZEEBX_ROM_MS", u64::from(MS_PADRAO)) as u32;
        let teto = Duration::from_secs(numero("ZEEBX_ROM_TETO", TETO_PADRAO));
        let saida = diretorio("ZEEBX_ROM_SAIDA");
        let base = diretorio("ZEEBX_ROM_BASE");
        let mut onde_prova = vec![crate::config::config_dir()];
        onde_prova.extend(saida.iter().cloned());
        exige_espaco(&onde_prova);
        com_cada_rom("a_rom_indicada_avanca", |rom| {
            let relatorio = examina(rom, ms, teto);
            println!("\n===== {} =====\n{}", rom.display(), relatorio.completo());
            let nome = arquivo_do_titulo(&relatorio.titulo);
            if let Some(dir) = &saida {
                let _ = std::fs::write(dir.join(&nome), relatorio.completo());
            }
            let mut queixas = Vec::new();
            if !relatorio.categoria.passa() {
                queixas.push(match &relatorio.motivo {
                    Some(motivo) => format!("{} — {motivo}", relatorio.categoria.rotulo()),
                    None => relatorio.categoria.rotulo().to_string(),
                });
            }
            if let Some(dir) = &base
                && let Err(diferenca) = confere_base(&dir.join(&nome), &relatorio)
            {
                queixas.push(diferenca);
            }
            match queixas.is_empty() {
                true => Ok(relatorio.linha()),
                false => Err(format!("{}\n  {}", relatorio.linha(), queixas.join("\n  "))),
            }
        });
    }

    /// Compara o resumo com a linha de base, gravando-a quando ela ainda não existe.
    ///
    /// Gravar o que falta, em vez de exigir que alguém escreva à mão, é o que torna possível
    /// adotar a linha de base de um jogo novo numa execução — e a diferença fica visível no
    /// `git diff`, que é onde ela deve ser revisada, jogo por jogo.
    fn confere_base(caminho: &Path, relatorio: &Relatorio) -> Result<(), String> {
        let atual = relatorio.resumo();
        let Ok(esperado) = std::fs::read_to_string(caminho) else {
            let _ = std::fs::write(caminho, &atual);
            println!("      linha de base gravada em {}", caminho.display());
            return Ok(());
        };
        if esperado == atual {
            return Ok(());
        }
        let linhas = |texto: &str| -> Vec<String> { texto.lines().map(str::to_string).collect() };
        let (antes, agora) = (linhas(&esperado), linhas(&atual));
        let mut diferenca = vec![format!("mudou desde {}:", caminho.display())];
        diferenca.extend(
            antes
                .iter()
                .filter(|l| !agora.contains(l))
                .map(|l| format!("  - {l}")),
        );
        diferenca.extend(
            agora
                .iter()
                .filter(|l| !antes.contains(l))
                .map(|l| format!("  + {l}")),
        );
        Err(diferenca.join("\n"))
    }

    // Daqui para baixo, testes que não precisam de ROM nenhuma: é a lógica da varredura sendo
    // verificada, e ela é justamente a parte que não dá para conferir olhando a saída de um
    // jogo. Sem eles, um erro de classificação passaria como resultado do jogo.

    #[test]
    fn um_arquivo_que_nao_e_modulo_vira_relatorio_com_motivo() {
        let caminho = std::env::temp_dir().join("zeebx-varredura-lixo.mod");
        std::fs::write(&caminho, b"isto nao e um modulo").unwrap();
        let relatorio = examina(&caminho, 10, Duration::from_secs(1));
        // O cabeçalho do `.mod` é frouxo: vinte bytes de texto passam pelo carregador e o que
        // recusa é o próprio ARM, ao executá-los. Por isso a categoria é "quebrou antes de
        // criar o applet" e não "não carrega" — e é essa a diferença que o relatório precisa
        // dizer, porque as duas apontam para lugares diferentes.
        assert_eq!(relatorio.categoria, Categoria::QuebrouAntesDoApplet);
        assert!(!relatorio.categoria.passa());
        // O motivo é o que se lê para saber o que fazer, então não pode vir vazio nem ficar de
        // fora do resumo — que é o texto que a linha de base guarda.
        assert!(relatorio.motivo.as_deref().is_some_and(|m| !m.is_empty()));
        assert!(
            relatorio
                .resumo()
                .contains("quebrou antes de criar o applet")
        );
        assert!(relatorio.desempenho.is_none());
        let _ = std::fs::remove_file(&caminho);
    }

    #[test]
    fn so_roda_e_terminou_contam_como_teste_passando() {
        for categoria in [Categoria::Roda, Categoria::Terminou] {
            assert!(categoria.passa(), "{categoria:?} devia passar");
        }
        for categoria in [
            Categoria::NaoCarrega,
            Categoria::QuebrouAntesDoApplet,
            Categoria::SemApplet,
            Categoria::QuebrouNaPartida,
            Categoria::QuebrouNoLaco,
            Categoria::LentoDemais,
        ] {
            assert!(!categoria.passa(), "{categoria:?} não devia passar");
        }
    }

    /// **Uma tela de uma cor só é tela apagada, e o número de cores diz isso.**
    ///
    /// É o que o `pixels` não fazia: um jogo que pinta 307.200 pixels de preto conta 307.200
    /// escritas e passava como se estivesse desenhando. Contando cores, ele fica com uma.
    #[test]
    fn um_quadro_de_uma_cor_so_tem_uma_cor() {
        use crate::video::display::{Framebuffer, Rgb};

        let preto = Framebuffer::new(64, 48);
        assert_eq!(cores_do_quadro(&preto), (1, 0x0000), "tela preta é uma cor");

        let mut branco = Framebuffer::new(64, 48);
        branco.fill_rect(
            crate::video::display::Rect {
                x: 0,
                y: 0,
                width: 64,
                height: 48,
            },
            Rgb {
                r: 255,
                g: 255,
                b: 255,
            },
        );
        assert_eq!(cores_do_quadro(&branco), (1, 0xffff), "branco também é uma");

        let mut duas = Framebuffer::new(64, 48);
        duas.set_pixel(0, 0, Rgb { r: 255, g: 0, b: 0 });
        let (distintas, dominante) = cores_do_quadro(&duas);
        assert_eq!(distintas, 2, "dois pixels diferentes são duas cores");
        assert_eq!(dominante, 0x0000, "e a dominante é a que ocupa o resto");
    }

    /// **O roteiro de controle é lido no relógio virtual, e por nome de botão.**
    ///
    /// O formato é o mesmo da linha de comando (`ms:botão`), e o nome tem de ser achado na tabela
    /// do aparelho: um nome errado vira passo sem botão, e um passo sem botão **solta tudo** —
    /// silenciosamente. O teste fixa as duas coisas que importam: a ordem por tempo e a recusa de
    /// nome desconhecido.
    #[test]
    fn o_roteiro_de_teclas_le_tempo_e_nome() {
        // A tabela é a do aparelho; o índice de `b1` é 0 e o de `down` é 14.
        let indice_de = |nome: &str| {
            crate::input::BUTTON_NAMES
                .iter()
                .position(|candidato| candidato.eq_ignore_ascii_case(nome))
        };
        assert_eq!(indice_de("b1"), Some(0));
        assert_eq!(indice_de("DOWN"), indice_de("down"));
        assert_eq!(indice_de("inexistente"), None);

        // Ordem: o roteiro é aplicado em ordem de tempo, mesmo escrito fora dela.
        let mut passos = vec![(2000u64, indice_de("b1")), (1000, indice_de("up"))];
        passos.sort_by_key(|(ms, _)| *ms);
        assert_eq!(passos[0].0, 1000, "o mais cedo vem primeiro");
    }

    /// **Disco cheio não é defeito de jogo.**
    ///
    /// A varredura das 62 ROMs, rodada com o disco cheio, registrou dois jogos como "não carrega"
    /// com `No space left on device` — o placar acusou os jogos, e o problema era o ambiente. A
    /// falta de espaço chega como erro de leitura, então é aqui que ela precisa ser separada.
    #[test]
    fn a_falta_de_espaco_nao_vira_jogo_quebrado() {
        let caso = |codigo| {
            let erro = std::io::Error::from_raw_os_error(codigo);
            Relatorio::recusado(Path::new("/tmp/x.mod"), &StartError::Unreadable(erro)).categoria
        };
        assert_eq!(caso(28), Categoria::SemEspaco, "ENOSPC no Unix");
        assert_eq!(caso(112), Categoria::SemEspaco, "ERROR_DISK_FULL no Windows");
        assert_eq!(caso(2), Categoria::NaoCarrega, "arquivo que falta é outra coisa");
        assert!(!Categoria::SemEspaco.passa());
    }

    /// A recusa é classificada pelo que ela diz do jogo, não pela etapa em que aconteceu.
    #[test]
    fn a_recusa_separa_quem_nao_carrega_de_quem_nao_cria_o_applet() {
        let arquivo = Path::new("/tmp/exemplo.mod");
        let caso = |erro: StartError| Relatorio::recusado(arquivo, &erro).categoria;
        assert_eq!(caso(StartError::NoApplet), Categoria::SemApplet);
        assert_eq!(caso(StartError::Refused(20)), Categoria::SemApplet);
        assert_eq!(
            caso(StartError::Stopped(Outcome::Budget)),
            Categoria::QuebrouAntesDoApplet
        );
        assert_eq!(
            caso(StartError::NotAModule("x".into())),
            Categoria::NaoCarrega
        );
        assert_eq!(
            caso(StartError::NotLoadable("x".into())),
            Categoria::NaoCarrega
        );
    }

    /// **A linha de base não pode enxergar desempenho.**
    ///
    /// Se enxergasse, trocar de máquina — ou rodar em depuração em vez de `--release` — acusaria
    /// regressão em todos os jogos de uma vez, e um teste que acusa sempre não é lido nunca.
    #[test]
    fn o_resumo_nao_muda_com_o_desempenho() {
        let modelo = Relatorio {
            arquivo: PathBuf::from("/tmp/x.mod"),
            titulo: "Exemplo".to_string(),
            abertura_pedida: None,
            categoria: Categoria::Roda,
            motivo: None,
            desempenho: Some(Desempenho {
                virtual_ms: 6_000,
                quadros: 300,
                instrucoes: 700_000_000,
                laco: Duration::from_secs(4),
                ..Default::default()
            }),
            pendencias: Pendencias {
                arquivos: vec!["config.cfg".to_string()],
                ..Default::default()
            },
            estado_da_falha: None,
            chamadas_maiores: vec![("IDisplay::Update".to_string(), 300)],
            custo_maiores: Vec::new(),
            custo_total: 0,
            log_do_jogo: vec!["carregando".to_string()],
            rastreio: Vec::new(),
            despejo: None,
            audio: None,
        };
        let devagar = Relatorio {
            desempenho: Some(Desempenho {
                virtual_ms: 6_000,
                quadros: 41,
                instrucoes: 700_000_000,
                laco: Duration::from_secs(70),
                ..Default::default()
            }),
            chamadas_maiores: Vec::new(),
            custo_maiores: Vec::new(),
            custo_total: 0,
            log_do_jogo: Vec::new(),
            rastreio: Vec::new(),
            despejo: None,
            audio: None,
            ..modelo.clone()
        };
        assert_eq!(modelo.resumo(), devagar.resumo());
        assert!(modelo.resumo().contains("config.cfg"));
        // O completo, sim, mostra a diferença — é o relatório que uma pessoa lê.
        assert_ne!(modelo.completo(), devagar.completo());
    }

    /// Uma pendência que aparece muda o resumo, e é isso que a linha de base cobra.
    #[test]
    fn uma_pendencia_nova_muda_o_resumo() {
        let limpo = Relatorio {
            arquivo: PathBuf::from("/tmp/x.mod"),
            titulo: "Exemplo".to_string(),
            abertura_pedida: None,
            categoria: Categoria::Roda,
            motivo: None,
            desempenho: None,
            pendencias: Pendencias::default(),
            estado_da_falha: None,
            chamadas_maiores: Vec::new(),
            custo_maiores: Vec::new(),
            custo_total: 0,
            log_do_jogo: Vec::new(),
            rastreio: Vec::new(),
            despejo: None,
            audio: None,
        };
        assert!(limpo.resumo().contains("nada a apontar"));
        let com_api = Relatorio {
            pendencias: Pendencias {
                apis: vec!["IShell::Método (de 0x00012345)".to_string()],
                ..Default::default()
            },
            ..limpo.clone()
        };
        assert_ne!(limpo.resumo(), com_api.resumo());
        assert!(com_api.resumo().contains("APIs que faltaram"));
        assert!(!com_api.resumo().contains("nada a apontar"));
    }

    /// A linha de base é gravada na primeira vez e cobrada na segunda.
    #[test]
    fn a_linha_de_base_e_gravada_e_depois_cobrada() {
        let caminho = std::env::temp_dir().join("zeebx-varredura-base.txt");
        let _ = std::fs::remove_file(&caminho);
        let modelo = Relatorio {
            arquivo: PathBuf::from("/tmp/x.mod"),
            titulo: "Exemplo".to_string(),
            abertura_pedida: None,
            categoria: Categoria::Roda,
            motivo: None,
            desempenho: None,
            pendencias: Pendencias::default(),
            estado_da_falha: None,
            chamadas_maiores: Vec::new(),
            custo_maiores: Vec::new(),
            custo_total: 0,
            log_do_jogo: Vec::new(),
            rastreio: Vec::new(),
            despejo: None,
            audio: None,
        };
        assert!(confere_base(&caminho, &modelo).is_ok(), "grava o que falta");
        assert!(confere_base(&caminho, &modelo).is_ok(), "e depois confere");
        let mudou = Relatorio {
            pendencias: Pendencias {
                classes: vec!["0x01003109".to_string()],
                ..Default::default()
            },
            ..modelo
        };
        let Err(diferenca) = confere_base(&caminho, &mudou) else {
            panic!("uma classe nova tinha de acusar diferença");
        };
        // A diferença precisa nomear o que apareceu: "mudou" sem dizer o quê não ajuda ninguém.
        assert!(diferenca.contains("+   0x01003109"), "{diferenca}");
        let _ = std::fs::remove_file(&caminho);
    }

    /// O estado da falha aparece no relatório completo e **não** no resumo.
    ///
    /// Registrador é material de investigação: dois computadores podem chegar ao mesmo defeito
    /// com pilha diferente, e cobrar isso na linha de base seria acusar regressão onde não há.
    #[test]
    fn o_estado_da_falha_fica_fora_do_resumo() {
        let relatorio = Relatorio {
            arquivo: PathBuf::from("/tmp/x.mod"),
            titulo: "Exemplo".to_string(),
            abertura_pedida: None,
            categoria: Categoria::QuebrouNoLaco,
            motivo: Some("acesso inválido a 0x00000024, em 0x00032b78".to_string()),
            desempenho: None,
            pendencias: Pendencias::default(),
            estado_da_falha: Some("  r0=0x0 r1=0x24\n  pilha: 0x1 0x2\n".to_string()),
            chamadas_maiores: Vec::new(),
            custo_maiores: Vec::new(),
            custo_total: 0,
            log_do_jogo: Vec::new(),
            rastreio: Vec::new(),
            despejo: None,
            audio: None,
        };
        assert!(relatorio.completo().contains("no instante da falha"));
        assert!(relatorio.completo().contains("r1=0x24"));
        assert!(!relatorio.resumo().contains("r1=0x24"));
        // O motivo, sim, está nos dois: é ele que diz o que quebrou.
        assert!(relatorio.resumo().contains("0x00032b78"));
    }

    #[test]
    fn o_titulo_vira_nome_de_arquivo_previsivel() {
        assert_eq!(arquivo_do_titulo("Quake 2"), "quake-2.txt");
        assert_eq!(
            arquivo_do_titulo("Alice no País das Maravilhas"),
            "alice-no-pa-s-das-maravilhas.txt"
        );
    }

    /// O pad do roteiro persiste entre os passos. Este teste existe porque o defeito era silencioso:
    /// o pad era recriado a cada passo, então `"20000:x=128,22000:b1"` soltava o manche no instante
    /// do aperto. O gesto de segurar o manche e apertar um botão — o que um jogo pede para abrir —
    /// nunca chegava inteiro, e a conclusão errada foi que o gesto não existia.
    #[test]
    fn o_pad_do_roteiro_persiste_entre_os_passos() {
        let pad = aplica_o_passo(crate::input::Pad::default(), Passo::Eixo(0, 128));
        assert_eq!(pad.eixo_do_console(0), 255, "o manche no máximo é 255");

        let pad = aplica_o_passo(pad, Passo::Botao(1));
        assert_eq!(
            pad.eixo_do_console(0),
            255,
            "o aperto do botão não pode soltar o manche"
        );
        assert_ne!(pad.buttons, 0, "o botão apertado continua apertado");

        let pad = aplica_o_passo(pad, Passo::Solto);
        assert_eq!(pad.eixo_do_console(0), 128, "o passo vazio solta o eixo");
        assert_eq!(pad.buttons, 0, "o passo vazio solta os botões");
    }



    /// **O parser do roteiro entende o evento**, e não só a entrega.
    ///
    /// O teste anterior exercita `passos_vencidos`, que recebe o passo já pronto; este exercita o
    /// **texto** que o jogador escreve. Os dois juntos fecham a linha: texto → passo → ordem.
    #[test]
    fn o_texto_do_roteiro_vira_evento() {
        // SAFETY: teste de thread única, e a variável é lida logo abaixo.
        unsafe { std::env::set_var("ZEEBX_ROM_TECLAS", "1000:e0x7000=0x4ea,2000:kselect") };
        let passos = roteiro_de_teclas();
        unsafe { std::env::remove_var("ZEEBX_ROM_TECLAS") };
        assert_eq!(
            passos,
            vec![
                (1_000, Passo::Evento(0x7000, 0x4ea)),
                (2_000, Passo::Tecla(crate::input::avk::SELECT)),
            ],
            "o texto do roteiro não virou evento e tecla"
        );
    }

    /// **A tecla sem o `k` também é tecla** — o `k` é opcional, não obrigatório.
    ///
    /// O teste existe por uma medição que saiu errada: eu escrevi o roteiro de teclas da Z-Wheel
    /// com a forma da bancada (`30500:0xe064`) e a varredura **descartou todos os passos em
    /// silêncio**, porque ali a tecla exigia o `k`. O relatório não mudou de forma, e a conclusão
    /// que saiu dele — "as teclas não chegam à roda" — era do instrumento, não do emulador. Com o
    /// `k` opcional, a medida passou a dizer o que se queria medir.
    #[test]
    fn a_tecla_do_roteiro_dispensa_o_k() {
        // SAFETY: teste de thread única, e a variável é lida logo abaixo.
        unsafe { std::env::set_var("ZEEBX_ROM_TECLAS", "30500:0xe064,31000:k0xe032,32000:up") };
        let passos = roteiro_de_teclas();
        unsafe { std::env::remove_var("ZEEBX_ROM_TECLAS") };
        assert_eq!(
            passos,
            vec![
                (30_500, Passo::Tecla(0xe064)),
                (31_000, Passo::Tecla(0xe032)),
                // `up` continua sendo **botão**: é nome dos dois, e o botão vem primeiro.
                (32_000, Passo::Botao(12)),
            ],
            "o nome de tecla sem o `k` virou passo, ou `up` deixou de ser botão"
        );
    }

    /// **O roteiro sabe entregar evento de widget**, e o passo não é descartado em silêncio.
    ///
    /// O teste existe porque eu escrevi o ramo **depois** do ramo dos eixos, e o `_ => continue`
    /// dele engolia `e0x7000=0x4ea`: o passo virava nada, sem aviso. É a mesma armadilha do pad
    /// recriado a cada quadro, uma camada acima — instrumento que descarta entrada em silêncio.
    #[test]
    fn o_roteiro_entrega_evento_de_widget() {
        // O caminho do parser é o mesmo do roteiro de teclas: aqui ele é exercitado direto.
        let passos = vec![(1_000u64, Passo::Evento(0x7000, 0x4ea))];
        let mut passo = 0usize;
        let mut teclas = Vec::new();
        let mut eventos = Vec::new();
        let _ = passos_vencidos(
            crate::input::Pad::default(),
            &passos,
            &mut passo,
            1_500,
            &mut teclas,
            &mut eventos,
        );
        assert_eq!(eventos, vec![(0x7000u32, 0x4eau16)]);
    }

    /// O roteiro sabe apertar **tecla**, e o passo vazio solta o que estiver preso.
    ///
    /// A Z-Wheel pede `AVK_0` e `AVK_CLR` para o formulário de abertura andar, e nenhum dos dois
    /// sai de botão de controle. Enquanto o roteiro só sabia de botão e eixo, a resposta honesta
    /// era "o gesto não está no controle" — e a pergunta nem podia ser feita.
    #[test]
    fn o_roteiro_de_teclas_aperta_e_solta() {
        let roteiro = vec![
            (1_000u64, Passo::Tecla(crate::input::avk::ZERO)),
            (2_000, Passo::Solto),
        ];
        let mut passo = 0usize;
        let mut teclas = Vec::new();
        let mut eventos = Vec::new();
        let _ = passos_vencidos(
            crate::input::Pad::default(),
            &roteiro,
            &mut passo,
            1_500,
            &mut teclas,
            &mut eventos,
        );
        assert_eq!(teclas, vec![(crate::input::avk::ZERO, true)]);

        let _ = passos_vencidos(
            crate::input::Pad::default(),
            &roteiro,
            &mut passo,
            2_500,
            &mut teclas,
            &mut eventos,
        );
        assert_eq!(
            teclas,
            vec![
                (crate::input::avk::ZERO, true),
                (crate::input::avk::ZERO, false)
            ],
            "o passo vazio tinha de soltar a tecla presa"
        );
    }

    /// O mesmo defeito, medido **através dos quadros**, que é onde ele vivia.
    ///
    /// O roteiro `"18000:x=128,20000:b1"` põe o manche no máximo e, dois segundos depois, aperta um
    /// botão. O pad precisa atravessar os quadros entre os dois passos: a primeira versão do
    /// conserto o recriava a cada quadro, e o instrumento mostrou `eixo0=255` num passo e `128` no
    /// seguinte. Este teste percorre a mesma travessia.
    #[test]
    fn o_manche_atravessa_os_quadros_ate_o_passo_seguinte() {
        let roteiro = vec![(18_000u64, Passo::Eixo(0, 128)), (20_000, Passo::Botao(0))];
        let mut passo = 0usize;

        // Primeiro quadro depois dos 18 s: só o manche venceu.
        let mut teclas = Vec::new();
        let mut eventos = Vec::new();
        let pad = passos_vencidos(
            crate::input::Pad::default(),
            &roteiro,
            &mut passo,
            18_500,
            &mut teclas,
            &mut eventos,
        );
        assert_eq!(pad.eixo_do_console(0), 255);
        assert_eq!(passo, 1);

        // Quadros intermediários: nada novo vence, e o manche continua onde estava.
        let pad = passos_vencidos(pad, &roteiro, &mut passo, 19_000, &mut teclas, &mut eventos);
        assert_eq!(
            pad.eixo_do_console(0),
            255,
            "o manche se perdeu entre dois quadros"
        );

        // Segundo passo: o botão aperta **sem** soltar o manche. É este o gesto de abrir.
        let pad = passos_vencidos(pad, &roteiro, &mut passo, 20_500, &mut teclas, &mut eventos);
        assert_eq!(
            pad.eixo_do_console(0),
            255,
            "o aperto do botão soltou o manche"
        );
        assert_ne!(pad.buttons, 0);
        assert_eq!(passo, 2);
    }
}
